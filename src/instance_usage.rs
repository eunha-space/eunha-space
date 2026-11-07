//! Capacity observations; no enforcement and no private connection details in output.
use super::*;
use sqlx::PgConnection;

#[derive(Serialize, sqlx::FromRow)]
pub(crate) struct Reading {
    tenant_id: Uuid,
    sampled_at: i64,
    users: Option<i64>,
    local_media_bytes: Option<i64>,
    remote_media_bytes: Option<i64>,
    database_bytes: Option<i64>,
}

pub(crate) async fn latest(
    State(s): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    admin_user(&headers, &s.db).await?;
    let SqlJson(rows) = sqlx::query_scalar::<_, SqlJson<serde_json::Value>>(r#"
SELECT COALESCE(jsonb_agg(to_jsonb(r)), '[]'::jsonb) FROM (
 SELECT t.id tenant_id,t.domain,t.lifecycle_state,
 EXISTS(SELECT 1 FROM tenant_imports i WHERE i.tenant_id=t.id AND i.external_media) media_exempt,
 p.name_en plan,p.user_limit,p.media_gb,p.database_gb,p.monthly_activities,
 u.sampled_at,u.users,u.local_media_bytes,u.remote_media_bytes,u.database_bytes,
 (SELECT sum(d.deliveries) FROM tenant_delivery_daily d WHERE d.tenant_id=t.id
   AND d.day >= date_trunc('month',now() AT TIME ZONE 'UTC')::date) monthly_deliveries,
 (SELECT sum(d.observed_ms) FROM tenant_delivery_daily d WHERE d.tenant_id=t.id
   AND d.day >= date_trunc('month',now() AT TIME ZONE 'UTC')::date) delivery_observed_ms
 FROM tenants t LEFT JOIN subscriptions s ON s.user_id=t.user_id
 LEFT JOIN plans p ON p.id=s.plan_id
 LEFT JOIN LATERAL (SELECT * FROM tenant_usage WHERE tenant_id=t.id ORDER BY sampled_at DESC LIMIT 1) u ON true
 WHERE t.lifecycle_state IN ('ready','failed','retained')
) r"#).fetch_one(&s.db).await?;
    Ok(Json(rows))
}

pub(crate) async fn history(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<Reading>>, ApiError> {
    admin_user(&headers, &s.db).await?;
    Ok(Json(sqlx::query_as::<_, Reading>("SELECT tenant_id,sampled_at,users,local_media_bytes,remote_media_bytes,database_bytes FROM tenant_usage_daily WHERE tenant_id=$1 ORDER BY day")
        .bind(id).fetch_all(&s.db).await?))
}

/// Uses the health probe's connection after its reading has been captured.
/// The caller supplies a deadline so capacity cannot keep extending a late pass.
pub(crate) async fn probe(
    tenant_id: Uuid,
    connection: Option<&mut PgConnection>,
    deadline: tokio::time::Instant,
) -> Reading {
    let mut row = Reading {
        tenant_id,
        sampled_at: (OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64,
        users: None,
        local_media_bytes: None,
        remote_media_bytes: None,
        database_bytes: None,
    };
    if let Some(db) = connection {
        // Preserve already collected fields if a later query fails or times out.
        // Capacity has its own budget, independent of the health query timeout.
        let _ = tokio::time::timeout_at(deadline, async {
            row.database_bytes = sqlx::query_scalar("SELECT pg_database_size(current_database())")
                .fetch_one(&mut *db).await.ok();
            row.users = sqlx::query_scalar("SELECT count(*) FROM users u JOIN accounts a ON a.id=u.account_id WHERE a.domain IS NULL AND a.id <> -99 AND u.confirmed_at IS NOT NULL AND u.approved AND NOT u.disabled AND a.suspended_at IS NULL")
                .fetch_one(&mut *db).await.ok();
            if let Ok((local, remote)) = sqlx::query_as::<_, (i64, i64)>(include_str!("instance_usage_media.sql"))
                .fetch_one(&mut *db).await {
                row.local_media_bytes = Some(local);
                row.remote_media_bytes = Some(remote);
            }
        }).await;
    }
    row
}

pub(crate) async fn persist(s: &Shared, rows: &[Reading]) -> Result<(), ApiError> {
    let now = (OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64;
    let mut tx = s.db.begin().await?;
    sqlx::query("INSERT INTO tenant_usage SELECT * FROM jsonb_populate_recordset(NULL::tenant_usage,$1) ON CONFLICT DO NOTHING").bind(SqlJson(&rows)).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO tenant_usage_daily SELECT u.*,(to_timestamp(sampled_at/1000.0) AT TIME ZONE 'UTC')::date FROM jsonb_populate_recordset(NULL::tenant_usage,$1) u ON CONFLICT (tenant_id,day) DO UPDATE SET sampled_at=EXCLUDED.sampled_at,users=EXCLUDED.users,local_media_bytes=EXCLUDED.local_media_bytes,remote_media_bytes=EXCLUDED.remote_media_bytes,database_bytes=EXCLUDED.database_bytes WHERE tenant_usage_daily.sampled_at<EXCLUDED.sampled_at").bind(SqlJson(&rows)).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM tenant_usage WHERE sampled_at<$1")
        .bind(now - usage_range::RETENTION_MS)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM tenant_usage_daily WHERE day < (now() AT TIME ZONE 'UTC')::date-365")
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "DELETE FROM tenant_delivery_daily WHERE day < (now() AT TIME ZONE 'UTC')::date-365",
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

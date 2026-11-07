//! Capacity observations; no enforcement and no private connection details in output.
use super::*;
use futures::{StreamExt, stream};
use sqlx::{Connection, PgConnection};

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

pub(crate) async fn collect(s: &Shared) -> Result<(), ApiError> {
    let now = (OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64;
    let targets = sqlx::query_as::<_, (Uuid, String)>("SELECT t.id,c.database_url FROM tenants t JOIN tenant_configuration c ON c.tenant_id=t.id WHERE t.lifecycle_state IN ('ready','failed','retained') AND NOT EXISTS(SELECT 1 FROM tenant_usage u WHERE u.tenant_id=t.id AND u.sampled_at>$1)")
        .bind(now-900_000).fetch_all(&s.db).await?;
    let rows = stream::iter(targets.into_iter().map(|(tenant_id,url)| async move {
        // Each query is independently nullable: an incompatible media schema
        // must not hide a valid database size or account count.
        let values = tokio::time::timeout(Duration::from_secs(4), async {
            let mut db = PgConnection::connect(&url).await.ok()?;
            let users = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM users u JOIN accounts a ON a.id=u.account_id WHERE a.domain IS NULL AND a.id <> -99 AND u.confirmed_at IS NOT NULL AND u.approved AND NOT u.disabled AND a.suspended_at IS NULL").fetch_one(&mut db).await.ok();
            let media = sqlx::query_as::<_, (i64,i64)>(include_str!("instance_usage_media.sql")).fetch_one(&mut db).await.ok();
            let size = sqlx::query_scalar::<_,i64>("SELECT pg_database_size(current_database())").fetch_one(&mut db).await.ok();
            let _ = db.close().await;
            Some((users,media,size))
        }).await.ok().flatten();
        Reading { tenant_id, sampled_at: now,
            users: values.as_ref().and_then(|v|v.0),
            local_media_bytes: values.as_ref().and_then(|v|v.1.map(|v|v.0)),
            remote_media_bytes: values.as_ref().and_then(|v|v.1.map(|v|v.1)),
            database_bytes: values.and_then(|v|v.2) }
    })).buffer_unordered(8).collect::<Vec<_>>().await;
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

pub(crate) async fn sampler(s: Shared) {
    loop {
        if collect(&s).await.is_err() {
            tracing::warn!("capacity sampling failed");
        }
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

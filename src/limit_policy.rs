//! Durable plan policy and owner-scoped readings. Secrets never enter this API.
use super::*;
fn bad(message: &str) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, message.into())
}

async fn snapshot(
    db: &PgPool,
    owner: Option<Uuid>,
    tenant: Option<Uuid>,
) -> Result<serde_json::Value, ApiError> {
    let SqlJson(rows)=sqlx::query_scalar::<_,SqlJson<serde_json::Value>>(r#"
SELECT COALESCE(jsonb_agg(jsonb_build_object('tenant_id',r.tenant_id,'domain',r.domain,
 'resource',r.resource,'value',r.value,'allowance',r.allowance,'sampled_at',r.sampled_at,
 'fresh',COALESCE(r.fresh,false),'exempt',r.exempt,'override_exempt',COALESCE(st.exempt,false),'over_since',st.over_since,'grace_until',st.grace_until,
 'enforced',st.enforced_at IS NOT NULL AND COALESCE(r.fresh,false),'mode',(SELECT usage_mode FROM billing_settings))), '[]'::jsonb)
FROM tenant_limit_readings r LEFT JOIN tenant_limit_states st USING(tenant_id,resource)
WHERE r.lifecycle_state IN ('ready','failed','retained') AND ($1::uuid IS NULL OR r.user_id=$1) AND ($2::uuid IS NULL OR r.tenant_id=$2)
 AND ($1::uuid IS NULL OR r.lifecycle_state IN ('ready','failed'))
"#).bind(owner).bind(tenant).fetch_one(db).await?;
    Ok(rows)
}

pub(crate) async fn customer_usage(
    State(s): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (id, _) = customer(&headers, &s.db).await?;
    Ok(Json(snapshot(&s.db, Some(id), None).await?))
}

pub(crate) async fn admin_usage(
    State(s): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    admin_user(&headers, &s.db).await?;
    Ok(Json(snapshot(&s.db, None, None).await?))
}

#[derive(Deserialize)]
pub(crate) struct Mode {
    mode: String,
    reason: String,
}
pub(crate) async fn set_mode(
    State(s): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Mode>,
) -> Result<StatusCode, ApiError> {
    let (actor, _) = admin_user(&headers, &s.db).await?;
    if !["show", "warn", "enforce"].contains(&body.mode.as_str())
        || body.reason.trim().is_empty()
        || body.reason.len() > 1000
    {
        return Err(bad("Choose a usage mode and provide a reason."));
    }
    let mut tx = s.db.begin().await?;
    sqlx::query("UPDATE billing_settings SET usage_mode=$1,updated_by=$2,updated_at=now()")
        .bind(&body.mode)
        .bind(actor)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO tenant_limit_events(actor_id,action,reason,detail) VALUES($1,'mode',$2,jsonb_build_object('mode',$3::text))")
        .bind(actor).bind(body.reason.trim()).bind(&body.mode).execute(&mut *tx).await?;
    refresh_all(&mut tx).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(crate) struct Override {
    resource: String,
    exempt: bool,
    extend_days: Option<i32>,
    reason: String,
}
pub(crate) async fn set_override(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<Override>,
) -> Result<StatusCode, ApiError> {
    let (actor, _) = admin_user(&headers, &s.db).await?;
    if !["users", "media", "database", "activities"].contains(&body.resource.as_str())
        || body.reason.trim().is_empty()
        || body.reason.len() > 1000
        || body.extend_days.is_some_and(|d| !(1..=365).contains(&d))
    {
        return Err(bad("Choose a limit, a valid extension and a reason."));
    }
    let mut tx = s.db.begin().await?;
    let exists = sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM tenants WHERE id=$1)")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    if !exists {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            "Instance not found.".into(),
        ));
    }
    sqlx::query("SELECT refresh_tenant_limits($1)")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE tenant_limit_states SET exempt=$3,grace_until=CASE WHEN $4::integer IS NOT NULL AND over_since IS NOT NULL THEN greatest(COALESCE(grace_until,now()),now())+make_interval(days=>$4) ELSE grace_until END WHERE tenant_id=$1 AND resource=$2")
        .bind(id).bind(&body.resource).bind(body.exempt).bind(body.extend_days).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO tenant_limit_events(tenant_id,resource,actor_id,action,reason,detail) VALUES($1,$2,$3,'override',$4,jsonb_build_object('exempt',$5::boolean,'extend_days',$6::integer))")
        .bind(id).bind(&body.resource).bind(actor).bind(body.reason.trim()).bind(body.exempt).bind(body.extend_days).execute(&mut *tx).await?;
    sqlx::query("SELECT refresh_tenant_limits($1)")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn check_now(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let (owner, _) = customer(&headers, &s.db).await?;
    let mut tx = s.db.begin().await?;
    let owned=sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM tenants WHERE id=$1 AND user_id=$2 AND lifecycle_state IN ('ready','failed'))")
        .bind(id).bind(owner).fetch_one(&mut *tx).await?;
    if !owned {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            "Instance not found.".into(),
        ));
    }
    // Make this tenant due without inventing a zero or changing stored samples.
    sqlx::query("INSERT INTO tenant_usage_rechecks(tenant_id,requested_at) VALUES($1,now()) ON CONFLICT(tenant_id) DO UPDATE SET requested_at=EXCLUDED.requested_at WHERE tenant_usage_rechecks.requested_at<now()-interval '1 minute'")
        .bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(StatusCode::ACCEPTED)
}

async fn refresh_all(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<(), sqlx::Error> {
    // One order everywhere prevents cross-tenant checks taking locks in reverse.
    let ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM tenants WHERE lifecycle_state IN ('ready','failed') ORDER BY id FOR UPDATE",
    )
    .fetch_all(&mut **tx)
    .await?;
    for id in ids {
        sqlx::query("SELECT refresh_tenant_limits($1)")
            .bind(id)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

pub(crate) async fn sampler(s: Shared) {
    loop {
        let result = async {
            let mut tx = s.db.begin().await?;
            // Serialize policy scans across control planes. Subscription and
            // sample triggers still decide immediately in their own transaction.
            let claimed = sqlx::query_scalar::<_, bool>(
                "SELECT pg_try_advisory_xact_lock(hashtextextended('instance-limit-policy',0))",
            )
            .fetch_one(&mut *tx)
            .await?;
            if claimed {
                refresh_all(&mut tx).await?;
                schedule_downgrade_notices(&mut tx).await?;
            }
            tx.commit().await
        }
        .await;
        if result.is_err() {
            tracing::warn!("instance limit policy check failed");
        }
        // The outbox also serves Polar-only installations.
        for _ in 0..20 {
            match billing_mail::send_due(&s).await {
                Ok(true) => {}
                Ok(false) => break,
                Err(_) => {
                    tracing::warn!("usage notice delivery failed");
                    break;
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

pub(crate) async fn schedule_downgrade_notices(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<(), sqlx::Error> {
    sqlx::query(r#"INSERT INTO billing_emails(user_id,kind,dedupe_key,locale,subject,body)
SELECT s.user_id,'usage_notice','downgrade-usage:'||s.user_id||':'||s.pending_plan_id||':'||s.current_period_end,COALESCE(u.locale,'en'),
 CASE WHEN u.locale='ko' THEN '은하공간 요금제 변경 전 사용량 안내' ELSE 'Usage before your eunha space plan changes' END,
 CASE WHEN u.locale='ko' THEN '3일 이내에 적용될 요금제의 허용량을 초과한 인스턴스가 있습니다. 계정 페이지에서 사용량을 확인하세요. 사용자 및 미디어 제한은 새 요금제 적용 시 14일의 유예 기간이 시작됩니다. 데이터베이스와 활동은 공정 이용 안내이며 인스턴스는 계속 운영됩니다.' ELSE 'Some instances exceed the allowances of the plan starting within three days. Review usage on your account page. Users and media receive a 14-day grace period when the new plan applies. Database and activities remain advisory; your instances keep running.' END
FROM subscriptions s JOIN users u ON u.id=s.user_id
WHERE s.pending_plan_id IS NOT NULL AND s.status='active' AND NOT u.is_admin AND NOT u.billing_exempt
 AND NOT s.cancel_at_period_end AND s.current_period_end>now() AND s.current_period_end<=now()+interval '3 days'
 AND instance_over_limits(s.user_id,s.pending_plan_id)<>'[]'::jsonb
 AND (SELECT usage_mode FROM billing_settings)<>'show'
ON CONFLICT(dedupe_key) DO NOTHING"#).execute(&mut **tx).await?;
    Ok(())
}

pub(crate) async fn get_mode(
    State(s): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    admin_user(&headers, &s.db).await?;
    let mode = sqlx::query_scalar::<_, String>("SELECT usage_mode FROM billing_settings")
        .fetch_one(&s.db)
        .await?;
    Ok(Json(serde_json::json!({"mode":mode})))
}
pub(crate) async fn events(
    State(s): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    admin_user(&headers, &s.db).await?;
    let SqlJson(rows)=sqlx::query_scalar::<_,SqlJson<serde_json::Value>>("SELECT COALESCE(jsonb_agg(to_jsonb(e)),'[]'::jsonb) FROM (SELECT id,tenant_id,resource,actor_id,action,reason,detail,created_at FROM tenant_limit_events ORDER BY id DESC LIMIT 100) e").fetch_one(&s.db).await?;
    Ok(Json(rows))
}

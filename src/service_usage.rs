//! Private, bounded scraping. Only known tenants and numeric readings are persisted.
use super::*;
use futures::{StreamExt, stream};

#[derive(Clone, Debug, Default, Serialize, Deserialize, sqlx::FromRow)]
pub(crate) struct Sample {
    pub tenant_id: Uuid,
    pub sampled_at: i64,
    #[serde(default)]
    pub epoch: Option<String>,
    pub requests: Option<f64>,
    pub errors: Option<f64>,
    pub duration_sum: Option<f64>,
    pub duration_count: Option<f64>,
    pub aborted: Option<f64>,
    pub rejected: Option<f64>,
    pub in_flight: Option<f64>,
    pub pool: Option<f64>,
    pub pool_idle: Option<f64>,
}

#[derive(Default, Deserialize)]
pub(crate) struct Scope {
    pub tenant_id: Option<Uuid>,
    pub instance_id: Option<Uuid>,
    #[serde(flatten)]
    pub range: usage_range::Range,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct History {
    eligible_tenants: usize,
    samples: Vec<Point>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Point {
    sampled_at: i64,
    reporting_tenants: usize,
    request_rate: Option<f64>,
    error_percent: Option<f64>,
    latency_ms: Option<f64>,
    aborted_rate: Option<f64>,
    rejected_rate: Option<f64>,
    in_flight: Option<f64>,
    pool: Option<f64>,
    pool_idle: Option<f64>,
}

fn now() -> i64 {
    (OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64
}

pub(crate) async fn history(
    State(s): State<Shared>,
    headers: HeaderMap,
    Query(scope): Query<Scope>,
) -> Result<Json<History>, ApiError> {
    operator(&headers, &s.db).await?;
    let ids = sqlx::query_scalar::<_, Uuid>("SELECT t.id FROM tenants t JOIN runtimes r ON r.id=t.runtime_id WHERE t.lifecycle_state IN ('ready','failed') AND ($1::uuid IS NULL OR t.id=$1) AND ($2::uuid IS NULL OR r.instance_id=$2)")
        .bind(scope.tenant_id).bind(scope.instance_id).fetch_all(&s.db).await?;
    let (cutoff, bucket) = scope.range.bounds(now())?;
    let SqlJson(history)=sqlx::query_scalar::<_,SqlJson<History>>(r#"
WITH previous AS (
 SELECT *,lag(sampled_at) OVER w previous_at,lag(epoch) OVER w previous_epoch,
 lag(requests) OVER w previous_requests,lag(errors) OVER w previous_errors,
 lag(duration_sum) OVER w previous_sum,lag(duration_count) OVER w previous_count,
 lag(aborted) OVER w previous_aborted,lag(rejected) OVER w previous_rejected
 FROM tenant_service_samples WHERE tenant_id=ANY($1) AND sampled_at >= $2-30000
 WINDOW w AS (PARTITION BY tenant_id ORDER BY sampled_at)
), rates AS (
 SELECT *, (sampled_at-previous_at)::double precision/1000 elapsed,
 epoch IS NOT NULL AND epoch=previous_epoch AND sampled_at-previous_at BETWEEN 1 AND 30000 valid
 FROM previous WHERE sampled_at >= $2
), totals AS (
 SELECT sampled_at,count(pool) reporting_tenants,
 CASE WHEN count(pool)=$4 THEN sum(pool) END pool,
 CASE WHEN count(pool_idle)=$4 THEN sum(pool_idle) END pool_idle,
 CASE WHEN count(in_flight)=$4 THEN sum(in_flight) END in_flight,
 CASE WHEN count(*) FILTER(WHERE valid AND requests>=previous_requests)=$4 THEN sum((requests-previous_requests)/elapsed) END request_rate,
 CASE WHEN count(*) FILTER(WHERE valid AND errors>=previous_errors)=$4 THEN sum((errors-previous_errors)/elapsed) END error_rate,
 CASE WHEN count(*) FILTER(WHERE valid AND duration_sum>=previous_sum)=$4 THEN sum((duration_sum-previous_sum)/elapsed) END sum_rate,
 CASE WHEN count(*) FILTER(WHERE valid AND duration_count>=previous_count)=$4 THEN sum((duration_count-previous_count)/elapsed) END count_rate,
 CASE WHEN count(*) FILTER(WHERE valid AND aborted>=previous_aborted)=$4 THEN sum((aborted-previous_aborted)/elapsed) END aborted_rate,
 CASE WHEN count(*) FILTER(WHERE valid AND rejected>=previous_rejected)=$4 THEN sum((rejected-previous_rejected)/elapsed) END rejected_rate
 FROM rates GROUP BY sampled_at
), rolled AS (
 SELECT max(sampled_at) sampled_at,min(reporting_tenants) reporting_tenants,
 CASE WHEN count(pool)=count(*) THEN avg(pool) END pool,
 CASE WHEN count(pool_idle)=count(*) THEN avg(pool_idle) END pool_idle,
 CASE WHEN count(in_flight)=count(*) THEN avg(in_flight) END in_flight,
 CASE WHEN count(request_rate)=count(*) THEN avg(request_rate) END request_rate,
 CASE WHEN count(aborted_rate)=count(*) THEN avg(aborted_rate) END aborted_rate,
 CASE WHEN count(rejected_rate)=count(*) THEN avg(rejected_rate) END rejected_rate,
 CASE WHEN count(error_rate)=count(*) AND count(request_rate)=count(*) THEN sum(error_rate)/nullif(sum(request_rate),0)*100 END error_percent,
 CASE WHEN count(sum_rate)=count(*) AND count(count_rate)=count(*) THEN sum(sum_rate)/nullif(sum(count_rate),0)*1000 END latency_ms
 FROM totals GROUP BY sampled_at/$3
) SELECT jsonb_build_object('eligible_tenants',$4,'samples',COALESCE((SELECT jsonb_agg(to_jsonb(r) ORDER BY sampled_at) FROM rolled r),'[]'::jsonb))
"#).bind(&ids).bind(cutoff).bind(bucket).bind(ids.len() as i64).fetch_one(&s.db).await?;
    Ok(Json(history))
}

#[derive(sqlx::FromRow)]
struct Target {
    id: Uuid,
    address: String,
    tenant_directory: String,
    service_label: String,
    blue_bind_address: String,
    green_bind_address: String,
    router_enabled: bool,
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

async fn scrape(target: &Target, domains: &[String]) -> Option<HashMap<String, Sample>> {
    if !target.router_enabled {
        return None;
    }
    let config = serde_json::json!({"directory":target.tenant_directory,"label":target.service_label,
        "blue":target.blue_bind_address,"green":target.green_bind_address,"domains":domains});
    let command = format!(
        "/usr/bin/python3 -c {} {}",
        quote(include_str!("../ops/scrape-service.py")),
        quote(&config.to_string())
    );
    let mut process = if matches!(target.address.as_str(), "localhost" | "127.0.0.1" | "::1") {
        let mut p = tokio::process::Command::new("/bin/sh");
        p.args(["-c", &command]);
        p
    } else {
        let mut p = tokio::process::Command::new("ssh");
        p.args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=3",
            "--",
            &target.address,
            &command,
        ]);
        p
    };
    let output = tokio::time::timeout(
        Duration::from_secs(8),
        process.stdin(Stdio::null()).kill_on_drop(true).output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() || output.stdout.len() > 1024 * 1024 {
        return None;
    }
    // Add identifiers ourselves; scrape output never determines tenant placement.
    let values: HashMap<String, serde_json::Value> = serde_json::from_slice(&output.stdout).ok()?;
    let mut rows = HashMap::new();
    for domain in domains {
        if let Some(value) = values.get(domain) {
            let mut value = value.clone();
            value["tenant_id"] = serde_json::json!(Uuid::nil());
            value["sampled_at"] = serde_json::json!(0);
            let row: Sample = serde_json::from_value(value).ok()?;
            if row.epoch.as_ref().is_none_or(|e| e.len() > 256) {
                return None;
            }
            if [
                row.requests,
                row.errors,
                row.duration_sum,
                row.duration_count,
                row.aborted,
                row.rejected,
                row.in_flight,
                row.pool,
                row.pool_idle,
            ]
            .iter()
            .any(|n| n.is_none_or(|n| !n.is_finite() || n < 0.0))
            {
                return None;
            }
            rows.insert(domain.clone(), row);
        }
    }
    Some(rows)
}

pub(crate) async fn collect(s: &Shared) -> Result<(), ApiError> {
    let targets=sqlx::query_as::<_,Target>("SELECT r.id,i.address,r.tenant_directory,r.service_label,r.blue_bind_address,r.green_bind_address,r.router_enabled FROM runtimes r JOIN instances i ON i.id=r.instance_id WHERE EXISTS(SELECT 1 FROM tenants t WHERE t.runtime_id=r.id AND t.lifecycle_state IN ('ready','failed'))").fetch_all(&s.db).await?;
    let tenants = sqlx::query_as::<_, (Uuid, Uuid, String)>(
        "SELECT id,runtime_id,domain FROM tenants WHERE lifecycle_state IN ('ready','failed')",
    )
    .fetch_all(&s.db)
    .await?;
    let groups = stream::iter(targets.into_iter().map(|target| {
        let tenants = tenants
            .iter()
            .filter(|(_, runtime, _)| *runtime == target.id)
            .cloned()
            .collect::<Vec<_>>();
        async move {
            let domains = tenants
                .iter()
                .map(|(_, _, domain)| domain.clone())
                .collect::<Vec<_>>();
            let mut scraped = scrape(&target, &domains).await.unwrap_or_default();
            tenants
                .into_iter()
                .map(|(tenant_id, _, domain)| {
                    let mut row = scraped.remove(&domain).unwrap_or_default();
                    row.tenant_id = tenant_id;
                    row
                })
                .collect::<Vec<_>>()
        }
    }))
    .buffer_unordered(4)
    .collect::<Vec<_>>()
    .await;
    let sampled_at = now();
    let rows = groups
        .into_iter()
        .flatten()
        .map(|mut row| {
            row.sampled_at = sampled_at;
            row
        })
        .collect::<Vec<_>>();
    let mut tx = s.db.begin().await?;
    sqlx::query("INSERT INTO tenant_service_samples SELECT * FROM jsonb_populate_recordset(NULL::tenant_service_samples,$1) ON CONFLICT DO NOTHING").bind(SqlJson(rows)).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM tenant_service_samples WHERE sampled_at<$1")
        .bind(sampled_at - usage_range::RETENTION_MS)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn sampler(s: Shared) {
    loop {
        if collect(&s).await.is_err() {
            tracing::warn!("service metrics persistence failed");
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

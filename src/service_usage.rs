//! Private, bounded scraping. Only known tenants and numeric readings are persisted.
use super::*;
use futures::{StreamExt, stream};
use std::collections::BTreeMap;

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
}

#[derive(Debug, Serialize)]
pub(crate) struct History {
    eligible_tenants: usize,
    samples: Vec<Point>,
}

#[derive(Debug, Serialize)]
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
    let cutoff = now() - 600_000;
    let rows = sqlx::query_as::<_, Sample>("SELECT * FROM tenant_service_samples WHERE tenant_id=ANY($1) AND sampled_at >= $2 ORDER BY sampled_at,tenant_id")
        .bind(&ids).bind(cutoff-30000).fetch_all(&s.db).await?;
    Ok(Json(aggregate(rows, ids.len(), cutoff)))
}

fn delta(
    current: &Sample,
    before: Option<&Sample>,
    field: fn(&Sample) -> Option<f64>,
) -> Option<f64> {
    let before = before?;
    if current.epoch.is_none()
        || current.epoch != before.epoch
        || !(1..=30000).contains(&(current.sampled_at - before.sampled_at))
    {
        return None;
    }
    let (a, b) = (field(current)?, field(before)?);
    (a >= b).then_some((a - b) / ((current.sampled_at - before.sampled_at) as f64 / 1000.0))
}

fn aggregate(rows: Vec<Sample>, expected: usize, cutoff: i64) -> History {
    let mut previous = HashMap::<Uuid, Sample>::new();
    let mut buckets = BTreeMap::<i64, Vec<(Sample, [Option<f64>; 6])>>::new();
    for row in rows {
        let before = previous.get(&row.tenant_id);
        let rates = [
            delta(&row, before, |r| r.requests),
            delta(&row, before, |r| r.errors),
            delta(&row, before, |r| r.duration_sum),
            delta(&row, before, |r| r.duration_count),
            delta(&row, before, |r| r.aborted),
            delta(&row, before, |r| r.rejected),
        ];
        previous.insert(row.tenant_id, row.clone());
        if row.sampled_at >= cutoff {
            buckets
                .entry(row.sampled_at)
                .or_default()
                .push((row, rates));
        }
    }
    let samples = buckets
        .into_iter()
        .map(|(sampled_at, rows)| {
            let sum = |values: Vec<Option<f64>>| -> Option<f64> {
                if expected == 0 || rows.len() != expected {
                    return None;
                }
                values.into_iter().sum()
            };
            let rate = |index| sum(rows.iter().map(|(_, rates)| rates[index]).collect());
            let requests = rate(0);
            let error_percent = requests
                .filter(|n| *n > 0.0)
                .zip(rate(1))
                .map(|(total, errors)| errors / total * 100.0);
            let latency_ms = rate(3)
                .filter(|n| *n > 0.0)
                .zip(rate(2))
                .map(|(count, sum)| sum / count * 1000.0);
            Point {
                sampled_at,
                reporting_tenants: rows.iter().filter(|(r, _)| r.pool.is_some()).count(),
                request_rate: requests,
                error_percent,
                latency_ms,
                aborted_rate: rate(4),
                rejected_rate: rate(5),
                in_flight: sum(rows.iter().map(|(r, _)| r.in_flight).collect()),
                pool: sum(rows.iter().map(|(r, _)| r.pool).collect()),
                pool_idle: sum(rows.iter().map(|(r, _)| r.pool_idle).collect()),
            }
        })
        .collect();
    History {
        eligible_tenants: expected,
        samples,
    }
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
        .bind(sampled_at - 86_400_000)
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rates_are_reset_aware_and_aggregates_are_weighted_and_complete() {
        let id = Uuid::new_v4();
        let other = Uuid::new_v4();
        let base = Sample {
            tenant_id: id,
            sampled_at: 10000,
            epoch: Some("blue:1".into()),
            requests: Some(10.0),
            errors: Some(1.0),
            duration_sum: Some(1.0),
            duration_count: Some(10.0),
            pool: Some(2.0),
            ..Sample::default()
        };
        let next = Sample {
            sampled_at: 20000,
            requests: Some(30.0),
            errors: Some(3.0),
            duration_sum: Some(5.0),
            duration_count: Some(30.0),
            ..base.clone()
        };
        let history = aggregate(vec![base.clone(), next.clone()], 1, 0);
        assert_eq!(history.samples[1].request_rate, Some(2.0));
        assert_eq!(history.samples[1].error_percent, Some(10.0));
        assert_eq!(history.samples[1].latency_ms, Some(200.0));
        let restart = Sample {
            sampled_at: 30000,
            epoch: Some("green:2".into()),
            ..next.clone()
        };
        assert_eq!(delta(&restart, Some(&next), |r| r.requests), None);
        let reset = Sample {
            sampled_at: 40000,
            requests: Some(0.0),
            ..restart.clone()
        };
        assert_eq!(delta(&reset, Some(&restart), |r| r.requests), None);
        let delayed = Sample {
            sampled_at: 60001,
            ..restart.clone()
        };
        assert_eq!(delta(&delayed, Some(&restart), |r| r.requests), None);
        let failed = Sample {
            sampled_at: 40000,
            ..Sample::default()
        };
        assert_eq!(delta(&failed, Some(&restart), |r| r.requests), None);
        assert_eq!(
            aggregate(vec![base.clone(), next.clone()], 2, 0).samples[1].pool,
            None
        );
        let sibling_base = Sample {
            tenant_id: other,
            ..base
        };
        let sibling_next = Sample {
            tenant_id: other,
            requests: Some(110.0),
            duration_count: Some(110.0),
            duration_sum: Some(11.0),
            ..next.clone()
        };
        let all = aggregate(vec![sibling_base.clone(), sibling_next.clone()], 1, 0);
        assert_eq!(all.samples[1].latency_ms, Some(100.0));
        let both = aggregate(
            vec![
                sibling_base,
                Sample {
                    tenant_id: id,
                    sampled_at: 10000,
                    requests: Some(10.0),
                    duration_count: Some(10.0),
                    duration_sum: Some(1.0),
                    ..next.clone()
                },
                sibling_next,
                next,
            ],
            2,
            0,
        );
        assert!((both.samples[1].latency_ms.unwrap() - 116.6666667).abs() < 0.001);
    }
}

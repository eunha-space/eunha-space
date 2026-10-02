//! Operator-only, bounded sampling. Connection strings and driver errors never leave here.
use super::*;
use futures::{StreamExt, stream};
use sqlx::{Connection, PgConnection};

#[derive(Clone, Debug, Serialize, Deserialize, sqlx::FromRow)]
pub(crate) struct Usage {
    tenant_id: Uuid,
    sampled_at: i64,
    database_bytes: Option<i64>,
    connections: Option<i64>,
    active_connections: Option<i64>,
    transactions: Option<i64>,
    stats_reset: Option<String>,
    #[serde(default)]
    #[sqlx(default)]
    rate: Option<f64>,
}

#[derive(Default)]
pub(crate) struct Cache {
    rows: Vec<Usage>,
}

pub(crate) async fn usage(
    State(s): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Vec<Usage>>, ApiError> {
    operator(&headers, &s.db).await?;
    let rows = s.usage.lock().await.rows.clone();
    Ok(Json(rows))
}

pub(crate) async fn history(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(range): Query<usage_range::Range>,
) -> Result<Json<Vec<Usage>>, ApiError> {
    operator(&headers, &s.db).await?;
    let (cutoff, bucket) = range.bounds(now_millis())?;
    let rows=sqlx::query_as::<_, Usage>(r#"WITH previous AS (
SELECT *,lag(sampled_at) OVER w previous_at,lag(transactions) OVER w previous_transactions,lag(stats_reset) OVER w previous_reset
FROM tenant_usage_samples WHERE tenant_id=$1 AND sampled_at >= $2-30000 WINDOW w AS (ORDER BY sampled_at))
 , readings AS (
SELECT *, CASE WHEN sampled_at-previous_at BETWEEN 1 AND 30000 AND stats_reset IS NOT DISTINCT FROM previous_reset AND transactions>=previous_transactions
THEN (transactions-previous_transactions)::double precision/((sampled_at-previous_at)::double precision/1000) END rate
FROM previous WHERE sampled_at >= $2)
SELECT tenant_id,max(sampled_at) sampled_at,
CASE WHEN count(database_bytes)=count(*) THEN avg(database_bytes)::bigint END database_bytes,
CASE WHEN count(connections)=count(*) THEN avg(connections)::bigint END connections,
CASE WHEN count(active_connections)=count(*) THEN avg(active_connections)::bigint END active_connections,
NULL::bigint transactions,NULL::text stats_reset,
CASE WHEN count(rate)=count(*) THEN avg(rate) END rate
FROM readings GROUP BY tenant_id,sampled_at/$3 ORDER BY sampled_at/$3"#)
        .bind(id).bind(cutoff).bind(bucket).fetch_all(&s.db).await?;
    Ok(Json(rows))
}

#[derive(Default, Deserialize)]
pub(crate) struct AggregateScope {
    pub instance_id: Option<Uuid>,
    #[serde(flatten)]
    pub range: usage_range::Range,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct AggregateHistory {
    eligible_tenants: i64,
    samples: Vec<AggregateSample>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AggregateSample {
    sampled_at: i64,
    reporting_tenants: i64,
    rate_reporting_tenants: i64,
    database_bytes: Option<i64>,
    connections: Option<i64>,
    active_connections: Option<i64>,
    rate: Option<f64>,
}

pub(crate) async fn aggregate_history(
    State(s): State<Shared>,
    headers: HeaderMap,
    Query(scope): Query<AggregateScope>,
) -> Result<Json<AggregateHistory>, ApiError> {
    operator(&headers, &s.db).await?;
    let (cutoff, bucket) = scope.range.bounds(now_millis())?;
    // One statement gives inventory and samples a consistent snapshot. Select
    // the latest reading per tenant/bucket so retries cannot double-count usage.
    let SqlJson(history) = sqlx::query_scalar::<_, SqlJson<AggregateHistory>>(r#"
        WITH eligible AS (
            SELECT t.id FROM tenants t
            JOIN tenant_configuration c ON c.tenant_id=t.id
            JOIN runtimes r ON r.id=t.runtime_id
            WHERE t.lifecycle_state IN ('ready','failed','retained')
              AND ($1::uuid IS NULL OR r.instance_id=$1)
        ), previous AS (
            SELECT u.*,lag(sampled_at) OVER w previous_at,
                lag(transactions) OVER w previous_transactions,
                lag(stats_reset) OVER w previous_reset
            FROM tenant_usage_samples u JOIN eligible e ON e.id=u.tenant_id
            WHERE sampled_at >= $2-30000
            WINDOW w AS (PARTITION BY tenant_id ORDER BY sampled_at)
        ), readings AS (
            SELECT DISTINCT ON (tenant_id,sampled_at/10000)
                tenant_id,sampled_at/10000*10000 bucket,
                database_bytes,connections,active_connections,
                CASE WHEN sampled_at-previous_at BETWEEN 1 AND 30000
                    AND stats_reset IS NOT DISTINCT FROM previous_reset
                    AND transactions >= previous_transactions
                THEN (transactions-previous_transactions)::double precision
                     / ((sampled_at-previous_at)::double precision/1000) END rate
            FROM previous WHERE sampled_at >= $2
            ORDER BY tenant_id,sampled_at/10000,sampled_at DESC
        ), totals AS (
            SELECT bucket sampled_at,count(database_bytes) reporting_tenants,
                count(rate) rate_reporting_tenants,
                sum(database_bytes)::bigint database_bytes,
                sum(connections)::bigint connections,
                sum(active_connections)::bigint active_connections,
                sum(rate) rate
            FROM readings GROUP BY bucket
        ), rolled AS (
            SELECT max(sampled_at) sampled_at,min(reporting_tenants) reporting_tenants,
                min(rate_reporting_tenants) rate_reporting_tenants,
                CASE WHEN count(database_bytes)=count(*) THEN avg(database_bytes)::bigint END database_bytes,
                CASE WHEN count(connections)=count(*) THEN avg(connections)::bigint END connections,
                CASE WHEN count(active_connections)=count(*) THEN avg(active_connections)::bigint END active_connections,
                CASE WHEN count(rate)=count(*) THEN avg(rate) END rate
            FROM totals GROUP BY sampled_at/$3
        ) SELECT jsonb_build_object(
            'eligible_tenants',(SELECT count(*) FROM eligible),
            'samples',COALESCE((SELECT jsonb_agg(to_jsonb(t) ORDER BY sampled_at) FROM rolled t),'[]'::jsonb))
        "#).bind(scope.instance_id).bind(cutoff).bind(bucket).fetch_one(&s.db).await?;
    Ok(Json(history))
}

fn now_millis() -> i64 {
    (OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64
}

pub(crate) async fn sampler(s: Shared) {
    // Serve the previous snapshot immediately after a restart, with its real age.
    if let Ok(rows) = sqlx::query_as::<_, Usage>("SELECT DISTINCT ON (tenant_id) tenant_id,sampled_at,database_bytes,connections,active_connections,transactions,stats_reset FROM tenant_usage_samples WHERE sampled_at >= $1 ORDER BY tenant_id,sampled_at DESC")
        .bind(now_millis() - usage_range::RETENTION_MS).fetch_all(&s.db).await {
        s.usage.lock().await.rows = rows;
    }
    loop {
        if collect(&s).await.is_err() {
            tracing::warn!("usage sampling failed; retaining previous readings");
        } else {
            s.health.tick("usage-sampler");
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

pub(crate) async fn collect(s: &Shared) -> Result<(), ApiError> {
    let tenants = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT t.id,c.database_url FROM tenants t JOIN tenant_configuration c ON c.tenant_id=t.id WHERE t.lifecycle_state IN ('ready','failed','retained') ORDER BY t.domain",
    ).fetch_all(&s.db).await.map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "Usage inventory unavailable.".into()))?;
    let rows = stream::iter(tenants.into_iter().map(|(tenant_id, url)| async move {
        let sample = tokio::time::timeout(Duration::from_secs(4), async {
            let mut connection = PgConnection::connect(&url).await?;
            let values = sqlx::query_as::<_, (i64, i64, i64, i64, Option<String>)>(
                "SELECT pg_database_size(current_database()), (SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid()), (SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() AND state='active'), xact_commit+xact_rollback, stats_reset::text FROM pg_stat_database WHERE datname=current_database()",
            ).fetch_one(&mut connection).await;
            // Explicit close also on a query failure; timeout drops the connection.
            let _ = connection.close().await;
            values
        }).await;
        let values = sample.ok().and_then(Result::ok);
        Usage {
            tenant_id,
            rate: None,
            sampled_at: (OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64,
            database_bytes: values.as_ref().map(|v| v.0),
            connections: values.as_ref().map(|v| v.1),
            active_connections: values.as_ref().map(|v| v.2),
            transactions: values.as_ref().map(|v| v.3),
            stats_reset: values.and_then(|v| v.4),
        }
    })).buffer_unordered(8).collect::<Vec<_>>().await;
    let mut tx = s.db.begin().await?;
    // One fixed insert; samples contain identifiers and numbers, never URLs.
    sqlx::query("INSERT INTO tenant_usage_samples SELECT tenant_id,sampled_at,database_bytes,connections,active_connections,transactions,stats_reset FROM jsonb_populate_recordset(NULL::tenant_usage_samples,$1) ON CONFLICT DO NOTHING")
        .bind(SqlJson(&rows)).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM tenant_usage_samples WHERE sampled_at < $1")
        .bind(now_millis() - usage_range::RETENTION_MS)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    s.usage.lock().await.rows = rows;
    Ok(())
}

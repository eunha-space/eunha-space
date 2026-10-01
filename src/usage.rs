//! Operator-only, bounded sampling. Connection strings and driver errors never leave here.
use super::*;
use futures::{StreamExt, stream};
use sqlx::{Connection, PgConnection};
use std::time::Instant;

#[derive(Clone, Serialize)]
pub(crate) struct Usage {
    tenant_id: Uuid,
    sampled_at: i64,
    database_bytes: Option<i64>,
    connections: Option<i64>,
    active_connections: Option<i64>,
    transactions: Option<i64>,
    stats_reset: Option<String>,
}

#[derive(Default)]
pub(crate) struct Cache {
    collected: Option<Instant>,
    rows: Vec<Usage>,
}

pub(crate) async fn usage(
    State(s): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Vec<Usage>>, ApiError> {
    operator(&headers, &s.db).await?;
    // Coalesce concurrent viewers and cap collection to once per ten seconds.
    let mut cache = s.usage.lock().await;
    if cache
        .collected
        .is_some_and(|at| at.elapsed() < Duration::from_secs(10))
    {
        return Ok(Json(cache.rows.clone()));
    }
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
            sampled_at: (OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64,
            database_bytes: values.as_ref().map(|v| v.0),
            connections: values.as_ref().map(|v| v.1),
            active_connections: values.as_ref().map(|v| v.2),
            transactions: values.as_ref().map(|v| v.3),
            stats_reset: values.and_then(|v| v.4),
        }
    })).buffer_unordered(8).collect::<Vec<_>>().await;
    cache.collected = Some(Instant::now());
    cache.rows = rows.clone();
    Ok(Json(rows))
}

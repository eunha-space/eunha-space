//! Moving a served tenant from one runtime to another.
//!
//! A tenant's runtime decides which eunha release serves it, and so which
//! release channel it is in. A move makes before it breaks: the tenant's
//! database is migrated with the new runtime's release, the new runtime starts
//! serving it, the tenant becomes the new runtime's, its hostnames are routed
//! to that runtime's origin, and only then does the old runtime let it go. For
//! the minutes in between both processes serve it, which eunha's job queue and
//! scheduler allow: they lease what they run in PostgreSQL.
//!
//! The steps run as a lifecycle job and are each safe to run again. Until the
//! tenant switches runtimes a failure undoes the move; after, the move stays
//! open, holding both runtimes against releases, until an operator retries it.
//! *docs/tenant-lifecycle.md* has the rules.

use std::time::Duration;

use anyhow::{Context, Result};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    ApiError, Shared, admin_user, connection_budget_error, healthcheck_url, jobs::Next,
    migrate_tenant_database_on, reload_runtime_on, render_tenant_on, unrender_tenant_from,
    verify_offline_at,
};

/// The steps of a move, in order.
pub(crate) const STEPS: [&str; 5] = ["migrate", "start", "switch", "route", "stop"];

/// How long after every hostname points at the new runtime the old one keeps
/// serving: Cloudflare's edge takes a moment to stop sending requests to the
/// old origin.
const ROUTE_GRACE: time::Duration = time::Duration::minutes(2);

/// How many runs a step gets before the move gives up on it.
const STEP_ATTEMPTS: i32 = 3;
// One flaky reload must not undo a move.
const _: () = assert!(STEP_ATTEMPTS > 1);

/// How long a step waits before it asks again.
const WAIT: Duration = Duration::from_secs(30);

/// Whether an open move holds `runtime_id`: a release of either runtime would
/// migrate or serve the moving tenant with a binary the move did not check.
pub(crate) async fn holds_runtime(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    runtime_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM tenant_moves WHERE finished_at IS NULL AND $1 IN (from_runtime_id,to_runtime_id))",
    )
    .bind(runtime_id)
    .fetch_one(&mut **tx)
    .await
}

/// Whether `tenant_id` has a move open, which other changes to it wait out.
pub(crate) async fn is_moving(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM tenant_moves WHERE tenant_id=$1 AND finished_at IS NULL)",
    )
    .bind(tenant_id)
    .fetch_one(&mut **tx)
    .await
}

#[derive(Deserialize)]
pub(crate) struct MoveRequest {
    runtime_id: Uuid,
}

/// A move, as the console shows it.
#[derive(Serialize, sqlx::FromRow)]
pub(crate) struct MoveSummary {
    id: Uuid,
    operation_id: Uuid,
    from_runtime_id: Uuid,
    from_runtime: String,
    to_runtime_id: Uuid,
    to_runtime: String,
    status: String,
    step: String,
    cancel_requested: bool,
    error: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    finished_at: Option<OffsetDateTime>,
}

pub(crate) async fn latest(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Option<MoveSummary>>, ApiError> {
    admin_user(&headers, &s.db).await?;
    Ok(Json(
        sqlx::query_as("SELECT m.id,m.operation_id,m.from_runtime_id,f.name from_runtime,m.to_runtime_id,t.name to_runtime,m.status,m.step,m.cancel_requested,m.error,m.created_at,m.finished_at FROM tenant_moves m JOIN runtimes f ON f.id=m.from_runtime_id JOIN runtimes t ON t.id=m.to_runtime_id WHERE m.tenant_id=$1 ORDER BY m.created_at DESC LIMIT 1")
            .bind(id)
            .fetch_optional(&s.db)
            .await?,
    ))
}

fn conflict(message: impl Into<String>) -> ApiError {
    ApiError(StatusCode::CONFLICT, message.into())
}

/// A runtime as a move needs it.
#[derive(sqlx::FromRow)]
struct Target {
    name: String,
    address: String,
    deployed_ref: Option<String>,
}

/// Start moving a tenant to another runtime.
pub(crate) async fn start(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<MoveRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let (admin_user_id, _) = admin_user(&headers, &s.db).await?;
    let configured_host = std::env::var("CONTROL_RUNTIME_HOST").unwrap_or_default();
    let (move_id, operation_id) = begin(
        &s,
        id,
        request.runtime_id,
        Some(admin_user_id),
        &configured_host,
    )
    .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({"id": move_id, "operation_id": operation_id})),
    ))
}

/// Admit a move of `id` to `to_runtime_id` and queue it, for this control
/// plane on `configured_host`. Returns the move and its operation.
pub(crate) async fn begin(
    s: &Shared,
    id: Uuid,
    to_runtime_id: Uuid,
    admin_user_id: Option<Uuid>,
    configured_host: &str,
) -> Result<(Uuid, Uuid), ApiError> {
    let request = MoveRequest {
        runtime_id: to_runtime_id,
    };
    let mut tx = s.db.begin().await?;
    // The lock release admission takes: either a release of one of these
    // runtimes is running and this move waits for it, or this move exists and
    // the release is refused.
    sqlx::query("SELECT pg_advisory_xact_lock(60001001)")
        .execute(&mut *tx)
        .await?;
    let tenant = sqlx::query_as::<_, (String, String, String, Uuid)>(
        "SELECT domain,lifecycle_state,desired_state,runtime_id FROM tenants WHERE id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Tenant not found.".into()))?;
    let (domain, lifecycle, desired, from_runtime_id) = tenant;
    if lifecycle != "ready" || desired != "running" {
        return Err(conflict("Only a running tenant can move."));
    }
    if from_runtime_id == request.runtime_id {
        return Err(conflict("The tenant is already on that runtime."));
    }
    let runtime = |runtime_id: Uuid| {
        sqlx::query_as::<_, Target>(
            "SELECT r.name,i.address,r.deployed_ref FROM runtimes r JOIN instances i ON i.id=r.instance_id WHERE r.id=$1",
        )
        .bind(runtime_id)
    };
    let from = runtime(from_runtime_id).fetch_one(&mut *tx).await?;
    let to = runtime(request.runtime_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Runtime not found.".into()))?;
    // The files are written, and the runtimes signalled, by this host.
    if configured_host.is_empty()
        || from.address != configured_host
        || to.address != configured_host
    {
        return Err(conflict(
            "Both runtimes must be on the instance this control plane manages.",
        ));
    }
    if sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM lifecycle_jobs WHERE tenant_id=$1 AND finished_at IS NULL)",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?
    {
        return Err(conflict(
            "The tenant has lifecycle work in progress; move it once that finishes.",
        ));
    }
    if is_moving(&mut tx, id).await? {
        return Err(conflict("The tenant is already moving."));
    }
    // An existing domain is routed by infrastructure to the runtime it was
    // set up on, which a move cannot change.
    if sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM tenant_domains WHERE tenant_id=$1 AND kind='legacy' AND status<>'removing')",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?
    {
        return Err(conflict(
            "Manage the tenant's existing domain with Cloudflare first, so that it can be routed to another runtime.",
        ));
    }
    if s.cloudflare.is_none()
        && sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM tenant_domains WHERE tenant_id=$1 AND kind IN ('provided','custom') AND status<>'removing')",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?
    {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Cloudflare for SaaS is not configured, so the tenant's hostnames cannot be routed to another runtime.".into(),
        ));
    }
    // Eunha refuses a tenants directory with no tenants in it, so the last
    // tenant would go on being served by the runtime it left.
    let others = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM tenants WHERE runtime_id=$1 AND id<>$2 AND lifecycle_state NOT IN ('deprovisioning','retained','purging','purged')",
    )
    .bind(from_runtime_id)
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    if others == 0 {
        return Err(conflict(format!(
            "{domain} is the last tenant on {}, and a runtime cannot be left with none.",
            from.name
        )));
    }
    release_order(&mut tx, &from, &to).await?;
    // The new runtime's tenants and this one must fit its share of PostgreSQL.
    let (declared, accepted) = sqlx::query_as::<_, (i64, i64)>(
        "SELECT (SELECT COALESCE(SUM(c.database_pool_max_connections),0) FROM tenants peer JOIN tenant_configuration c ON c.tenant_id=peer.id WHERE (peer.runtime_id=$1 OR peer.id=$2) AND peer.lifecycle_state NOT IN ('deprovisioning','retained','purging','purged'))::bigint,current_setting('max_connections')::bigint-current_setting('superuser_reserved_connections')::bigint",
    )
    .bind(request.runtime_id)
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    if let Some(message) = connection_budget_error(declared, accepted) {
        return Err(conflict(format!(
            "{} cannot take the tenant: {message}.",
            to.name
        )));
    }

    let operation_id = Uuid::new_v4();
    sqlx::query("INSERT INTO operations(id,tenant_id,runtime_id,kind,status,requested_by,detail,started_at) VALUES($1,$2,$3,'runtime_move','running',$4,jsonb_build_object('from',$5::text,'to',$6::text),now())")
        .bind(operation_id)
        .bind(id)
        .bind(request.runtime_id)
        .bind(admin_user_id)
        .bind(&from.name)
        .bind(&to.name)
        .execute(&mut *tx)
        .await?;
    let move_id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenant_moves(id,tenant_id,operation_id,from_runtime_id,to_runtime_id) VALUES($1,$2,$3,$4,$5)")
        .bind(move_id)
        .bind(id)
        .bind(operation_id)
        .bind(from_runtime_id)
        .bind(request.runtime_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO operation_events(operation_id,message) VALUES($1,$2)")
        .bind(operation_id)
        .bind(format!(
            "Moving {domain} from {} to {}.",
            from.name, to.name
        ))
        .execute(&mut *tx)
        .await?;
    crate::jobs::enqueue(&mut tx, id, "runtime_move").await?;
    tx.commit().await?;
    Ok((move_id, operation_id))
}

/// A tenant may move to a runtime on its own release or a newer one. Its
/// database is migrated by the new runtime's release; one older than the
/// database would serve a schema it does not know.
async fn release_order(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    from: &Target,
    to: &Target,
) -> Result<(), ApiError> {
    let Some(to_ref) = &to.deployed_ref else {
        return Err(conflict(format!(
            "{} has never been released; release it first.",
            to.name
        )));
    };
    if from.deployed_ref.as_ref() == Some(to_ref) {
        return Ok(());
    }
    let Some(from_ref) = &from.deployed_ref else {
        return Ok(());
    };
    let built = |reference: &str| {
        sqlx::query_scalar::<_, OffsetDateTime>(
            "SELECT created_at FROM eunha_releases WHERE git_sha=$1",
        )
        .bind(reference.to_owned())
    };
    let from_built = built(from_ref).fetch_optional(&mut **tx).await?;
    let to_built = built(to_ref).fetch_optional(&mut **tx).await?;
    match (from_built, to_built) {
        (Some(from_built), Some(to_built)) if to_built >= from_built => Ok(()),
        (Some(_), Some(_)) => Err(conflict(format!(
            "{} runs an older release than {}; release it first.",
            to.name, from.name
        ))),
        _ => Err(conflict(format!(
            "Which of {} and {} runs the newer release is not recorded; release {} first.",
            from.name, to.name, to.name
        ))),
    }
}

/// Ask a move that has not switched runtimes yet to stop and undo itself.
pub(crate) async fn cancel(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    admin_user(&headers, &s.db).await?;
    let mut tx = s.db.begin().await?;
    let changed = sqlx::query(
        "UPDATE tenant_moves SET cancel_requested=true,updated_at=now() WHERE tenant_id=$1 AND status='running' AND step IN ('migrate','start','switch')",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(conflict(
            "Only a move that has not switched runtimes yet can be cancelled.",
        ));
    }
    crate::jobs::enqueue(&mut tx, id, "runtime_move").await?;
    tx.commit().await?;
    Ok(StatusCode::ACCEPTED)
}

/// Take up a move that failed after the tenant switched runtimes, from the
/// step it failed at.
pub(crate) async fn retry(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    admin_user(&headers, &s.db).await?;
    let mut tx = s.db.begin().await?;
    let operation_id = sqlx::query_scalar::<_, Uuid>(
        "UPDATE tenant_moves SET status='running',error=NULL,updated_at=now() WHERE tenant_id=$1 AND status='failed' AND finished_at IS NULL RETURNING operation_id",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| conflict("No part-way move to retry."))?;
    sqlx::query("UPDATE operations SET status='running',error=NULL,finished_at=NULL WHERE id=$1")
        .bind(operation_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO operation_events(operation_id,message) VALUES($1,'Retried by an operator.')",
    )
    .bind(operation_id)
    .execute(&mut *tx)
    .await?;
    crate::jobs::enqueue(&mut tx, id, "runtime_move").await?;
    tx.commit().await?;
    Ok(StatusCode::ACCEPTED)
}

/// The tenant's running move.
#[derive(sqlx::FromRow)]
struct Running {
    id: Uuid,
    tenant_id: Uuid,
    operation_id: Uuid,
    from_runtime_id: Uuid,
    to_runtime_id: Uuid,
    step: String,
    cancel_requested: bool,
    routed_at: Option<OffsetDateTime>,
    domain: String,
    lifecycle_state: String,
    /// The runtime the tenant belongs to now: `from_runtime_id` until switch.
    runtime_id: Uuid,
    from_bind_address: String,
    to_bind_address: String,
    to_origin_hostname: String,
}

async fn running(db: &sqlx::PgPool, tenant_id: Uuid) -> Result<Option<Running>> {
    Ok(sqlx::query_as(
        "SELECT m.id,m.tenant_id,m.operation_id,m.from_runtime_id,m.to_runtime_id,m.step,m.cancel_requested,m.routed_at,t.domain,t.lifecycle_state,t.runtime_id,f.bind_address from_bind_address,r.bind_address to_bind_address,r.origin_hostname to_origin_hostname FROM tenant_moves m JOIN tenants t ON t.id=m.tenant_id JOIN runtimes f ON f.id=m.from_runtime_id JOIN runtimes r ON r.id=m.to_runtime_id WHERE m.tenant_id=$1 AND m.status='running'",
    )
    .bind(tenant_id)
    .fetch_optional(db)
    .await?)
}

async fn event(db: &sqlx::PgPool, operation_id: Uuid, message: &str) {
    let message: String = message.chars().take(500).collect();
    let _ = sqlx::query("INSERT INTO operation_events(operation_id,message) VALUES($1,$2)")
        .bind(operation_id)
        .bind(message)
        .execute(db)
        .await;
}

/// Whether a release is running on either runtime, which a step waits out.
async fn releasing(db: &sqlx::PgPool, change: &Running) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM operations WHERE kind='runtime_release' AND status IN ('pending','running') AND runtime_id IN ($1,$2))",
    )
    .bind(change.from_runtime_id)
    .bind(change.to_runtime_id)
    .fetch_one(db)
    .await?)
}

/// Run the tenant's move, step by step. `failures` is how many runs in a row
/// have failed, which decides when a failing step is given up on.
pub(crate) async fn run(state: &Shared, tenant_id: Uuid, failures: i32) -> Next {
    let change = match running(&state.db, tenant_id).await {
        Ok(Some(change)) => change,
        // Finished, failed or cancelled: nothing left to do.
        Ok(None) => return Next::Done,
        Err(_) => return Next::Retry("The move could not be read; retrying.".into(), WAIT),
    };
    let switched = change.runtime_id == change.to_runtime_id;
    if change.cancel_requested && !switched {
        undo(
            state,
            &change,
            "cancelled",
            "Cancelled by an operator; the tenant stays where it was.",
        )
        .await;
        return Next::Done;
    }
    let position = |name: &str| STEPS.iter().position(|step| *step == name).unwrap_or(0);
    for step in STEPS.iter().skip(position(&change.step)) {
        // Every step reads the move afresh: the previous one changed it.
        let change = match running(&state.db, tenant_id).await {
            Ok(Some(change)) => change,
            Ok(None) => return Next::Done,
            Err(_) => return Next::Retry("The move could not be read; retrying.".into(), WAIT),
        };
        let outcome =
            tokio::time::timeout(Duration::from_secs(600), run_step(state, &change, step))
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!("timed out")));
        match outcome {
            Ok(true) => {
                let next = STEPS.get(position(step) + 1).copied().unwrap_or("done");
                let _ = sqlx::query("UPDATE tenant_moves SET step=$2,updated_at=now() WHERE id=$1")
                    .bind(change.id)
                    .bind(next)
                    .execute(&state.db)
                    .await;
            }
            Ok(false) => return Next::After(WAIT),
            Err(error) => {
                let message = format!("{step} failed: {error}");
                if failures + 1 < STEP_ATTEMPTS {
                    return Next::Retry(format!("{step} failed; retrying."), WAIT);
                }
                fail(state, &change, &message).await;
                return Next::Done;
            }
        }
    }
    finish(state, tenant_id).await;
    Next::Done
}

async fn run_step(state: &Shared, change: &Running, step: &str) -> Result<bool> {
    let switched = change.runtime_id == change.to_runtime_id;
    // Until the switch, a tenant that stopped running — taken offline, say —
    // has nothing to move, and its new runtime must not serve it.
    if !switched && change.lifecycle_state != "ready" {
        anyhow::bail!("the tenant is no longer running");
    }
    if matches!(step, "migrate" | "start" | "switch") && releasing(&state.db, change).await? {
        return Ok(false);
    }
    match step {
        "migrate" => {
            migrate_tenant_database_on(state, tenant_id(change), Some(change.to_runtime_id))
                .await
                .context("the database could not be migrated with the new runtime's release")?;
            event(
                &state.db,
                change.operation_id,
                "Migrated the database with the new runtime's release.",
            )
            .await;
            Ok(true)
        }
        "start" => {
            serve_on_target(state, change).await?;
            event(
                &state.db,
                change.operation_id,
                "The new runtime serves the tenant.",
            )
            .await;
            Ok(true)
        }
        "switch" => {
            // Rendered again: the configuration may have changed since start.
            serve_on_target(state, change).await?;
            sqlx::query(
                "UPDATE tenants SET runtime_id=$2,updated_at=now() WHERE id=$1 AND runtime_id=$3",
            )
            .bind(tenant_id(change))
            .bind(change.to_runtime_id)
            .bind(change.from_runtime_id)
            .execute(&state.db)
            .await?;
            event(
                &state.db,
                change.operation_id,
                "The tenant now belongs to the new runtime; routing its hostnames there.",
            )
            .await;
            Ok(true)
        }
        "route" => route(state, change).await,
        "stop" => {
            unrender_tenant_from(state, tenant_id(change), Some(change.from_runtime_id)).await?;
            reload_runtime_on(
                state,
                tenant_id(change),
                Some(change.from_runtime_id),
                false,
            )
            .await?;
            verify_offline_at(state, &change.from_bind_address, &change.domain)
                .await
                .context("the old runtime still serves the tenant")?;
            event(
                &state.db,
                change.operation_id,
                "The old runtime no longer serves the tenant.",
            )
            .await;
            Ok(true)
        }
        _ => anyhow::bail!("unknown step"),
    }
}

fn tenant_id(change: &Running) -> Uuid {
    change.tenant_id
}

async fn serve_on_target(state: &Shared, change: &Running) -> Result<()> {
    let tenant = tenant_id(change);
    render_tenant_on(state, tenant, Some(change.to_runtime_id)).await?;
    reload_runtime_on(state, tenant, Some(change.to_runtime_id), true).await?;
    healthcheck_url(
        state,
        &change.to_bind_address,
        &change.domain,
        &change.domain,
    )
    .await
}

/// Wait until every hostname of the tenant is routed to the new runtime's
/// origin, and a little longer for Cloudflare's edge to follow.
async fn route(state: &Shared, change: &Running) -> Result<bool> {
    // A tenant taken offline meanwhile has no hostnames to route.
    if change.lifecycle_state == "ready" {
        let unrouted = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM tenant_domains WHERE tenant_id=$1 AND kind IN ('provided','custom') AND status<>'removing' AND routed_origin_hostname IS DISTINCT FROM $2",
        )
        .bind(tenant_id(change))
        .bind(&change.to_origin_hostname)
        .fetch_one(&state.db)
        .await?;
        if unrouted > 0 {
            return Ok(false);
        }
    }
    let Some(routed_at) = change.routed_at else {
        sqlx::query("UPDATE tenant_moves SET routed_at=now(),updated_at=now() WHERE id=$1")
            .bind(change.id)
            .execute(&state.db)
            .await?;
        event(
            &state.db,
            change.operation_id,
            "Every hostname is routed to the new runtime.",
        )
        .await;
        return Ok(false);
    };
    Ok(routed_at + ROUTE_GRACE <= OffsetDateTime::now_utc())
}

async fn finish(state: &Shared, tenant_id: Uuid) {
    let Ok(mut tx) = state.db.begin().await else {
        return;
    };
    let operation = sqlx::query_scalar::<_, Uuid>(
        "UPDATE tenant_moves SET status='succeeded',step='done',finished_at=now(),updated_at=now() WHERE tenant_id=$1 AND status='running' RETURNING operation_id",
    )
    .bind(tenant_id)
    .fetch_optional(&mut *tx)
    .await;
    if let Ok(Some(operation_id)) = operation {
        let _ =
            sqlx::query("UPDATE operations SET status='succeeded',finished_at=now() WHERE id=$1")
                .bind(operation_id)
                .execute(&mut *tx)
                .await;
        let _ =
            sqlx::query("INSERT INTO operation_events(operation_id,message) VALUES($1,'Moved.')")
                .bind(operation_id)
                .execute(&mut *tx)
                .await;
    }
    let _ = tx.commit().await;
}

/// A step gave up. Before the switch the move is undone and finished; after
/// it, the move stays open for an operator.
async fn fail(state: &Shared, change: &Running, message: &str) {
    if change.runtime_id != change.to_runtime_id {
        undo(state, change, "failed", message).await;
        return;
    }
    let _ = sqlx::query(
        "UPDATE tenant_moves SET status='failed',error=$2,updated_at=now() WHERE id=$1",
    )
    .bind(change.id)
    .bind(message)
    .execute(&state.db)
    .await;
    let _ =
        sqlx::query("UPDATE operations SET status='failed',error=$2,finished_at=now() WHERE id=$1")
            .bind(change.operation_id)
            .bind(message)
            .execute(&state.db)
            .await;
    event(
        &state.db,
        change.operation_id,
        &format!("{message} The tenant is on its new runtime; retry the move to finish it."),
    )
    .await;
}

/// Take the tenant off its new runtime, which it never switched to, and end
/// the move. Its database stays migrated: the release that did it is the one
/// its runtime gets next.
async fn undo(state: &Shared, change: &Running, status: &str, message: &str) {
    let tenant = tenant_id(change);
    let mut message = message.to_owned();
    let cleaned = match unrender_tenant_from(state, tenant, Some(change.to_runtime_id)).await {
        Ok(()) => reload_runtime_on(state, tenant, Some(change.to_runtime_id), false)
            .await
            .is_ok(),
        Err(_) => false,
    };
    if !cleaned {
        message.push_str(" The new runtime could not be told to let the tenant go; check it.");
    }
    let _ = sqlx::query(
        "UPDATE tenant_moves SET status=$2,error=$3,finished_at=now(),updated_at=now() WHERE id=$1",
    )
    .bind(change.id)
    .bind(status)
    .bind(&message)
    .execute(&state.db)
    .await;
    let _ =
        sqlx::query("UPDATE operations SET status='failed',error=$2,finished_at=now() WHERE id=$1")
            .bind(change.operation_id)
            .bind(&message)
            .execute(&state.db)
            .await;
    event(&state.db, change.operation_id, &message).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_steps_make_before_they_break() {
        assert_eq!(STEPS, ["migrate", "start", "switch", "route", "stop"]);
    }
}

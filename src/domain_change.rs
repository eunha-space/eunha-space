//! Changing a served instance's domain.
//!
//! An instance's accounts are ActivityPub actors under its domain, so moving
//! it to another is a migration, not a setting: the addresses it has handed
//! out are rewritten, it is served under the new domain alone, and every
//! account's followers are sent a Move to the new actor, which they honour
//! because each new actor names its old self in `alsoKnownAs`. The old domain
//! serves nothing afterwards; the Moves are signed with keys the followers'
//! servers already hold, so it does not have to. *docs/tenant-lifecycle.md*
//! has the steps and why they are in that order.
//!
//! The steps are durable in the same way provisioning's are: a Temporal
//! workflow runs them in order, a step that is not ready yet asks to wait,
//! and PostgreSQL's `domain_changes` row is the record everyone reads.

use std::{process::Stdio, time::Duration};

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
    ApiError, ProvisionStepOutcome, Shared, customer, enqueue_temporal_dispatch,
    healthcheck_provisioned_tenant, is_safe_domain, is_valid_domain, operator,
    reload_provisioned_runtime, render_provisioned_tenant,
};

/// The steps of a domain change, in order.
pub(crate) const STEPS: [&str; 6] = ["attach", "rename", "switch", "move", "deliver", "release"];

/// How long a new custom domain may take to connect before the change is
/// given up on and the instance stays where it is.
const ATTACH_WINDOW: &str = "7 days";

/// How long the Moves are given before a follower's server that has not taken
/// one is given up on. Long enough to outlast a server's maintenance window;
/// the change is not finished until they are through or given up on.
const MOVE_GIVE_UP_AFTER: &str = "12h";

/// What a customer or an operator asks for: a provided subdomain, or a custom
/// domain of their own.
#[derive(Deserialize)]
pub(crate) struct DomainChangeRequest {
    #[serde(default)]
    subdomain: String,
    custom_domain: Option<String>,
}

/// A domain change, as the consoles show it.
#[derive(Serialize, sqlx::FromRow)]
pub(crate) struct DomainChangeSummary {
    id: Uuid,
    from_domain: String,
    to_domain: String,
    status: String,
    step: String,
    pending: Option<i32>,
    given_up: Option<i32>,
    error: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    finished_at: Option<OffsetDateTime>,
}

/// The latest domain change of `tenant_id`, if it has had one.
pub(crate) async fn latest(
    db: &sqlx::PgPool,
    tenant_id: Uuid,
) -> Result<Option<DomainChangeSummary>> {
    Ok(sqlx::query_as(
        "SELECT id,from_domain,to_domain,status,step,pending,given_up,error,created_at,finished_at FROM domain_changes WHERE tenant_id=$1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(tenant_id)
    .fetch_optional(db)
    .await?)
}

/// The hostnames a request names: the new primary domain first, then any
/// provided siblings it is served under too.
fn requested_hostnames(
    s: &Shared,
    request: &DomainChangeRequest,
) -> Result<(String, Vec<String>), ApiError> {
    let bad = |message: &str| ApiError(StatusCode::BAD_REQUEST, message.into());
    let custom = request
        .custom_domain
        .as_deref()
        .map(|value| value.trim().trim_end_matches('.').to_ascii_lowercase())
        .filter(|value| !value.is_empty());
    if let Some(custom) = custom {
        let provided = s
            .provided_domain_suffixes
            .iter()
            .any(|suffix| custom == *suffix || custom.ends_with(&format!(".{suffix}")));
        if !is_valid_domain(&custom) || provided {
            return Err(bad("Enter a valid externally owned custom domain."));
        }
        return Ok(("custom".into(), vec![custom]));
    }
    let label = request.subdomain.trim().to_ascii_lowercase();
    if label.is_empty()
        || label.len() > 63
        || label.starts_with('-')
        || label.ends_with('-')
        || !label
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(bad("Enter a valid subdomain or custom domain."));
    }
    if s.provided_domain_suffixes.is_empty() {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "No provided domain suffix is configured.".into(),
        ));
    }
    Ok((
        "provided".into(),
        s.provided_domain_suffixes
            .iter()
            .map(|suffix| format!("{label}.{suffix}"))
            .collect(),
    ))
}

/// Start changing `tenant_id`'s domain. The new domain is added as a pending
/// hostname and an alias at once, so that it is served, and can be checked,
/// before anything about the instance's identity changes.
async fn start(
    s: &Shared,
    tenant_id: Uuid,
    request: DomainChangeRequest,
    operator_id: Option<Uuid>,
    customer_id: Option<Uuid>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let (kind, hostnames) = requested_hostnames(s, &request)?;
    let to_domain = hostnames[0].clone();
    if kind == "provided" {
        let label = to_domain.split('.').next().unwrap_or_default();
        if sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM reserved_subdomains WHERE label=$1)",
        )
        .bind(label)
        .fetch_one(&s.db)
        .await?
        {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "That subdomain is reserved.".into(),
            ));
        }
    }
    let cname_target = s
        .cloudflare
        .as_ref()
        .ok_or_else(|| {
            ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "Cloudflare for SaaS is not configured.".into(),
            )
        })?
        .cname_target
        .clone();

    let mut tx = s.db.begin().await?;
    let tenant = sqlx::query_as::<_, (String, String, Option<Uuid>, Uuid)>(
        "SELECT domain,lifecycle_state,customer_id,runtime_id FROM tenants WHERE id=$1 FOR UPDATE",
    )
    .bind(tenant_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Instance not found".into()))?;
    let (from_domain, lifecycle, owner, runtime_id) = tenant;
    if customer_id.is_some() && owner != customer_id {
        return Err(ApiError(StatusCode::NOT_FOUND, "Instance not found".into()));
    }
    if lifecycle != "ready" {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Only a running instance can change its domain.".into(),
        ));
    }
    if hostnames.contains(&from_domain) {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "The instance already has that domain.".into(),
        ));
    }
    let operation_id = Uuid::new_v4();
    sqlx::query("INSERT INTO operations(id,tenant_id,runtime_id,kind,status,requested_by,requested_by_customer,detail,started_at) VALUES($1,$2,$3,'domain_change','running',$4,$5,jsonb_build_object('from',$6::text,'to',$7::text),now())")
        .bind(operation_id).bind(tenant_id).bind(runtime_id).bind(operator_id).bind(customer_id)
        .bind(&from_domain).bind(&to_domain).execute(&mut *tx).await?;
    let change_id = Uuid::new_v4();
    sqlx::query("INSERT INTO domain_changes(id,tenant_id,operation_id,from_domain,to_domain) VALUES($1,$2,$3,$4,$5)")
        .bind(change_id).bind(tenant_id).bind(operation_id).bind(&from_domain).bind(&to_domain)
        .execute(&mut *tx).await
        .map_err(|error| if error.as_database_error().and_then(|e| e.constraint()) == Some("domain_changes_one_running") {
            ApiError(StatusCode::CONFLICT, "The instance is already changing its domain.".into())
        } else {
            ApiError::from(error)
        })?;
    for hostname in &hostnames {
        // The first check waits until the workflow has rendered the new
        // hostname, so that it is not counted a failure for not serving yet.
        sqlx::query("INSERT INTO tenant_domains(id,tenant_id,hostname,kind,status,is_primary,cname_target,next_check_at) VALUES($1,$2,$3,$4,'pending',false,$5,now()+interval '1 minute')")
            .bind(Uuid::new_v4()).bind(tenant_id).bind(hostname).bind(&kind).bind(&cname_target)
            .execute(&mut *tx).await
            .map_err(|error| if error.as_database_error().and_then(|e| e.constraint()) == Some("tenant_domains_hostname_key") {
                ApiError(StatusCode::CONFLICT, "That domain is already managed.".into())
            } else {
                ApiError::from(error)
            })?;
    }
    sqlx::query("UPDATE tenants SET aliases=ARRAY(SELECT DISTINCT unnest(aliases || $1::text[])),config_revision=config_revision+1,updated_at=now() WHERE id=$2")
        .bind(&hostnames).bind(tenant_id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO operation_events(operation_id,message) VALUES($1,$2)")
        .bind(operation_id)
        .bind(format!(
            "Moving from {from_domain} to {to_domain}: waiting for {to_domain} to connect."
        ))
        .execute(&mut *tx)
        .await?;
    enqueue_temporal_dispatch(&mut tx, tenant_id, "domain_change").await?;
    tx.commit().await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({"id": change_id, "operation_id": operation_id})),
    ))
}

pub(crate) async fn customer_start(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<DomainChangeRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let (customer_id, _) = customer(&headers, &s.db).await?;
    start(&s, id, request, None, Some(customer_id)).await
}

pub(crate) async fn operator_start(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<DomainChangeRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let (operator_id, _) = operator(&headers, &s.db).await?;
    start(&s, id, request, Some(operator_id), None).await
}

/// Take up again a change that failed after it began moving the instance.
/// Every step can run again, so it continues from the step that failed.
pub(crate) async fn operator_retry(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    operator(&headers, &s.db).await?;
    let mut tx = s.db.begin().await?;
    let operation_id = sqlx::query_scalar::<_, Uuid>(
        "UPDATE domain_changes SET status='running',error=NULL,finished_at=NULL,updated_at=now() WHERE id=(SELECT id FROM domain_changes WHERE tenant_id=$1 ORDER BY created_at DESC LIMIT 1) AND status='failed' AND step<>'attach' RETURNING operation_id",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| if error.as_database_error().and_then(|e| e.constraint()) == Some("domain_changes_one_running") {
        ApiError(StatusCode::CONFLICT, "Another domain change is running.".into())
    } else {
        ApiError::from(error)
    })?
    .ok_or_else(|| ApiError(StatusCode::CONFLICT, "No part-way domain change to retry.".into()))?;
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
    enqueue_temporal_dispatch(&mut tx, id, "domain_change").await?;
    tx.commit().await?;
    Ok(StatusCode::ACCEPTED)
}

pub(crate) async fn operator_latest(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Option<DomainChangeSummary>>, ApiError> {
    operator(&headers, &s.db).await?;
    Ok(Json(latest(&s.db, id).await?))
}

/// Stop a change that has not touched the instance yet: while it waits for
/// the new domain to connect. After that the instance's addresses have begun
/// to move, and it finishes.
async fn cancel(
    s: &Shared,
    tenant_id: Uuid,
    customer_id: Option<Uuid>,
) -> Result<StatusCode, ApiError> {
    let mut tx = s.db.begin().await?;
    let change = sqlx::query_as::<_, (Uuid, String, Uuid)>(
        "SELECT c.id,c.step,c.operation_id FROM domain_changes c JOIN tenants t ON t.id=c.tenant_id WHERE c.tenant_id=$1 AND c.status='running' AND ($2::uuid IS NULL OR t.customer_id=$2) FOR UPDATE OF c",
    )
    .bind(tenant_id)
    .bind(customer_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "No domain change is running.".into()))?;
    let (change_id, step, operation_id) = change;
    if step != "attach" {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "The instance has begun moving and can no longer stop.".into(),
        ));
    }
    let message = "The domain change was cancelled; the instance stays where it was.";
    abandon(&mut tx, change_id, operation_id, "cancelled", message).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn customer_cancel(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let (customer_id, _) = customer(&headers, &s.db).await?;
    cancel(&s, id, Some(customer_id)).await
}

pub(crate) async fn operator_cancel(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    operator(&headers, &s.db).await?;
    cancel(&s, id, None).await
}

/// End a change before it has moved anything: its new hostnames are released,
/// and the instance keeps its domain.
async fn abandon(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    change_id: Uuid,
    operation_id: Uuid,
    status: &str,
    message: &str,
) -> Result<(), sqlx::Error> {
    let (tenant_id, to_domain) = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT tenant_id,to_domain FROM domain_changes WHERE id=$1",
    )
    .bind(change_id)
    .fetch_one(&mut **tx)
    .await?;
    // The new hostnames: the one asked for and, for a provided subdomain, its
    // siblings. A custom one is handed back to the domain reconciler to take
    // off Cloudflare; a provided one is released the same way.
    let label = to_domain.split('.').next().unwrap_or_default().to_owned();
    let released: Vec<String> = sqlx::query_scalar(
        "UPDATE tenant_domains SET status='removing',error=$3,updated_at=now() WHERE tenant_id=$1 AND NOT is_primary AND (hostname=$2 OR (kind='provided' AND split_part(hostname,'.',1)=$4 AND created_at >= (SELECT created_at FROM domain_changes WHERE id=$5))) RETURNING hostname",
    )
    .bind(tenant_id)
    .bind(&to_domain)
    .bind(message)
    .bind(&label)
    .bind(change_id)
    .fetch_all(&mut **tx)
    .await?;
    sqlx::query("UPDATE tenants SET aliases=ARRAY(SELECT a FROM unnest(aliases) a WHERE a <> ALL($1)),config_revision=config_revision+1,updated_at=now() WHERE id=$2")
        .bind(&released).bind(tenant_id).execute(&mut **tx).await?;
    sqlx::query("UPDATE domain_changes SET status=$2,error=$3,finished_at=now(),updated_at=now() WHERE id=$1")
        .bind(change_id).bind(status).bind(message).execute(&mut **tx).await?;
    sqlx::query("UPDATE operations SET status='failed',error=$2,finished_at=now() WHERE id=$1")
        .bind(operation_id)
        .bind(message)
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO operation_events(operation_id,message) VALUES($1,$2)")
        .bind(operation_id)
        .bind(message)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// The running change of `tenant_id`: its id, where it is going and from,
/// its operation, the step it is at, and the batch its Moves went out in.
struct Running {
    id: Uuid,
    operation_id: Uuid,
    from_domain: String,
    to_domain: String,
    step: String,
    batch_tag: Option<String>,
}

async fn running(db: &sqlx::PgPool, tenant_id: Uuid) -> Result<Option<Running>> {
    let row = sqlx::query_as::<_, (Uuid, Uuid, String, String, String, Option<String>)>(
        "SELECT id,operation_id,from_domain,to_domain,step,batch_tag FROM domain_changes WHERE tenant_id=$1 AND status='running'",
    )
    .bind(tenant_id)
    .fetch_optional(db)
    .await?;
    Ok(row.map(
        |(id, operation_id, from_domain, to_domain, step, batch_tag)| Running {
            id,
            operation_id,
            from_domain,
            to_domain,
            step,
            batch_tag,
        },
    ))
}

async fn event(db: &sqlx::PgPool, operation_id: Uuid, message: &str) {
    let message: String = message.chars().take(500).collect();
    let _ = sqlx::query("INSERT INTO operation_events(operation_id,message) VALUES($1,$2)")
        .bind(operation_id)
        .bind(message)
        .execute(db)
        .await;
}

/// Run `step` of `tenant_id`'s running domain change.
pub(crate) async fn step(state: &Shared, tenant_id: Uuid, step: &str) -> ProvisionStepOutcome {
    let change = match running(&state.db, tenant_id).await {
        Ok(Some(change)) => change,
        // Cancelled or finished: nothing left to do.
        Ok(None) => return ProvisionStepOutcome::Halted,
        Err(_) => return ProvisionStepOutcome::Halted,
    };
    let position = |name: &str| STEPS.iter().position(|s| *s == name).unwrap_or(0);
    // Steps already behind the change are done; a replay passes over them.
    if position(step) < position(&change.step) || change.step == "done" {
        return ProvisionStepOutcome::Completed;
    }
    let outcome = match step {
        "attach" => attach(state, tenant_id, &change).await,
        "rename" => rename(state, tenant_id, &change).await,
        "switch" => switch(state, tenant_id, &change).await,
        "move" => send_moves(state, tenant_id, &change).await,
        "deliver" => deliver(state, tenant_id, &change).await,
        "release" => release(state, tenant_id, &change).await,
        _ => Err(anyhow::anyhow!("unknown step")),
    };
    match outcome {
        Ok(true) => {
            let next = STEPS.get(position(step) + 1).copied().unwrap_or("done");
            let _ = sqlx::query("UPDATE domain_changes SET step=$2,updated_at=now() WHERE id=$1")
                .bind(change.id)
                .bind(next)
                .execute(&state.db)
                .await;
            ProvisionStepOutcome::Completed
        }
        Ok(false) => ProvisionStepOutcome::Wait,
        Err(error) => {
            let message = format!("{step} failed: {error}");
            // Before `rename` nothing about the instance has changed, and it
            // stays where it was. After it, the instance is part-way moved and
            // an operator has to finish it: retrying the step is safe.
            if step == "attach" {
                if let Ok(mut tx) = state.db.begin().await
                    && abandon(&mut tx, change.id, change.operation_id, "failed", &message)
                        .await
                        .is_ok()
                {
                    let _ = tx.commit().await;
                }
            } else {
                let _ = sqlx::query("UPDATE domain_changes SET status='failed',error=$2,finished_at=now(),updated_at=now() WHERE id=$1")
                    .bind(change.id).bind(&message).execute(&state.db).await;
                let _ = sqlx::query(
                    "UPDATE operations SET status='failed',error=$2,finished_at=now() WHERE id=$1",
                )
                .bind(change.operation_id)
                .bind(&message)
                .execute(&state.db)
                .await;
                event(&state.db, change.operation_id, &message).await;
            }
            ProvisionStepOutcome::Failed
        }
    }
}

/// Wait for the new domain to connect, serving it as an alias meanwhile.
async fn attach(state: &Shared, tenant_id: Uuid, change: &Running) -> Result<bool> {
    let lifecycle =
        sqlx::query_scalar::<_, String>("SELECT lifecycle_state FROM tenants WHERE id=$1")
            .bind(tenant_id)
            .fetch_one(&state.db)
            .await?;
    anyhow::ensure!(lifecycle == "ready", "the instance is no longer running");
    // The new hostname has to be served before it can be checked; the
    // reconciler would get to it within a minute, this gets to it now.
    let drifted = sqlx::query_scalar::<_, bool>(
        "SELECT config_revision IS DISTINCT FROM rendered_revision FROM tenants WHERE id=$1",
    )
    .bind(tenant_id)
    .fetch_one(&state.db)
    .await?;
    if drifted {
        render_provisioned_tenant(state, tenant_id).await?;
        reload_provisioned_runtime(state, tenant_id).await?;
    }
    let status = sqlx::query_as::<_, (String, Option<String>, bool)>(&format!(
        "SELECT status,error,created_at < now()-interval '{ATTACH_WINDOW}' FROM tenant_domains WHERE tenant_id=$1 AND hostname=$2"
    ))
    .bind(tenant_id)
    .bind(&change.to_domain)
    .fetch_optional(&state.db)
    .await?
    .context("the new domain is no longer attached")?;
    match status {
        (status, _, _) if status == "active" => {
            event(
                &state.db,
                change.operation_id,
                &format!("{} connected.", change.to_domain),
            )
            .await;
            Ok(true)
        }
        (status, error, _) if status == "failed" => anyhow::bail!(
            "{} could not be connected: {}",
            change.to_domain,
            error.unwrap_or_default()
        ),
        (_, _, true) => anyhow::bail!(
            "{} was not connected within {ATTACH_WINDOW}",
            change.to_domain
        ),
        _ => Ok(false),
    }
}

/// Rewrite the addresses the instance handed out under its old domain.
async fn rename(state: &Shared, tenant_id: Uuid, change: &Running) -> Result<bool> {
    eunha(
        state,
        tenant_id,
        &[
            "rename-domain",
            "--from",
            &change.from_domain,
            "--to",
            &change.to_domain,
        ],
        Duration::from_secs(600),
    )
    .await?;
    event(
        &state.db,
        change.operation_id,
        "The instance's addresses were rewritten.",
    )
    .await;
    Ok(true)
}

/// Serve the instance under its new domain alone, with the old one among its
/// previous domains so that each actor names its old self.
async fn switch(state: &Shared, tenant_id: Uuid, change: &Running) -> Result<bool> {
    let mut tx = state.db.begin().await?;
    let new_hostnames: Vec<String> = sqlx::query_scalar(
        "SELECT hostname FROM tenant_domains WHERE tenant_id=$1 AND (hostname=$2 OR (kind='provided' AND split_part(hostname,'.',1)=split_part($2,'.',1) AND created_at >= (SELECT created_at FROM domain_changes WHERE id=$3))) ORDER BY hostname=$2 DESC,created_at",
    )
    .bind(tenant_id)
    .bind(&change.to_domain)
    .bind(change.id)
    .fetch_all(&mut *tx)
    .await?;
    // Two statements: one primary per tenant is a unique index, checked row by
    // row, so the old primary has to go before the new one comes.
    sqlx::query(
        "UPDATE tenant_domains SET is_primary=false,updated_at=now() WHERE tenant_id=$1 AND is_primary",
    )
    .bind(tenant_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE tenant_domains SET is_primary=true,updated_at=now() WHERE tenant_id=$1 AND hostname=$2",
    )
    .bind(tenant_id)
    .bind(&change.to_domain)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE tenants SET domain=$2,aliases=$3,previous_domains=CASE WHEN $4=ANY(previous_domains) THEN previous_domains ELSE array_append(previous_domains,$4) END,config_revision=config_revision+1,updated_at=now() WHERE id=$1")
        .bind(tenant_id)
        .bind(&change.to_domain)
        .bind(&new_hostnames[1.min(new_hostnames.len())..])
        .bind(&change.from_domain)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    render_provisioned_tenant(state, tenant_id).await?;
    reload_provisioned_runtime(state, tenant_id).await?;
    healthcheck_provisioned_tenant(state, tenant_id).await?;
    event(
        &state.db,
        change.operation_id,
        &format!(
            "The instance is served at {}; {} no longer answers.",
            change.to_domain, change.from_domain
        ),
    )
    .await;
    Ok(true)
}

/// Send every account's followers a Move, and follow again from the new
/// actors what the accounts followed, as one batch.
async fn send_moves(state: &Shared, tenant_id: Uuid, change: &Running) -> Result<bool> {
    // Sent once: a step retried after the batch went out follows it instead.
    if change.batch_tag.is_some() {
        return Ok(true);
    }
    let stdout = eunha(
        state,
        tenant_id,
        &[
            "accounts",
            "move",
            "--from",
            &change.from_domain,
            "--all",
            "--give-up-after",
            MOVE_GIVE_UP_AFTER,
        ],
        Duration::from_secs(1800),
    )
    .await?;
    let tag = batch_tag(&stdout).context("eunha named no batch")?;
    sqlx::query("UPDATE domain_changes SET batch_tag=$2,updated_at=now() WHERE id=$1")
        .bind(change.id)
        .bind(&tag)
        .execute(&state.db)
        .await?;
    let summary = stdout
        .lines()
        .find(|line| line.contains(" accounts queued for "))
        .unwrap_or("The Moves were queued.");
    event(&state.db, change.operation_id, summary).await;
    Ok(true)
}

/// Follow the batch until every delivery has gone through or been given up
/// on.
async fn deliver(state: &Shared, tenant_id: Uuid, change: &Running) -> Result<bool> {
    let tag = change.batch_tag.as_deref().context("no batch was sent")?;
    let stdout = eunha(
        state,
        tenant_id,
        &["accounts", "batch-status", tag],
        Duration::from_secs(120),
    )
    .await?;
    let (pending, given_up) = batch_counts(&stdout).context("eunha said nothing of the batch")?;
    sqlx::query("UPDATE domain_changes SET pending=$2,given_up=$3,updated_at=now() WHERE id=$1")
        .bind(change.id)
        .bind(i32::try_from(pending).unwrap_or(i32::MAX))
        .bind(i32::try_from(given_up).unwrap_or(i32::MAX))
        .execute(&state.db)
        .await?;
    if pending > 0 {
        return Ok(false);
    }
    let mut summary =
        format!("Every delivery has gone through or been given up on: {given_up} given up on.");
    for line in stdout
        .lines()
        .filter(|line| line.starts_with("  "))
        .take(10)
    {
        summary.push('\n');
        summary.push_str(line.trim());
    }
    event(&state.db, change.operation_id, &summary).await;
    Ok(true)
}

/// Take the old hostnames off Cloudflare and finish. They are free for
/// anyone to take afterwards, a provided subdomain included.
async fn release(state: &Shared, tenant_id: Uuid, change: &Running) -> Result<bool> {
    let old = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM tenant_domains WHERE tenant_id=$1 AND NOT is_primary AND hostname <> ALL(COALESCE((SELECT aliases FROM tenants WHERE id=$1),'{}'::text[])) AND status<>'removing'",
    )
    .bind(tenant_id)
    .fetch_all(&state.db)
    .await?;
    for id in old {
        crate::delete_domain(state, id).await?;
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE domain_changes SET status='succeeded',step='done',finished_at=now(),updated_at=now() WHERE id=$1")
        .bind(change.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE operations SET status='succeeded',finished_at=now() WHERE id=$1")
        .bind(change.operation_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO operation_events(operation_id,message) VALUES($1,$2)")
        .bind(change.operation_id)
        .bind(format!("Moved to {}.", change.to_domain))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

/// The tag an `accounts move` printed for its batch.
fn batch_tag(stdout: &str) -> Option<String> {
    stdout.lines().find_map(|line| {
        let rest = line.strip_prefix("Batch ")?;
        let tag = rest.split(';').next()?.trim();
        (!tag.is_empty()
            && tag
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'-' | b'_')))
        .then(|| tag.to_owned())
    })
}

/// How many deliveries are pending and given up on, from `accounts
/// batch-status`.
fn batch_counts(stdout: &str) -> Option<(u64, u64)> {
    stdout.lines().find_map(|line| {
        let (pending, rest) = line.split_once(" pending, ")?;
        let given_up = rest.strip_suffix(" given up on")?;
        Some((pending.trim().parse().ok()?, given_up.trim().parse().ok()?))
    })
}

/// Run eunha's `args` against `tenant_id`, on the runtime this control plane
/// manages, and return what it printed. Its errors are not kept: they can
/// name a connection string.
async fn eunha(
    state: &Shared,
    tenant_id: Uuid,
    args: &[&str],
    timeout: Duration,
) -> Result<String> {
    let (domain, tenant_directory, runtime_host) = sqlx::query_as::<_, (String, String, String)>(
        "SELECT t.domain,r.tenant_directory,i.address FROM tenants t JOIN runtimes r ON r.id=t.runtime_id JOIN instances i ON i.id=r.instance_id WHERE t.id=$1",
    )
    .bind(tenant_id)
    .fetch_one(&state.db)
    .await?;
    let configured_host = std::env::var("CONTROL_RUNTIME_HOST").unwrap_or_default();
    anyhow::ensure!(
        !configured_host.is_empty() && configured_host == runtime_host,
        "runtime is managed elsewhere"
    );
    anyhow::ensure!(is_safe_domain(&domain), "invalid tenant domain");
    let runtime_root = std::path::PathBuf::from(&tenant_directory)
        .parent()
        .context("runtime directory has no parent")?
        .to_path_buf();
    let output = tokio::time::timeout(
        timeout,
        tokio::process::Command::new(runtime_root.join("current/eunha"))
            .arg("--tenants")
            .arg(&tenant_directory)
            .args(args)
            .arg(format!("--instance={domain}"))
            .current_dir(runtime_root.join("current"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .with_context(|| format!("eunha {} timed out", args[0]))??;
    anyhow::ensure!(
        output.status.success(),
        "eunha {} failed ({})",
        args.join(" "),
        output.status
    );
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_eunha_prints_is_read() {
        let moved = "alice: queued for 3 inboxes\n1 accounts queued for 3 inboxes, 0 skipped\nBatch move:1234567; `eunha accounts batch-status move:1234567` says where it stands\n";
        assert_eq!(batch_tag(moved).as_deref(), Some("move:1234567"));
        assert_eq!(batch_tag("Batch ; x"), None);
        assert_eq!(batch_tag("Batch move:1; rm -rf /"), Some("move:1".into()));
        assert_eq!(batch_tag("Batch a b;"), None);

        let status = "2 pending, 1 given up on\n  https://a.example/inbox: HTTP 503\n";
        assert_eq!(batch_counts(status), Some((2, 1)));
        assert_eq!(batch_counts("0 pending, 0 given up on\n"), Some((0, 0)));
        assert_eq!(batch_counts("nothing"), None);
    }

    #[test]
    fn the_steps_run_in_order() {
        assert_eq!(
            STEPS,
            ["attach", "rename", "switch", "move", "deliver", "release"]
        );
    }
}

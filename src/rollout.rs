//! Incremental rollouts of eunha releases through release channels.
//!
//! A runtime is one eunha process with one binary, so a release reaches
//! tenants runtime by runtime. Runtimes are grouped into channels, which a
//! release reaches in order: each runtime of a channel takes it, one at a time,
//! and the channel soaks, healthy, before the next one starts. A release that
//! fails a runtime too often halts the rollout, so a bad build stops at the
//! channel that found it rather than going on to the next runtime.
//!
//! The decision of what to do next is [`plan`], a pure function of what
//! PostgreSQL says; [`reconciler`] reads that state, asks it, and acts.
//! *docs/releases.md* has the rules an operator works with.

use std::time::Duration;

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    ApiError, Shared, admin_user, latest_remote_ref, queue_runtime_release, release_order,
};

/// How often a rollout is moved along.
const POLL: Duration = Duration::from_secs(30);

/// How often CI is asked for a newer build. GitHub's API is rate limited, and
/// a build takes minutes anyway.
const BUILD_CHECK: Duration = Duration::from_secs(300);

/// How many times a runtime may fail to take one rollout's release before the
/// rollout halts.
pub(crate) const ATTEMPTS: i64 = 3;

/// How long after a failed release the same runtime is tried again.
const RETRY_AFTER: time::Duration = time::Duration::minutes(5);

/// A runtime as a rollout sees it.
#[derive(Debug, Clone)]
pub(crate) struct RuntimeState {
    pub id: Uuid,
    pub name: String,
    /// Whether the runtime takes automatic releases; one that does not is
    /// pinned to what it runs.
    pub follows: bool,
    /// A release is checked against a served tenant, so a runtime without one
    /// cannot take it yet.
    pub has_tenants: bool,
    /// Runs the rollout's build, or a newer one an operator deployed by hand,
    /// which a rollout never takes back.
    pub on_release: bool,
    /// Releases of this rollout that failed on it since its attempts were
    /// last given back.
    pub failures: i64,
    pub last_failure_at: Option<OffsetDateTime>,
    /// Tenants on it, ready and meant to be running, last seen unhealthy.
    pub unhealthy: i64,
    /// Why it cannot be released now: a tenant is moving on or off it, or
    /// whether it runs a newer build than the rollout's is not known.
    pub held: Option<String>,
}

impl RuntimeState {
    fn takes_part(&self) -> bool {
        self.follows && self.has_tenants
    }
}

/// A channel, and how far the rollout has got in it.
#[derive(Debug, Clone)]
pub(crate) struct ChannelState {
    pub id: Uuid,
    pub name: String,
    pub soak_minutes: i32,
    pub completed_at: Option<OffsetDateTime>,
    pub promoted_at: Option<OffsetDateTime>,
    /// By name: the order a channel's runtimes take a release.
    pub runtimes: Vec<RuntimeState>,
}

/// What a rollout does next.
#[derive(Debug, PartialEq)]
pub(crate) enum Action {
    Release {
        channel_id: Uuid,
        runtime_id: Uuid,
    },
    /// Every runtime of the channel that takes part runs the release.
    CompleteStage {
        channel_id: Uuid,
    },
    Halt(String),
    /// Every channel has the release.
    Finish,
    /// Nothing to do yet, and why.
    Wait(String),
}

fn tenants(count: i64) -> String {
    match count {
        1 => "1 tenant is".into(),
        n => format!("{n} tenants are"),
    }
}

/// Decide what a rolling rollout does next. `channels` are in rollout order.
pub(crate) fn plan(
    channels: &[ChannelState],
    release_in_flight: bool,
    now: OffsetDateTime,
) -> Action {
    for (index, channel) in channels.iter().enumerate() {
        let taking_part: Vec<&RuntimeState> = channel
            .runtimes
            .iter()
            .filter(|runtime| runtime.takes_part())
            .collect();
        let promoted = channel.promoted_at.is_some();
        let unhealthy: i64 = taking_part
            .iter()
            .filter(|runtime| runtime.on_release)
            .map(|runtime| runtime.unhealthy)
            .sum();
        let Some(completed_at) = channel.completed_at else {
            // A channel with nothing to release holds nothing up.
            if taking_part.is_empty() {
                continue;
            }
            if let Some(runtime) = taking_part
                .iter()
                .find(|runtime| !runtime.on_release && runtime.failures >= ATTEMPTS)
            {
                return Action::Halt(format!(
                    "{} failed to take the release {ATTEMPTS} times.",
                    runtime.name
                ));
            }
            let pending: Vec<&&RuntimeState> = taking_part
                .iter()
                .filter(|runtime| !runtime.on_release)
                .collect();
            if pending.is_empty() {
                return Action::CompleteStage {
                    channel_id: channel.id,
                };
            }
            // The release broke something where it already went: the rest of
            // the channel waits for an operator to look.
            if !promoted && unhealthy > 0 {
                return Action::Wait(format!(
                    "{} unhealthy in {} since the release; promote to carry on anyway.",
                    tenants(unhealthy),
                    channel.name
                ));
            }
            if release_in_flight {
                return Action::Wait("Waiting for the release in progress to finish.".into());
            }
            let ready = pending.iter().find(|runtime| {
                runtime.held.is_none()
                    && runtime
                        .last_failure_at
                        .is_none_or(|failed| failed + RETRY_AFTER <= now)
            });
            return match ready {
                Some(runtime) => Action::Release {
                    channel_id: channel.id,
                    runtime_id: runtime.id,
                },
                None => match pending.iter().find_map(|runtime| runtime.held.clone()) {
                    Some(held) => Action::Wait(held),
                    None => Action::Wait(format!(
                        "Trying {} again shortly after its failed release.",
                        pending[0].name
                    )),
                },
            };
        };
        // The last channel has nobody to soak for.
        if index + 1 == channels.len() || promoted {
            continue;
        }
        if unhealthy > 0 {
            return Action::Wait(format!(
                "{} unhealthy in {}; promote to carry on anyway.",
                tenants(unhealthy),
                channel.name
            ));
        }
        let soaked = completed_at + time::Duration::minutes(i64::from(channel.soak_minutes));
        if now < soaked {
            let minutes = ((soaked - now).whole_seconds() + 59) / 60;
            return Action::Wait(format!(
                "Soaking in {} for {minutes} more minute{}.",
                channel.name,
                if minutes == 1 { "" } else { "s" }
            ));
        }
    }
    Action::Finish
}

/// The rollout in flight, if there is one.
#[derive(Debug, sqlx::FromRow)]
pub(crate) struct Active {
    pub id: Uuid,
    pub git_sha: String,
    pub status: String,
    pub attempts_since: OffsetDateTime,
}

pub(crate) async fn active(db: &PgPool) -> Result<Option<Active>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id,git_sha,status,attempts_since FROM release_rollouts WHERE status IN ('rolling','paused','halted')",
    )
    .fetch_optional(db)
    .await
}

/// Start rolling `reference` out, unless it already is or was. The rollout it
/// replaces is superseded; one an operator paused leaves the new one paused,
/// so that a pause is a freeze rather than a stop for one build.
pub(crate) async fn begin(db: &PgPool, reference: &str) -> Result<bool, sqlx::Error> {
    let mut tx = db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(60001002)")
        .execute(&mut *tx)
        .await?;
    let known = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM release_rollouts WHERE git_sha=$1)",
    )
    .bind(reference)
    .fetch_one(&mut *tx)
    .await?;
    if known {
        return Ok(false);
    }
    let previous = sqlx::query_scalar::<_, String>(
        "SELECT status FROM release_rollouts WHERE status IN ('rolling','paused','halted') FOR UPDATE",
    )
    .fetch_optional(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE release_rollouts SET status='superseded',finished_at=now(),updated_at=now() WHERE status IN ('rolling','paused','halted')",
    )
    .execute(&mut *tx)
    .await?;
    let status = match previous.as_deref() {
        Some("paused") => "paused",
        _ => "rolling",
    };
    sqlx::query("INSERT INTO release_rollouts(id,git_sha,status) VALUES($1,$2,$3)")
        .bind(Uuid::new_v4())
        .bind(reference)
        .bind(status)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

/// Read what [`plan`] decides from.
pub(crate) async fn channels(
    db: &PgPool,
    rollout: &Active,
) -> Result<Vec<ChannelState>, sqlx::Error> {
    let channels = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            i32,
            Option<OffsetDateTime>,
            Option<OffsetDateTime>,
        ),
    >(
        "SELECT c.id,c.name,c.soak_minutes,s.completed_at,s.promoted_at FROM release_channels c LEFT JOIN release_rollout_stages s ON s.channel_id=c.id AND s.rollout_id=$1 ORDER BY c.position",
    )
    .bind(rollout.id)
    .fetch_all(db)
    .await?;
    let runtimes = sqlx::query_as::<
        _,
        (
            Uuid,
            Uuid,
            String,
            bool,
            bool,
            Option<String>,
            i64,
            Option<OffsetDateTime>,
            i64,
            bool,
        ),
    >(
        "SELECT r.channel_id,r.id,r.name,r.auto_deploy_enabled,
                EXISTS(SELECT 1 FROM tenants t WHERE t.runtime_id=r.id AND t.lifecycle_state='ready'),
                r.deployed_ref,
                (SELECT count(*) FROM runtime_deployments d WHERE d.runtime_id=r.id AND d.rollout_id=$1 AND d.status IN ('failed','rolled_back') AND d.created_at>=$2),
                (SELECT max(d.finished_at) FROM runtime_deployments d WHERE d.runtime_id=r.id AND d.rollout_id=$1 AND d.status IN ('failed','rolled_back') AND d.created_at>=$2),
                (SELECT count(*) FROM tenants t WHERE t.runtime_id=r.id AND t.lifecycle_state='ready' AND t.desired_state='running' AND t.observed_state='unhealthy'),
                EXISTS(SELECT 1 FROM tenant_moves m WHERE m.finished_at IS NULL AND r.id IN (m.from_runtime_id,m.to_runtime_id))
         FROM runtimes r WHERE r.channel_id IS NOT NULL ORDER BY r.name",
    )
    .bind(rollout.id)
    .bind(rollout.attempts_since)
    .fetch_all(db)
    .await?;
    let mut states = Vec::with_capacity(runtimes.len());
    for runtime in runtimes {
        let (on_release, unordered) = match &runtime.5 {
            Some(deployed) => match release_order::compare(db, &rollout.git_sha, deployed).await {
                Ok(order) => (order.at_least(), None),
                Err(error) => (
                    false,
                    Some(format!(
                        "Whether {} runs a newer build is not known: {error}",
                        runtime.2
                    )),
                ),
            },
            None => (false, None),
        };
        let held = if runtime.9 {
            Some(format!("A tenant is moving on or off {}.", runtime.2))
        } else {
            unordered
        };
        states.push((
            runtime.0,
            RuntimeState {
                id: runtime.1,
                name: runtime.2,
                follows: runtime.3,
                has_tenants: runtime.4,
                on_release,
                failures: runtime.6,
                last_failure_at: runtime.7,
                unhealthy: runtime.8,
                held,
            },
        ));
    }
    Ok(channels
        .into_iter()
        .map(
            |(id, name, soak_minutes, completed_at, promoted_at)| ChannelState {
                id,
                name,
                soak_minutes,
                completed_at,
                promoted_at,
                runtimes: states
                    .iter()
                    .filter(|(channel_id, _)| *channel_id == id)
                    .map(|(_, runtime)| runtime.clone())
                    .collect(),
            },
        )
        .collect())
}

async fn release_in_flight(db: &PgPool) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM operations WHERE kind='runtime_release' AND status IN ('pending','running'))",
    )
    .fetch_one(db)
    .await
}

async fn note(db: &PgPool, rollout_id: Uuid, reason: Option<&str>) {
    let _ = sqlx::query(
        "UPDATE release_rollouts SET reason=$2,updated_at=now() WHERE id=$1 AND reason IS DISTINCT FROM $2",
    )
    .bind(rollout_id)
    .bind(reason)
    .execute(db)
    .await;
}

/// Move the rollout in flight along as far as it can go now.
pub(crate) async fn step(state: &Shared) -> Result<(), sqlx::Error> {
    let db = &state.db;
    // Completing a stage can let the next one start at once, so a pass goes on
    // until it has to wait; each channel completes once, which bounds it.
    for _ in 0..32 {
        let Some(rollout) = active(db).await? else {
            return Ok(());
        };
        if rollout.status != "rolling" {
            return Ok(());
        }
        let channels = channels(db, &rollout).await?;
        let in_flight = release_in_flight(db).await?;
        match plan(&channels, in_flight, OffsetDateTime::now_utc()) {
            Action::Wait(reason) => {
                note(db, rollout.id, Some(&reason)).await;
                return Ok(());
            }
            Action::CompleteStage { channel_id } => {
                sqlx::query("INSERT INTO release_rollout_stages(rollout_id,channel_id,completed_at) VALUES($1,$2,now()) ON CONFLICT (rollout_id,channel_id) DO UPDATE SET completed_at=COALESCE(release_rollout_stages.completed_at,now())")
                    .bind(rollout.id)
                    .bind(channel_id)
                    .execute(db)
                    .await?;
            }
            Action::Release {
                channel_id,
                runtime_id,
            } => {
                sqlx::query("INSERT INTO release_rollout_stages(rollout_id,channel_id) VALUES($1,$2) ON CONFLICT DO NOTHING")
                    .bind(rollout.id)
                    .bind(channel_id)
                    .execute(db)
                    .await?;
                let queued = queue_runtime_release(
                    state.clone(),
                    runtime_id,
                    None,
                    "automatic",
                    Some(rollout.git_sha.clone()),
                    Some(rollout.id),
                )
                .await;
                match queued {
                    Ok(_) => note(db, rollout.id, Some("Releasing.")).await,
                    // Admission refused it — another release won the race, the
                    // control plane is deploying, a tenant began moving. The
                    // reason is fixed text; the next pass asks again.
                    Err(ApiError(_, reason)) => note(db, rollout.id, Some(&reason)).await,
                }
                return Ok(());
            }
            Action::Halt(reason) => {
                sqlx::query("UPDATE release_rollouts SET status='halted',reason=$2,updated_at=now() WHERE id=$1 AND status='rolling'")
                    .bind(rollout.id)
                    .bind(&reason)
                    .execute(db)
                    .await?;
                tracing::warn!(rollout = %rollout.id, "rollout halted");
                return Ok(());
            }
            Action::Finish => {
                sqlx::query("UPDATE release_rollouts SET status='completed',reason=NULL,finished_at=now(),updated_at=now() WHERE id=$1 AND status='rolling'")
                    .bind(rollout.id)
                    .execute(db)
                    .await?;
                return Ok(());
            }
        }
    }
    Ok(())
}

/// Starts a rollout for each new build and moves it along, for as long as the
/// control plane runs.
pub async fn reconciler(state: Shared) {
    // A local process pointed at a shared database must not release the fleet.
    if std::env::var_os("CONTROL_JOBS_DISABLED").is_some() {
        return std::future::pending().await;
    }
    let mut checked_at: Option<std::time::Instant> = None;
    loop {
        if checked_at.is_none_or(|at| at.elapsed() >= BUILD_CHECK) {
            checked_at = Some(std::time::Instant::now());
            if let Some(reference) = latest_remote_ref().await
                && let Err(_error) = begin(&state.db, &reference).await
            {
                tracing::warn!("could not record a new release to roll out");
            }
        }
        if let Err(_error) = step(&state).await {
            tracing::warn!("could not move the rollout along");
        }
        tokio::time::sleep(POLL).await;
    }
}

// The console's API.

#[derive(Serialize, sqlx::FromRow)]
pub(crate) struct Channel {
    id: Uuid,
    name: String,
    position: i32,
    soak_minutes: i32,
    runtime_count: i64,
}

pub(crate) async fn list_channels(
    State(s): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Vec<Channel>>, ApiError> {
    admin_user(&headers, &s.db).await?;
    Ok(Json(
        sqlx::query_as("SELECT c.id,c.name,c.position,c.soak_minutes,(SELECT count(*) FROM runtimes r WHERE r.channel_id=c.id) runtime_count FROM release_channels c ORDER BY c.position")
            .fetch_all(&s.db)
            .await?,
    ))
}

#[derive(Deserialize)]
pub(crate) struct ChannelRequest {
    name: String,
    soak_minutes: i32,
}

fn checked(request: &ChannelRequest) -> Result<String, ApiError> {
    let name = request.name.trim().to_ascii_lowercase();
    let valid = (1..=32).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !name.starts_with('-');
    if !valid {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "A channel name is up to 32 lowercase letters, digits and hyphens.".into(),
        ));
    }
    if !(0..=10080).contains(&request.soak_minutes) {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "A soak is between 0 minutes and 7 days.".into(),
        ));
    }
    Ok(name)
}

fn taken(error: sqlx::Error) -> ApiError {
    if error.as_database_error().and_then(|e| e.constraint()) == Some("release_channels_name_key") {
        ApiError(StatusCode::CONFLICT, "A channel has that name.".into())
    } else {
        ApiError::from(error)
    }
}

async fn audit(
    db: &PgPool,
    admin_user_id: Uuid,
    runtime_id: Option<Uuid>,
    detail: serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO operations(id,runtime_id,kind,status,requested_by,detail,finished_at) VALUES($1,$2,'release_policy','succeeded',$3,$4,now())")
        .bind(Uuid::new_v4())
        .bind(runtime_id)
        .bind(admin_user_id)
        .bind(detail)
        .execute(db)
        .await?;
    Ok(())
}

/// A new channel goes last: a release reaches it after every other.
pub(crate) async fn create_channel(
    State(s): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<ChannelRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let (admin_user_id, _) = admin_user(&headers, &s.db).await?;
    let name = checked(&request)?;
    let id = Uuid::new_v4();
    let mut tx = s.db.begin().await?;
    sqlx::query("LOCK TABLE release_channels IN SHARE ROW EXCLUSIVE MODE")
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO release_channels(id,name,position,soak_minutes) VALUES($1,$2,(SELECT COALESCE(max(position)+1,0) FROM release_channels),$3)")
        .bind(id)
        .bind(&name)
        .bind(request.soak_minutes)
        .execute(&mut *tx)
        .await
        .map_err(taken)?;
    tx.commit().await?;
    audit(
        &s.db,
        admin_user_id,
        None,
        serde_json::json!({"channel_created": name, "soak_minutes": request.soak_minutes}),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(serde_json::json!({"id": id}))))
}

pub(crate) async fn update_channel(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<ChannelRequest>,
) -> Result<StatusCode, ApiError> {
    let (admin_user_id, _) = admin_user(&headers, &s.db).await?;
    let name = checked(&request)?;
    let changed = sqlx::query("UPDATE release_channels SET name=$2,soak_minutes=$3 WHERE id=$1")
        .bind(id)
        .bind(&name)
        .bind(request.soak_minutes)
        .execute(&s.db)
        .await
        .map_err(taken)?
        .rows_affected();
    if changed == 0 {
        return Err(ApiError(StatusCode::NOT_FOUND, "Channel not found.".into()));
    }
    audit(
        &s.db,
        admin_user_id,
        None,
        serde_json::json!({"channel_updated": name, "soak_minutes": request.soak_minutes}),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Only an empty channel can go: its runtimes would otherwise drop out of
/// rollouts without anyone deciding they should.
pub(crate) async fn delete_channel(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let (admin_user_id, _) = admin_user(&headers, &s.db).await?;
    let deleted = sqlx::query_scalar::<_, String>(
        "DELETE FROM release_channels c WHERE id=$1 AND NOT EXISTS(SELECT 1 FROM runtimes r WHERE r.channel_id=c.id) RETURNING name",
    )
    .bind(id)
    .fetch_optional(&s.db)
    .await?;
    let Some(name) = deleted else {
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM release_channels WHERE id=$1)",
        )
        .bind(id)
        .fetch_one(&s.db)
        .await?;
        return Err(match exists {
            true => ApiError(
                StatusCode::CONFLICT,
                "Move this channel's runtimes to another channel first.".into(),
            ),
            false => ApiError(StatusCode::NOT_FOUND, "Channel not found.".into()),
        });
    };
    audit(
        &s.db,
        admin_user_id,
        None,
        serde_json::json!({"channel_deleted": name}),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(crate) struct Reorder {
    /// `earlier` or `later` in the rollout order.
    direction: String,
}

/// Trade places with the neighbouring channel.
pub(crate) async fn reorder_channel(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<Reorder>,
) -> Result<StatusCode, ApiError> {
    let (admin_user_id, _) = admin_user(&headers, &s.db).await?;
    let earlier = match request.direction.as_str() {
        "earlier" => true,
        "later" => false,
        _ => {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "Move a channel earlier or later.".into(),
            ));
        }
    };
    let mut tx = s.db.begin().await?;
    sqlx::query("LOCK TABLE release_channels IN SHARE ROW EXCLUSIVE MODE")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET CONSTRAINTS release_channels_position_key DEFERRED")
        .execute(&mut *tx)
        .await?;
    let position =
        sqlx::query_scalar::<_, i32>("SELECT position FROM release_channels WHERE id=$1")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Channel not found.".into()))?;
    let neighbour = sqlx::query_as::<_, (Uuid, i32)>(if earlier {
        "SELECT id,position FROM release_channels WHERE position<$1 ORDER BY position DESC LIMIT 1"
    } else {
        "SELECT id,position FROM release_channels WHERE position>$1 ORDER BY position LIMIT 1"
    })
    .bind(position)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((neighbour, neighbour_position)) = neighbour else {
        return Ok(StatusCode::NO_CONTENT);
    };
    sqlx::query("UPDATE release_channels SET position=$2 WHERE id=$1")
        .bind(id)
        .bind(neighbour_position)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE release_channels SET position=$2 WHERE id=$1")
        .bind(neighbour)
        .bind(position)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    audit(
        &s.db,
        admin_user_id,
        None,
        serde_json::json!({"channel_reordered": id, "direction": request.direction}),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(crate) struct Assignment {
    channel_id: Option<Uuid>,
}

/// Put a runtime in a channel, or in none, which takes it out of rollouts.
pub(crate) async fn assign_runtime(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<Assignment>,
) -> Result<StatusCode, ApiError> {
    let (admin_user_id, _) = admin_user(&headers, &s.db).await?;
    let changed = sqlx::query("UPDATE runtimes SET channel_id=$2 WHERE id=$1")
        .bind(id)
        .bind(request.channel_id)
        .execute(&s.db)
        .await
        .map_err(|error| {
            if error.as_database_error().and_then(|e| e.constraint())
                == Some("runtimes_channel_id_fkey")
            {
                ApiError(StatusCode::NOT_FOUND, "Channel not found.".into())
            } else {
                ApiError::from(error)
            }
        })?
        .rows_affected();
    if changed == 0 {
        return Err(ApiError(StatusCode::NOT_FOUND, "Runtime not found.".into()));
    }
    audit(
        &s.db,
        admin_user_id,
        Some(id),
        serde_json::json!({"channel_id": request.channel_id}),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, sqlx::FromRow)]
pub(crate) struct Stage {
    channel_id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    started_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    completed_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    promoted_at: Option<OffsetDateTime>,
}

#[derive(Serialize, sqlx::FromRow)]
pub(crate) struct Deployment {
    runtime_id: Uuid,
    status: String,
    tenants_checked: Option<i32>,
    tenants_unhealthy: Option<i32>,
    #[serde(with = "time::serde::rfc3339::option")]
    finished_at: Option<OffsetDateTime>,
}

#[derive(Serialize)]
pub(crate) struct Rollout {
    id: Uuid,
    git_sha: String,
    status: String,
    reason: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    finished_at: Option<OffsetDateTime>,
    stages: Vec<Stage>,
    /// The latest release of this rollout on each runtime it reached.
    deployments: Vec<Deployment>,
    /// Runtimes an operator deployed a newer build to by hand.
    ahead: Vec<Uuid>,
}

/// The rollout in flight first, then the most recent ones.
pub(crate) async fn list_rollouts(
    State(s): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Vec<Rollout>>, ApiError> {
    admin_user(&headers, &s.db).await?;
    let rows = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            String,
            Option<String>,
            OffsetDateTime,
            Option<OffsetDateTime>,
        ),
    >(
        "SELECT id,git_sha,status,reason,created_at,finished_at FROM release_rollouts ORDER BY status IN ('rolling','paused','halted') DESC,created_at DESC LIMIT 10",
    )
    .fetch_all(&s.db)
    .await?;
    let mut rollouts = Vec::with_capacity(rows.len());
    for (id, git_sha, status, reason, created_at, finished_at) in rows {
        let stages = sqlx::query_as(
            "SELECT channel_id,started_at,completed_at,promoted_at FROM release_rollout_stages WHERE rollout_id=$1",
        )
        .bind(id)
        .fetch_all(&s.db)
        .await?;
        let deployments = sqlx::query_as(
            "SELECT DISTINCT ON (runtime_id) runtime_id,status,tenants_checked,tenants_unhealthy,finished_at FROM runtime_deployments WHERE rollout_id=$1 ORDER BY runtime_id,created_at DESC",
        )
        .bind(id)
        .fetch_all(&s.db)
        .await?;
        // Only the rollouts still going are compared: a finished one's
        // runtimes have all moved on.
        let mut ahead = Vec::new();
        if matches!(status.as_str(), "rolling" | "paused" | "halted") {
            let runtimes = sqlx::query_as::<_, (Uuid, String)>(
                "SELECT id,deployed_ref FROM runtimes WHERE deployed_ref IS NOT NULL AND deployed_ref<>$1",
            )
            .bind(&git_sha)
            .fetch_all(&s.db)
            .await?;
            for (runtime_id, deployed) in runtimes {
                if release_order::compare(&s.db, &git_sha, &deployed).await
                    == Ok(release_order::Order::Ahead)
                {
                    ahead.push(runtime_id);
                }
            }
        }
        rollouts.push(Rollout {
            id,
            git_sha,
            status,
            reason,
            created_at,
            finished_at,
            stages,
            deployments,
            ahead,
        });
    }
    Ok(Json(rollouts))
}

async fn transition(
    s: &Shared,
    headers: &HeaderMap,
    id: Uuid,
    statement: &str,
    refused: &str,
    detail: &str,
) -> Result<StatusCode, ApiError> {
    let (admin_user_id, _) = admin_user(headers, &s.db).await?;
    let changed = sqlx::query(statement)
        .bind(id)
        .execute(&s.db)
        .await?
        .rows_affected();
    if changed == 0 {
        return Err(ApiError(StatusCode::CONFLICT, refused.into()));
    }
    audit(
        &s.db,
        admin_user_id,
        None,
        serde_json::json!({"rollout": id, "action": detail}),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Stop starting releases. One already running finishes.
pub(crate) async fn pause(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    transition(
        &s,
        &headers,
        id,
        "UPDATE release_rollouts SET status='paused',reason='Paused by an operator.',updated_at=now() WHERE id=$1 AND status IN ('rolling','halted')",
        "Only a rollout in flight can be paused.",
        "pause",
    )
    .await
}

/// Carry on, with every runtime's attempts given back.
pub(crate) async fn resume(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let status = transition(
        &s,
        &headers,
        id,
        "UPDATE release_rollouts SET status='rolling',reason=NULL,attempts_since=now(),updated_at=now() WHERE id=$1 AND status IN ('paused','halted')",
        "Only a paused or halted rollout can be resumed.",
        "resume",
    )
    .await?;
    let _ = step(&s).await;
    Ok(status)
}

/// Let the rollout past the channel it is in: its soak, and the hold on
/// unhealthy tenants, no longer stop the next channel.
pub(crate) async fn promote(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let (admin_user_id, _) = admin_user(&headers, &s.db).await?;
    let rollout = active(&s.db)
        .await?
        .filter(|rollout| rollout.id == id && rollout.status == "rolling")
        .ok_or_else(|| {
            ApiError(
                StatusCode::CONFLICT,
                "Only a rolling rollout can be promoted.".into(),
            )
        })?;
    let channels = channels(&s.db, &rollout).await?;
    // The channel it is in: the first that has not been promoted and is either
    // unfinished or still soaking.
    let last = channels.len().saturating_sub(1);
    let current = channels.iter().enumerate().find(|(index, channel)| {
        channel.promoted_at.is_none()
            && channel.runtimes.iter().any(RuntimeState::takes_part)
            && (channel.completed_at.is_none() || *index < last)
    });
    let Some((_, channel)) = current else {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "There is no channel left to promote past.".into(),
        ));
    };
    sqlx::query("INSERT INTO release_rollout_stages(rollout_id,channel_id,promoted_at,promoted_by) VALUES($1,$2,now(),$3) ON CONFLICT (rollout_id,channel_id) DO UPDATE SET promoted_at=now(),promoted_by=$3")
        .bind(rollout.id)
        .bind(channel.id)
        .bind(admin_user_id)
        .execute(&s.db)
        .await?;
    audit(
        &s.db,
        admin_user_id,
        None,
        serde_json::json!({"rollout": id, "action": "promote", "channel": channel.name}),
    )
    .await?;
    let _ = step(&s).await;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime(name: &str, on_release: bool) -> RuntimeState {
        RuntimeState {
            id: Uuid::new_v4(),
            name: name.into(),
            follows: true,
            has_tenants: true,
            on_release,
            failures: 0,
            last_failure_at: None,
            unhealthy: 0,
            held: None,
        }
    }

    fn channel(name: &str, soak_minutes: i32, runtimes: Vec<RuntimeState>) -> ChannelState {
        ChannelState {
            id: Uuid::new_v4(),
            name: name.into(),
            soak_minutes,
            completed_at: None,
            promoted_at: None,
            runtimes,
        }
    }

    fn now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
    }

    #[test]
    fn the_first_channel_takes_the_release_first() {
        let channels = [
            channel("canary", 60, vec![runtime("a", false)]),
            channel("stable", 0, vec![runtime("b", false)]),
        ];
        assert_eq!(
            plan(&channels, false, now()),
            Action::Release {
                channel_id: channels[0].id,
                runtime_id: channels[0].runtimes[0].id
            }
        );
    }

    #[test]
    fn a_channel_whose_runtimes_all_run_it_completes() {
        let channels = [
            channel("canary", 60, vec![runtime("a", true)]),
            channel("stable", 0, vec![runtime("b", false)]),
        ];
        assert_eq!(
            plan(&channels, false, now()),
            Action::CompleteStage {
                channel_id: channels[0].id
            }
        );
    }

    #[test]
    fn the_next_channel_waits_out_the_soak() {
        let mut channels = [
            channel("canary", 60, vec![runtime("a", true)]),
            channel("stable", 0, vec![runtime("b", false)]),
        ];
        channels[0].completed_at = Some(now() - time::Duration::minutes(20));
        assert_eq!(
            plan(&channels, false, now()),
            Action::Wait("Soaking in canary for 40 more minutes.".into())
        );
        channels[0].completed_at = Some(now() - time::Duration::minutes(61));
        assert_eq!(
            plan(&channels, false, now()),
            Action::Release {
                channel_id: channels[1].id,
                runtime_id: channels[1].runtimes[0].id
            }
        );
    }

    #[test]
    fn unhealthy_tenants_hold_the_rollout_until_promoted() {
        let mut broken = runtime("a", true);
        broken.unhealthy = 2;
        let mut channels = [
            channel("canary", 0, vec![broken, runtime("c", false)]),
            channel("stable", 0, vec![runtime("b", false)]),
        ];
        assert_eq!(
            plan(&channels, false, now()),
            Action::Wait(
                "2 tenants are unhealthy in canary since the release; promote to carry on anyway."
                    .into()
            )
        );
        channels[0].promoted_at = Some(now());
        assert_eq!(
            plan(&channels, false, now()),
            Action::Release {
                channel_id: channels[0].id,
                runtime_id: channels[0].runtimes[1].id
            }
        );
        // A tenant unwell before the release reached a runtime does not hold it.
        let mut unwell_before = runtime("a", false);
        unwell_before.unhealthy = 1;
        let channels = [channel("canary", 0, vec![unwell_before])];
        assert!(matches!(
            plan(&channels, false, now()),
            Action::Release { .. }
        ));
    }

    #[test]
    fn repeated_failures_halt_and_a_recent_one_waits() {
        let mut failing = runtime("a", false);
        failing.failures = ATTEMPTS;
        let channels = [channel("canary", 0, vec![failing.clone()])];
        assert_eq!(
            plan(&channels, false, now()),
            Action::Halt("a failed to take the release 3 times.".into())
        );
        failing.failures = 1;
        failing.last_failure_at = Some(now() - time::Duration::minutes(1));
        let channels = [channel("canary", 0, vec![failing.clone()])];
        assert_eq!(
            plan(&channels, false, now()),
            Action::Wait("Trying a again shortly after its failed release.".into())
        );
        failing.last_failure_at = Some(now() - time::Duration::minutes(6));
        let channels = [channel("canary", 0, vec![failing])];
        assert!(matches!(
            plan(&channels, false, now()),
            Action::Release { .. }
        ));
    }

    #[test]
    fn pinned_empty_and_moving_runtimes() {
        let mut pinned = runtime("pinned", false);
        pinned.follows = false;
        let mut empty = runtime("empty", false);
        empty.has_tenants = false;
        // A channel with nothing that takes part is passed over without a soak.
        let channels = [
            channel("canary", 60, vec![pinned, empty]),
            channel("stable", 0, vec![runtime("b", true)]),
        ];
        assert_eq!(
            plan(&channels, false, now()),
            Action::CompleteStage {
                channel_id: channels[1].id
            }
        );
        let mut moving = runtime("moving", false);
        moving.held = Some("A tenant is moving on or off moving.".into());
        let channels = [channel("canary", 0, vec![moving.clone()])];
        assert_eq!(
            plan(&channels, false, now()),
            Action::Wait("A tenant is moving on or off moving.".into())
        );
        let channels = [channel("canary", 0, vec![moving, runtime("other", false)])];
        assert_eq!(
            plan(&channels, false, now()),
            Action::Release {
                channel_id: channels[0].id,
                runtime_id: channels[0].runtimes[1].id
            }
        );
    }

    #[test]
    fn one_release_at_a_time_and_the_end() {
        let channels = [channel("canary", 0, vec![runtime("a", false)])];
        assert_eq!(
            plan(&channels, true, now()),
            Action::Wait("Waiting for the release in progress to finish.".into())
        );
        let mut done = [
            channel("canary", 60, vec![runtime("a", true)]),
            channel("stable", 60, vec![runtime("b", true)]),
        ];
        done[0].completed_at = Some(now() - time::Duration::minutes(61));
        done[1].completed_at = Some(now());
        // The last channel does not soak.
        assert_eq!(plan(&done, false, now()), Action::Finish);
        assert_eq!(plan(&[], false, now()), Action::Finish);
    }
}

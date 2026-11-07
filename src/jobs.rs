//! Durable tenant lifecycle work, run from PostgreSQL.
//!
//! A job is a row in `lifecycle_jobs`, requested in the same transaction as the
//! state change that needs it. It holds no position of its own: every run walks
//! its steps from the first, and the step tables say which are already done, so
//! a run that dies is simply run again once its lease expires.
//! *docs/tenant-lifecycle.md* has the states these move a tenant through.

use std::{panic::AssertUnwindSafe, sync::Arc, time::Duration};

use anyhow::Result;
use futures::FutureExt;
use sqlx::PgPool;
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::{
    DEPROVISION, LIFECYCLE_RETRY_SECONDS, LifecycleFlow, PURGE, ProvisionStepOutcome, Shared,
    finish_provisioning, local_runtimes, provision_tenant_step, reconcile_runtime_drift,
    run_lifecycle_flow,
};

/// How long a job that found nothing to do waits for the next one to come due.
const POLL_INTERVAL: Duration = Duration::from_secs(3);

/// How many jobs run at once. A waiting job holds no slot, so these are only
/// ever spent on work that is really happening.
const MAX_RUNNING: usize = 16;

/// How long a run may go unheard from before its job is given to another. This
/// is what notices a control plane that died, however long the step's deadline.
const LEASE: Duration = Duration::from_secs(120);

/// How often a running job renews its lease. Well inside `LEASE`, so that one
/// slow renewal does not hand the job over.
const LEASE_RENEWAL: Duration = Duration::from_secs(20);

/// How long a step that is not ready yet waits before it is asked again. A wake
/// cuts this short; missing one only costs the rest of the wait.
const WAIT: Duration = Duration::from_secs(30);

/// What a step gets when it is bounded by work rather than by data. Long enough
/// never to be why a slow step fails: a lease notices a dead control plane, so
/// this is only for a step that has hung.
const STEP_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// What a step gets when it is bounded by how much of somebody's server there
/// is. A restore is one transaction over a whole instance and a media upload is
/// its whole library; neither is improved by being killed partway.
const LONG_STEP_TIMEOUT: Duration = Duration::from_secs(12 * 60 * 60);

/// The steps that move an instance's data, rather than deciding something about
/// it.
const LONG_STEPS: [&str; 2] = ["import", "media"];

/// Every step a tenant can have, in the only order they make sense in. A tenant
/// without one — a created tenant has no `import`, an imported one no `owner` —
/// completes it without running anything.
const PROVISION_STEPS: [&str; 9] = [
    "database", "migrate", "import", "render", "media", "reload", "health", "routing", "owner",
];

/// The longest a retry waits after something unexpected.
const MAX_BACKOFF: Duration = Duration::from_secs(100);

/// How often the retention sweep looks for a tenant whose time has come.
const PURGE_SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// How often the reconciler looks for a runtime that is due a pass.
const RECONCILE_POLL: Duration = Duration::from_secs(10);

/// How long between reconciliation passes over a runtime. Nothing asks for one:
/// configuration is written straight to PostgreSQL, by a migration or by hand,
/// so there is no code path that could. A tenant is at most this far behind its
/// stored configuration.
const RECONCILE_INTERVAL: Duration = Duration::from_secs(60);

/// A reload signals launchd and then waits on the runtime, so a pass has to
/// cover a whole fleet's worth of health checks.
const RECONCILE_TIMEOUT: Duration = Duration::from_secs(600);

fn step_timeout(step: &str) -> Duration {
    match LONG_STEPS.contains(&step) {
        true => LONG_STEP_TIMEOUT,
        false => STEP_TIMEOUT,
    }
}

/// How long after this many failures in a row to try again.
fn backoff(failures: i32) -> Duration {
    let exponent = failures.clamp(0, 16) as u32;
    Duration::from_secs(2_u64.pow(exponent)).min(MAX_BACKOFF)
}

/// One claimed run of a job.
#[derive(Debug, sqlx::FromRow)]
pub(crate) struct Job {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub kind: String,
    /// The requests this run has seen. One more arriving before it ends means
    /// it has to run again.
    pub generation: i64,
    pub failures: i32,
}

/// What a run of a job asks for next.
#[derive(Debug, PartialEq)]
pub(crate) enum Next {
    Done,
    /// Not ready yet, which is not a failure.
    After(Duration),
    /// Something went wrong that another run may get past. The message reaches
    /// the operators' console, so it must be fixed text, never a driver error.
    Retry(String, Duration),
}

/// Request a job. A tenant has one live job of a kind, so a request for one that
/// exists runs that one again, from the top, even if it is running now.
pub(crate) async fn enqueue(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: Uuid,
    kind: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO lifecycle_jobs(id,tenant_id,kind) VALUES($1,$2,$3) ON CONFLICT (tenant_id,kind) WHERE finished_at IS NULL DO UPDATE SET generation=lifecycle_jobs.generation+1,run_after=now(),failures=0,last_error=NULL")
        .bind(Uuid::new_v4())
        .bind(tenant_id)
        .bind(kind)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Bring a waiting provision job forward, after PostgreSQL has committed a
/// condition that can unblock it. Best effort: if this fails, the job's own wait
/// still ends.
pub async fn wake_provisioning(state: &Shared, tenant_id: Uuid) {
    if let Err(_error) = sqlx::query("UPDATE lifecycle_jobs SET generation=generation+1,run_after=now() WHERE tenant_id=$1 AND kind='provision' AND finished_at IS NULL")
        .bind(tenant_id)
        .execute(&state.db)
        .await
    {
        tracing::warn!(%tenant_id, "could not wake provisioning");
    }
}

/// Take the job that has been due longest, if there is one, under a lease.
pub(crate) async fn claim(db: &PgPool) -> Result<Option<Job>, sqlx::Error> {
    sqlx::query_as(
        "UPDATE lifecycle_jobs SET lease_until=now()+make_interval(secs => $1) WHERE id=(SELECT id FROM lifecycle_jobs WHERE finished_at IS NULL AND run_after<=now() AND (lease_until IS NULL OR lease_until<now()) ORDER BY run_after,created_at LIMIT 1 FOR UPDATE SKIP LOCKED) RETURNING id,tenant_id,kind,generation,failures",
    )
    .bind(LEASE.as_secs_f64())
    .fetch_optional(db)
    .await
}

/// Record what a run asked for. A request that arrived during the run wins over
/// all of it: the run may have read the state from before that request, so the
/// job runs again now, whatever this run concluded.
pub(crate) async fn settle(db: &PgPool, job: &Job, next: &Next) -> Result<(), sqlx::Error> {
    let (finished, delay, error) = match next {
        Next::Done => (true, Duration::ZERO, None),
        Next::After(delay) => (false, *delay, None),
        Next::Retry(error, delay) => (false, *delay, Some(error.as_str())),
    };
    sqlx::query(
        "UPDATE lifecycle_jobs SET lease_until=NULL,
            finished_at=CASE WHEN generation=$2 AND $3 THEN now() END,
            run_after=CASE WHEN generation=$2 THEN now()+make_interval(secs => $4) ELSE now() END,
            failures=CASE WHEN generation=$2 AND $5::text IS NOT NULL THEN failures+1 WHEN generation=$2 THEN 0 ELSE failures END,
            last_error=CASE WHEN generation=$2 THEN $5 ELSE last_error END
         WHERE id=$1",
    )
    .bind(job.id)
    .bind(job.generation)
    .bind(finished)
    .bind(delay.as_secs_f64())
    .bind(error)
    .execute(db)
    .await?;
    Ok(())
}

/// Provision a tenant, one step at a time.
async fn provision(state: &Shared, tenant_id: Uuid) -> Next {
    // What a tenant has to wait for is each step's to say: `database` holds back
    // an instance whose custom domain has not validated, and `routing` holds the
    // end of the job until its hostnames are active. A provided hostname only
    // goes active once the tenant is served, so nothing here can wait for one.
    for step in PROVISION_STEPS {
        let outcome = tokio::time::timeout(
            step_timeout(step),
            provision_tenant_step(state, tenant_id, step),
        )
        .await;
        match outcome {
            Ok(ProvisionStepOutcome::Completed) => {}
            Ok(ProvisionStepOutcome::Wait) => return Next::After(WAIT),
            Ok(ProvisionStepOutcome::Halted | ProvisionStepOutcome::Failed) => return Next::Done,
            Err(_) => {
                return Next::Retry(format!("{step} timed out; retrying."), WAIT);
            }
        }
    }
    finish_provisioning(state, tenant_id).await;
    Next::Done
}

/// Move a served instance to another domain, the same way: each step either
/// completes, asks to wait, or ends the change.
async fn domain_change(state: &Shared, tenant_id: Uuid) -> Next {
    for step in crate::domain_change::STEPS {
        let outcome = tokio::time::timeout(
            STEP_TIMEOUT,
            crate::domain_change::step(state, tenant_id, step),
        )
        .await;
        match outcome {
            Ok(ProvisionStepOutcome::Completed) => {}
            Ok(ProvisionStepOutcome::Wait) => return Next::After(WAIT),
            Ok(ProvisionStepOutcome::Halted | ProvisionStepOutcome::Failed) => return Next::Done,
            Err(_) => {
                return Next::Retry(format!("{step} timed out; retrying."), WAIT);
            }
        }
    }
    Next::Done
}

/// Take a tenant offline or destroy it, resuming at its first unfinished step.
/// The flow counts its own attempts and ends once it has used them. What it asks
/// to retry waits until the step that failed is old enough to be tried again.
async fn lifecycle(state: &Shared, tenant_id: Uuid, flow: &LifecycleFlow) -> Next {
    match tokio::time::timeout(
        LONG_STEP_TIMEOUT,
        run_lifecycle_flow(state, tenant_id, flow),
    )
    .await
    {
        Ok(Ok(())) => Next::Done,
        Ok(Err(retry)) => Next::Retry(
            retry.to_string(),
            Duration::from_secs(LIFECYCLE_RETRY_SECONDS as u64 + 5),
        ),
        Err(_) => Next::Retry("The lifecycle step timed out; retrying.".into(), WAIT),
    }
}

async fn execute(state: &Shared, job: &Job) -> Next {
    match job.kind.as_str() {
        "provision" => provision(state, job.tenant_id).await,
        "deprovision" => lifecycle(state, job.tenant_id, &DEPROVISION).await,
        "purge" => lifecycle(state, job.tenant_id, &PURGE).await,
        "domain_change" => domain_change(state, job.tenant_id).await,
        "runtime_move" => crate::tenant_move::run(state, job.tenant_id, job.failures).await,
        kind => {
            tracing::error!(kind, "unknown lifecycle job");
            Next::Done
        }
    }
}

/// One run of a job, holding its lease until it ends. A panic is a failure like
/// any other: every step can run again, so the job gets another run.
pub(crate) async fn run(state: &Shared, job: &Job) {
    let work = AssertUnwindSafe(execute(state, job)).catch_unwind();
    tokio::pin!(work);
    let next = loop {
        tokio::select! {
            result = &mut work => break result.unwrap_or_else(|_| {
                tracing::error!(job = %job.id, "lifecycle job panicked");
                Next::Retry("The lifecycle job stopped unexpectedly; retrying.".into(), backoff(job.failures))
            }),
            _ = tokio::time::sleep(LEASE_RENEWAL) => {
                // A failed renewal is PostgreSQL being away, which this run has
                // no better answer to than carrying on.
                let _ = sqlx::query("UPDATE lifecycle_jobs SET lease_until=now()+make_interval(secs => $1) WHERE id=$2")
                    .bind(LEASE.as_secs_f64())
                    .bind(job.id)
                    .execute(&state.db)
                    .await;
            }
        }
    };
    // If this cannot be recorded the lease runs out and the job runs again,
    // which every step allows.
    if settle(&state.db, job, &next).await.is_err() {
        tracing::warn!(job = %job.id, "could not record a lifecycle job's outcome");
    }
}

/// Runs what is due, for as long as the control plane runs.
pub async fn runner(state: Shared) {
    let slots = Arc::new(Semaphore::new(MAX_RUNNING));
    loop {
        while let Ok(slot) = slots.clone().try_acquire_owned() {
            match claim(&state.db).await {
                Ok(Some(job)) => {
                    let state = state.clone();
                    tokio::spawn(async move {
                        run(&state, &job).await;
                        drop(slot);
                    });
                }
                Ok(None) => break,
                Err(_error) => {
                    tracing::warn!("could not read the lifecycle job queue");
                    break;
                }
            }
        }
        state.health.tick("lifecycle-jobs");
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Starts the purge of every retained tenant whose retention has ended. Each is
/// decided again under its row lock, where a restore wins.
pub(crate) async fn sweep_due_purges(state: &Shared) -> Result<()> {
    let due = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM tenants WHERE lifecycle_state='retained' AND purge_scheduled_for<=now()",
    )
    .fetch_all(&state.db)
    .await?;
    for tenant_id in due {
        schedule_due_purge(state, tenant_id).await?;
    }
    Ok(())
}

pub async fn purge_sweeper(state: Shared) {
    loop {
        if sweep_due_purges(&state).await.is_err() {
            tracing::warn!("could not start the purges that are due");
        }
        state.health.tick("purge-sweep");
        tokio::time::sleep(PURGE_SWEEP_INTERVAL).await;
    }
}

pub(crate) async fn schedule_due_purge(state: &Shared, tenant_id: Uuid) -> Result<()> {
    let mut tx = state.db.begin().await?;
    let due = sqlx::query_scalar::<_, bool>(
        "SELECT true FROM tenants WHERE id=$1 AND lifecycle_state='retained' AND purge_scheduled_for <= now() FOR UPDATE",
    )
    .bind(tenant_id)
    .fetch_optional(&mut *tx)
    .await
    ?
    .is_some();
    if due {
        crate::start_purge(
            &mut tx,
            tenant_id,
            None,
            None,
            true,
            "The retention period ended without a restore; the tenant's data is being destroyed.",
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Take a runtime's next reconciliation pass, if it is due and none is in
/// flight. The date is moved past the pass's own timeout, so a control plane that
/// dies mid-pass holds the runtime up for no longer than that.
pub(crate) async fn claim_reconcile(db: &PgPool, runtime_id: Uuid) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query_scalar::<_, bool>(
        "UPDATE runtimes SET reconcile_next_at=now()+make_interval(secs => $2),reconcile_lease_until=now()+make_interval(secs => $2),reconcile_requested=false WHERE id=$1 AND (reconcile_next_at<=now() OR reconcile_requested) AND (reconcile_lease_until IS NULL OR reconcile_lease_until<=now()) RETURNING true",
    )
    .bind(runtime_id)
    .bind((RECONCILE_TIMEOUT + RECONCILE_INTERVAL).as_secs_f64())
    .fetch_optional(db)
    .await?
    .is_some())
}

/// One pass over a runtime: scanning, rendering, reloading and checking are its
/// business, and the next one is due a minute after it ends.
async fn reconcile_pass(state: &Shared, runtime_id: Uuid) {
    let pass = tokio::time::timeout(
        RECONCILE_TIMEOUT,
        AssertUnwindSafe(reconcile_runtime_drift(state, runtime_id)).catch_unwind(),
    )
    .await;
    match pass {
        Ok(Ok(Ok(converged))) if converged > 0 => {
            tracing::info!(%runtime_id, converged, "reconciled drifted tenants");
        }
        Ok(Ok(Ok(_))) => {}
        _ => tracing::warn!(%runtime_id, "runtime reconciliation failed; it will run again"),
    }
    let _ = sqlx::query(
        "UPDATE runtimes SET reconcile_next_at=CASE WHEN reconcile_requested THEN now() ELSE now()+make_interval(secs => $2) END,reconcile_lease_until=NULL WHERE id=$1",
    )
    .bind(runtime_id)
    .bind(RECONCILE_INTERVAL.as_secs_f64())
    .execute(&state.db)
    .await;
}

/// Keeps every runtime on this host on its stored configuration. A controller
/// rather than a job: it has no end, and nothing asks for it.
pub async fn reconciler(state: Shared) {
    loop {
        match local_runtimes(&state).await {
            Ok(runtimes) => {
                for runtime_id in runtimes {
                    match claim_reconcile(&state.db, runtime_id).await {
                        Ok(true) => {
                            let state = state.clone();
                            tokio::spawn(async move { reconcile_pass(&state, runtime_id).await });
                        }
                        Ok(false) => {}
                        Err(_error) => {
                            tracing::warn!(%runtime_id, "could not claim a reconciliation pass")
                        }
                    }
                }
            }
            Err(_error) => tracing::warn!("could not read the runtimes to reconcile"),
        }
        state.health.tick("reconcile");
        tokio::time::sleep(RECONCILE_POLL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_steps_that_move_data_are_given_hours() {
        for step in ["import", "media"] {
            assert_eq!(step_timeout(step), LONG_STEP_TIMEOUT);
        }
        for step in [
            "database", "migrate", "render", "reload", "health", "routing", "owner",
        ] {
            assert_eq!(step_timeout(step), STEP_TIMEOUT);
        }
    }

    #[test]
    fn a_lease_outlasts_several_renewals() {
        assert!(LEASE >= LEASE_RENEWAL * 3);
    }

    #[test]
    fn backoff_grows_to_its_limit() {
        assert_eq!(backoff(0), Duration::from_secs(1));
        assert_eq!(backoff(3), Duration::from_secs(8));
        assert_eq!(backoff(1000), MAX_BACKOFF);
    }
}

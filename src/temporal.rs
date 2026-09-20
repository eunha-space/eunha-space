//! Temporal workers for durable tenant lifecycle operations.
//!
//! Workflow code is intentionally limited to stable identifiers and activity
//! orchestration. All reads, writes, and external side effects stay in the
//! activity so replay never sees non-deterministic I/O or tenant secrets.

use std::{sync::Arc, time::Duration};

use anyhow::Result;
use temporalio_client::{
    Client, ClientOptions, Connection, WorkflowSignalOptions, WorkflowStartOptions,
    envconfig::LoadClientConfigProfileOptions, errors::WorkflowStartError,
};
use temporalio_macros::{activities, workflow, workflow_methods};
use temporalio_sdk::{
    ActivityOptions, ContinueAsNewOptions, Runtime, SyncWorkflowContext, Worker, WorkerOptions,
    WorkflowContext, WorkflowContextView, WorkflowResult,
    activities::{ActivityContext, ActivityError},
};
use uuid::Uuid;

use crate::{
    DEPROVISION, PURGE, ProvisionStepOutcome, Shared, finish_provisioning, local_runtimes,
    provision_tenant_step, reconcile_runtime_drift, run_lifecycle_flow,
};

const TASK_QUEUE: &str = "eunha-space-lifecycle";

struct TenantActivities {
    state: Shared,
}

#[activities]
impl TenantActivities {
    #[activity]
    async fn provision_step(
        self: Arc<Self>,
        _ctx: ActivityContext,
        request: String,
    ) -> Result<ProvisionStepOutcome, ActivityError> {
        let (tenant_id, step) = request
            .split_once(':')
            .ok_or_else(|| ActivityError::from(anyhow::anyhow!("invalid provisioning step")))?;
        let tenant_id = Uuid::parse_str(tenant_id).map_err(ActivityError::from)?;
        Ok(provision_tenant_step(&self.state, tenant_id, step).await)
    }

    #[activity]
    async fn finish_provision(
        self: Arc<Self>,
        _ctx: ActivityContext,
        tenant_id: String,
    ) -> Result<(), ActivityError> {
        let tenant_id = Uuid::parse_str(&tenant_id).map_err(ActivityError::from)?;
        finish_provisioning(&self.state, tenant_id).await;
        Ok(())
    }

    #[activity]
    async fn lifecycle(
        self: Arc<Self>,
        _ctx: ActivityContext,
        request: String,
    ) -> Result<(), ActivityError> {
        let (flow, tenant_id) = lifecycle_request(&request).map_err(ActivityError::from)?;
        run_lifecycle_flow(&self.state, tenant_id, flow).await;
        Ok(())
    }

    #[activity]
    async fn reconcile_runtime(
        self: Arc<Self>,
        _ctx: ActivityContext,
        runtime_id: String,
    ) -> Result<(), ActivityError> {
        let runtime_id = Uuid::parse_str(&runtime_id).map_err(ActivityError::from)?;
        let converged = reconcile_runtime_drift(&self.state, runtime_id)
            .await
            .map_err(ActivityError::from)?;
        if converged > 0 {
            tracing::info!(%runtime_id, converged, "reconciled drifted tenants");
        }
        Ok(())
    }

    #[activity]
    async fn schedule_due_purge(
        self: Arc<Self>,
        _ctx: ActivityContext,
        tenant_id: String,
    ) -> Result<(), ActivityError> {
        let tenant_id = Uuid::parse_str(&tenant_id).map_err(ActivityError::from)?;
        let mut tx = self.state.db.begin().await.map_err(ActivityError::from)?;
        let due = sqlx::query_scalar::<_, bool>(
            "SELECT true FROM tenants WHERE id=$1 AND lifecycle_state='retained' AND purge_scheduled_for <= now() FOR UPDATE",
        )
        .bind(tenant_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(ActivityError::from)?
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
            .await
            .map_err(ActivityError::from)?;
        }
        tx.commit().await.map_err(ActivityError::from)?;
        Ok(())
    }
}

fn lifecycle_request(request: &str) -> Result<(&'static crate::LifecycleFlow, Uuid)> {
    let (kind, tenant_id) = request
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("invalid lifecycle workflow request"))?;
    let flow = match kind {
        "deprovision" => &DEPROVISION,
        "purge" => &PURGE,
        _ => anyhow::bail!("unknown lifecycle workflow"),
    };
    Ok((flow, Uuid::parse_str(tenant_id)?))
}

#[workflow]
struct TenantProvisionWorkflow {
    tenant_id: String,
    wake_version: u32,
    wait_attempts: u16,
}

#[workflow_methods]
impl TenantProvisionWorkflow {
    #[init]
    fn new(_ctx: &WorkflowContextView, tenant_id: String) -> Self {
        Self {
            tenant_id,
            wake_version: 0,
            wait_attempts: 0,
        }
    }

    #[run]
    async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<()> {
        let tenant_id = ctx.state(|state| state.tenant_id.clone());
        for step in [
            "database", "migrate", "render", "reload", "health", "routing", "owner",
        ] {
            loop {
                let outcome = ctx
                    .execute_activity(
                        TenantActivities::provision_step,
                        format!("{tenant_id}:{step}"),
                        ActivityOptions::with_start_to_close_timeout(Duration::from_secs(300))
                            .build(),
                    )
                    .await?;
                match outcome {
                    ProvisionStepOutcome::Completed => break,
                    ProvisionStepOutcome::Wait => {
                        let wake_version = ctx.state(|state| state.wake_version);
                        let wait_attempts = ctx.state_mut(|state| {
                            state.wait_attempts += 1;
                            state.wait_attempts
                        });
                        let timer_fired = temporalio_sdk::workflows::select! {
                            _ = ctx.timer(Duration::from_secs(30)) => true,
                            _ = ctx.wait_condition(|state: &Self| state.wake_version != wake_version) => false,
                        };
                        // A workflow that waits for DNS or capacity for a long time must
                        // not grow one unbounded history. Completed activities are
                        // idempotently skipped after the new run starts.
                        if timer_fired && wait_attempts >= 240 {
                            ctx.continue_as_new(
                                tenant_id.clone(),
                                ContinueAsNewOptions::default(),
                            )?;
                        }
                    }
                    ProvisionStepOutcome::Halted | ProvisionStepOutcome::Failed => return Ok(()),
                }
            }
        }
        ctx.execute_activity(
            TenantActivities::finish_provision,
            tenant_id,
            ActivityOptions::with_start_to_close_timeout(Duration::from_secs(60)).build(),
        )
        .await?;
        Ok(())
    }

    /// Wake a workflow waiting for a domain, routing, or lifecycle change.
    #[signal]
    fn wake(&mut self, _ctx: &mut SyncWorkflowContext<Self>) {
        self.wake_version += 1;
    }
}

/// A controller for one runtime, not a unit of work for one tenant.
///
/// PostgreSQL holds what a tenant's configuration should be and the rendered
/// file holds what it is; nothing used to close the gap but an operator
/// pressing render. This workflow runs for as long as the runtime does,
/// comparing the two and rendering what differs. Temporal owns the timer, the
/// retry, and — through a workflow ID fixed per runtime — the guarantee that
/// only one pass is ever in flight, which the in-process reload lock cannot
/// give once a second control plane exists.
#[workflow]
struct RuntimeReconcileWorkflow {
    runtime_id: String,
    passes: u16,
}

#[workflow_methods]
impl RuntimeReconcileWorkflow {
    #[init]
    fn new(_ctx: &WorkflowContextView, runtime_id: String) -> Self {
        Self {
            runtime_id,
            passes: 0,
        }
    }

    #[run]
    async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<()> {
        let runtime_id = ctx.state(|state| state.runtime_id.clone());
        loop {
            // Scanning, rendering, reloading and checking are one activity:
            // convergence is the activity's business, and the workflow owns
            // only the timer, the retry and the single execution. A reload
            // signals launchd and then waits on the runtime, so the timeout
            // has to cover a whole fleet's worth of health checks.
            ctx.execute_activity(
                TenantActivities::reconcile_runtime,
                runtime_id.clone(),
                ActivityOptions::with_start_to_close_timeout(Duration::from_secs(600)).build(),
            )
            .await?;

            ctx.timer(RECONCILE_INTERVAL).await;

            // This workflow never finishes, so its history has to be cut
            // somewhere. Nothing carries across but the runtime it watches:
            // the drift it acts on is read from PostgreSQL every pass.
            let passes = ctx.state_mut(|state| {
                state.passes += 1;
                state.passes
            });
            if passes >= 500 {
                ctx.continue_as_new(runtime_id.clone(), ContinueAsNewOptions::default())?;
            }
        }
    }
}

/// How long between reconciliation passes over a runtime. Nothing signals this
/// workflow: configuration is written straight to PostgreSQL, by a migration or
/// by hand, so there is no code path that could. A tenant is at most this far
/// behind its stored configuration, where it used to be behind until somebody
/// noticed.
const RECONCILE_INTERVAL: Duration = Duration::from_secs(60);

#[workflow]
struct TenantLifecycleWorkflow {
    request: String,
}

#[workflow_methods]
impl TenantLifecycleWorkflow {
    #[init]
    fn new(_ctx: &WorkflowContextView, request: String) -> Self {
        Self { request }
    }

    #[run]
    async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<()> {
        let request = ctx.state(|state| state.request.clone());
        ctx.execute_activity(
            TenantActivities::lifecycle,
            request.clone(),
            ActivityOptions::with_start_to_close_timeout(Duration::from_secs(300)).build(),
        )
        .await?;
        if let Some(tenant_id) = request.strip_prefix("deprovision:") {
            // The timer is persisted by Temporal. A restore changes the
            // PostgreSQL lifecycle state before it fires, making the guarded
            // activity below a no-op and preserving the retained data.
            ctx.timer(Duration::from_secs(7 * 24 * 60 * 60)).await;
            ctx.execute_activity(
                TenantActivities::schedule_due_purge,
                tenant_id.to_owned(),
                ActivityOptions::with_start_to_close_timeout(Duration::from_secs(60)).build(),
            )
            .await?;
            ctx.execute_activity(
                TenantActivities::lifecycle,
                format!("purge:{tenant_id}"),
                ActivityOptions::with_start_to_close_timeout(Duration::from_secs(300)).build(),
            )
            .await?;
        }
        Ok(())
    }
}

async fn client() -> Result<Client> {
    let (connection_options, client_options) = ClientOptions::load_from_config(
        LoadClientConfigProfileOptions::builder()
            .disable_file(true)
            .build(),
    )
    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let connection = Connection::connect(connection_options).await?;
    Ok(Client::new(connection, client_options)?)
}

pub async fn run_worker(state: Shared) -> Result<()> {
    let runtime = Runtime::from_current_tokio(Default::default())?;
    tracing::info!(task_queue = TASK_QUEUE, "connecting Temporal worker");
    let client = client().await?;
    tracing::info!(task_queue = TASK_QUEUE, "Temporal worker connected");
    let worker_options = WorkerOptions::new(TASK_QUEUE)
        .register_activities(TenantActivities { state })
        .register_workflow::<TenantProvisionWorkflow>()?
        .register_workflow::<TenantLifecycleWorkflow>()?
        .register_workflow::<RuntimeReconcileWorkflow>()?
        .build();
    let mut worker = Worker::new(&runtime, client, worker_options)?;
    tracing::info!(task_queue = TASK_QUEUE, "Temporal worker polling");
    worker.run().await?;
    Ok(())
}

/// Best-effort notification after PostgreSQL has committed a condition that can
/// unblock provisioning. PostgreSQL remains the source of truth: a missed
/// signal only falls back to the workflow's durable 30-second timer.
pub async fn wake_provisioning(state: &Shared, tenant_id: Uuid) {
    let workflow_id = match sqlx::query_scalar::<_, String>(
        "SELECT workflow_id FROM temporal_dispatches WHERE tenant_id=$1 AND workflow_kind='provision' AND delivered_at IS NOT NULL ORDER BY created_at DESC LIMIT 1",
    )
    .bind(tenant_id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(Some(workflow_id)) => workflow_id,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(error = %error, %tenant_id, "could not find provisioning workflow to wake");
            return;
        }
    };
    let client = match client().await {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(error = %error, %tenant_id, "could not connect to Temporal to wake provisioning");
            return;
        }
    };
    let handle = client.get_workflow_handle::<TenantProvisionWorkflow>(workflow_id);
    if let Err(error) = handle
        .signal(
            TenantProvisionWorkflow::wake,
            (),
            WorkflowSignalOptions::default(),
        )
        .await
    {
        tracing::debug!(error = %error, %tenant_id, "provisioning workflow was not available to wake");
    }
}

/// Dispatches lifecycle flows and backfills the durable retention timer for
/// tenants retained before Temporal was enabled. PostgreSQL stays the audit
/// record and the guard before any destructive transition.
pub async fn lifecycle_dispatcher(state: Shared) {
    loop {
        let queued = sqlx::query_as::<_, (Uuid, String, String, Uuid)>(
            "SELECT d.tenant_id,d.workflow_kind,d.workflow_id,d.id FROM temporal_dispatches d WHERE d.delivered_at IS NULL AND d.workflow_kind IN ('deprovision','purge') ORDER BY d.created_at LIMIT 10",
        )
        .fetch_all(&state.db)
        .await;
        match queued {
            Ok(tenants) => match client().await {
                Ok(client) => {
                    for (tenant_id, kind, workflow_id, dispatch_id) in tenants {
                        match client
                            .start_workflow(
                                TenantLifecycleWorkflow::run,
                                format!("{kind}:{tenant_id}"),
                                WorkflowStartOptions::new(TASK_QUEUE, workflow_id).build(),
                            )
                            .await
                        {
                            Ok(_) => {
                                let _ = sqlx::query("UPDATE temporal_dispatches SET delivered_at=now(),attempts=attempts+1,last_error=NULL WHERE id=$1").bind(dispatch_id).execute(&state.db).await;
                            }
                            Err(error) => {
                                let _ = sqlx::query("UPDATE temporal_dispatches SET attempts=attempts+1,last_error=$1 WHERE id=$2").bind(error.to_string()).bind(dispatch_id).execute(&state.db).await;
                            }
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(error = %error, "Temporal lifecycle dispatcher is unavailable")
                }
            },
            Err(error) => tracing::warn!(error = %error, "could not read lifecycle work queue"),
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

/// Keeps one reconcile workflow running per runtime on this host.
///
/// Unlike the lifecycle dispatchers there is no outbox behind this: a
/// reconciler is not a unit of queued work but a controller that should exist
/// whenever the runtime does, so the loop asserts that rather than draining
/// anything. The workflow ID is fixed per runtime, which is what makes the
/// assertion safe to repeat — Temporal refuses a duplicate while one is
/// running, and that refusal is this loop's success case.
pub async fn reconcile_starter(state: Shared) {
    loop {
        match local_runtimes(&state).await {
            Ok(runtimes) if !runtimes.is_empty() => match client().await {
                Ok(client) => {
                    for runtime_id in runtimes {
                        let workflow_id = format!("runtime-reconcile-v1-{runtime_id}");
                        match client
                            .start_workflow(
                                RuntimeReconcileWorkflow::run,
                                runtime_id.to_string(),
                                WorkflowStartOptions::new(TASK_QUEUE, &workflow_id).build(),
                            )
                            .await
                        {
                            Ok(_) => tracing::info!(%runtime_id, "started runtime reconciler"),
                            // Already running is the steady state, not a fault:
                            // the server rejecting the duplicate is what makes
                            // the fixed workflow ID a single-execution guarantee.
                            Err(WorkflowStartError::AlreadyStarted { .. }) => {}
                            Err(error) => {
                                tracing::warn!(error = %error, %runtime_id, "could not start runtime reconciler")
                            }
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(error = %error, "Temporal reconcile starter is unavailable")
                }
            },
            Ok(_) => {}
            Err(error) => tracing::warn!(error = %error, "could not read the runtimes to reconcile"),
        }
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::lifecycle_request;

    #[test]
    fn lifecycle_requests_accept_only_known_operations_and_tenant_ids() {
        let id = "44444444-4444-4444-4444-444444444444";
        assert_eq!(
            lifecycle_request(&format!("deprovision:{id}"))
                .unwrap()
                .0
                .running_state,
            "deprovisioning"
        );
        assert_eq!(
            lifecycle_request(&format!("purge:{id}"))
                .unwrap()
                .0
                .running_state,
            "purging"
        );
        assert!(lifecycle_request("drop:44444444-4444-4444-4444-444444444444").is_err());
        assert!(lifecycle_request("purge:not-a-tenant").is_err());
    }
}

/// Starts a workflow per queued tenant. The fixed workflow ID makes dispatch
/// idempotent: concurrent control-plane instances never create duplicate work.
pub async fn provisioning_dispatcher(state: Shared) {
    loop {
        let queued = sqlx::query_as::<_, (Uuid, String, Uuid)>(
            "SELECT d.tenant_id,d.workflow_id,d.id FROM temporal_dispatches d JOIN tenants t ON t.id=d.tenant_id WHERE d.delivered_at IS NULL AND d.workflow_kind='provision' AND t.lifecycle_state='provisioning' AND EXISTS(SELECT 1 FROM tenant_domains h WHERE h.tenant_id=d.tenant_id AND h.status='active') ORDER BY d.created_at LIMIT 10",
        )
        .fetch_all(&state.db)
        .await;
        match queued {
            Ok(tenant_ids) => match client().await {
                Ok(client) => {
                    for (tenant_id, workflow_id, dispatch_id) in tenant_ids {
                        match client
                            .start_workflow(
                                TenantProvisionWorkflow::run,
                                tenant_id.to_string(),
                                WorkflowStartOptions::new(TASK_QUEUE, workflow_id).build(),
                            )
                            .await
                        {
                            Ok(_) => {
                                let _ = sqlx::query("UPDATE temporal_dispatches SET delivered_at=now(),attempts=attempts+1,last_error=NULL WHERE id=$1").bind(dispatch_id).execute(&state.db).await;
                            }
                            Err(error) => {
                                let _ = sqlx::query("UPDATE temporal_dispatches SET attempts=attempts+1,last_error=$1 WHERE id=$2").bind(error.to_string()).bind(dispatch_id).execute(&state.db).await;
                            }
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(error = %error, "Temporal provisioning dispatcher is unavailable");
                }
            },
            Err(error) => {
                tracing::warn!(error = %error, "Temporal provisioning dispatcher is unavailable");
            }
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

# Tenant lifecycle

PostgreSQL is the source of truth for tenant state. Temporal records durable
workflow progress and timers; it never replaces the tenant row as the authority.
The provision, deprovision, and purge operations are audited separately.

```mermaid
stateDiagram-v2
    [*] --> provisioning: tenant created or restored

    provisioning --> provisioning: wait for domain, routing, or capacity\nTemporal timer (30 s) or wake signal
    provisioning --> ready: database → migrate → render → reload → health → routing → owner
    provisioning --> failed: terminal provisioning step failure
    failed --> provisioning: explicit retry

    provisioning --> deprovisioning: customer or admin takes offline
    ready --> deprovisioning: customer or admin takes offline
    failed --> deprovisioning: customer or admin takes offline

    deprovisioning --> retained: unrender → reload → offline check
    retained --> provisioning: restore cancels scheduled purge

    retained --> purging: seven-day Temporal timer fires\nor explicitly confirmed permanent deletion
    provisioning --> purging: explicitly confirmed permanent deletion
    ready --> purging: explicitly confirmed permanent deletion
    failed --> purging: explicitly confirmed permanent deletion
    purging --> purged: guarded destructive purge completes

    purged --> [*]
```

## Durable waiting and retrying

- A provision step that is not ready returns a durable wait outcome. Temporal
  persists a 30-second timer and rechecks that same step; earlier completed
  steps are skipped idempotently.
- A domain activation or lifecycle change sends a best-effort Temporal signal
  to wake the provision workflow immediately. Missing a signal is safe because
  the persisted timer remains.
- Unexpected activity failures use Temporal activity retry. Expected terminal
  provisioning errors are recorded on the tenant, step, and operation instead.
- Long-running provision workflows use continue-as-new after 240 timed waits,
  preventing unbounded workflow histories.

## Keeping a ready tenant on its configuration

The lifecycle workflows above each run once and end. One workflow does not:
`RuntimeReconcileWorkflow` exists for as long as its runtime does, and is what
makes a configuration change reach a tenant that is already serving.

Every write to `tenant_configuration` bumps the tenant's `config_revision`
through a database trigger, and a render records the revision it came from in
`rendered_revision`. The two differing is the only definition of drift. Once a
minute the workflow renders every ready tenant on its runtime whose revisions
differ, then reloads the runtime once for all of them — a reload makes the
runtime re-read its whole tenants directory, so a fleet that drifted together
costs one signal — and health-checks each tenant it touched.

The workflow ID is fixed per runtime, `runtime-reconcile-v1-<runtime id>`, so
the server refuses a duplicate and only one pass is ever in flight. The starter
asserts the workflow exists rather than queueing work for it, which is why
reconciliation has no outbox row: it is a controller, not an operation.

What it will not touch:

- A tenant that is not `ready` and `running`. One still provisioning has its own
  workflow rendering and reloading it; a retained or purging one was taken off
  disk deliberately.
- A runtime on another host. Only the control plane named by
  `CONTROL_RUNTIME_HOST` reconciles a runtime, and the render refuses anyway.
- A tenant that has failed five consecutive passes. The count and the reason sit
  on the tenant row, and an operator's render clears both.

A failed render does not hold back the reload for the rest of the runtime. A
failed reload does: every tenant rendered in that pass gives up its
`rendered_revision`, because a render that recorded its revision would leave no
drift for the next pass to find. Each pass writes one `reconcile` operation per
tenant, with no `requested_by`, which is what tells it apart from an operator's
render in the operations log.

## Watching an execution

The tenant inventory in `/admin` shows the durable workflow kind and the step
it is waiting on, taken from PostgreSQL. When `TEMPORAL_UI_URL` is configured,
that summary also links to the execution in the Temporal UI for history and
timers. The link is the only thing the console borrows from Temporal: workflow
input, activity results, and history stay in the Temporal UI, behind its own
private network, and never pass through this API. PostgreSQL remains the record
an operator acts on.

## Retention and destructive purge

Deprovisioning is deliberately non-destructive. A tenant reaches `retained`
only after it is offline; its database, media, configuration, backups, and
Cloudflare resources remain available for restoration. The retention timer is
seven days. Restore clears the scheduled purge in PostgreSQL, so the guarded
Temporal purge activity becomes a no-op even if its timer has already fired.
An immediate purge requires separate explicit confirmation and is audited.

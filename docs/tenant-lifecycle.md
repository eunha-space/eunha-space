# Tenant lifecycle

PostgreSQL is the source of truth for tenant state. Temporal records durable
workflow progress and timers; it never replaces the tenant row as the authority.
The provision, deprovision, and purge operations are audited separately.

Domains can be reused after purge completes. Disabled, retained, and purging
tenants continue reserving their primary names. Purged records retain their
original names, but no longer prevent another tenant from using them.

```mermaid
stateDiagram-v2
    [*] --> provisioning: tenant created or restored

    provisioning --> provisioning: wait for domain, routing, or capacity\nTemporal timer (30 s) or wake signal
    provisioning --> ready: database → migrate → render → reload → health → routing → owner
    provisioning --> ready: imported: database → migrate → import → render → media → reload → health → routing
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

## One list of steps, two kinds of tenant

The workflow iterates every step a tenant can have, in the only order they make
sense in. A tenant without one completes it without running anything: a created
tenant has no `import` or `media`, and an imported one has no `owner`, because
its owner account arrived with its database. Which steps a tenant has is decided
once, in PostgreSQL, when it is created or imported.

Adding a step to that list changes what a replay of an already-running
provision workflow expects. A workflow parked in a wait — the common long-lived
case, an instance whose custom domain has not validated — is unaffected, because
nothing after the step it is waiting on has run yet. One that is midway through
its steps at the moment the control plane restarts is not: deploy while no
tenant is actively provisioning, or expect that workflow to need a retry.

The two kinds also wait for opposite things. A created instance with a custom
domain provisions nothing until that domain validates, so an instance whose
domain never validates leaves nothing behind. An imported instance is restored,
rendered, served and checked *first*, and waits only at `routing` — because its
domain is already serving somewhere else, and moving it is the cutover. The
health check reaches the runtime directly with a `Host` header, so it proves the
instance works before any public DNS points at it.

## Durable waiting and retrying

- A provision step that is not ready returns a durable wait outcome. Temporal
  persists a 30-second timer and rechecks that same step; earlier completed
  steps are skipped idempotently.
- `import` and `media` are bounded by how much of somebody's server there is
  rather than by work, so they are given twelve hours instead of five minutes
  and heartbeat every twenty seconds. The heartbeat is what keeps a long
  deadline from also being how long a crashed worker goes unnoticed: a step
  that goes quiet for two minutes gives up that attempt.
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

## Changing a ready tenant's domain

A tenant's accounts are ActivityPub actors under its domain, so moving it to
another domain is a migration rather than a setting. Until eunha could send
Moves, a custom domain verified after the tenant had accounts only ever became
an alias. Now a customer or an operator can move a ready tenant to a provided
subdomain or a custom domain, and `TenantDomainChangeWorkflow` walks it through
six steps, recorded in `domain_changes`:

1. **attach**: the new hostname, with its provided siblings, is added as a
   pending domain and as an alias, so it is served and can be checked before
   anything else changes. The domain reconciler validates it through Cloudflare
   as it does any other. The change waits here, for up to seven days for a
   custom domain, and can be cancelled until it moves on; nothing about the
   tenant has changed yet, so a cancelled or failed attach leaves it where it
   was.
2. **rename**: `eunha rename-domain` rewrites the addresses the tenant has
   handed out under the old domain (its accounts', its posts', its follows').
   Without it, the new domain would serve posts claiming to be from the old
   one, which other servers refuse.
3. **switch**: the new hostname becomes primary, the old one leaves the
   configuration and joins `previous_domains`, and the tenant is rendered,
   reloaded and health-checked under the new domain alone. The old domain stops
   answering here, on purpose.
4. **move**: `eunha accounts move --from OLD --all` queues, as one batch, a Move
   from every account's old actor to its new one for its followers, signed with
   the old key id, which their servers already hold, and a Follow from every
   new actor to whoever the account followed, whose servers would otherwise go
   on delivering to the old one. Each actor names its old self in
   `alsoKnownAs`, because `previous_domains` is rendered into eunha's
   configuration, and that is what a server checks before it honours a Move.
5. **deliver**: the batch is followed with `eunha accounts batch-status` every 30
   seconds until nothing in it is pending. A server that has not taken its
   delivery within twelve hours is given up on, so the change always finishes;
   how many were given up on is recorded, and their users can follow again by
   hand.
6. **release**: the old hostnames are taken off Cloudflare and freed, a
   provided subdomain included, for anyone to take. Whoever takes one next
   answers at the old actors' addresses; a server that missed the Move and
   still follows an old actor would take a new one there for it, once the
   keys it holds stop verifying and it fetches that actor again.

A step that fails after `attach` leaves the tenant part-way moved, and the
change `failed` for an operator. Every step can run again (the rename is a
no-op the second time, and the Moves are not resent once their batch is
recorded), so **Retry** in the console's domains dialog continues from the step
that failed.

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

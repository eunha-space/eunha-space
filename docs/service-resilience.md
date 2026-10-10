# Control-plane resilience

## Health and operations

`GET /health/live` checks that HTTP is serving. `GET /health/ready` returns 200
only when PostgreSQL answers within two seconds, Temporal is reachable, its
worker is running, and the domain, release recovery, and Temporal dispatch
controllers have progressed in the last two minutes. Shutdown marks the process
as draining and stops accepting new HTTP requests. Health responses expose only
booleans and a release identifier; no addresses, credentials, or driver errors.

Background controllers are supervised and restarted after an unexpected exit.
The admin console checks readiness every thirty seconds and displays a warning
when dependency availability or background progress fails. The host checks
readiness every minute and logs its changes; it does not restart the service
when a dependency fails. An external uptime monitor can probe `/health/ready`.

API driver errors become fixed public messages with correlation IDs. Logs retain
only error type and ID, and HTTP traces omit query strings (including email-link
tokens). Temporal dispatch errors are generic. Authentication budgets are
persisted in PostgreSQL: ten attempts per identity and 200 per client IP in a
fifteen-minute window. Cloudflare client addresses are accepted only from a
loopback peer; this assumes the loopback listener is behind the trusted tunnel.
Password hashing runs on blocking threads with at most four concurrent jobs.

## Deployment and rollback

A deploy activates what the `release` job of `Checks` built for a commit on
`origin/main`, after its tests passed; nothing is compiled on the host. What it
guarantees, and what the code relies on:

- It refuses while a runtime release is running, and holds the
  `control_plane_maintenance` lease for its length, so the control plane starts
  none meanwhile.
- The executable and frontend assets switch together, and the prior release's
  assets stay available to browser sessions already open.
- The new release must report its own release ID from `/health/ready` within
  two minutes; otherwise the prior release is restored and checked.
- The runtime router helper comes from the release (`ops/` beside the
  executable), so every runtime release gets the one the control plane shipped
  with.

## Verification

`mise exec -- cargo test` runs unit checks, `mise run test-ops` the ops
scripts' tests, and `mise run test-integration` starts an isolated
socket-only PostgreSQL cluster and runs the migrations and integration suite.
Integration cases cover atomic rate budgets, interrupted transactions, retained
data and seven-day scheduling, concurrent restore/purge, failed metric probes,
authenticated history, deployment admission, and database outages. The temporary
cluster requires PostgreSQL 18 or newer (migration 013 renames a named NOT NULL
constraint introduced in PostgreSQL 18). Linux CI installs PostgreSQL 18 from
the official PGDG repository. Override `PG_BIN` to select those binaries.

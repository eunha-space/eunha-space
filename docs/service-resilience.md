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
when dependency availability or background progress fails. On macOS run
`python3 ops/install-health-watchdog.py` to install a minute-by-minute host check.
It logs readiness changes to `~/Library/Logs/eunha-space/health.log` and reports
failure through the launchd job's exit status. It does not restart the service
when a dependency fails. An external uptime monitor can probe `/health/ready`.

API driver errors become fixed public messages with correlation IDs. Logs retain
only error type and ID, and HTTP traces omit query strings (including email-link
tokens). Temporal dispatch errors are generic. Authentication budgets are
persisted in PostgreSQL: ten attempts per identity and 200 per client IP in a
fifteen-minute window. Cloudflare client addresses are accepted only from a
loopback peer; this assumes the loopback listener is behind the trusted tunnel.
Password hashing runs on blocking threads with at most four concurrent jobs.

## Deployment and rollback

Run `./deploy.sh` on the production host (`ssh 100.113.148.66`, checkout
`~/Git/eunha-space`). Under the deployment lock it pulls `origin/main` with
`--ff-only` and requires a clean `main` checkout matching that remote commit.
Local changes, local commits, and diverged branches prevent deployment without
discarding work. It uses mise, runs unit tests, and builds into
`.control-releases/<release-id>`.
With `CONTROL_GITHUB_TOKEN` in the launchd environment it compiles nothing:
it waits up to 75 minutes for the `Checks` run for that commit, whose
`release` job builds on macOS arm64 only after the tests pass, then unpacks
that run's `eunha-space-macos-arm64` artifact after checking its digest. It
waits before taking the maintenance lease, so runtime releases are not held
off for the CI run. A failed run, or an expired or incomplete artifact,
stops the deployment before anything switches.
The deployed executable and frontend assets are activated together through an
atomic `current` link. Assets from the prior release remain available to open
browser sessions. The original launchd environment is preserved, adding release,
frontend, and project-root paths. Initial adoption snapshots the legacy binary
and assets before rebuilding. Readiness must return the new release ID within
two minutes; otherwise the prior executable, assets, and launchd configuration
are restored and checked. Releases are retained rather than implicitly deleted.

A PostgreSQL maintenance lease blocks new runtime releases during the build and
switch; admission is serialized with the lease transition using an advisory
transaction lock. Existing runtime releases prevent deployment. The lease is
renewed before switching and expires after thirty minutes if the deployer dies.
The deployer also uses a local process lock. Database migrations must remain
backward compatible with the prior binary: rollback does not undo migrations.
The first adoption pauses automatic runtime deployment and restores its policies
afterward, because the legacy binary predates admission guards. The active
release check is repeated immediately before switching. Avoid manual runtime
release requests during that initial cutover.

## Verification

`mise exec -- cargo test` runs unit checks, `mise run test-deploy` simulates
activation and rollback, and `mise run test-integration` starts an isolated
socket-only PostgreSQL cluster and runs the migrations and integration suite.
Integration cases cover atomic rate budgets, interrupted transactions, retained
data and seven-day scheduling, concurrent restore/purge, failed metric probes,
authenticated history, deployment admission, and database outages. The temporary
cluster requires PostgreSQL 18 or newer (migration 013 renames a named NOT NULL
constraint introduced in PostgreSQL 18). Linux CI installs PostgreSQL 18 from
the official PGDG repository. Override `PG_BIN` to select those binaries.

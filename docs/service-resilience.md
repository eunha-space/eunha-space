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

Pull the requested commit on the production host, then run `./deploy.sh`. It
uses mise, runs unit tests, and builds into `.control-releases/<release-id>`.
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

## Backup coverage and restore drills

The existing host job discovers and backs up all PostgreSQL databases to B2 and
copies R2 media with remote version retention. Run
`python3 ops/backup/install-coverage.py` on the host to extend that job with:

- Control-plane and tunnel launchd configuration, including process-only secrets.
- Tunnel YAML and credentials, and the production Temporal environment.
- PostgreSQL global roles and password hashes.
- The active immutable control-plane executable, frontend, and release ID.

These archives contain secrets. Keep the backup bucket private and recovery
credentials in an independently accessible password manager. Generated tenant
TOML remains disposable; restore authoritative tenant configuration from the
control-plane database. Restoring an instance within retention still cancels its
scheduled purge. A destructive purge remains explicitly confirmed or scheduled
seven days after confirmed customer deletion.

After a successful backup, run:

```sh
python3 ops/backup/restore-drill.py --report /private/tmp/eunha-restore-report.json
```

Use `--existing-backups` to verify the existing database/media archives while
the sensitive recovery extension awaits approval. This mode explicitly reports
its narrower scope and does not claim to restore service credentials or roles.

The drill selects the latest completed backup, verifies service and tunnel
configuration and the runnable release, restores global roles and every Eunha
and Temporal database into a disposable PostgreSQL cluster, then reads every
restored table through counts. The cluster listens only on a private Unix socket.
Tenant contents, connection strings, and restored SQL errors are never printed.
The report contains backup identity, table counts, elapsed time, and pass/fail
flags. Production databases are never modified. Temporary restored data is
removed on exit. Run drills after schema changes and regularly; archive only the
credential-free report. `python3 ops/backup/install-restore-drill.py` installs a
weekly Sunday 04:30 host-time drill after the nightly backup. The drill also compares a bounded media sample in each populated tenant
namespace against the off-host copy using SHA-256. This samples media integrity;
it is not a full-bucket recovery proof.

The nightly snapshot schedule gives a worst-case database recovery point of
approximately 24 hours when every backup succeeds. This does not provide
continuous WAL archival or point-in-time recovery. Decide on a tighter recovery
point before introducing WAL retention and an off-host archive.

## Verification

`mise exec -- cargo test` runs unit checks, `mise run test-deploy` simulates
activation and rollback, and `mise run test-integration` starts an isolated
socket-only PostgreSQL cluster and runs the migrations and integration suite.
Integration cases cover atomic rate budgets, interrupted transactions, retained
data and seven-day scheduling, concurrent restore/purge, failed metric probes,
authenticated history, deployment admission, and database outages. The temporary
cluster requires local PostgreSQL binaries (override `PG_BIN` when necessary).

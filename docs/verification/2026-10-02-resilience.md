# Resilience verification — 2026-10-02 (Asia/Seoul)

The production control plane was deployed through immutable activation and
readiness verification. Its active release is `d43ff95839c5-20261001T161133Z`.
The previous release remains available for rollback. Readiness confirmed the
database, Temporal worker/connection, background controllers, and HTTP admission.
The local launchd watchdog is installed. Persisted metrics contained readings
for six tenants; a production check found the latest sample eight seconds old.

A production smoke check also confirmed ten invalid authentication attempts
return 401 and the next returns 429 with a 900-second retry header.

Local checks: 30 unit tests, 11 PostgreSQL integration tests, and four Python
operational-tool tests passed. The integration suite uses a disposable socket-only
PostgreSQL cluster and applies the full migration set. Linux CI passed on commit defcd52 (run 36891426003). Subsequent audit-boundary
regression checks also passed locally.

An existing off-host backup taken 2026-10-01 at 03:17:03 KST was restored on the
production host into an isolated socket-only PostgreSQL cluster. This restored
one control-plane database (23 tables), all five archived tenant databases
(118 tables each), and Temporal's two databases (39 and three tables).
Every restored table was read through a count. One stable media-object sample
matched its off-host copy by SHA-256. Total drill duration: 412.8 seconds. The
disposable cluster and restored data were removed afterward. Production
readiness remained healthy during the drill.

This proves the tested database/media recovery path. It does not prove a full
machine rebuild or a full media-bucket restore. Service configuration, PostgreSQL
roles, and the runnable release were absent from that legacy backup set.
The proposed coverage installer and full recovery drill are implemented and
tested, but production installation was rejected by automatic approval review
because it would add sensitive credentials and role hashes to B2. Explicit
approval for that export is pending. The existing backup job remains unchanged.

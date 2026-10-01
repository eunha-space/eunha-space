# Resilience verification — 2026-10-02 (Asia/Seoul)

The production control plane was deployed through immutable activation and
readiness verification. Its active release is `a10ec8011243-20261001T163701Z`.
A failed activation restored the previous healthy release automatically. A bounded
launchd registration retry subsequently allowed the new release to activate.
The previous release remains available for rollback. Readiness confirmed the
database, Temporal worker/connection, background controllers, and HTTP admission.
The local launchd watchdog is installed. Persisted metrics contained readings
for six tenants; the final production check found the latest sample one second old.

A production smoke check also confirmed ten invalid authentication attempts
return 401 and the next returns 429 with a 900-second retry header.

Local checks: 30 unit tests, 11 PostgreSQL integration tests, and seven Python
operational-tool tests passed. The integration suite uses a disposable socket-only
PostgreSQL cluster and applies the full migration set. Linux CI passed on code commit `35b7848` ([run 36894603846](https://github.com/limeburst/eunha-space/actions/runs/36894603846)),
including the non-public schema recovery regression and frontend build. The interrupted provisioning test runs
the real health probe after recreating application state and verifies that a
completed step performs no second request.

An existing off-host backup taken 2026-10-01 at 03:17:03 KST was restored on the
production host into an isolated socket-only PostgreSQL cluster. This restored
one control-plane database (23 tables), all five archived tenant databases
(126 application tables each, including eight outside `public`), and Temporal's two databases (39 and three tables).
Every application table in every non-system schema was read through a count. One stable media-object sample
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

## Completion audit

| Requirement | Evidence | Status |
| --- | --- | --- |
| Dependency readiness and stalled work detection | Production readiness returned true for database, Temporal, background progress, and admission; host watchdog installed | Verified |
| Immutable deployment, graceful shutdown, rollback, release admission | Active release a10ec8011243-20261001T163701Z; actual failed activation rolled back; admission and bounded launchd retry tests passed | Verified |
| Safe API, audit, and workflow errors | Secret-payload audit regression and API error test passed; raw provisioning, reconciliation, and manual reload messages removed | Verified |
| Background metrics and bounded persisted history | Six tenants sampled; latest reading one second old; authenticated history and failed probe tests passed | Verified |
| Backup coverage and actual recovery | Eight databases and media sample restored; roles, service configuration, runnable release, and full weekly drill still need the sensitive backup extension | Incomplete: export approval pending |
| Failure/concurrency checks and authentication controls | 30 unit tests, 11 SQLx integration tests, seven Python tests, real non-public schema verification, frontend build, and Linux CI passed | Verified |

The code verification above applies to commit `35b7848`. This report update changes
only documentation. The production checkout includes the recovery tooling updates;
the application binary remains the separately identified immutable release.

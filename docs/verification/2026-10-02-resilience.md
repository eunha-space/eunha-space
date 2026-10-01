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

Local checks: 30 unit tests, 11 PostgreSQL integration tests, and five Python
operational-tool tests passed. The integration suite uses a disposable socket-only
PostgreSQL cluster and applies the full migration set. Linux CI passed on code commit `35b7848` ([run 36894603846](https://github.com/limeburst/eunha-space/actions/runs/36894603846)),
including the frontend build. The interrupted provisioning test runs
the real health probe after recreating application state and verifies that a
completed step performs no second request.

Machine and database backups are managed by the existing backup system.
The additional backup extension and restore-drill tooling were reverted at
the user’s request. The production backup job was never changed.

## Completion audit

| Requirement | Evidence | Status |
| --- | --- | --- |
| Dependency readiness and stalled work detection | Production readiness returned true for database, Temporal, background progress, and admission; host watchdog installed | Verified |
| Immutable deployment, graceful shutdown, rollback, release admission | Active release a10ec8011243-20261001T163701Z; actual failed activation rolled back; admission and bounded launchd retry tests passed | Verified |
| Safe API, audit, and workflow errors | Secret-payload audit regression and API error test passed; raw provisioning, reconciliation, and manual reload messages removed | Verified |
| Background metrics and bounded persisted history | Six tenants sampled; latest reading one second old; authenticated history and failed probe tests passed | Verified |
| Failure/concurrency checks and authentication controls | 30 unit tests, 11 SQLx integration tests, five Python tests, frontend build, and Linux CI passed | Verified |

The code verification above applies to commit `35b7848`. The backup tooling and its two Python checks have since been removed; the
remaining five operational checks and PostgreSQL suite were rerun. The application
binary remains the separately identified immutable release.

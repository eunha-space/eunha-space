# Temporal

Temporal executes Eunha Space's long-running work. PostgreSQL remains the
authoritative store for tenants, configuration, operations, audit events,
retention, and the proof required before a resource can be destroyed. The
Temporal service is never exposed to the public Internet.

## Local development

Copy the untracked development environment file and start the stack:

```sh
cp ops/temporal/.env.dev.example ops/temporal/.env.dev
mise run temporal
```

The gRPC endpoint is `127.0.0.1:7233` and the UI is
`http://127.0.0.1:8080`. This uses `temporalio/auto-setup` and is only for
local development. The persistent volumes hold Temporal's execution history;
they never replace the control-plane database.

In a second terminal, point Eunha Space at the local service before starting
it:

```sh
export TEMPORAL_ADDRESS=127.0.0.1:7233
export TEMPORAL_NAMESPACE=default
cargo run
```

## Production contract

Deploy Temporal on a private network using pinned `temporalio/server` images,
with its PostgreSQL persistence and optional visibility store managed separately
from `CONTROL_DATABASE_URL`. Run its schema migrations with a version-matched
`temporalio/admin-tools` image before starting or upgrading Server. TLS and
network policy must limit gRPC access to the control plane and workflow workers.

Create an `eunha-space` namespace with a retention period chosen for
operational debugging; workflow histories may contain tenant IDs and operation
IDs but must never carry database URLs, provider credentials, VAPID keys,
rendered TOML, or command output.

## Application integration

`TenantProvision`, `TenantDeprovision`, and `TenantPurge` have corresponding
Temporal workflows. Each activity accepts only stable identifiers (`tenant_id` and
`operation_id`) and reloads all mutable data from PostgreSQL. Activities must
be idempotent because Temporal can retry them. The existing step tables remain
the audit and safety record; workflow completion updates an operation only after
the corresponding durable PostgreSQL step succeeds.

Use Temporal timers for the seven-day retained-to-purge delay, but re-check
`purge_scheduled_for`, lifecycle state, and restoration status in PostgreSQL
under a row lock before starting any destruction. This preserves the invariant
that a restore wins over a previously scheduled purge.

`docker-compose.production.yml` is for a host with a local PostgreSQL service
and OrbStack/Docker. It connects through `host.docker.internal`, keeps only the
Temporal gRPC endpoint and UI on loopback, and creates the `eunha-space`
namespace with a 30-day workflow-history retention. Provide its untracked
`POSTGRES_PWD` environment file from the deployment host; do not add it to the
repository.

The control plane runs its workers directly with Temporal's Rust SDK. Pin its
release as a group and replay representative production histories before each
SDK upgrade, because workflow code must remain deterministic and replay-safe.
Workflow code only coordinates activities; it never performs direct I/O, reads
secrets, or accesses PostgreSQL. Activities run in this process and receive
only stable identifiers, then reload the needed state from PostgreSQL.

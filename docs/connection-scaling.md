# Connection scaling

A PostgreSQL backend is a process. Every tenant has its own role and database,
so nothing is shared between them, and a fleet's cost used to follow the number
of tenants rather than the amount of work: an instance nobody was reading held
its whole pool open anyway. Seven tenants held 71 of 100 connections. A thousand
would have wanted five thousand.

The fix is in three layers — give the server room, stop idle tenants holding
anything, and stop a connection meaning a backend at all.

## What a connection costs

Eunha's own benchmarks (`MULTITENANCY.md` in the runtime) put a warm backend at
19–24 MiB of private memory, and a small tenant's useful pool at two
connections, with no measurable throughput between three and five. Connections
are a memory budget, not only a slot budget.

Memory is not the first wall on macOS, though. `kern.maxprocperuid` caps the
processes one user may own — 2666 on this host — and PostgreSQL's backends
are processes owned by the same account that runs everything else. The fleet
runs out of process slots before it runs out of anything else, so the useful
question is never "how many connections may we have" but "what makes a tenant
hold one".

## Three things used to hold one open

- **The queue loops.** A tenant's delivery, inbox and media loops back off when
  they find nothing, but only up to `queue_idle_poll_seconds`. With that ceiling
  at 30s and the pool's `idle_timeout_seconds` at 600s, no connection was ever
  unused long enough to be reaped.
- **The health sweep.** The control plane checked every tenant every 60 seconds
  by fetching `/api/v2/instance`, which counts distinct domains and posting
  accounts and takes a pool connection per statistic. That alone pinned every
  tenant at its pool's maximum, whatever the maximum was.
- **Both together** made the pool cap a floor. Lowering the cap lowered the
  peak and the floor by the same amount; it did not make an idle tenant cheaper.

The order matters when changing any of this: a poll or a sweep that runs more
often than `idle_timeout_seconds` cancels the whole arrangement, no matter what
the pool sizes say.

## What the fleet does now

**The server** keeps `max_connections` at 300, with `shared_buffers` and
`effective_cache_size` derived from the host's memory. This machine also runs
the control plane, the tenant runtime, Redis, Temporal and unrelated sites, so
`shared_buffers` takes 15% rather than the 25% a dedicated database host would.
`ops/provision/provision.sh` sets these through `ALTER SYSTEM`, so
`postgresql.auto.conf` owns them and `postgresql.conf` stays stock.

**An idle tenant holds nothing.** `queue_idle_poll_seconds` is 300 and the
pool's `idle_timeout_seconds` is 20, so a quiet loop's connection is reaped
long before that loop looks again. The health sweep runs every 300 seconds
against `/api/eunha/v1/health`, which is one trivial query — 1.2ms against the
35ms the statistics endpoint took — and a tenant counts as stale only after 10
minutes, comfortably more than one missed sweep.

**A pool is small.** Two connections, and three for an instance with enough
public traffic to justify it. Promote on evidence, not on plan names.

**A connection is not a backend.** Tenants reach PostgreSQL through PgBouncer in
transaction mode, so a tenant holds a real backend only while a transaction is
in flight. Backends follow concurrent work instead of tenant count.

Seven tenants now hold single digits between them, and every database on the
host together stays under a fifth of what the server accepts.

## What pooling requires

PgBouncer pools are keyed by `(user, database)`, so tenants never share a
backend with each other; what is gained is time, not sharing. Four things make
it work, and three of them fail in ways that are hard to read backwards from the
symptom:

- **Migrations stay direct.** Only the request and queue pool moves, through the
  rendered `pooled_database_url`; `database_url` still points at PostgreSQL
  itself. sqlx holds a session-scoped advisory lock across a migration run, and
  a transaction pooler would hand the unlock to a different server connection
  than the lock.
- **`search_path` belongs to the role.** Eunha keeps its tables in the `eunha`
  schema and its pool issues `SET search_path` on connect, which a pooled server
  connection does not keep: it is handed to clients that never ran it, and one
  opened later never ran it either. Provisioning sets
  `ALTER ROLE ... SET search_path TO eunha, public`.
- **`extra_float_digits` must be ignored.** sqlx sends it as a startup
  parameter and PgBouncer rejects unknown ones outright, so every tenant fails
  with `unsupported startup parameter` and answers 503 until it is listed in
  `ignore_startup_parameters`.
- **The capacity preflight needs the right limit.** The runtime refuses to start
  when its tenants' pools add up to more than the server accepts. Through a
  pooler, PostgreSQL's `max_connections` is the wrong number, so the rendered
  `pooled_client_slots` says what the pooler itself accepts.

Prepared statements are fine: PgBouncer tracks them in transaction mode through
`max_prepared_statements`, and the pooler's own statistics show binds without
re-parses when it is working.

## What owns what

| Setting | Owner |
| --- | --- |
| `max_connections`, `shared_buffers`, `effective_cache_size` | `postgresql.auto.conf`, written by `ops/provision/provision.sh` |
| Pooler configuration | `ops/pgbouncer/pgbouncer.ini`, rendered by the same script |
| Pool sizes, poll intervals, tenant URLs | `tenant_configuration` in PostgreSQL |
| Pooler address, its client slots | The control-plane LaunchAgent, not the repository |

A `tenant_configuration` change reaches a running tenant within a minute. Every
write to that table bumps the tenant's `config_revision`, and a render records
the revision it came from in `rendered_revision`; a reconcile workflow per
runtime renders whatever differs and reloads the runtime once for all of them.
An operator's render in `/admin` is now a way to skip the wait, not the only
way it happens. A tenant whose render keeps failing is retired from the loop
after five attempts, with the reason on the tenant row, and an explicit render
puts it back.

## Reading the fleet

```sql
-- What the host is holding, against what it accepts.
SELECT count(*) FILTER (WHERE datname LIKE 'eunha_t_%') AS tenant_backends,
       count(*) AS backends,
       current_setting('max_connections')::int AS accepts
FROM pg_stat_activity WHERE backend_type = 'client backend';
```

A tenant sitting exactly at its pool's maximum, with every connection last
used seconds ago, is the signature of something touching it on a timer faster
than `idle_timeout_seconds` — look at what those connections last ran before
changing any pool size.

`ops/provision/provision.sh status` reports the server settings and whether the
pooler is listening; rollback from pooling is unsetting the control plane's
pooler variables and rendering every tenant again.

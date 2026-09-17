# Tenant connection pooler

A PostgreSQL backend is a process and costs 19–24 MiB, so a host runs out of
backends long before it runs out of tenants: every tenant's pool holds real
backends whether or not anyone is reading the instance. PgBouncer unties the
two. In transaction mode a tenant holds a backend only while a transaction is
in flight, so the count follows concurrent work instead of tenant count.

Every tenant has its own role and database, so a pool here is per tenant and
none of them share a backend. What is gained is time, not sharing: with
`min_pool_size = 0` and a short `server_idle_timeout`, an idle tenant holds
nothing at all.

## What the control plane renders

`CONTROL_TENANT_POOLER_ADDRESS` and `CONTROL_TENANT_POOLER_CLIENT_SLOTS` on the
control-plane process add `pooled_database_url` and `pooled_client_slots` to
every tenant file. The stored `database_url` stays pointed at PostgreSQL
itself. Only a runtime's request and queue pool moves, because `eunha migrate`
holds a session-scoped advisory lock across a migration run and a transaction
pooler would hand the unlock to a different server connection than the lock.

Unset both and the next render takes every tenant back off the pooler, which is
the rollback.

## Two things that are not optional

Tenant roles carry their own `search_path`; provisioning sets it with
`ALTER ROLE ... SET search_path TO eunha, public`. Eunha keeps its tables in the
`eunha` schema and its pool issues `SET search_path` on connect, which a pooled
server connection does not keep: it is handed to clients that never ran it, and
one opened later never ran it either.

`ignore_startup_parameters` must list `extra_float_digits`. sqlx sends it at
startup, PgBouncer rejects unknown startup parameters outright, and every tenant
fails to connect with `unsupported startup parameter` until it is listed.

## Installing

```sh
brew install pgbouncer
cp ops/pgbouncer/pgbouncer.ini /opt/homebrew/etc/pgbouncer.ini
```

Run it under launchd as `space.eunha.pgbouncer`, with
`SoftResourceLimits.NumberOfFiles` above `max_client_conn`: a login shell's 256
descriptors would cap the whole fleet at a couple of hundred clients, and
PgBouncer says so at startup, in its log, as `max expected fd use`.

Roles are looked up in PostgreSQL through `auth_query` rather than listed in a
file, so provisioning a tenant needs no pooler change. Reload after editing:

```sh
kill -HUP "$(pgrep -f 'pgbouncer /opt/homebrew/etc/pgbouncer.ini')"
```

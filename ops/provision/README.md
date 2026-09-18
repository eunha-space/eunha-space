# Host provisioning

`provision.sh` brings a macOS host to the state eunha.space expects. Every step
is safe to re-run: it reports what is already true and changes only what is not.

```sh
ops/provision/provision.sh status     # what is installed, loaded and set
ops/provision/provision.sh plan       # what the postgres step would change
ops/provision/provision.sh all        # packages, postgres, pgbouncer
```

It also runs on a host with no checkout, which is how a new machine is brought
up:

```sh
ssh <host> 'bash -s status' < ops/provision/provision.sh
```

The `pgbouncer` step needs this repository, for `ops/pgbouncer/pgbouncer.ini`.
Piped over stdin the script cannot find itself, so it looks at
`EUNHA_REPO_ROOT`, then `~/Git/eunha-space`.

## Steps

| Step | What it does |
| --- | --- |
| `status` | Reports packages, services and the settings that matter. Changes nothing. |
| `plan` | The `postgres` step as a dry run. Changes nothing. |
| `packages` | Installs PostgreSQL, Redis, HAProxy, PgBouncer and cloudflared. |
| `postgres` | Sets `max_connections`, `shared_buffers` and `effective_cache_size` from this host's memory. |
| `restart-postgres` | Fast-restarts PostgreSQL so a pending setting takes effect. |
| `pgbouncer` | Writes the pooler config and its LaunchAgent, and starts it. |
| `runtime <name>` | Creates one runtime's directory layout. |

## What it will not do

**It never restarts a service on its own.** PostgreSQL on this host also serves
databases that have nothing to do with eunha, so `postgres` writes the settings
and says which ones are waiting on a restart; `restart-postgres` is a separate,
deliberate step. `pgbouncer` likewise leaves a healthy pooler alone when its
configuration has not changed, because restarting it drops every tenant's
connections.

**It provisions no secrets.** The control plane reads its database URL,
Cloudflare and Resend credentials from its own LaunchAgent, which is host
configuration and stays off the repository. A new host still needs that file,
Temporal (`ops/temporal`), cloudflared's tunnel credentials, and the backup
job's storage credentials, all by hand.

## Tuning

`shared_buffers` defaults to 15% of memory rather than the usual 25%, because
this host also runs the control plane, the tenant runtime, Redis, Temporal and
unrelated sites. A dedicated database host should raise it:

```sh
PG_SHARED_BUFFERS_PCT=25 ops/provision/provision.sh postgres
```

`max_connections` defaults to 300. With tenants pooling through PgBouncer
(`ops/pgbouncer`), backends follow concurrent transactions rather than tenant
count, so this is a ceiling rather than something to raise per tenant.

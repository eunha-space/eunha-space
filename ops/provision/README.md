# Host provisioning

`provision.sh` brings a macOS host to the state eunha.space expects. Every step
is safe to re-run: it reports what is already true and changes only what is not.

```sh
ops/provision/provision.sh status     # what is installed, loaded and set
ops/provision/provision.sh all        # packages, pgbouncer
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
| `packages` | Installs PostgreSQL, Redis, HAProxy, PgBouncer and cloudflared. |
| `pgbouncer` | Writes the pooler config and its LaunchAgent, and starts it. |
| `runtime <name>` | Creates one runtime's directory layout. |

## What it will not do

**It never restarts a service on its own.** `pgbouncer` leaves a healthy pooler
alone when its configuration has not changed, because restarting it drops every
tenant's connections.

**It does not configure PostgreSQL.** PostgreSQL on this host also serves
databases that have nothing to do with eunha, so its settings, `max_connections`
and memory among them, belong to the host's own configuration. `status` reports
the ones eunha cares about.

**It provisions no secrets.** The control plane reads its database URL,
Cloudflare and SMTP credentials from its own LaunchAgent, which is host
configuration and stays off the repository. A new host still needs that file,
a Temporal server (see `ops/temporal`), cloudflared's tunnel credentials, and the backup
job's storage credentials, all by hand.

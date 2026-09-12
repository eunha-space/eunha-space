# eunha.space

The control plane for hosted Eunha instances. PostgreSQL's typed tenant,
configuration, and R2-resource rows are authoritative; runtime TOML files are
generated projections and never a second source of truth. A tenant's runtime
filename is derived from its domain (for example, `seoul.earth` becomes
`seoul-earth.toml`).

## Development

Set `CONTROL_DATABASE_URL` to a dedicated PostgreSQL database, then:

```sh
mise run frontend
CONTROL_DATABASE_URL=postgresql://... cargo run
```

Tenant provisioning uses `CONTROL_PROVISION_DATABASE_URL` for PostgreSQL role
and database creation. The account must have `CREATEROLE` and `CREATEDB`; when
the variable is omitted, the control-plane connection is used. New tenants copy
shared provider settings from the oldest ready tenant on their selected runtime,
while receiving fresh database, encryption, and VAPID credentials plus
`t/<lowercase-hyphenated-uuid>` media isolation. No provider secret is returned
by the API or written to an operation event.

Provisioning is a persisted sequence of database creation, migration, render,
reload, and health-check steps. Interrupted work resumes after restart. Failed
work remains retained and can be retried from the tenant inventory; it is never
implicitly deprovisioned or purged.

`EUNHA_PROVIDED_DOMAIN_SUFFIXES` is a comma-separated, primary-first list of
platform domains (default `eunha.space`). Provisioning `garden` with
`eunha.space,eunha.site` reserves `garden.eunha.space` as primary and
`garden.eunha.site` as an alias. Infrastructure labels are reserved in the
`reserved_subdomains` table.

Custom domains require `CLOUDFLARE_ZONE_ID` and `CLOUDFLARE_API_TOKEN`; the
token needs SSL and Certificates Write and remains process-only. Customers are
instructed to CNAME to `t.eunha.space`, configurable with
`CLOUDFLARE_SAAS_CNAME_TARGET`. Cloudflare IDs, activation state, and public
DNS validation instructions are stored, but the token is not. A first custom
domain becomes canonical only before a local account exists. Later verified
custom domains are aliases and do not mutate ActivityPub identity.

Create the first operator without putting a password in shell history:

```sh
read -s EUNHA_SPACE_ADMIN_PASSWORD
export EUNHA_SPACE_ADMIN_PASSWORD
cargo run -- set-operator admin
unset EUNHA_SPACE_ADMIN_PASSWORD
```

The import command records an existing instance and its complete configuration
without changing any runtime file or process. Secret values remain server-side;
the console APIs return only operational metadata.

## Reconciliation

Set `CONTROL_RUNTIME_HOST` to the runtime's exact `host` value on the machine
that owns its tenant directory. The console's **Render & restart** action then
atomically replaces only the adopted tenant filename in `<tenant-directory>`, gives it mode
`0600`, and restarts that runtime's launchd service. It validates the stored
TOML and its instance domain before writing. A control plane on another machine
refuses the action rather than attempting to reach into a remote filesystem.

## Runtime releases

Eunha code is released once per shared runtime, never per tenant. eunha.space
builds an immutable bundle from its sibling `../eunha` checkout, including the
release binary and built frontend, then transfers that bundle over SSH to the
runtime host. Runtime hosts need neither a source checkout nor build tooling.
The release operation runs the embedded migrations before an atomic `current`
switch, health-checks the service, and restores the prior release on failure.
Its command output is never persisted. The control plane serializes the fleet:
only one runtime can deploy at a time.

The admin console shows a live, persisted release tail while a runtime is
running. It contains only fixed operational milestones; deploy command output is
deliberately excluded so tenant secrets cannot reach the database or UI.

Operators can use **Deploy latest** for a selected runtime. Enabling automatic
deployment makes the control plane poll `origin/main` every five minutes; each runtime is advanced
only after the prior runtime completed and only when its recorded deployed
commit differs. Automatic deployment is opt-in and disabled by default.

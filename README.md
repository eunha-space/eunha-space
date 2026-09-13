# eunha space

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

## Customer self-service

The public portal at `/` lets customers register, verify their email, and
provision up to two instances. `/admin` remains operator-only. Verification
mail uses Resend's HTTPS API and requires process-only configuration:

```sh
EUNHA_SPACE_RESEND_API_KEY=...
EUNHA_SPACE_EMAIL_FROM='eunha space <noreply@eunha.space>'
EUNHA_SPACE_PUBLIC_URL=https://eunha.space
```

Customers may choose one custom hostname, and only while creating an instance.
Neither customers nor operators can attach a domain to an existing instance,
because an instance's accounts fix its ActivityPub identity to its primary
domain. The create form asks for the CNAME to `customers.eunha.space` before
creation and checks it through DNS-over-HTTPS, warning without blocking. Setup
then waits for the domain before creating the owner, so the domain becomes the
instance's identity. A domain whose DNS is not ready is re-checked with backoff,
from 30 seconds up to 15 minutes, for 24 hours. When that window closes, when
Cloudflare reports the hostname or its certificate as terminal, or when the
customer chooses **Continue without custom domain**, the domain is removed from
Cloudflare and released. Setup then finishes on the provided hostname, and the
instance page names the domain that was dropped. Runtime placement is automatic
and favors the least-loaded eligible runtime.
Signup requires only an email address. The verification link signs the customer
in and requires password setup; later sessions can use either that password or
a fresh email sign-in link.

Each customer instance gets its first account the way a Mastodon server does.
The customer picks an admin username when creating the instance, and the final
`owner` provisioning step runs the runtime's
`eunha accounts create <username> --role=Owner --confirmed --approve` with the
customer's email. Instances start with registrations closed, as Mastodon's do,
so nobody can sign up first. The step waits for a custom domain requested at
creation to connect or fail, because local accounts fix the instance's
ActivityPub identity to its primary domain. Runtimes must run an Eunha release
that includes the `accounts` commands.

The account's password is never stored. The instance page's **Create password**
and **Reset password** action runs
`eunha accounts modify <username> --reset-password`, shows the printed password
to the customer once with `Cache-Control: no-store`, and records an
`owner_password_reset` operation without the password. A reset signs the
account out of every session and app. Like other runtime commands, these must
run on the machine whose `CONTROL_RUNTIME_HOST` matches the runtime.

Customers delete an instance in two stages. **Delete instance** schedules
deprovisioning seven days out while the instance keeps running; until then the
customer can cancel, or choose **Delete now**. Deprovisioning removes the
tenant's runtime file, reloads the runtime, and confirms the domain is no longer
served, retrying failed steps each minute up to ten times. The instance then
becomes `retained`: its database, media, configuration and Cloudflare hostnames
are kept, nothing is announced to other servers, and it no longer counts
against the customer's two-instance limit. The customer can restore it while
they have a free slot, which re-runs migration, rendering, reload and health
checks. Only an operator purge destroys retained data, and nothing purges
automatically. Retained tenants are ignored by domain reconciliation, health
checks, operator renders and runtime-release health checks.

Operators can **Restore** a retained tenant without checking the customer's
instance limit. They can **Purge** any tenant that is not already being
deprovisioned or purged, whether it is ready, failed, stuck provisioning or
retained, after typing its domain. Purging runs as durable, retried steps. A
tenant that was not retained is first taken offline in a single step that
removes its runtime file, reloads the runtime and confirms the domain is no
longer served. Its customer gets no grace period, and provisioning stops before
its next step. The purge then deletes the tenant's Cloudflare for SaaS hostnames
and domain records, which frees a customer's custom domain for use elsewhere.
It drops the tenant's database and role, deletes its media under `t/<tenant id>/`
and its Redis keys under its own prefix, and discards its configuration and
VAPID keys. Each resource is removed only when it provably belongs to the tenant:
the database and role must carry the names provisioning derives from the tenant
id, and the media and Redis prefixes must be the tenant's own. Anything else,
such as an imported tenant's database, is left in place and noted on the
operation. If Redis refuses, the purge notes it and continues rather than
blocking. The tenant row stays as `purged`, keeping its domain reserved, because
remote servers still remember the accounts that lived there.

`EUNHA_PROVIDED_DOMAIN_SUFFIXES` is a comma-separated, primary-first list of
platform domains (default `eunha.space`). Provisioning `garden` with
`eunha.space,eunha.site` reserves `garden.eunha.space` as primary and
`garden.eunha.site` as an alias. Infrastructure labels are reserved in the
`reserved_subdomains` table.

Each runtime has an explicit `origin_hostname`, such as
`shared-small.kr-seongsu-m4.origin.eunha.space`. Infrastructure must configure
that hostname as a proxied DNS record for the runtime's dedicated Cloudflare
Tunnel. The platform domains use one proxied wildcard DNS record. Provisioned
hostnames are registered as Cloudflare for SaaS custom hostnames and routed to
their runtime origin, so tenant count does not consume the zone's DNS-record
quota.

Provided and customer-owned domains require `CLOUDFLARE_ZONE_ID` and
`CLOUDFLARE_API_TOKEN`; the token needs SSL and Certificates Write and remains
process-only. Each hostname is registered with its tenant runtime's
`origin_hostname`, while
customers are instructed to CNAME to `customers.eunha.space`, configurable with
`CLOUDFLARE_SAAS_CNAME_TARGET`. Cloudflare IDs, activation state, and public
DNS validation instructions are stored, but the token is not. A custom domain
becomes canonical only before a local account exists. One that finishes
verifying after an account exists, which only a race with **Continue without
custom domain** can cause, becomes an alias and does not change the ActivityPub
identity.

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

Eunha code is released once per shared runtime, never per tenant. eunha space
builds an immutable bundle from its sibling `../eunha` checkout, including the
release binary and built frontend, then transfers that bundle over SSH to the
runtime host. Runtime hosts need neither a source checkout nor build tooling.
The release operation runs the embedded migrations before an atomic `current`
switch, health-checks the service, and restores the prior release on failure.
Its command output is never persisted. The control plane serializes the fleet:
only one runtime can deploy at a time.

A release runs inside the control-plane process that queued it and records a
heartbeat every 15 seconds. If that process exits, for example because
`deploy.sh` restarts it, the release stops. After two minutes without a
heartbeat, the release is marked failed, which frees the release lock. The
failure says whether the release was still building, in which case the runtime
was untouched and retrying is safe, or had begun rolling out, in which case the
runtime's active slot and health need checking first. Avoid deploying the
control plane while a release is running.

The admin console shows a live, persisted release tail while a runtime is
running. It contains only fixed operational milestones; deploy command output is
deliberately excluded so tenant secrets cannot reach the database or UI.

Operators can use **Deploy latest** for a selected runtime. Enabling automatic
deployment makes the control plane poll `origin/main` every five minutes; each runtime is advanced
only after the prior runtime completed and only when its recorded deployed
commit differs. Automatic deployment is opt-in and disabled by default.

Router-enabled runtimes use HAProxy on their stable `bind_address` and two
launchd-managed Eunha slots on `blue_bind_address` and `green_bind_address`.
The deployer starts and health-checks the inactive slot, gracefully reloads
HAProxy so new connections use it, drains the old slot, and then stops it. The
one-time `ops/eunha-runtime-router bootstrap` cutover is required before setting
`router_enabled`; subsequent releases are blue/green.

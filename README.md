# eunha space

The control plane for hosted Eunha instances. PostgreSQL's typed tenant,
configuration, and R2-resource rows are authoritative; runtime TOML files are
generated projections and never a second source of truth. A tenant's runtime
file is named by its tenant id, `<tenant id>.toml`. Eunha identifies a tenant
by the domain inside the file, so the name only has to be unique and stable,
which a name derived from the domain is not: hyphens are legal in domains, and
domains change. On startup, the control plane renames files on its runtime host
that still carry the old domain-derived name, such as `seoul-earth.toml`. The
rename is atomic and leaves the contents unchanged, so the next reload keeps the
tenant running.

A host is brought to the state this expects with `ops/provision/provision.sh`,
which installs PostgreSQL, Redis, HAProxy, PgBouncer and cloudflared, tunes
PostgreSQL for the host's memory, and sets the pooler up. Every step is safe to
re-run and none of them restart a running service on their own; see
`ops/provision/README.md`.

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

`CONTROL_TENANT_POOLER_ADDRESS`, as `host:port`, adds a pooled database URL to
every rendered tenant file, and `CONTROL_TENANT_POOLER_CLIENT_SLOTS` tells the
runtime what that pooler accepts in total so it can size its tenants' pools
against the limit that applies. The stored `database_url` stays pointed at
PostgreSQL itself: only a runtime's request and queue pool moves, because
`eunha migrate` holds a session-scoped advisory lock that a transaction pooler
would break. Unset, tenants connect directly and nothing is rendered. Roles
provisioned for a pooled deployment carry `search_path` themselves, since a
pooled connection reaches clients that never ran the `SET` a pool issues on
connect.

Provisioning is a persisted sequence of database creation, migration, render,
reload, and health-check steps. Interrupted work resumes after restart. Failed
work remains retained and can be retried from the tenant inventory; it is never
implicitly deprovisioned or purged.

Connection scaling — what makes a tenant hold a PostgreSQL connection, and the
three layers that keep a fleet's cost tied to work rather than tenant count — is
described in [docs/connection-scaling.md](docs/connection-scaling.md).

## Customer self-service

The public portal at `/` lets customers register, verify their email, and
provision up to two instances. `/admin` remains operator-only. Verification
mail uses SMTP and requires process-only configuration:

```sh
EUNHA_SPACE_SMTP_HOST=smtp.example.com
EUNHA_SPACE_SMTP_PORT=465
EUNHA_SPACE_SMTP_USERNAME=...
EUNHA_SPACE_SMTP_PASSWORD=...
EUNHA_SPACE_EMAIL_FROM='eunha space <noreply@eunha.space>'
EUNHA_SPACE_PUBLIC_URL=https://eunha.space
```

Customers may choose one custom hostname, and only while creating an instance.
An instance's accounts fix its ActivityPub identity to its primary domain, so
a running instance moves to another domain rather than adding one: its
addresses are rewritten, it is served under the new domain alone, and every
account's followers are sent a Move. Customers and operators start a move from
the instance's page or the console's domains dialog; *docs/tenant-lifecycle.md*
has the steps. The create form asks for the CNAME to `customers.eunha.space`
before creation and checks it through DNS-over-HTTPS, warning without
blocking.

An instance lives either at a `*.eunha.space` hostname or at the customer's
own domain, never both. With a custom domain, that domain is the instance's
primary domain from the start, and no `*.eunha.space` hostname is created. The
instance is not set up at all until the domain validates: no database, runtime
file or owner exists, and only the hostname is registered with Cloudflare. The
domain is checked with backoff, from 30 seconds up to 15 minutes, for seven
days. Cloudflare is asked to validate again even after it reports the hostname
or certificate as failed, since the customer may still fix what stopped it.
Once it validates, provisioning runs under it. If it has not validated after
seven days, the hostname is removed from Cloudflare, and the never-set-up
instance is deleted in the ordinary way: it becomes retained and frees its slot.
Its tenant row gives up the domain for a `<tenant id>.invalid` placeholder and
remembers it as `abandoned_custom_domain`. A purge of any tenant that was never
set up frees its domain the same way, since no account ever lived under it.
Restoring such an instance tries the same domain again for another seven days,
and is refused if another instance has claimed the domain meanwhile. Runtime
placement is automatic and favors the least-loaded eligible runtime.

Signup requires only an email address. The verification link signs the customer
in and requires password setup; later sessions can use either that password or
a fresh email sign-in link.

Each customer instance gets its first account the way a Mastodon server does.
The customer picks an admin username when creating the instance, and the final
`owner` provisioning step runs the runtime's
`eunha accounts create <username> --role=Owner --confirmed --approve` with the
customer's email. Instances start with registrations closed, as Mastodon's do,
so nobody can sign up first. With a custom domain, the owner is created only
after setup has waited for that domain, so the account's identity is on it.
Runtimes must run an Eunha release that includes the `accounts` commands.

The account's password is never stored. The instance page's **Create password**
and **Reset password** action runs
`eunha accounts modify <username> --reset-password`, shows the printed password
to the customer once with `Cache-Control: no-store`, and records an
`owner_password_reset` operation without the password. A reset signs the
account out of every session and app. Like other runtime commands, these must
run on the machine whose `CONTROL_RUNTIME_HOST` matches the runtime.

**Delete instance** takes an instance offline at once, whether it is ready,
failed or still setting up. Deprovisioning removes the tenant's runtime file,
reloads the runtime, and confirms the domain is no longer served, retrying failed
steps each minute up to ten times. The instance then becomes `retained`. Its
database, media, configuration and Cloudflare hostnames are kept, nothing is
announced to other servers, and it no longer counts against the customer's
two-instance limit. The deletion schedules a purge seven days later
(`purge_scheduled_for`). Until then the customer can restore the instance while
they have a free slot, which cancels the purge and re-runs migration, rendering,
reload and health checks. They can also choose **Delete permanently now**,
confirmed by typing the instance's address. When the seven days pass, the purge
runs by itself. Retained tenants are ignored by domain reconciliation, health
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
chosen at creation becomes canonical only before a local account exists; one
that verifies later becomes an alias and does not change the ActivityPub
identity. A domain change is the way to move that identity. An alias is
health-checked by confirming that it reaches the tenant, which reports its
primary domain. Set `CLOUDFLARE_API_BASE` to point the custom-hostname API at a
local stand-in for testing; it defaults to `https://api.cloudflare.com/client/v4`.
Retrying failed provisioning also re-checks provided hostnames that failed.

Administrators sign in through their user account using the same session as
customers. Admin APIs and pages require `users.is_admin`. Accounts, sessions,
and email verification links live in `users`, `user_sessions`, and
`user_email_verifications`. Tenant ownership and operation actors reference
users; historical operator identities without an account are retained as
users with no email, credentials, or admin access for audit attribution.
The existing customer API URLs and cookie name remain compatible.

Create or update an administrator customer account without putting a password in shell history:

```sh
read -s EUNHA_SPACE_ADMIN_PASSWORD
export EUNHA_SPACE_ADMIN_PASSWORD
mise exec -- cargo run -- set-admin admin@example.com
unset EUNHA_SPACE_ADMIN_PASSWORD
```

## Importing an existing Mastodon instance

**Import instance** in `/admin` takes over a server that already exists, rather
than creating one. It is operator-only: an import handles a customer's database
dump and the secrets their data is encrypted with, and a mistimed cutover is not
undoable.

Creation mints a tenant — an unused domain, an empty database, fresh secrets,
and an Owner account made at the end. An import inherits one, and nearly every
difference follows from that:

- **It keeps its domain.** Its accounts' ActivityPub identity is on it and
  remote servers know them there, so an import is never also a move. The domain
  is always the customer's own, never a `*.eunha.space` hostname.
- **It brings its secrets.** `ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY` and
  `..._KEY_DERIVATION_SALT` are required, because the instance's local signing
  keys are encrypted with them and a server that cannot decrypt them cannot
  sign anything it federates. The VAPID keypair is optional and generated when
  absent, which silently ends every Web Push subscription the instance has
  handed out — so it is both halves or neither.
- **It has no owner step.** The admin username names an account already in the
  dump; it is recorded, never created.
- **It is set up before its domain moves.** A created instance provisions
  nothing until its domain validates, so an abandoned one leaves nothing behind.
  An imported one would then be waiting for a cutover onto a server with no data
  on it, so it is restored, rendered, served and health-checked first — against
  the runtime directly, which needs no public DNS — and becomes `ready` only
  once the customer repoints DNS and Cloudflare validates the hostname. The
  seven-day custom-domain window does not apply to it.

The dump and the media are staged on the runtime host as absolute paths, and the
runtime's own binary does the work, because it is what knows which Mastodon
release this fleet builds:

```
database → migrate → import → render → media → reload → health → routing
```

`import` runs `eunha import-mastodon`, which refuses a dump from another
Mastodon release, a database that is not empty, and data belonging to a
different instance. It connects as `CONTROL_PROVISION_DATABASE_URL`, which for
imports must be a superuser: a data-only restore loads through the schema's
foreign keys and only a superuser may disable them.

`media` runs `eunha import-media` over the instance's `public/system` tree,
which uploads under the tenant's own `t/<tenant id>` storage prefix. The
instance's own files move; its cache of other servers' avatars, attachments,
emojis and link previews does not, since that is fetched again when missing and
is usually most of the bytes. A retry asks for each object before sending it, so
an upload that stopped partway resumes cheaply. Omit the media path for an
instance whose existing object storage goes on serving its media, and give that
storage's bucket and credentials instead; nothing provisions it and the purge
refuses to delete from it.

Both steps are given hours rather than the usual five minutes, and heartbeat
while they run so a dead worker is still noticed in two minutes. What they
report — the domain the data was proved to belong to, row counts, file counts,
the storage prefix — is kept on the tenant and written to the operation log.
Only counts, a domain and a storage prefix are kept from their output, so
nothing else can accumulate there.

A purge treats an imported tenant like any other except where the evidence says
a resource was never ours: its database and role must carry the names
provisioning derives from the tenant id, and its media must be under the
tenant's own prefix. Storage the instance brought with it is left in place and
noted on the operation.

Secret values remain server-side; the console APIs return only operational
metadata.

## Reconciliation

Set `CONTROL_RUNTIME_HOST` to the runtime's exact `host` value on the machine
that owns its tenant directory. The console's **Render & restart** action then
atomically replaces only that tenant's `<tenant id>.toml` in `<tenant-directory>`, gives it mode
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

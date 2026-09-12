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

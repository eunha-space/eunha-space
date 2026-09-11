# eunha.space

The control plane for hosted Eunha instances. PostgreSQL is authoritative for
tenant configuration; runtime TOML files are generated projections and never a
second source of truth.

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

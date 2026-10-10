# eunha.space

Control plane for Eunha hosting. PostgreSQL is the source of truth for tenant
configuration. Never expose tenant secrets through APIs, logs, audit details,
or rendered UI. Runtime TOML files are disposable generated artifacts.

Use mise for project tasks. Match Eunha's navy/cream/mint/lilac light and dark
themes. Use shadcn/ui components when adding standard UI primitives.

Provisioning and deprovisioning must be durable, auditable operations. A
deprovision never immediately destroys a database, media, configuration, or
backup; destructive purge is a separate explicitly confirmed operation. A
customer's confirmed deletion schedules that purge seven days later, and
restoring the instance before then cancels it.

The control plane is deployed from outside this repository, from what CI
builds for a commit on `origin/main`. After completing and verifying changes,
push them to `origin/main`; do not copy patches to production.

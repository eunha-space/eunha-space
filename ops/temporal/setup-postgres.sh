#!/bin/sh
set -eu

: "${POSTGRES_SEEDS:?POSTGRES_SEEDS is required}"
: "${POSTGRES_USER:?POSTGRES_USER is required}"
: "${DB_PORT:?DB_PORT is required}"

tool() {
  temporal-sql-tool --plugin postgres12 --ep "$POSTGRES_SEEDS" -u "$POSTGRES_USER" -p "$DB_PORT" "$@"
}

# The production bootstrap creates these two databases ahead of time so the
# Temporal role has no broader PostgreSQL privileges. This script is idempotent
# only for a brand-new database; future version changes use Temporal's schema
# migration procedure deliberately.
tool --db temporal setup-schema -v 0.0 || true
tool --db temporal update-schema -d /etc/temporal/schema/postgresql/v12/temporal/versioned
tool --db temporal_visibility setup-schema -v 0.0 || true
tool --db temporal_visibility update-schema -d /etc/temporal/schema/postgresql/v12/visibility/versioned

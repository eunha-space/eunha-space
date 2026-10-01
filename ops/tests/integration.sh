#!/bin/bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PG_BIN="${PG_BIN:-$(pg_config --bindir)}"
PG_VERSION="$("$PG_BIN/postgres" --version)"
if [[ ! "$PG_VERSION" =~ ([0-9]+)\. ]] || (( BASH_REMATCH[1] < 18 )); then
  echo "Integration tests require PostgreSQL 18 or newer." >&2
  exit 1
fi
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/eunha-space-test.XXXXXX")"
cleanup() {
  "$PG_BIN/pg_ctl" -D "$TEST_ROOT/data" -m immediate stop >/dev/null 2>&1 || true
  rm -rf "$TEST_ROOT"
}
trap cleanup EXIT
"$PG_BIN/initdb" -D "$TEST_ROOT/data" -A trust --no-locale >/dev/null
# Unix socket only: the trust-authenticated test cluster has no network listener.
"$PG_BIN/pg_ctl" -D "$TEST_ROOT/data" -l "$TEST_ROOT/server.log" -o "-c listen_addresses='' -k $TEST_ROOT -p 56439" -w start >/dev/null
cd "$ROOT"
DATABASE_URL="postgresql:///postgres?host=$TEST_ROOT&port=56439" mise exec -- cargo test integration_tests -- --ignored

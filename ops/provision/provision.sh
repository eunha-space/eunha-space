#!/bin/bash
# Bring a macOS host to the state eunha.space expects, one step at a time.
#
# Every step is safe to re-run: it reports what is already true and changes only
# what is not. Nothing here restarts a running service on its own, because
# PostgreSQL on this host also serves databases that have nothing to do with
# eunha; `restart-postgres` is a separate, deliberate step.
#
# Secrets are not provisioned here. The control-plane process reads its
# credentials from its own LaunchAgent, which stays off the repository.
set -euo pipefail

BREW_PREFIX="${BREW_PREFIX:-/opt/homebrew}"
PG_FORMULA="${PG_FORMULA:-postgresql@18}"
PG_DATA="$BREW_PREFIX/var/$PG_FORMULA"
PG_BIN="$BREW_PREFIX/opt/$PG_FORMULA/bin"
PGBOUNCER_INI="$BREW_PREFIX/etc/pgbouncer.ini"
PGBOUNCER_LABEL="space.eunha.pgbouncer"

# A tenant's pool is small, but a host holds many tenants and each PostgreSQL
# backend is a process costing 19-24 MiB. These are the ceiling, not a target.
PG_MAX_CONNECTIONS="${PG_MAX_CONNECTIONS:-300}"
# PgBouncer holds a descriptor per client connection; a login shell's 256 would
# cap the fleet at a couple of hundred clients regardless of anything else.
PGBOUNCER_MAX_FILES="${PGBOUNCER_MAX_FILES:-20000}"
# A quarter of memory is the usual advice for a host that does nothing but serve
# PostgreSQL. This one also runs the control plane, the tenant runtime, Redis,
# Temporal and unrelated sites, so it keeps a smaller share by default. Raise it
# on a dedicated database host.
PG_SHARED_BUFFERS_PCT="${PG_SHARED_BUFFERS_PCT:-15}"
PG_EFFECTIVE_CACHE_PCT="${PG_EFFECTIVE_CACHE_PCT:-50}"

say() { printf '  %s\n' "$*"; }
step() { printf '\n== %s\n' "$*"; }
fail() { printf 'provision: %s\n' "$*" >&2; exit 1; }

# Where this repository is. Piped over stdin the script cannot find itself, so
# an explicit path wins, then its own location, then the deploy checkout.
repo_root() {
  local candidate self
  for candidate in "${EUNHA_REPO_ROOT:-}" "$HOME/Git/eunha-space"; do
    [ -n "$candidate" ] || continue
    [ -f "$candidate/ops/pgbouncer/pgbouncer.ini" ] && { printf '%s' "$candidate"; return 0; }
  done
  self="${BASH_SOURCE[0]:-}"
  if [ -n "$self" ] && [ -f "$self" ]; then
    (cd "$(dirname "$self")/../.." && pwd)
    return 0
  fi
  fail "cannot find the repository; set EUNHA_REPO_ROOT to its path"
}

need_brew() {
  [ -x "$BREW_PREFIX/bin/brew" ] || fail "Homebrew is not installed at $BREW_PREFIX"
}

# Install a formula only when it is missing, so a re-run costs nothing.
install_formula() {
  local formula="$1"
  if "$BREW_PREFIX/bin/brew" list --formula "$formula" >/dev/null 2>&1; then
    say "$formula already installed"
  else
    say "installing $formula"
    "$BREW_PREFIX/bin/brew" install "$formula"
  fi
}

# Memory this machine has, in MiB. Tuning follows the host rather than a number
# written down once and carried to a differently sized machine.
memory_mib() { echo $(( $(sysctl -n hw.memsize) / 1024 / 1024 )); }

psql_admin() { "$PG_BIN/psql" -X -q -At -d postgres "$@"; }

# Compare in bytes, so that '2GB' and '2048MB' are not mistaken for a change.
setting_differs() {
  local name="$1" want="$2"
  case "$name" in
    max_connections)
      [ "$(psql_admin -c "SELECT current_setting('$name')")" != "$want" ] ;;
    *)
      [ "$(psql_admin -c "SELECT pg_size_bytes(current_setting('$name')) <> pg_size_bytes('$want')")" = t ] ;;
  esac
}

# ALTER SYSTEM writes postgresql.auto.conf, which overrides postgresql.conf and
# cannot accumulate duplicate lines, so re-running this is free.
#
# It runs even when the value is already live, because "already correct" and
# "declared here" are different things: a setting that matches only because
# somebody hand-edited postgresql.conf would silently revert the next time that
# file was tidied. What the report calls a change is the running value moving,
# not whether the statement ran. DRY_RUN says what would change and touches
# nothing.
alter_system() {
  local name="$1" want="$2" current
  current="$(psql_admin -c "SELECT current_setting('$name')")"
  local changing=0
  setting_differs "$name" "$want" && changing=1
  if [ -n "${DRY_RUN:-}" ]; then
    if [ "$changing" -eq 1 ]; then
      say "would set $name: $current -> $want"
    else
      say "$name already $current, would restate it in postgresql.auto.conf"
    fi
    return 0
  fi
  psql_admin -c "ALTER SYSTEM SET $name = '$want'"
  if [ "$changing" -eq 1 ]; then
    say "set $name: $current -> $want"
  else
    say "$name already $current"
  fi
}

cmd_packages() {
  step "Packages"
  need_brew
  # haproxy fronts the runtime's blue/green slots, redis backs every tenant's
  # cache and queues, cloudflared publishes the instances.
  for formula in "$PG_FORMULA" redis haproxy pgbouncer cloudflared; do
    install_formula "$formula"
  done
}

cmd_postgres() {
  step "PostgreSQL"
  need_brew
  install_formula "$PG_FORMULA"
  [ -d "$PG_DATA" ] || fail "no data directory at $PG_DATA; run 'brew services start $PG_FORMULA' once to initialise it"
  "$PG_BIN/pg_isready" -q -h 127.0.0.1 || fail "PostgreSQL is not accepting connections; start it before tuning"

  local total shared cache
  total="$(memory_mib)"
  shared=$(( total * PG_SHARED_BUFFERS_PCT / 100 ))
  cache=$(( total * PG_EFFECTIVE_CACHE_PCT / 100 ))
  say "host has ${total} MiB (shared_buffers ${PG_SHARED_BUFFERS_PCT}%, effective_cache_size ${PG_EFFECTIVE_CACHE_PCT}%)"

  alter_system max_connections "$PG_MAX_CONNECTIONS"
  alter_system shared_buffers "${shared}MB"
  alter_system effective_cache_size "${cache}MB"
  [ -n "${DRY_RUN:-}" ] || psql_admin -c "SELECT pg_reload_conf()" >/dev/null

  # Say plainly what is live and what is only written down. PostgreSQL tracks
  # that itself: `pending_restart` is set when a postmaster-context setting has
  # been changed in a file but the running server still has the old value.
  local line
  while read -r line; do say "$line"; done < <(psql_admin -F' = ' -c "
    SELECT name, current_setting(name) FROM pg_settings
    WHERE name IN ('max_connections','shared_buffers','effective_cache_size')
    ORDER BY name")
  local pending
  pending="$(psql_admin -c "SELECT string_agg(name, ', ' ORDER BY name) FROM pg_settings WHERE pending_restart")"
  if [ -n "$pending" ]; then
    say "waiting on a restart: $pending"
    say "run 'provision.sh restart-postgres' when a brief outage for every database on this host is acceptable"
  fi
}

cmd_restart_postgres() {
  step "Restarting PostgreSQL"
  local pidfile="$PG_DATA/postmaster.pid"
  local before after i
  before="$(head -1 "$pidfile" 2>/dev/null || echo none)"
  say "postmaster is pid $before"

  # A fast shutdown disconnects clients and checkpoints; a plain SIGTERM would
  # be a smart shutdown, which waits for idle clients that never leave.
  #
  # Do not wait for the stop. launchd's KeepAlive relaunches PostgreSQL as soon
  # as it exits, sometimes within the same second, and `pg_ctl -w stop` then
  # sits watching a server that is already back and eventually calls it a
  # failure. What matters is that a *new* postmaster is serving, so watch for
  # that instead.
  "$PG_BIN/pg_ctl" -D "$PG_DATA" -m fast -W stop

  for i in $(seq 1 90); do
    after="$(head -1 "$pidfile" 2>/dev/null || echo none)"
    if [ "$after" != none ] && [ "$after" != "$before" ] && "$PG_BIN/pg_isready" -q -h 127.0.0.1; then
      say "restarted as pid $after after ${i}s"
      local line
      while read -r line; do say "$line"; done < <(psql_admin -F' = ' -c "
        SELECT name, current_setting(name) FROM pg_settings
        WHERE name IN ('max_connections','shared_buffers') ORDER BY name")
      local pending
      pending="$(psql_admin -c "SELECT string_agg(name, ', ' ORDER BY name) FROM pg_settings WHERE pending_restart")"
      [ -z "$pending" ] || say "still waiting on a restart: $pending"
      return 0
    fi
    sleep 1
  done

  # Nothing brought it back, which means no supervisor is watching it.
  if [ "$(head -1 "$pidfile" 2>/dev/null || echo none)" = none ]; then
    fail "PostgreSQL stopped and nothing restarted it; start it with 'brew services start $PG_FORMULA'"
  fi
  fail "PostgreSQL did not come back; check $BREW_PREFIX/var/log/$PG_FORMULA.log"
}

cmd_pgbouncer() {
  step "PgBouncer"
  need_brew
  install_formula pgbouncer
  local reload=0
  local template
  template="$(repo_root)/ops/pgbouncer/pgbouncer.ini"
  [ -f "$template" ] || fail "missing $template"

  # The pooler authenticates clients by asking PostgreSQL about the role, so the
  # account it asks as is the one thing that differs between hosts.
  local auth_user="${PGBOUNCER_AUTH_USER:-$(whoami)}"
  mkdir -p "$BREW_PREFIX/var/log" "$BREW_PREFIX/var/run"
  local rendered
  rendered="$(mktemp)"
  sed "s/@AUTH_USER@/$auth_user/g" "$template" >"$rendered"
  if [ -f "$PGBOUNCER_INI" ] && cmp -s "$rendered" "$PGBOUNCER_INI"; then
    say "$PGBOUNCER_INI already current"
    rm -f "$rendered"
  else
    [ ! -f "$PGBOUNCER_INI" ] || cp -p "$PGBOUNCER_INI" "$PGBOUNCER_INI.bak.$(date +%Y%m%d%H%M%S)"
    mv "$rendered" "$PGBOUNCER_INI"
    chmod 0644 "$PGBOUNCER_INI"
    say "wrote $PGBOUNCER_INI (auth_user=$auth_user)"
    reload=1
  fi

  local plist="$HOME/Library/LaunchAgents/$PGBOUNCER_LABEL.plist"
  local draft
  draft="$(mktemp -d)/$PGBOUNCER_LABEL.plist"
  /usr/bin/plutil -create xml1 "$draft"
  /usr/bin/plutil -insert Label -string "$PGBOUNCER_LABEL" "$draft"
  /usr/bin/plutil -insert ProgramArguments -json "[\"$BREW_PREFIX/opt/pgbouncer/bin/pgbouncer\",\"$PGBOUNCER_INI\"]" "$draft"
  /usr/bin/plutil -insert RunAtLoad -bool true "$draft"
  /usr/bin/plutil -insert KeepAlive -bool true "$draft"
  /usr/bin/plutil -insert ProcessType -string Background "$draft"
  /usr/bin/plutil -insert SoftResourceLimits -json "{\"NumberOfFiles\":$PGBOUNCER_MAX_FILES}" "$draft"
  /usr/bin/plutil -insert StandardOutPath -string "$HOME/Library/Logs/$PGBOUNCER_LABEL.out.log" "$draft"
  /usr/bin/plutil -insert StandardErrorPath -string "$HOME/Library/Logs/$PGBOUNCER_LABEL.err.log" "$draft"
  /usr/bin/plutil -lint "$draft" >/dev/null
  if [ -f "$plist" ] && cmp -s "$draft" "$plist"; then
    say "LaunchAgent already current"
  else
    mkdir -p "$HOME/Library/LaunchAgents"
    cp "$draft" "$plist"
    say "wrote $plist (NumberOfFiles=$PGBOUNCER_MAX_FILES)"
    reload=1
  fi

  # Restarting the pooler drops every tenant's connections, so do it only when
  # something actually changed or it is not serving.
  if /usr/sbin/lsof -nP -iTCP:6432 -sTCP:LISTEN >/dev/null 2>&1; then
    if [ "$reload" -eq 0 ]; then
      say "already listening on 6432 with this configuration; leaving it alone"
      grep "file descriptor limit" "$BREW_PREFIX/var/log/pgbouncer.log" | tail -1 | sed 's/^/  /'
      return 0
    fi
  else
    reload=1
  fi

  launchctl bootout "gui/$(id -u)/$PGBOUNCER_LABEL" 2>/dev/null || true
  launchctl bootstrap "gui/$(id -u)" "$plist"
  local i
  for i in $(seq 1 30); do
    if /usr/sbin/lsof -nP -iTCP:6432 -sTCP:LISTEN >/dev/null 2>&1; then
      say "listening on 6432 after ${i}s"
      # The descriptor limit fails quietly and only under load, so read back
      # what the running process actually got.
      grep "file descriptor limit" "$BREW_PREFIX/var/log/pgbouncer.log" | tail -1 | sed 's/^/  /'
      return 0
    fi
    sleep 1
  done
  fail "PgBouncer did not start; check $BREW_PREFIX/var/log/pgbouncer.log"
}

cmd_runtime() {
  step "Runtime directories"
  local name="${1:?usage: provision.sh runtime <runtime-name>}"
  local base="$HOME/eunha-runtimes/$name"
  # `releases` holds unpacked builds, `current` points at one of them, `slots`
  # holds the blue and green symlinks, `tenants` holds the rendered files the
  # control plane writes. Only the directories are made here; the router script
  # fills the rest on its first bootstrap.
  local dir
  for dir in releases slots tenants router; do
    if [ -d "$base/$dir" ]; then
      say "$base/$dir exists"
    else
      mkdir -p "$base/$dir"
      say "created $base/$dir"
    fi
  done
  chmod 0700 "$base/tenants"
  say "tenants directory is 0700; rendered files carry tenant secrets"
}

cmd_status() {
  step "Status"
  local formula
  for formula in "$PG_FORMULA" redis haproxy pgbouncer cloudflared; do
    if "$BREW_PREFIX/bin/brew" list --formula "$formula" >/dev/null 2>&1; then
      say "package $formula: installed"
    else
      say "package $formula: MISSING"
    fi
  done
  local label
  for label in "homebrew.mxcl.$PG_FORMULA" homebrew.mxcl.redis "$PGBOUNCER_LABEL" space.eunha.control-plane; do
    if launchctl list 2>/dev/null | grep -q "	$label\$"; then
      say "service $label: loaded"
    else
      say "service $label: not loaded"
    fi
  done
  if "$PG_BIN/pg_isready" -q -h 127.0.0.1; then
    psql_admin -c "SELECT 'postgres ' || name || ' = ' || current_setting(name) FROM pg_settings WHERE name IN ('max_connections','shared_buffers','effective_cache_size') ORDER BY name" | while read -r line; do say "$line"; done
  else
    say "postgres: not accepting connections"
  fi
  if /usr/sbin/lsof -nP -iTCP:6432 -sTCP:LISTEN >/dev/null 2>&1; then
    say "pgbouncer: listening on 6432"
  else
    say "pgbouncer: not listening"
  fi
}

usage() {
  cat >&2 <<'USAGE'
usage: provision.sh <step> [argument]

  status              what is installed, loaded and set right now
  plan                what the postgres step would change, changing nothing
  packages            install the Homebrew formulas a host needs
  postgres            install and tune PostgreSQL for this host's memory
  restart-postgres    fast-restart PostgreSQL so pending settings take effect
  pgbouncer           write the pooler config and its LaunchAgent, then start it
  runtime <name>      create the directory layout for one runtime
  all                 packages, postgres, pgbouncer (no restart)

Environment: BREW_PREFIX, PG_FORMULA, PG_MAX_CONNECTIONS, PG_SHARED_BUFFERS_PCT,
PG_EFFECTIVE_CACHE_PCT, PGBOUNCER_AUTH_USER, PGBOUNCER_MAX_FILES,
EUNHA_REPO_ROOT, DRY_RUN.
USAGE
  exit 64
}

case "${1:-}" in
  status) cmd_status ;;
  plan) DRY_RUN=1 cmd_postgres ;;
  packages) cmd_packages ;;
  postgres) cmd_postgres ;;
  restart-postgres) cmd_restart_postgres ;;
  pgbouncer) cmd_pgbouncer ;;
  runtime) shift; cmd_runtime "${1:-}" ;;
  all) cmd_packages; cmd_postgres; cmd_pgbouncer ;;
  *) usage ;;
esac

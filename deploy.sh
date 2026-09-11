#!/bin/bash
set -euo pipefail

export PATH="/opt/homebrew/bin:$HOME/.cargo/bin:$PATH"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

( cd "$ROOT" && mise exec -- pnpm --dir frontend install --frozen-lockfile )
( cd "$ROOT" && mise exec -- pnpm --dir frontend run build )
( cd "$ROOT" && mise exec -- cargo build --release )

UID_N="$(id -u)"
LABEL="space.eunha.control-plane"
launchctl kickstart -k "gui/$UID_N/$LABEL"

for _ in $(seq 1 30); do
    if curl -sf -o /dev/null http://127.0.0.1:61000/api/session/me; then
        exit 0
    fi
    # A protected endpoint answering 401 proves the HTTP service is ready too.
    if [ "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:61000/api/session/me)" = 401 ]; then
        exit 0
    fi
    sleep 1
done

echo "eunha.space did not become ready" >&2
exit 1

#!/bin/bash
set -euo pipefail
export PATH="/opt/homebrew/bin:$HOME/.cargo/bin:$PATH"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec mise exec -- python3 "$ROOT/ops/deploy-control-plane.py"

#!/usr/bin/env python3
"""Record readiness changes for launchd/host monitoring, without exposing secrets."""
import json
import os
from pathlib import Path
import sys
import urllib.request

url = os.environ.get("CONTROL_HEALTH_URL", "http://127.0.0.1:60001/health/ready")
state_file = Path.home() / "Library/Application Support/eunha-space/health-state.json"
try:
    with urllib.request.urlopen(url, timeout=6) as response:
        body = json.load(response)
        ready = body.get("ready") is True
except Exception:
    ready = False
try:
    previous = json.loads(state_file.read_text()).get("ready")
except (OSError, ValueError):
    previous = None
state_file.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
state_file.write_text(json.dumps({"ready": ready}) + "\n")
state_file.chmod(0o600)
if previous != ready:
    print("Control plane readiness " + ("recovered." if ready else "FAILED; check dependencies and controller progress."), flush=True)
sys.exit(0 if ready else 1)

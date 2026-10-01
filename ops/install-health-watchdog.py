#!/usr/bin/env python3
from pathlib import Path
import os
import plistlib
import subprocess
import sys

root = Path(__file__).resolve().parent.parent
label = "space.eunha.health-watchdog"
path = Path.home() / "Library/LaunchAgents" / (label + ".plist")
logs = Path.home() / "Library/Logs/eunha-space"
logs.mkdir(parents=True, exist_ok=True)
config = {"Label": label, "ProgramArguments": [sys.executable, str(root / "ops/health-watchdog.py")], "StartInterval": 60, "RunAtLoad": True, "StandardOutPath": str(logs / "health.log"), "StandardErrorPath": str(logs / "health.err")}
with path.open("wb") as stream:
    plistlib.dump(config, stream)
path.chmod(0o600)
domain = f"gui/{os.getuid()}"
subprocess.run(["launchctl", "bootout", domain + "/" + label], capture_output=True)
subprocess.run(["launchctl", "bootstrap", domain, str(path)], check=True, capture_output=True)
print("Readiness watchdog installed; checks every 60 seconds and logs state changes.")

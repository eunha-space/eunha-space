#!/usr/bin/env python3
from pathlib import Path
import os
import plistlib
import subprocess
import sys

root = Path(__file__).resolve().parent.parent
label = "space.eunha.restore-drill"
path = Path.home() / "Library/LaunchAgents" / (label + ".plist")
logs = Path.home() / "Library/Logs/eunha-space"
logs.mkdir(parents=True, exist_ok=True)
report = Path.home() / "Library/Application Support/eunha-space/restore-latest.json"
config = {"Label": label, "ProgramArguments": [sys.executable, str(root / "ops/backup/restore-drill.py"), "--report", str(report)], "StartCalendarInterval": {"Weekday": 0, "Hour": 4, "Minute": 30}, "StandardOutPath": str(logs / "restore-drill.log"), "StandardErrorPath": str(logs / "restore-drill.err")}
with path.open("wb") as stream:
    plistlib.dump(config, stream)
path.chmod(0o600)
domain = f"gui/{os.getuid()}"
subprocess.run(["launchctl", "bootout", domain + "/" + label], capture_output=True)
subprocess.run(["launchctl", "bootstrap", domain, str(path)], check=True, capture_output=True)
print("Weekly restore drill installed for Sunday 04:30 host time.")

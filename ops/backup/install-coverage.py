#!/usr/bin/env python3
"""Extend the existing host backup with the service's recovery configuration.
This edits only the CONFIG_FILES list and adds a globals archive before completion.
Original script is retained alongside it. Run on the production host.
"""
from pathlib import Path
import os
import plistlib
import re
import shutil
import subprocess

HOME = Path.home()
SCRIPT = HOME / "bin/backup-postgres-r2.zsh"
MARKER = "# eunha-space recovery coverage"


def main():
    source = SCRIPT.read_text()
    files = [HOME / "Library/LaunchAgents/space.eunha.control-plane.plist", HOME / "Library/LaunchAgents/space.eunha.cloudflared.plist"]
    temporal = next((directory for directory in [HOME / "eunha-temporal", HOME / "Git/eunha-space/ops/temporal"] if (directory / ".env.production").is_file()), None)
    if temporal is None:
        raise RuntimeError("Temporal environment file missing")
    files.append(temporal / ".env.production")
    compose = temporal / "docker-compose.yml"
    if not compose.is_file():
        compose = temporal / "docker-compose.production.yml"
    files.append(compose)
    tunnel = files[1]
    with tunnel.open("rb") as stream:
        config = plistlib.load(stream)
    for argument in config.get("ProgramArguments", [])[1:]:
        path = Path(argument)
        if path.is_absolute() and path.is_file():
            files.append(path)
            if path.suffix in (".yaml", ".yml"):
                match = re.search(r"^\s*credentials-file:\s*['\"]?([^'\"\n]+)", path.read_text(), re.M)
                if match:
                    files.append(Path(os.path.expandvars(match.group(1).strip())).expanduser())
    if not all(path.is_file() for path in files):
        raise RuntimeError("required recovery configuration missing")
    if MARKER not in source:
        additions = MARKER + "\n" + "\n".join('  "' + str(path).replace(str(HOME), "$HOME") + '"' for path in files) + "\n"
        source = source.replace("CONFIG_FILES=(\n", "CONFIG_FILES=(\n" + additions, 1)
        # Dump global roles with password hashes so restored tenant connection
        # identities can be recovered. This file is sensitive, like tenant TOML.
        marker = 'echo "$(ts) backup completed:'
        index = source.index(marker)
        source = source[:index] + '''# eunha-space globals archive: a backup is complete only if role recovery is present.
if ! "$PG_BIN/pg_dumpall" --globals-only > "$work/globals.sql" \\
    || ! rclone_ copyto "$work/globals.sql" "b2:${B2_BUCKET}/${SET}/postgres-globals.sql"; then
  fail "PostgreSQL role backup failed; backup set is incomplete."
fi
# Keep the runnable release paired with its process configuration.
if ! rclone_ copy "$HOME/Git/eunha-space/.control-releases/current" \
    "b2:${B2_BUCKET}/${SET}/control-plane-release" --copy-links; then
  fail "Control-plane release backup failed; backup set is incomplete."
fi
''' + source[index:]
        backup = SCRIPT.with_suffix(".zsh.before-eunha-recovery")
        if not backup.exists():
            shutil.copy2(SCRIPT, backup)
            backup.chmod(0o700)
        temporary = SCRIPT.with_suffix(".zsh.next")
        temporary.write_text(source)
        temporary.chmod(SCRIPT.stat().st_mode & 0o777)
        subprocess.run(["/bin/zsh", "-n", str(temporary)], check=True, capture_output=True)
        os.replace(temporary, SCRIPT)
    print(f"Recovery coverage installed: {len(files)} service/tunnel configuration files and PostgreSQL roles.")


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"Backup coverage installation failed ({type(error).__name__}).")
        raise SystemExit(1)

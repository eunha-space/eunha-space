#!/usr/bin/env python3
"""Build immutable releases, atomically activate them, and roll back unhealthy starts.
Never print the service environment or captured database/tool error output.
"""
import datetime
import glob
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
LABEL = "space.eunha.control-plane"
PLIST = Path.home() / "Library/LaunchAgents" / f"{LABEL}.plist"
RELEASES = ROOT / ".control-releases"
CURRENT = RELEASES / "current"


def run(args, **kwargs):
    return subprocess.run(args, check=True, **kwargs)


def atomic_link(target, destination=None):
    destination = destination or CURRENT
    temporary = destination.with_name(destination.name + ".next")
    temporary.unlink(missing_ok=True)
    temporary.symlink_to(target)
    os.replace(temporary, destination)


def atomic_plist(config):
    temporary = PLIST.with_suffix(".plist.next")
    with temporary.open("wb") as output:
        os.chmod(temporary, 0o600)
        plistlib.dump(config, output)
    os.replace(temporary, PLIST)


def database_env(config):
    from urllib.parse import urlparse, unquote
    url = urlparse(config["EnvironmentVariables"]["CONTROL_DATABASE_URL"])
    env = os.environ.copy()
    env.update(PGHOST=url.hostname or "localhost", PGPORT=str(url.port or 5432),
               PGUSER=unquote(url.username or ""), PGPASSWORD=unquote(url.password or ""),
               PGDATABASE=unquote(url.path.lstrip("/")), PGCONNECT_TIMEOUT="5")
    return env


def sql(config, statement):
    clients = glob.glob("/opt/homebrew/opt/postgresql*/bin/psql")
    if not clients:
        raise RuntimeError("PostgreSQL client unavailable")
    result = subprocess.run([clients[-1], "-X", "-At", "-v", "ON_ERROR_STOP=1"],
                            input=statement, env=database_env(config), capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError("Deployment database guard failed; service was not switched")
    return result.stdout.strip()


def restart():
    domain = f"gui/{os.getuid()}"
    subprocess.run(["launchctl", "bootout", f"{domain}/{LABEL}"], capture_output=True)
    # bootout can return before launchd finishes removing a gracefully stopping
    # service. Retry registration for a bounded interval before rolling back.
    for attempt in range(30):
        registration = subprocess.run(["launchctl", "bootstrap", domain, str(PLIST)], capture_output=True)
        if registration.returncode == 0:
            break
        time.sleep(1)
    else:
        raise RuntimeError("Service registration failed after shutdown")
    run(["launchctl", "kickstart", f"{domain}/{LABEL}"], capture_output=True)


def wait_ready(url, expected, seconds=120, legacy=False):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(url + "/health/ready", timeout=5) as response:
                result = json.load(response)
                if result.get("ready") and result.get("release") == expected:
                    return True
        except (urllib.error.URLError, ValueError, OSError):
            if legacy:
                try:
                    with urllib.request.urlopen(url + "/api/session/me", timeout=5):
                        return True
                except urllib.error.HTTPError as error:
                    if error.code == 401:
                        return True
                except (urllib.error.URLError, OSError):
                    pass
        time.sleep(1)
    return False


def main():
    with PLIST.open("rb") as source:
        original = plistlib.load(source)
    RELEASES.mkdir(mode=0o700, exist_ok=True)
    # Local process lock stops two operators racing the symlink or plist.
    import fcntl
    with (RELEASES / "deploy.lock").open("w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        deploy(original)


def deploy(original):
    # Bootstrap this additive table before the first hardened binary is active.
    sql(original, (ROOT / "migrations/043_control_plane_maintenance.sql").read_text())
    sql(original, """BEGIN;
SELECT pg_advisory_xact_lock(60001001);
DO $$ BEGIN
IF EXISTS(SELECT 1 FROM operations WHERE kind='runtime_release' AND status IN ('pending','running'))
THEN RAISE EXCEPTION 'runtime release active'; END IF;
END $$;
UPDATE control_plane_maintenance SET deploy_until=now()+interval '30 minutes' WHERE singleton;
COMMIT;""")
    previous = CURRENT.resolve() if CURRENT.is_symlink() else None
    switched = False
    config_changed = False
    automation_record = RELEASES / "paused-automation.json"
    automation_ids = json.loads(automation_record.read_text()) if automation_record.exists() else []
    try:
        if previous is None and not automation_ids:
            paused = sql(original, "UPDATE runtimes SET auto_deploy_enabled=false WHERE auto_deploy_enabled RETURNING id;")
            automation_ids = re.findall(r"[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}", paused)
            automation_record.write_text(json.dumps(automation_ids))
            automation_record.chmod(0o600)
        commit = subprocess.check_output(["git", "rev-parse", "--short=12", "HEAD"], cwd=ROOT, text=True).strip()
        release_id = commit + "-" + datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
        release = RELEASES / release_id
        release.mkdir()
        # Preserve the pre-immutable executable AND assets before rebuilding either.
        if previous is None:
            legacy = RELEASES / ("legacy-" + datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ"))
            legacy.mkdir()
            shutil.copy2(original["ProgramArguments"][0], legacy / "eunha-space")
            shutil.copytree(ROOT / "frontend/dist", legacy / "frontend/dist")
            (legacy / "release-id").write_text("legacy")
            previous = legacy
        env = os.environ.copy()
        env["CI"] = "true"
        run(["mise", "exec", "--", "pnpm", "--dir", "frontend", "install", "--frozen-lockfile"], cwd=ROOT, env=env)
        run(["mise", "exec", "--", "pnpm", "exec", "tsc", "-b"], cwd=ROOT / "frontend", env=env)
        run(["mise", "exec", "--", "pnpm", "exec", "vite", "build", "--outDir", str(release / "frontend/dist")], cwd=ROOT / "frontend", env=env)
        run(["mise", "exec", "--", "cargo", "test"], cwd=ROOT)
        run(["mise", "exec", "--", "cargo", "build", "--release"], cwd=ROOT)
        shutil.copy2(ROOT / "target/release/eunha-space", release / "eunha-space")
        old_assets = previous / "frontend/dist/assets"
        if old_assets.is_dir():
            for asset in old_assets.iterdir():
                destination = release / "frontend/dist/assets" / asset.name
                if asset.is_file() and not destination.exists():
                    shutil.copy2(asset, destination)
        (release / "release-id").write_text(release_id)
        # Immutable files; retained releases are not implicitly deleted.
        for file in release.rglob("*"):
            if file.is_file():
                file.chmod(0o555 if file.name == "eunha-space" else 0o444)
        new_config = dict(original)
        new_config["ProgramArguments"] = [str(CURRENT / "eunha-space"), *original["ProgramArguments"][1:]]
        new_config["WorkingDirectory"] = str(ROOT)
        new_config["EnvironmentVariables"] = dict(original.get("EnvironmentVariables", {}))
        new_config["EnvironmentVariables"].update(CONTROL_PROJECT_ROOT=str(ROOT), CONTROL_FRONTEND_DIRECTORY=str(CURRENT / "frontend/dist"), CONTROL_RELEASE_ID=release_id)
        sql(original, """BEGIN;
SELECT pg_advisory_xact_lock(60001001);
DO $$ BEGIN
IF EXISTS(SELECT 1 FROM operations WHERE kind='runtime_release' AND status IN ('pending','running'))
THEN RAISE EXCEPTION 'runtime release active'; END IF;
END $$;
UPDATE control_plane_maintenance SET deploy_until=now()+interval '30 minutes' WHERE singleton;
COMMIT;""")
        atomic_link(release)
        switched = True
        atomic_plist(new_config)
        config_changed = True
        restart()
        address = new_config["EnvironmentVariables"].get("BIND_ADDRESS", "127.0.0.1:60001")
        url = "http://" + address
        if not wait_ready(url, release_id):
            raise RuntimeError("New release did not become ready")
        atomic_link(previous, RELEASES / "previous")
        print(f"Deployed {release_id}; readiness passed.")
    except BaseException:
        if switched:
            atomic_link(previous)
        if config_changed:
            rollback = dict(original)
            # On the first deployment the legacy binary must come from its snapshot,
            # since the old target/release binary has already been overwritten.
            if (previous / "release-id").read_text() == "legacy":
                rollback["ProgramArguments"] = [str(previous / "eunha-space"), *original["ProgramArguments"][1:]]
                # Legacy binary has no frontend-directory setting: restore its assets.
                shutil.rmtree(ROOT / "frontend/dist")
                shutil.copytree(previous / "frontend/dist", ROOT / "frontend/dist")
            atomic_plist(rollback)
            restart()
            address = rollback.get("EnvironmentVariables", {}).get("BIND_ADDRESS", "127.0.0.1:60001")
            restored = wait_ready("http://" + address, rollback.get("EnvironmentVariables", {}).get("CONTROL_RELEASE_ID"), legacy=(previous / "release-id").read_text() == "legacy")
            print("Previous release restored; health " + ("passed." if restored else "FAILED; operator action required."), file=sys.stderr)
        raise
    finally:
        if automation_ids:
            # UUIDs came from our database; validate again before SQL construction.
            if not all(re.fullmatch(r"[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}", identity) for identity in automation_ids):
                raise RuntimeError("invalid paused automation record")
            identifiers = ",".join("'" + identity + "'" for identity in automation_ids)
            sql(original, "UPDATE runtimes SET auto_deploy_enabled=true WHERE id IN (" + identifiers + ");")
        automation_record.unlink(missing_ok=True)
        sql(original, "UPDATE control_plane_maintenance SET deploy_until=NULL WHERE singleton;")


if __name__ == "__main__":
    try:
        main()
    except BaseException as error:
        # subprocess exceptions can contain captured output; omit all exception text.
        print(f"Deployment failed ({type(error).__name__}).", file=sys.stderr)
        sys.exit(1)

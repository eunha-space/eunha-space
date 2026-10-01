#!/usr/bin/env python3
"""Restore off-host Eunha backups in a private, socket-only disposable cluster.
No production database is modified and no database contents or credentials are printed.
"""
import argparse
import datetime
import hashlib
import gzip
import json
import os
from pathlib import Path
import re
import plistlib
import shutil
import subprocess
import tempfile
import time


def environment():
    raw = subprocess.check_output(["/bin/zsh", "-c", 'set -a; source "$HOME/.config/r2-postgres-backup/env"; /usr/bin/env -0'])
    env = os.environ.copy()
    env.update(item.decode().split("=", 1) for item in raw.split(b"\0") if b"=" in item)
    env.update(RCLONE_CONFIG_B2_TYPE="b2", RCLONE_CONFIG_B2_ACCOUNT=env["B2_KEY_ID"], RCLONE_CONFIG_B2_KEY=env["B2_APPLICATION_KEY"],
               RCLONE_CONFIG_R2_TYPE="s3", RCLONE_CONFIG_R2_PROVIDER="Cloudflare", RCLONE_CONFIG_R2_ACCESS_KEY_ID=env["R2_MIRROR_READ_ACCESS_KEY_ID"],
               RCLONE_CONFIG_R2_SECRET_ACCESS_KEY=env["R2_MIRROR_READ_SECRET_ACCESS_KEY"], RCLONE_CONFIG_R2_ENDPOINT=env["R2_ENDPOINT_URL"])
    return env


def execute(args, **kwargs):
    result = subprocess.run(args, capture_output=True, **kwargs)
    if result.returncode:
        raise RuntimeError("restore command failed: " + Path(args[0]).name)
    return result.stdout


def latest_completed_set():
    log = Path.home() / "Library/Logs/r2-postgres-backup/backup.log"
    matches = re.findall(r"backup completed: .*? in b2:[^/]+/(sets/[^/]+/[0-9TZ]+)/", log.read_text())
    if not matches:
        raise RuntimeError("no completed backup set recorded")
    return matches[-1]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--set", default=None, help="completed set path; defaults to most recent successful backup")
    parser.add_argument("--report", required=True, type=Path)
    args = parser.parse_args()
    backup_set = args.set or latest_completed_set()
    if not re.fullmatch(r"sets/[A-Za-z0-9_.-]+/[0-9TZ]+", backup_set):
        raise RuntimeError("invalid backup set")
    env = environment()
    remote = f"b2:{env['B2_BUCKET']}/{backup_set}"
    rclone = "/opt/homebrew/bin/rclone"
    files = json.loads(execute([rclone, "lsjson", remote, "--files-only", "--config", "/dev/null"], env=env))
    archive_names = {item["Name"] for item in files}
    recovery_files = {"postgres-globals.sql", "space.eunha.control-plane.plist", "space.eunha.cloudflared.plist", "eunha.yml", ".env.production"}
    if not recovery_files.issubset(archive_names):
        raise RuntimeError("backup lacks service recovery configuration or PostgreSQL roles")
    compose = next((name for name in ["docker-compose.yml", "docker-compose.production.yml"] if name in archive_names), None)
    if compose is None:
        raise RuntimeError("Temporal compose archive missing")
    recovery_files.add(compose)
    names = sorted(item["Name"] for item in files if re.fullmatch(r"(?:eunha_space|eunha_t_[a-f0-9]{32}|temporal|temporal_visibility)\.dump\.gz", item["Name"]))
    required = {"eunha_space.dump.gz", "temporal.dump.gz", "temporal_visibility.dump.gz"}
    if not required.issubset(names) or not any(name.startswith("eunha_t_") for name in names):
        raise RuntimeError("backup lacks required control-plane, Temporal, or tenant databases")
    pg = Path(env.get("PG_BIN", "/opt/homebrew/opt/postgresql@18/bin"))
    started = time.monotonic()
    report = {"backup_set": backup_set, "databases": [], "passed": False, "roles_restored": False, "service_configuration_verified": False, "release_verified": False, "media_samples_verified": 0}
    with tempfile.TemporaryDirectory(prefix="eunha-restore-", dir="/private/tmp") as temporary:
        root = Path(temporary)
        os.chmod(root, 0o700)
        for filename in sorted(recovery_files):
            execute([rclone, "copyto", remote + "/" + filename, str(root / filename), "--config", "/dev/null"], env=env)
        with (root / "space.eunha.control-plane.plist").open("rb") as stream:
            service = plistlib.load(stream)
        if not all(service.get("EnvironmentVariables", {}).get(key) for key in ["CONTROL_DATABASE_URL", "TEMPORAL_ADDRESS"]):
            raise RuntimeError("backed-up service configuration is incomplete")
        credential = re.search(r"^\s*credentials-file:\s*['\"]?([^'\"\n]+)", (root / "eunha.yml").read_text(), re.M)
        if not credential or Path(credential.group(1).strip()).name not in archive_names:
            raise RuntimeError("tunnel credential archive is missing")
        execute([rclone, "copyto", remote + "/" + Path(credential.group(1).strip()).name, str(root / "tunnel-credentials.json"), "--config", "/dev/null"], env=env)
        if not json.loads((root / "tunnel-credentials.json").read_text()).get("TunnelSecret"):
            raise RuntimeError("tunnel credential archive is invalid")
        report["service_configuration_verified"] = True
        release = root / "release"
        execute([rclone, "copy", remote + "/control-plane-release", str(release), "--config", "/dev/null"], env=env)
        if not all((release / path).is_file() and (release / path).stat().st_size > 0 for path in ["eunha-space", "frontend/dist/index.html", "release-id"]):
            raise RuntimeError("release archive is incomplete")
        report["release_verified"] = True
        data = root / "postgres"
        execute([str(pg / "initdb"), "-D", str(data), "-A", "trust", "-U", "eunha_restore_drill", "--no-locale"])
        socket_env = os.environ.copy()
        socket_env.update(PGHOST=str(root), PGPORT="56440", PGUSER="eunha_restore_drill")
        execute([str(pg / "pg_ctl"), "-D", str(data), "-l", str(root / "server.log"), "-o", f"-c listen_addresses='' -k {root} -p 56440", "-w", "start"])
        try:
            execute([str(pg / "psql"), "-X", "-v", "ON_ERROR_STOP=1", "--dbname", "postgres", "-f", str(root / "postgres-globals.sql")], env=socket_env)
            report["roles_restored"] = True
            for name in names:
                database = name[:-len(".dump.gz")]
                step = time.monotonic()
                print(f"Restoring {database} into isolated cluster…", flush=True)
                compressed = root / "backup.gz"
                execute([rclone, "copyto", remote + "/" + name, str(compressed), "--config", "/dev/null"], env=env)
                dump = root / "backup.dump"
                with gzip.open(compressed, "rb") as source, dump.open("wb") as output:
                    shutil.copyfileobj(source, output)
                execute([str(pg / "createdb"), database], env=socket_env)
                execute([str(pg / "pg_restore"), "--exit-on-error", "--no-acl", "--dbname", database, str(dump)], env=socket_env)
                table_count = int(execute([str(pg / "psql"), "-X", "-At", "--dbname", database, "-c", "SELECT count(*) FROM information_schema.tables WHERE table_schema='public' AND table_type='BASE TABLE'"], env=socket_env))
                if table_count == 0:
                    raise RuntimeError("restored database has no tables")
                # Read every table through an aggregate; output includes no row contents.
                execute([str(pg / "psql"), "-X", "-v", "ON_ERROR_STOP=1", "--dbname", database, "-c", "DO $$ DECLARE t record; n bigint; BEGIN FOR t IN SELECT tablename FROM pg_tables WHERE schemaname='public' LOOP EXECUTE format('SELECT count(*) FROM public.%I',t.tablename) INTO n; END LOOP; END $$;"], env=socket_env)
                report["databases"].append({"name": database, "tables": table_count, "restore_seconds": round(time.monotonic() - step, 1)})
                execute([str(pg / "dropdb"), database], env=socket_env)
                dump.unlink()
                compressed.unlink()
            # Verify a bounded content sample from each populated tenant namespace.
            # Ignore objects newer than the backup began; they may not be copied yet.
            cutoff = datetime.datetime.strptime(backup_set.rsplit("/", 1)[1], "%Y%m%dT%H%M%SZ").replace(tzinfo=datetime.timezone.utc)
            objects = json.loads(execute([rclone, "lsjson", "r2:eunha-space-media", "--recursive", "--files-only", "--config", "/dev/null"], env=env))
            samples = {}
            for item in sorted(objects, key=lambda item: item["Size"]):
                path = item["Path"]
                modified = datetime.datetime.fromisoformat(item["ModTime"].replace("Z", "+00:00"))
                if not 0 < item["Size"] <= 1024 * 1024 or modified > cutoff:
                    continue
                namespace = "/".join(path.split("/")[:2]) if path.startswith("t/") else "legacy"
                samples.setdefault(namespace, item)
            if objects and not samples:
                raise RuntimeError("no stable bounded media samples available")
            for item in list(samples.values())[:100]:
                current = execute([rclone, "cat", "r2:eunha-space-media/" + item["Path"], "--config", "/dev/null"], env=env)
                archived = execute([rclone, "cat", f"b2:{env['B2_BUCKET']}/buckets/eunha-space-media/" + item["Path"], "--config", "/dev/null"], env=env)
                if len(current) != item["Size"] or hashlib.sha256(current).digest() != hashlib.sha256(archived).digest():
                    raise RuntimeError("media sample checksum mismatch")
                report["media_samples_verified"] += 1
            report["passed"] = True
        finally:
            execute([str(pg / "pg_ctl"), "-D", str(data), "-m", "immediate", "-w", "stop"])
            report["total_seconds"] = round(time.monotonic() - started, 1)
            args.report.parent.mkdir(parents=True, exist_ok=True)
            args.report.write_text(json.dumps(report, indent=2) + "\n")
            args.report.chmod(0o600)
    print(f"Restore drill passed for {len(names)} databases in {report['total_seconds']} seconds.")


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        # Avoid exceptions that may include connection strings or restored SQL.
        print(f"Restore drill failed ({type(error).__name__}); no production databases were touched.")
        raise SystemExit(1)

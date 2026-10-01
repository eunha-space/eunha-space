import importlib.util
import json
import sys
from pathlib import Path
import plistlib
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("coverage", Path(__file__).parents[1] / "backup/install-coverage.py")
coverage = importlib.util.module_from_spec(spec)
spec.loader.exec_module(coverage)

restore_spec = importlib.util.spec_from_file_location("restore", Path(__file__).parents[1] / "backup/restore-drill.py")
restore = importlib.util.module_from_spec(restore_spec)
restore_spec.loader.exec_module(restore)


class RestoreReportTests(unittest.TestCase):
    def test_early_failure_replaces_stale_success_without_error_contents(self):
        with tempfile.TemporaryDirectory() as temporary:
            report = Path(temporary) / "report.json"
            report.write_text('{"passed":true}')
            with patch.object(sys, "argv", ["restore", "--existing-backups", "--set", "sets/host/20261001T000000Z", "--report", str(report)]), patch.object(restore, "environment", side_effect=RuntimeError("private-credential")):
                with self.assertRaises(RuntimeError):
                    restore.main()
            result = json.loads(report.read_text())
            self.assertFalse(result["passed"])
            self.assertIn("started_at", result)
            self.assertNotIn("private-credential", report.read_text())
            self.assertEqual(report.stat().st_mode & 0o777, 0o600)



class BackupCoverageTests(unittest.TestCase):
    def test_recovery_coverage_is_added_once_and_original_is_preserved(self):
        with tempfile.TemporaryDirectory() as temporary:
            home = Path(temporary)
            scripts = home / "bin"
            scripts.mkdir()
            script = scripts / "backup-postgres-r2.zsh"
            original = 'CONFIG_FILES=(\n)\necho "$(ts) backup completed: success"\n'
            script.write_text(original)
            script.chmod(0o700)
            agents = home / "Library/LaunchAgents"
            agents.mkdir(parents=True)
            cloud = home / ".cloudflared"
            cloud.mkdir()
            (cloud / "eunha.yml").write_text(f'credentials-file: {cloud}/tunnel.json\n')
            (cloud / "tunnel.json").write_text('{"TunnelSecret":"private"}')
            (agents / "space.eunha.control-plane.plist").write_bytes(plistlib.dumps({"ProgramArguments": ["/test"]}))
            (agents / "space.eunha.cloudflared.plist").write_bytes(plistlib.dumps({"ProgramArguments": ["cloudflared", "--config", str(cloud / "eunha.yml")]}))
            temporal = home / "eunha-temporal"
            temporal.mkdir()
            (temporal / ".env.production").write_text("POSTGRES_PWD=private\n")
            (temporal / "docker-compose.yml").write_text("services: {}\n")
            with patch.object(coverage, "HOME", home), patch.object(coverage, "SCRIPT", script), patch.object(coverage.subprocess, "run"):
                coverage.main()
                first = script.read_text()
                coverage.main()
                self.assertEqual(script.read_text(), first)
            self.assertEqual(script.with_suffix(".zsh.before-eunha-recovery").read_text(), original)
            self.assertEqual(first.count(coverage.MARKER), 1)
            self.assertIn("pg_dumpall", first)
            self.assertIn("postgres-globals.sql", first)
            self.assertIn("control-plane-release", first)
            self.assertIn("tunnel.json", first)
            self.assertIn("docker-compose.yml", first)
            self.assertNotIn("POSTGRES_PWD", first)
            self.assertNotIn("TunnelSecret", first)


if __name__ == "__main__":
    unittest.main()

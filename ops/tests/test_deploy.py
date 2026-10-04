import importlib.util
from pathlib import Path
import plistlib
import tempfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace

spec = importlib.util.spec_from_file_location("deploy", Path(__file__).parents[1] / "deploy-control-plane.py")
deploy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(deploy)


class DeploymentTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.releases = self.root / ".control-releases"
        self.releases.mkdir()
        self.original_binary = self.root / "target/release/eunha-space"
        self.original_binary.parent.mkdir(parents=True)
        self.original_binary.write_text("old binary")
        self.assets = self.root / "frontend/dist"
        self.assets.mkdir(parents=True)
        (self.assets / "index.html").write_text("old page")
        (self.assets / "assets").mkdir()
        (self.assets / "assets/old.js").write_text("old asset")
        self.plist = self.root / "service.plist"
        self.original = {"ProgramArguments": [str(self.original_binary)], "EnvironmentVariables": {"CONTROL_DATABASE_URL": "postgresql://user:private@localhost/control"}}
        self.plist.write_bytes(plistlib.dumps(self.original))
        self.sql = []
        for name, value in [("ROOT", self.root), ("RELEASES", self.releases), ("CURRENT", self.releases / "current"), ("PLIST", self.plist)]:
            patcher = patch.object(deploy, name, value)
            patcher.start()
            self.addCleanup(patcher.stop)
        (self.root / "migrations").mkdir()
        (self.root / "migrations/043_control_plane_maintenance.sql").write_text("SELECT 1;")

    def fake_run(self, args, **kwargs):
        if "vite" in args:
            output = Path(args[-1])
            output.mkdir(parents=True)
            (output / "index.html").write_text("new page")
            (output / "assets").mkdir()
            (output / "assets/new.js").write_text("new asset")
        if "cargo" in args and "build" in args:
            self.original_binary.write_text("new binary")

    def execute(self, readiness):
        with patch.object(deploy, "sql", side_effect=lambda config, statement: self.sql.append(statement) or ""), patch.object(deploy, "run", side_effect=self.fake_run), patch.object(deploy.subprocess, "check_output", return_value="abc123\n"), patch.object(deploy, "restart") as restart, patch.object(deploy, "wait_ready", side_effect=readiness):
            deploy.deploy(self.original)
            return restart.call_count

    def test_pull_main_fast_forwards_and_checks_remote_commit(self):
        with patch.object(deploy.subprocess, "check_output", side_effect=["main\n", "", "abc123\n", "abc123\n"]), patch.object(deploy, "run") as command:
            deploy.pull_main()
        command.assert_called_once_with(["git", "pull", "--ff-only", "origin", "main"], cwd=self.root, capture_output=True)

    def test_pull_main_rejects_local_changes_before_network_or_deployment(self):
        for changes in [" M src/main.rs\n", "?? new-file\n"]:
            with self.subTest(changes=changes), patch.object(deploy.subprocess, "check_output", side_effect=["main\n", changes]), patch.object(deploy, "run") as command:
                with self.assertRaisesRegex(RuntimeError, "clean checkout"):
                    deploy.pull_main()
                command.assert_not_called()

    def test_pull_main_rejects_other_branches(self):
        with patch.object(deploy.subprocess, "check_output", return_value="feature\n"), patch.object(deploy, "run") as command:
            with self.assertRaisesRegex(RuntimeError, "main branch"):
                deploy.pull_main()
            command.assert_not_called()

    def test_pull_main_rejects_local_commits_ahead_of_origin(self):
        with patch.object(deploy.subprocess, "check_output", side_effect=["main\n", "", "local\n", "remote\n"]), patch.object(deploy, "run"):
            with self.assertRaisesRegex(RuntimeError, "match origin/main"):
                deploy.pull_main()

    def test_pull_failure_never_reaches_deployment(self):
        with patch.object(deploy, "pull_main", side_effect=RuntimeError("pull failed")), patch.object(deploy, "deploy") as activate:
            with self.assertRaisesRegex(RuntimeError, "pull failed"):
                deploy.main()
            activate.assert_not_called()


    def test_restart_waits_for_launchd_to_finish_shutdown(self):
        results = [SimpleNamespace(returncode=0), SimpleNamespace(returncode=5), SimpleNamespace(returncode=0)]
        with patch.object(deploy.subprocess, "run", side_effect=results) as command, patch.object(deploy, "run") as checked, patch.object(deploy.time, "sleep") as sleep:
            deploy.restart()
        self.assertEqual(command.call_count, 3)
        sleep.assert_called_once_with(1)
        self.assertEqual(checked.call_args.args[0][1], "kickstart")

    def test_restart_registration_failure_is_bounded(self):
        with patch.object(deploy.subprocess, "run", return_value=SimpleNamespace(returncode=5)) as command, patch.object(deploy, "run") as checked, patch.object(deploy.time, "sleep"):
            with self.assertRaisesRegex(RuntimeError, "registration failed"):
                deploy.restart()
        self.assertEqual(command.call_count, 31)
        checked.assert_not_called()

    def test_success_uses_immutable_assets_and_retains_previous(self):
        self.assertEqual(self.execute([True]), 1)
        config = plistlib.loads(self.plist.read_bytes())
        self.assertEqual(config["ProgramArguments"][0], str(self.releases / "current/eunha-space"))
        self.assertEqual((self.assets / "index.html").read_text(), "old page")
        self.assertEqual((self.releases / "current/frontend/dist/index.html").read_text(), "new page")
        self.assertTrue((self.releases / "current/frontend/dist/assets/old.js").exists())
        self.assertEqual((self.releases / "previous/eunha-space").read_text(), "old binary")
        self.assertIn("deploy_until=NULL", self.sql[-1])

    def test_failed_readiness_restores_legacy_binary_and_assets(self):
        with self.assertRaises(RuntimeError):
            self.execute([False, True])
        config = plistlib.loads(self.plist.read_bytes())
        self.assertEqual(Path(config["ProgramArguments"][0]).read_text(), "old binary")
        self.assertEqual((self.assets / "index.html").read_text(), "old page")
        self.assertEqual(config["EnvironmentVariables"], self.original["EnvironmentVariables"])
        self.assertIn("deploy_until=NULL", self.sql[-1])

    def test_failed_build_never_switches_service_or_assets(self):
        with patch.object(deploy, "sql", side_effect=lambda config, statement: self.sql.append(statement) or ""), patch.object(deploy, "run", side_effect=RuntimeError("build failed")), patch.object(deploy.subprocess, "check_output", return_value="abc123\n"), patch.object(deploy, "restart") as restart:
            with self.assertRaises(RuntimeError):
                deploy.deploy(self.original)
            restart.assert_not_called()
        self.assertFalse((self.releases / "current").exists())
        self.assertEqual((self.assets / "index.html").read_text(), "old page")
        self.assertIn("deploy_until=NULL", self.sql[-1])


if __name__ == "__main__":
    unittest.main()

from pathlib import Path
import json
import os
import shlex
import subprocess
import tempfile
import unittest


class RuntimeSlotPlistTests(unittest.TestCase):
    def plutil_calls(self):
        """What write_slot_plist asks plutil, which CI's Linux runner lacks."""
        router = (Path(__file__).parents[1] / "eunha-runtime-router").read_text()
        function = router.split("write_slot_plist() {", 1)[1].split("\n}\n", 1)[0]
        with tempfile.TemporaryDirectory() as home:
            calls = Path(home) / "calls"
            stub = Path(home) / "plutil"
            stub.write_text(f'#!/bin/bash\nprintf "%s\\0" "$@" >>{shlex.quote(str(calls))}\n')
            stub.chmod(0o755)
            script = (
                'metrics_address() { echo 127.0.0.1:62000; }\nwrite_slot_plist() {'
                + function.replace("/usr/bin/plutil", shlex.quote(str(stub)))
                + '\n}\nwrite_slot_plist blue 127.0.0.1:61000'
            )
            subprocess.run(
                ["bash", "-c", script],
                check=True,
                env={
                    **os.environ,
                    "HOME": home,
                    "ROOT": "/runtime",
                    "TENANTS": "/runtime/tenants",
                    "BASE_LABEL": "space.eunha.test",
                },
            )
            return calls.read_text().split("\0")

    def test_slot_finds_homebrews_ffmpeg(self):
        calls = self.plutil_calls()
        environment = json.loads(calls[calls.index("EnvironmentVariables") + 2])
        self.assertEqual(environment["PATH"].split(":")[0], "/opt/homebrew/bin")
        self.assertIn("/usr/bin", environment["PATH"].split(":"))


if __name__ == "__main__":
    unittest.main()

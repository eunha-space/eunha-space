from pathlib import Path
import subprocess
import unittest


class RuntimeMetricsTests(unittest.TestCase):
    def address(self, address):
        router = (Path(__file__).parents[1] / "eunha-runtime-router").read_text()
        function = router.split("metrics_address() {", 1)[1].split("\n}\n", 1)[0]
        return subprocess.run(
            ["bash", "-c", "metrics_address() {" + function + '\n}\nmetrics_address "$1"', "test", address],
            capture_output=True,
            text=True,
        )

    def test_blue_and_green_have_distinct_private_ports(self):
        for port in (61000, 61001):
            result = self.address(f"127.0.0.1:{port}")
            self.assertEqual(result.returncode, 0)
            self.assertEqual(result.stdout.strip(), f"127.0.0.1:{port + 1000}")

    def test_port_overflow_is_rejected(self):
        result = self.address("127.0.0.1:65000")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")


if __name__ == "__main__":
    unittest.main()

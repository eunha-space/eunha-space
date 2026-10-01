import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch, Mock

spec = importlib.util.spec_from_file_location("scrape", Path(__file__).parents[1] / "scrape-service.py")
scrape = importlib.util.module_from_spec(spec)
spec.loader.exec_module(scrape)


class ServiceScrapeTests(unittest.TestCase):
    gauges = '''eunha_database_pool_connections{tenant="a.example"} 3
eunha_database_pool_idle_connections{tenant="a.example"} 2
eunha_http_in_flight{tenant="a.example"} 1
'''

    def test_only_known_tenants_and_numeric_fields_leave_scraper(self):
        text = self.gauges + '''eunha_http_requests_total{route="/private-path",tenant="a.example",status_class="2xx"} 10
eunha_http_requests_total{tenant="a.example",status_class="5xx",method="GET"} 2
eunha_http_requests_total{tenant="foreign.example",status_class="2xx"} 1000
eunha_http_response_duration_seconds_sum{tenant="a.example",route="/private-path"} 3
eunha_http_response_duration_seconds_count{tenant="a.example",route="/private-path"} 12
eunha_http_response_duration_seconds_bucket{tenant="a.example",le="+Inf"} 12
'''
        rows = scrape.parse_metrics(text, ["a.example", "missing.example"])
        self.assertEqual(list(rows), ["a.example"])
        self.assertEqual(rows["a.example"]["requests"], 12)
        self.assertEqual(rows["a.example"]["errors"], 2)
        self.assertEqual(rows["a.example"]["duration_sum"], 3)
        self.assertEqual(rows["a.example"]["rejected"], 0)
        self.assertNotIn("private-path", str(rows))
        self.assertNotIn("foreign.example", str(rows))

    def test_partial_gauges_and_invalid_values_are_not_successful_samples(self):
        self.assertEqual(scrape.parse_metrics(self.gauges.splitlines()[0], ["a.example"]), {})
        for value in ("NaN", "+Inf", "-1"):
            with self.assertRaises(ValueError):
                scrape.parse_metrics(self.gauges + f'eunha_http_requests_total{{tenant="a.example"}} {value}', ["a.example"])

    def test_slot_changes_and_oversized_responses_invalidate_the_scrape(self):
        config = {"directory": "/tmp/runtime/tenants", "label": "test.runtime", "blue": "127.0.0.1:61000", "domains": ["a.example"]}
        process = Mock(stdout="pid = 123\n")
        response = Mock()
        response.__enter__ = Mock(return_value=response)
        response.__exit__ = Mock(return_value=False)
        opener = Mock()
        opener.open.return_value = response
        for body, slots, reason in ((self.gauges.encode(), ["blue", "green"], "slot changed"), (b"x" * (8 * 1024 * 1024 + 1), ["blue"], "too large")):
            response.read.return_value = body
            with patch.object(scrape.Path, "read_text", side_effect=slots), patch.object(scrape.subprocess, "run", return_value=process), patch.object(scrape.urllib.request, "build_opener", return_value=opener):
                with self.assertRaisesRegex(ValueError, reason):
                    scrape.scrape(config)
            self.assertEqual(opener.open.call_args.args[0], "http://127.0.0.1:62000/metrics")


if __name__ == "__main__":
    unittest.main()

"""Executed privately on the runtime host. Emit only approved numeric metrics."""
import json
import math
import os
from pathlib import Path
import re
import subprocess
import sys
import urllib.request


def parse_metrics(text, domains):
    fields = {
        "eunha_http_requests_total": "requests",
        "eunha_http_response_duration_seconds_sum": "duration_sum",
        "eunha_http_response_duration_seconds_count": "duration_count",
        "eunha_http_aborted_requests_total": "aborted",
        "eunha_http_capacity_rejections_total": "rejected",
        "eunha_http_in_flight": "in_flight",
        "eunha_database_pool_connections": "pool",
        "eunha_database_pool_idle_connections": "pool_idle",
    }
    rows = {domain: dict.fromkeys([*fields.values(), "errors"], 0.0) for domain in domains}
    present = {domain: set() for domain in domains}
    for line in text.splitlines():
        match = re.fullmatch(r'(\w+)\{(.*)\}\s+([^\s]+)', line)
        if not match or match[1] not in fields:
            continue
        labels = match[2]
        tenant = re.search(r'(?:^|,)tenant=("(?:\\.|[^"\\])*")(?:,|$)', labels)
        if not tenant:
            continue
        domain = json.loads(tenant[1])
        if domain not in rows:
            continue
        value = float(match[3])
        if not math.isfinite(value) or value < 0:
            raise ValueError("invalid metric value")
        rows[domain][fields[match[1]]] += value
        if fields[match[1]] in ("pool", "pool_idle", "in_flight"):
            present[domain].add(fields[match[1]])
        if match[1] == "eunha_http_requests_total" and re.search(r'(?:^|,)status_class="5xx"(?:,|$)', labels):
            rows[domain]["errors"] += value
    # The periodically refreshed pool gauge proves a tenant is actually served.
    return {domain: row for domain, row in rows.items() if len(present[domain]) == 3}


def scrape(config):
    root = Path(config["directory"]).parent
    state = root / "router/active-slot"
    slot = state.read_text().strip()
    if slot not in ("blue", "green"):
        raise ValueError("invalid slot")
    address = config[slot]
    port = int(address.rsplit(":", 1)[1]) + 1000
    if not 1 <= port <= 65535:
        raise ValueError("invalid port")
    label = f'gui/{os.getuid()}/{config["label"]}.{slot}'
    process = subprocess.run(["/bin/launchctl", "print", label], capture_output=True, text=True, check=True, timeout=2)
    pid = re.search(r'\bpid = (\d+)', process.stdout)
    if not pid:
        raise ValueError("missing pid")
    started = subprocess.run(["/bin/ps", "-p", pid[1], "-o", "lstart="], capture_output=True, text=True, check=True, timeout=2).stdout.strip()
    class NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, request, file, code, message, headers, new_url):
            return None

    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    with opener.open(f"http://127.0.0.1:{port}/metrics", timeout=3) as response:
        body = response.read(8 * 1024 * 1024 + 1)
        if len(body) > 8 * 1024 * 1024:
            raise ValueError("metrics too large")
    if state.read_text().strip() != slot:
        raise ValueError("slot changed during scrape")
    rows = parse_metrics(body.decode(), config["domains"])
    epoch = f"{slot}:{pid[1]}:{started}"
    for row in rows.values():
        row["epoch"] = epoch
    return rows


if __name__ == "__main__":
    try:
        print(json.dumps(scrape(json.loads(sys.argv[1])), allow_nan=False))
    except Exception:
        # No raw errors: urllib/SSH/process details can contain private data.
        sys.exit(1)

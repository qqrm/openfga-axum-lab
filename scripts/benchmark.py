"""Diagnostic only: sequential end-to-end requests, not a regression gate."""
import json
import os
import platform
import statistics
import time
import urllib.error
import urllib.request

url = os.getenv("APP_URL", "http://127.0.0.1:3000") + "/dashboards/one"
request = urllib.request.Request(url, headers={"x-demo-user": "alice"})
samples = []
errors = []
warmup, count = 50, 500
for index in range(warmup + count):
    start = time.perf_counter_ns()
    try:
        with urllib.request.urlopen(request, timeout=5) as response:
            body = json.load(response)
            if response.status != 200 or body != {"id": "one"}:
                raise ValueError("unexpected response")
        elapsed = time.perf_counter_ns() - start
        if index >= warmup:
            samples.append(elapsed)
    except (OSError, ValueError, urllib.error.URLError) as exc:
        errors.append({"iteration": index, "error": str(exc)})

ordered = sorted(samples)
def percentile(p):
    return ordered[min(len(ordered) - 1, int((len(ordered) - 1) * p))] if ordered else None
report = {
    "purpose": "diagnostic only; no comparison, capacity or regression claim",
    "scenario": "sequential HTTP GET /dashboards/one with OpenFGA check",
    "commit": os.getenv("GITHUB_SHA", "unknown"),
    "runner": os.getenv("RUNNER_NAME", "local"),
    "platform": platform.platform(),
    "cpu_count": os.cpu_count(),
    "clock": vars(time.get_clock_info("perf_counter")),
    "warmup_requests": warmup,
    "requested_samples": count,
    "achieved_samples": len(samples),
    "concurrency": 1,
    "errors": errors,
    "latency_ns": {"median": statistics.median(samples) if samples else None, "p95": percentile(.95), "p99": percentile(.99)},
    "raw_latency_ns": samples,
}
os.makedirs("artifacts", exist_ok=True)
with open("artifacts/benchmark.json", "w", encoding="utf-8") as f:
    json.dump(report, f, indent=2)
print(json.dumps({k: v for k, v in report.items() if k != "raw_latency_ns"}, indent=2))
if errors or len(samples) != count:
    raise SystemExit("invalid diagnostic run: errors or missing samples")

"""Closed-loop HTTP diagnostic. No comparison or regression decision."""
import concurrent.futures
import datetime
import json
import os
import platform
import statistics
import time
import urllib.error
import urllib.request
from client import login

BASE = os.getenv("APP_URL", "http://127.0.0.1:3000")
WARMUP = 50
SAMPLES = 400
TIMEOUT = 5
TOKEN = login(BASE, "alice")

def request_once(index):
    request = urllib.request.Request(BASE + "/pets/fox", headers={"Authorization": "Bearer " + TOKEN})
    start = time.perf_counter_ns()
    error = None
    status = None
    try:
        with urllib.request.urlopen(request, timeout=TIMEOUT) as response:
            status = response.status
            body = json.load(response)
            if status != 200 or body.get("id") != "fox":
                error = "unexpected response"
    except (OSError, ValueError, urllib.error.URLError) as exc:
        error = type(exc).__name__ + ": " + str(exc)
    return {"index": index, "latency_ns": time.perf_counter_ns() - start, "status": status, "error": error}

def percentile(samples, p):
    ordered = sorted(samples)
    return ordered[int((len(ordered) - 1) * p)] if ordered else None

def lane(concurrency):
    warmup = [request_once(i) for i in range(WARMUP)]
    start = time.perf_counter_ns()
    cpu_start = time.process_time_ns()
    with concurrent.futures.ThreadPoolExecutor(max_workers=concurrency) as pool:
        raw = list(pool.map(request_once, range(SAMPLES)))
    elapsed_ns = time.perf_counter_ns() - start
    cpu_ns = time.process_time_ns() - cpu_start
    errors = [x for x in warmup + raw if x["error"] is not None]
    values = [x["latency_ns"] for x in raw if x["error"] is None]
    return {
        "workload": "GET /pets/fox, PostgreSQL identity and data, OpenFGA check",
        "load_model": "closed-loop fixed concurrency, one HTTP connection per request",
        "concurrency": concurrency,
        "warmup_requests": WARMUP,
        "requested": SAMPLES,
        "completed": len(raw),
        "successful": len(values),
        "errors_including_warmup": errors,
        "retries": 0,
        "timeout_seconds": TIMEOUT,
        "elapsed_ns": elapsed_ns,
        "client_process_cpu_ns": cpu_ns,
        "completed_per_second": len(raw) * 1e9 / elapsed_ns,
        "latency_ns": {"p50": statistics.median(values) if values else None, "p95": percentile(values, .95), "p99": percentile(values, .99)},
        "raw_requests": raw,
    }

report = {
    "purpose": "report-only, not a regression or capacity claim",
    "timestamp_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "commit": os.getenv("GITHUB_SHA", "unrecorded local working tree"),
    "runner": os.getenv("RUNNER_NAME", "local"),
    "platform": platform.platform(),
    "cpu_count": os.cpu_count(),
    "clock": vars(time.get_clock_info("perf_counter")),
    "lanes": [lane(1), lane(8)],
}
os.makedirs("artifacts", exist_ok=True)
with open("artifacts/benchmark.json", "w", encoding="utf-8") as f:
    json.dump(report, f, indent=2)
print(json.dumps({**report, "lanes": [{k:v for k,v in x.items() if k != "raw_requests"} for x in report["lanes"]]}, indent=2))
if any(lane["errors_including_warmup"] or lane["completed"] != SAMPLES for lane in report["lanes"]):
    raise SystemExit("invalid run: unexpected errors or missing observations")

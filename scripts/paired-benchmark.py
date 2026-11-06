"""Randomized balanced A/A diagnostic. Not a regression verdict."""
import concurrent.futures
import datetime
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import random
import statistics
import time
import urllib.error
import urllib.request
from client import login

base = os.getenv("APP_URL", "http://127.0.0.1:3000")
token = login(base, "alice")
rng = random.Random(2606)
order = ["AB"] * 3 + ["BA"] * 3
rng.shuffle(order)
samples_per_arm = 120
warmup = 20
concurrency = 4

def one(index):
    request = urllib.request.Request(base + "/pets/fox", headers={"Authorization": "Bearer " + token})
    start = time.perf_counter_ns()
    error = None
    try:
        with urllib.request.urlopen(request, timeout=5) as response:
            data = json.load(response)
            if response.status != 200 or data.get("id") != "fox":
                error = "unexpected response"
    except (OSError, ValueError, urllib.error.URLError) as exc:
        error = type(exc).__name__ + ": " + str(exc)
    return {"index": index, "latency_ns": time.perf_counter_ns() - start, "error": error}

def run_arm():
    with concurrent.futures.ThreadPoolExecutor(max_workers=concurrency) as pool:
        warm = list(pool.map(one, range(warmup)))
        start = time.perf_counter_ns()
        raw = list(pool.map(one, range(samples_per_arm)))
    elapsed = time.perf_counter_ns() - start
    errors = [item for item in warm + raw if item["error"] is not None]
    return {"elapsed_ns": elapsed, "errors": errors, "raw": raw,
            "p50_ns": statistics.median([item["latency_ns"] for item in raw])}

blocks = []
for index, sequence in enumerate(order):
    measurements = {}
    for label in sequence:
        measurements[label] = run_arm()
    blocks.append({"index": index, "order": sequence, "arms": measurements})
errors = sum(len(arm["errors"]) for block in blocks for arm in block["arms"].values())
ratios = [block["arms"]["B"]["p50_ns"] / block["arms"]["A"]["p50_ns"] for block in blocks]
def geometric_mean(values):
    return math.exp(sum(math.log(value) for value in values) / len(values))
bootstrap = sorted(geometric_mean([rng.choice(ratios) for _ in ratios]) for _ in range(5000))
report = {
    "purpose": "A/A calibration only; both labels call the same deployed binary",
    "decision": "no performance verdict; six blocks in one VM do not estimate between-VM variability",
    "commit": os.getenv("GITHUB_SHA", "unrecorded local working tree"),
    "utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "platform": platform.platform(), "cpu_count": os.cpu_count(),
    "clock": vars(time.get_clock_info("perf_counter")),
    "benchmark_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
    "load": {"model":"closed-loop", "concurrency":concurrency, "samples_per_arm":samples_per_arm,
             "warmup_per_arm":warmup, "timeout_seconds":5, "retries":0, "operation":"GET /pets/fox"},
    "seed":2606, "blocks":blocks, "error_count":errors,
    "ratio_b_over_a":geometric_mean(ratios),
    "same_run_bootstrap_95pct":[bootstrap[125],bootstrap[4874]],
}
os.makedirs("artifacts", exist_ok=True)
with open("artifacts/paired-aa.json", "w", encoding="utf-8") as file:
    json.dump(report, file, indent=2)
print(json.dumps({key:value for key,value in report.items() if key != "blocks"}, indent=2))
if errors:
    raise SystemExit("invalid measurement: request errors were observed")

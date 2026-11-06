"""Randomized A/B comparison of the same binary with authorization cache on/off.

A = disabled, B = enabled. Report-only: never infer significance from single-run
request bootstrap samples. Requires the experiment's two running containers.
"""
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

BASES = {
    "A": os.getenv("UNCACHED_URL", "http://127.0.0.1:3001"),
    "B": os.getenv("CACHED_URL", "http://127.0.0.1:3000"),
}
SEED = int(os.getenv("AB_SEED", "2606"))
BLOCKS = int(os.getenv("AB_BLOCKS", "8"))
SAMPLES = int(os.getenv("AB_SAMPLES", "240"))
WARMUP = int(os.getenv("AB_WARMUP", "40"))
WORKERS = int(os.getenv("AB_WORKERS", "4"))
TIMEOUT = 5
assert BLOCKS > 0 and BLOCKS % 2 == 0
assert SAMPLES > 0 and WARMUP > 0 and WORKERS > 0
rng = random.Random(SEED)
order = ["AB"] * (BLOCKS // 2) + ["BA"] * (BLOCKS // 2)
rng.shuffle(order)
tokens = {label: login(url, "alice") for label, url in BASES.items()}


def http(url, method="GET", token=None, body=None):
    headers = {"Accept": "application/json"}
    if token:
        headers["Authorization"] = "Bearer " + token
    if body is not None:
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(
        url, data=None if body is None else json.dumps(body).encode(),
        headers=headers, method=method,
    )
    try:
        with urllib.request.urlopen(request, timeout=TIMEOUT) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as exc:
        return exc.code, exc.read()


def metrics(label):
    status, data = http(BASES[label] + "/metrics")
    if status != 200:
        raise RuntimeError("metrics unavailable")
    counters = {}
    for line in data.decode().splitlines():
        if not line or line.startswith("#"):
            continue
        key, sep, value = line.rpartition(" ")
        if sep:
            try:
                counters[key] = float(value)
            except ValueError:
                pass
    return counters


def counter_delta(before, after, prefix):
    return {
        k: after.get(k, 0) - before.get(k, 0)
        for k in sorted(set(before) | set(after))
        if k.startswith(prefix)
    }


def request_once(label, index, endpoint="/pets/fox", expected=200):
    start = time.perf_counter_ns()
    status, error = None, None
    try:
        status, body = http(BASES[label] + endpoint, token=tokens[label])
        if status != expected:
            error = f"expected {expected}, received {status}: {body[:120]!r}"
        elif expected == 200:
            parsed = json.loads(body)
            if parsed.get("id") != "fox":
                error = "incorrect response body"
    except (OSError, ValueError) as exc:
        error = f"{type(exc).__name__}: {exc}"
    return {
        "index": index,
        "latency_ns": time.perf_counter_ns() - start,
        "status": status,
        "error": error,
    }


def percentile(values, percent):
    ordered = sorted(values)
    return ordered[math.ceil(len(ordered) * percent) - 1] if ordered else None


def arm(label):
    before = metrics(label)
    with concurrent.futures.ThreadPoolExecutor(max_workers=WORKERS) as pool:
        warm = list(pool.map(lambda i: request_once(label, i), range(WARMUP)))
        start = time.perf_counter_ns()
        raw = list(pool.map(lambda i: request_once(label, i), range(SAMPLES)))
    elapsed = time.perf_counter_ns() - start
    after = metrics(label)
    valid = [x["latency_ns"] for x in raw if x["error"] is None]
    events = counter_delta(before, after, "authz_cache_events_total")
    return {
        "warmup_errors": [x for x in warm if x["error"]],
        "errors": [x for x in raw if x["error"]],
        "raw": raw,
        "elapsed_ns": elapsed,
        "throughput_rps": len(raw) * 1e9 / elapsed,
        "p50_ns": statistics.median(valid) if valid else None,
        "p95_ns": percentile(valid, .95),
        "p99_ns": percentile(valid, .99),
        "cache_events": events,
        "revision_reads": counter_delta(before, after, "authz_revision_reads_total"),
        "openfga_requests": counter_delta(before, after, "authz_openfga_requests_total"),
    }


def revoke_test(label):
    """Control each arm separately: carol owns fox, dave is caregiver."""
    base = BASES[label]
    admin = login(base, "carol")
    target = login(base, "dave")
    relation = "/pets/fox/relations/caregiver/dave"

    def check(expected):
        status, body = http(base + "/pets/fox", token=target)
        if status != expected:
            raise RuntimeError(f"{label} revoke expected {expected}, got {status}: {body[:120]!r}")

    # Always restore, even if a previous run was interrupted.
    status, body = http(base + relation, method="PUT", token=admin)
    if status != 204:
        raise RuntimeError(f"{label} grant failed: {status}: {body[:120]!r}")
    try:
        check(200)
        for _ in range(3):
            check(200)  # warm cache
        start = time.perf_counter_ns()
        status, body = http(base + relation, method="DELETE", token=admin)
        if status != 204:
            raise RuntimeError(f"{label} revoke failed: {status}: {body[:120]!r}")
        committed_ns = time.perf_counter_ns()
        check(403)
        first_denial_ns = time.perf_counter_ns()
        return {
            "revoke_http_latency_ns": committed_ns - start,
            "first_denial_after_revoke_response_ns": first_denial_ns - committed_ns,
            "verified": True,
        }
    finally:
        status, body = http(base + relation, method="PUT", token=admin)
        if status != 204:
            raise RuntimeError(f"{label} restore failed: {status}: {body[:120]!r}")


def geometric_mean(values):
    return math.exp(sum(math.log(value) for value in values) / len(values))


def main():
    # Fail before measurement if the arms do not correspond to the intended modes.
    for label, expected in [("A", "uncached"), ("B", "miss")]:
        before = metrics(label)
        request_once(label, 0)
        after = metrics(label)
        changes = counter_delta(before, after, "authz_cache_events_total")
        if not any(expected in k and v > 0 for k, v in changes.items()):
            raise RuntimeError(f"{label} mode not observed in metrics: {changes}")
    blocks = []
    for index, sequence in enumerate(order):
        runs = {label: arm(label) for label in sequence}
        blocks.append({"index": index, "order": sequence, "arms": runs})
    errors = sum(len(a["errors"]) + len(a["warmup_errors"])
                 for block in blocks for a in block["arms"].values())
    revocations, revoke_error = {}, None
    try:
        for label in ["A", "B"]:
            revocations[label] = revoke_test(label)
    except Exception as exc:
        revoke_error = str(exc)
    valid = errors == 0 and revoke_error is None
    ratios = [
        block["arms"]["B"]["p50_ns"] / block["arms"]["A"]["p50_ns"]
        for block in blocks
    ] if valid else []
    report = {
        "purpose": "cache A/B exploratory comparison, no performance claim",
        "valid": valid,
        "timestamp_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "commit": os.getenv("GITHUB_SHA", "unrecorded working tree"),
        "seed": SEED,
        "order": order,
        "platform": platform.platform(),
        "cpu_count": os.cpu_count(),
        "script_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "load": {"workers": WORKERS, "samples_per_arm": SAMPLES, "warmup": WARMUP,
                 "timeout_seconds": TIMEOUT, "retries": 0,
                 "operation": "GET /pets/fox", "model": "closed-loop, new connection/request"},
        "arms": {"A": "cache disabled", "B": "cache enabled"},
        "blocks": blocks,
        "error_count": errors,
        "revocation": revocations,
        "revocation_error": revoke_error,
        "median_B_over_A_geomean": geometric_mean(ratios) if ratios else None,
        "limitations": [
            "No between-runner variation estimated",
            "Arms run in the same VM with separate processes sharing the database",
            "PostgreSQL counters cover revision reads only, not all application SQL",
            "Revocation correctness is a sequential post-response check, not a concurrency proof",
            "No statistical significance verdict without independent runs",
        ],
    }
    os.makedirs("artifacts", exist_ok=True)
    with open("artifacts/cache-ab.json", "w", encoding="utf-8") as file:
        json.dump(report, file, indent=2)
    print(json.dumps({k: v for k, v in report.items() if k != "blocks"}, indent=2))
    if not valid:
        raise SystemExit("invalid comparison: authorization or HTTP failures")


if __name__ == "__main__":
    main()

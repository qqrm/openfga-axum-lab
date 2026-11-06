"""Exploratory paired HTTP benchmark: A/A-off, A/A-on and cache A/B.

All labels use one image and one PostgreSQL/OpenFGA dataset. The independent
replication unit is a runner, not an individual request. No capacity claim.
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
import subprocess
import time
import urllib.error
import urllib.request

BASES = {"off": "http://127.0.0.1:3001", "on": "http://127.0.0.1:3000"}
COMPOSE = ["docker", "compose", "-f", "compose.yaml", "-f", "compose.cache-ab.yaml"]
SEED = int(os.getenv("AB_SEED", "2606"))
BLOCKS = int(os.getenv("AB_BLOCKS", "8"))
SAMPLES = int(os.getenv("AB_SAMPLES", "2000"))
AA_SAMPLES = int(os.getenv("AA_SAMPLES", "1000"))
WARMUP = int(os.getenv("AB_WARMUP", "200"))
WORKERS = int(os.getenv("AB_WORKERS", "4"))
TIMEOUT = 5
OUTPUT = Path("artifacts/cache-ab.json")


def http(mode, path, token=None, method="GET", body=None):
    headers = {} if token is None else {"Authorization": "Bearer " + token}
    data = None if body is None else json.dumps(body).encode()
    if data is not None:
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(BASES[mode] + path, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(request, timeout=TIMEOUT) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as exc:
        return exc.code, exc.read()


def require(mode, path, token, expected, method="GET", body=None):
    status, raw = http(mode, path, token, method, body)
    if status != expected:
        raise RuntimeError(f"{mode} {method} {path}: expected {expected}, got {status}")
    return raw


def login(mode, user):
    raw = require(mode, "/session", None, 200, "POST", {
        "username": user, "password": os.getenv("DEMO_PASSWORD", "local-demo-password")})
    return json.loads(raw)["token"]


def wait_ready(mode):
    for _ in range(90):
        try:
            if http(mode, "/healthz")[0] == 200:
                return
        except OSError:
            pass
        time.sleep(1)
    raise RuntimeError(f"{mode}: app not ready")


def metrics(mode):
    raw = require(mode, "/metrics", None, 200)
    result = {}
    for line in raw.decode().splitlines():
        if line.startswith("authz_"):
            key, value = line.rsplit(" ", 1)
            result[key] = float(value)
    return result


def sql(query):
    result = subprocess.run(COMPOSE + ["exec", "-T", "postgres", "psql", "-U", "petlab",
        "-d", "postgres", "-XAt", "-v", "ON_ERROR_STOP=1", "-c", query],
        text=True, capture_output=True, check=True, timeout=20)
    return result.stdout.strip()


def pg_snapshot():
    # Observe from the maintenance DB: the observation query is not counted
    # in either measured database. These are executed statements, not packets.
    return json.loads(sql("""SELECT coalesce(jsonb_object_agg(datname, calls), '{}'::jsonb)
        FROM (SELECT d.datname, sum(s.calls) AS calls FROM pg_stat_statements s
        JOIN pg_database d ON d.oid=s.dbid WHERE d.datname IN ('petlab','openfga')
        GROUP BY d.datname) totals"""))


def delta(before, after):
    result = {key: after.get(key, 0) - before.get(key, 0) for key in sorted(set(before) | set(after))}
    if any(value < 0 for value in result.values()):
        raise RuntimeError("counter reset during observation")
    return result


def host_snapshot():
    return {"cpu_ticks": [int(x) for x in Path("/proc/stat").read_text().splitlines()[0].split()[1:]],
            "loadavg": os.getloadavg(), "cpu_pressure": Path("/proc/pressure/cpu").read_text()}


def request_once(mode, token, index, expected_body):
    start = time.perf_counter_ns()
    status, error = None, None
    try:
        status, body = http(mode, "/pets/fox", token)
        if status != 200 or json.loads(body) != expected_body:
            error = "unexpected status or body"
    except (OSError, ValueError) as exc:
        error = f"{type(exc).__name__}: {exc}"
    return {"index": index, "latency_ns": time.perf_counter_ns() - start,
            "status": status, "error": error}


def percentile(values, p):
    ordered = sorted(values)
    return ordered[max(0, math.ceil(len(ordered) * p) - 1)] if ordered else None


def arm(mode, token, count, expected_body):
    with concurrent.futures.ThreadPoolExecutor(max_workers=WORKERS) as pool:
        run = lambda i: request_once(mode, token, i, expected_body)
        warm = list(pool.map(run, range(WARMUP)))
        pg_before = pg_snapshot()
        before = metrics(mode)
        host_before = host_snapshot()
        cpu = time.process_time_ns()
        start = time.perf_counter_ns()
        raw = list(pool.map(run, range(count)))
        elapsed = time.perf_counter_ns() - start
        client_cpu = time.process_time_ns() - cpu
        host_after = host_snapshot()
        after = metrics(mode)
        pg_after = pg_snapshot()
    counters = delta(before, after)
    event = lambda name: counters.get('authz_cache_events_total{event="' + name + '"}', 0)
    decisions = sum(event(x) for x in ("hit_allow", "hit_deny", "miss", "uncached", "bypass_pending"))
    errors = sum(item["error"] is not None for item in warm + raw)
    counter_errors = []
    if decisions != count:
        counter_errors.append(f"expected {count} decisions, observed {decisions}")
    if mode == "off" and event("uncached") != count:
        counter_errors.append("uncached mode not observed")
    if mode == "on" and event("hit_allow") <= 0:
        counter_errors.append("no allow cache hits observed")
    valid = [item["latency_ns"] for item in raw if item["error"] is None]
    return {"mode": mode, "warmup_raw": warm, "raw": raw, "error_count": errors,
        "counter_errors": counter_errors, "elapsed_ns": elapsed,
        "client_process_cpu_ns": client_cpu, "throughput_rps": len(valid) * 1e9 / elapsed,
        "p50_ns": statistics.median(valid) if valid else None,
        "p95_ns": percentile(valid, .95), "p99_ns": percentile(valid, .99),
        "cache_hit_rate": (event("hit_allow") + event("hit_deny")) / decisions if decisions else None,
        "counters": counters, "pg_statements": delta(pg_before, pg_after),
        "snapshots": {"metrics_before": before, "metrics_after": after,
            "pg_before": pg_before, "pg_after": pg_after,
            "host_before": host_before, "host_after": host_after}}


def geomean(values):
    return math.exp(statistics.mean(math.log(x) for x in values))


def summarize(blocks, seed):
    result = {}
    for metric in ("p50_ns", "p95_ns", "p99_ns", "throughput_rps"):
        ratios = [b["arms"]["B"][metric] / b["arms"]["A"][metric] for b in blocks]
        rng = random.Random(seed)
        boot = [geomean(rng.choices(ratios, k=len(ratios))) for _ in range(5000)]
        result[metric] = {"ratio_B_over_A": geomean(ratios), "block_ratios": ratios,
            "within_runner_paired_block_bootstrap_95": [percentile(boot, .025), percentile(boot, .975)],
            "A_median_of_blocks": statistics.median(b["arms"]["A"][metric] for b in blocks),
            "B_median_of_blocks": statistics.median(b["arms"]["B"][metric] for b in blocks)}
    return result


def revocations(tokens):
    records = []
    path = "/pets/fox/relations/caregiver/dave"
    for writer in BASES:
        require(writer, path, tokens["carol"], 204, "PUT")
        try:
            hit_before = metrics("on").get('authz_cache_events_total{event="hit_allow"}', 0)
            for mode in BASES:
                for _ in range(3):
                    require(mode, "/pets/fox", tokens["dave"], 200)
            hit_after = metrics("on").get('authz_cache_events_total{event="hit_allow"}', 0)
            if hit_after <= hit_before:
                raise RuntimeError("revocation fixture did not warm the allow cache")
            start = time.perf_counter_ns()
            require(writer, path, tokens["carol"], 204, "DELETE")
            acknowledged = time.perf_counter_ns()
            observations = []
            for mode in BASES:
                for _ in range(10):
                    begin = time.perf_counter_ns()
                    require(mode, "/pets/fox", tokens["dave"], 403)
                    observations.append({"mode": mode, "status": 403,
                        "started_after_ack_ns": begin - acknowledged,
                        "latency_ns": time.perf_counter_ns() - begin})
            records.append({"writer": writer, "revoke_latency_ns": acknowledged - start,
                "post_ack_denials": observations, "warm_hits_observed": hit_after - hit_before})
        finally:
            # Restore the original fixture (dave has no access to fox).
            require(writer, path, tokens["carol"], 204, "DELETE")
    return records


def fail_closed(tokens):
    for mode in BASES:
        for _ in range(3):
            require(mode, "/pets/fox", tokens["alice"], 200)
    subprocess.run(COMPOSE + ["stop", "-t", "1", "openfga"], check=True, timeout=30)
    result = {}
    try:
        for mode in BASES:
            require(mode, "/pets/fox", tokens["alice"], 503)
            result[mode] = 503
    finally:
        subprocess.run(COMPOSE + ["start", "openfga"], check=True, timeout=30)
    return result


def save(report):
    OUTPUT.parent.mkdir(exist_ok=True)
    temporary = OUTPUT.with_suffix(".tmp")
    temporary.write_text(json.dumps(report, indent=2))
    temporary.replace(OUTPUT)


def main():
    if BLOCKS < 4 or BLOCKS % 2 or min(SAMPLES, AA_SAMPLES, WARMUP, WORKERS) < 1:
        raise ValueError("positive sample counts and an even number of at least four blocks required")
    report = {"valid": False, "purpose": "exploratory, not a capacity or production claim",
        "commit": os.getenv("GITHUB_SHA", "unrecorded"), "seed": SEED,
        "replica": os.getenv("BENCH_REPLICA", "local"),
        "timestamp_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "platform": platform.platform(), "python": platform.python_version(), "cpu_count": os.cpu_count(),
        "script_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "load": {"workers": WORKERS, "samples_per_arm": SAMPLES, "aa_samples_per_arm": AA_SAMPLES,
            "warmup_per_arm": WARMUP, "blocks_per_stage": BLOCKS, "timeout_seconds": TIMEOUT,
            "model": "closed-loop, new HTTP connection per request", "operation": "GET /pets/fox as alice",
            "retries": 0, "primary_metric": "p50", "practical_effect_threshold": .05},
        "stages": [], "limitations": ["Small hot-key dataset; no mixed-write or capacity workload",
            "Client, app, PostgreSQL and OpenFGA share a hosted VM; generator can bottleneck",
            "Bootstrap intervals resample paired blocks within a VM, not independent hosts",
            "PG counts are successful statement executions in the observation windows, not network packets",
            "No automatic performance verdict; compare independent replicas and A/A noise"]}
    save(report)
    try:
        for mode in BASES:
            wait_ready(mode)
        tokens = {user: login("on", user) for user in ("alice", "carol", "dave")}
        for mode in BASES:
            require(mode, "/pets/fox", None, 401)
            require(mode, "/pets/fox", tokens["dave"], 403)
        report["revocation_before"] = revocations(tokens)
        expected = json.loads(require("off", "/pets/fox", tokens["alice"], 200))
        if json.loads(require("on", "/pets/fox", tokens["alice"], 200)) != expected:
            raise RuntimeError("different datasets between modes")
        report["response_fixture"] = expected
        definitions = [("AA-off", "off", "off", AA_SAMPLES),
            ("AA-on", "on", "on", AA_SAMPLES), ("AB", "off", "on", SAMPLES)]
        rng = random.Random(SEED)
        rng.shuffle(definitions)
        report["stage_order"] = [stage[0] for stage in definitions]
        for name, a, b, count in definitions:
            orders = ["AB"] * (BLOCKS // 2) + ["BA"] * (BLOCKS // 2)
            rng.shuffle(orders)
            stage = {"name": name, "mapping": {"A": a, "B": b}, "blocks": []}
            report["stages"].append(stage)
            for index, order in enumerate(orders):
                block = {"index": index, "order": order, "arms": {}}
                stage["blocks"].append(block)
                for label in order:
                    result = arm(stage["mapping"][label], tokens["alice"], count, expected)
                    block["arms"][label] = result
                    save(report)
                    if result["error_count"] or result["counter_errors"]:
                        raise RuntimeError(f"invalid measurement: {name} block {index} arm {label}")
            stage["summary"] = summarize(stage["blocks"], SEED + len(report["stages"]))
            print(json.dumps({"stage": name, "summary": stage["summary"]}), flush=True)
            save(report)
        report["revocation_after"] = revocations(tokens)
        report["fail_closed"] = fail_closed(tokens)
        report["valid"] = True
    except Exception as exc:
        report["failure"] = f"{type(exc).__name__}: {exc}"
        raise
    finally:
        save(report)
    print(json.dumps({"valid": report["valid"], "replica": report["replica"],
        "stages": [{"name": s["name"], "summary": s["summary"]} for s in report["stages"]]}), flush=True)


if __name__ == "__main__":
    main()

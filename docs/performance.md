# Performance measurement

The checked-in short diagnostic measures a complete HTTP read: token lookup in
PostgreSQL, object lookup, OpenFGA check and data read. It uses closed-loop
clients, one connection per request, reports actual completions, errors, raw
latencies, clock and environment. It does not establish a capacity or SLO.

`paired-benchmark.py` is a same-binary A/A calibration. Six randomized,
balanced AB/BA blocks each contain warm-up and measured requests with four
closed-loop workers. It retains all samples and computes a paired
within-run bootstrap interval of median-latency ratios. This is an exploratory
noise check, not a regression gate: one runner cannot estimate between-VM
variation, samples within a block are not independent hosts, and the generator
itself may bottleneck. The manual workflow is never scheduled.

For a comparison, pin both binaries, image digests, schema, model, dataset,
toolchain, workload and host; validate correctness and zero unexpected errors
for both; randomize balanced order in nearby blocks; measure enough independent
blocks and fresh runs; report effect and uncertainty relative to a
predeclared practical threshold. Do not compare the old demo with this service:
their authentication, persistence and operation semantics differ.

GitHub-hosted runners are VMs. First use A/A repetitions to learn whether their
noise can resolve a proposed effect. A smartphone can provide physical ARM
hardware, but Android scheduling, thermal throttling, battery/charging, CPU clusters,
Termux/proot or virtualization and process interference affect results.
Hardware and workload are not equivalent to x86 cloud VMs. No device settings,
processes or benchmark runner have been changed or installed on the phone.

## Example calibration

The [2026-11-06 GitHub Actions run](https://github.com/qqrm/openfga-axum-lab/actions/runs/42250134346)
recorded six A/A blocks with four closed-loop clients and 120 measured requests
per arm per block. It reported zero errors, a geometric mean of block-level
B/A median-latency ratios of 1.003, and a within-run bootstrap interval of
0.916–1.075. The raw artifact has limited
retention. These numbers describe only this VM and run; they do not establish a
regression threshold, cross-run variability, or production capacity.

## Cache A/B experiment (manual, report-only)

The [manual cache A/B workflow](../.github/workflows/cache-ab.yml) compares
the same source and Docker build with two instances: A uses
\`AUTHZ_CACHE_ENABLED=false\`, B defaults to \`true\`.
The environment variable only disables the local decision cache; mutations
still use PostgreSQL's pending-generation fence in both modes. Production
defaults remain unchanged. Both instances share the same PostgreSQL primary,
OpenFGA store and model; measurement arms execute sequentially, not concurrently.
The order is seeded, randomized, and balanced across eight AB/BA blocks.

Run via **Actions → Manual authz cache A/B → Run workflow** or locally:

\`\`\`sh
docker compose -f compose.yaml -f compose.cache-ab.yaml up -d --build app app-uncached
python3 scripts/cache-ab.py
docker compose -f compose.yaml -f compose.cache-ab.yaml down -v
\`\`\`

Output: \`artifacts/cache-ab.json\`, containing each request's latency/status,
warmups, order, environment, per-block throughput and p50/p95/p99, cache
events, PostgreSQL authorization-revision reads and OpenFGA HTTP calls by type.
Revision-read counts **do not** include all application PostgreSQL queries.
Each A/B run must have no unexpected HTTP failures and must pass the sequential
grant/warm/revoke/deny/restore check for both arms. Warmup errors invalidate
the comparison. Failed runs still upload their available diagnostics.

A single hosted runner cannot establish a generally significant performance
effect. Compare independent workflow runs, report the paired block-level
distributions and between-run variation, and confirm practical relevance
before asserting speedup. Existing A/A calibration (ratio 1.003,
within-run interval 0.916–1.075) is a noise reference, **not** a significance
threshold. Client-side closed-loop load is not a saturation/capacity test.

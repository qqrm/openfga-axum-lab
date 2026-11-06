# Performance measurement

Measurements cover a complete HTTP read: PostgreSQL token lookup, object lookup,
authorization, and data retrieval. They do not establish production capacity or
an SLO. Correct responses and zero unexpected errors are prerequisites for any
performance comparison.

## Historical A/A calibration

`scripts/paired-benchmark.py` is the original same-binary A/A diagnostic. The
[2026-11-06 calibration](https://github.com/qqrm/openfga-axum-lab/actions/runs/42250134346)
used six balanced randomized blocks, four closed-loop workers and 120 measured
requests per arm. Its p50 B/A ratio was 1.003 with a within-run bootstrap interval
of 0.916–1.075. This is a historical noise observation, not a regression threshold.

## Executed cache experiment: 2026-11-06

Measured commit: `96de1f2bb0622f51bcc03a87698de0b1b621bdec`.

The [three-runner experiment](https://github.com/qqrm/openfga-axum-lab/actions/runs/42280462589)
and [standard CI for that commit](https://github.com/qqrm/openfga-axum-lab/actions/runs/42280462489)
completed successfully. Standard CI includes Rust formatting, checking, Clippy,
nextest, the existing cross-replica race/fault tests, full integration scenarios,
formal domain checks and dependency checks. Results remain in draft PR #8; main
was not changed. Subsequent documentation-only changes are not measured code.

For the hot allowed-read workload below, paired-block p50 decreased by
32.7–44.4% and achieved throughput increased 1.83–2.39 times across three VMs.
These are observed effects for this experiment, not arbitrary-workload claims.

Every table cell shows **cache off → cache on**. Values are medians of eight
block-level statistics, not pooled percentiles. RPS is achieved throughput at
four closed-loop clients, not maximum sustainable throughput.

| VM | p50, ms | p95, ms | p99, ms | requests/s |
|---|---:|---:|---:|---:|
| 1 | 3.795 → 2.079 | 14.998 → 2.866 | 24.598 → 3.556 | 755 → 1827 |
| 2 | 5.770 → 3.887 | 18.252 → 5.201 | 32.015 → 6.216 | 541 → 987 |
| 3 | 5.688 → 3.871 | 18.269 → 5.190 | 33.364 → 6.298 | 536 → 991 |

Runner 1 reported AMD EPYC 9V45; runners 2 and 3 reported AMD EPYC 7763.
All exposed four vCPUs. Absolute results must not be compared as though the
VMs were identical hardware. Each cache comparison is paired within one VM.

### Noise and uncertainty

Each VM ran A/A-off, A/A-on, and A/B, with seeded randomized stage order and
balanced AB/BA block order. Primary metric: p50. Predeclared practical-effect
threshold: 5%. Intervals below use 5,000 paired-block bootstrap resamples within
one runner; temporal dependence can reduce their coverage. They do not estimate
arbitrary-host uncertainty. Individual requests are not independent experiments.

| VM | A/A-off p50 B/A [95% interval] | A/A-on p50 B/A [95% interval] | A/B p50 on/off [95% interval] |
|---|---|---|---|
| 1 | 0.988 [0.959, 1.019] | 0.986 [0.964, 1.004] | 0.556 [0.543, 0.569] |
| 2 | 0.963 [0.910, 1.003] | 1.000 [0.992, 1.007] | 0.673 [0.662, 0.682] |
| 3 | 1.018 [0.989, 1.043] | 0.998 [0.992, 1.005] | 0.667 [0.635, 0.690] |

All six A/A p50 intervals include 1; all three A/B intervals are well below 1.
The direction repeats across VMs and the observed effect exceeds the A/A point
offsets. Three VMs remain a small sample. No pooled request-level confidence
interval or universal production-performance verdict is reported. Ratios use
geometric means of paired block ratios, not ratios of the displayed medians.

### Backend work and correctness

The experiment recorded 192,000 measured requests (96,000 A/B and 96,000 A/A),
plus 28,800 warmup requests. All returned the expected status and full response
object. There were zero unexpected errors. Warmups are retained separately and
excluded from latency, throughput and backend-counter measurement windows.

A/B cache hit rates were 99.956%, 99.856%, and 99.813% on VMs 1–3. Executed SQL
statements per read were:

| Database | Cache off | Cache on |
|---|---:|---:|
| Application PostgreSQL | 3 | 5 |
| OpenFGA PostgreSQL | approximately 6 | approximately 0.003–0.011 |
| Combined | approximately 9 | approximately 5 |

These are `pg_stat_statements.calls`, not disk I/O or network packets. Observer
queries run in the separate maintenance database. Observation-window boundaries
and cancelled internal work can affect fractional counts. The cache adds two
application revision reads, but almost all graph SQL disappears on this hot key.
Application-to-OpenFGA HTTP calls remain one per read: Check when uncached,
mostly readiness/health when cached. HTTP calls were not eliminated.

Before and after timing, both writer directions were tested: grant access,
warm the allow cache, revoke, then check both replicas after the revoke response.
All 240 post-acknowledgement checks returned 403. Stopping OpenFGA produced 503
for both modes on all three VMs. These sequential probes do not prove absence of
all concurrent authorization races; the existing Rust cross-replica race/fault
tests also passed in the standard CI run for the measured commit.

### Configuration and reproduction

`scripts/cache-ab.py` executes all three stages. Defaults: eight blocks per
stage, 2,000 requests per A/B arm, 1,000 per A/A arm, 200 warmup requests per arm,
four workers, five-second timeout, no explicit request retries. Seeds were 2607,
2608 and 2609. Workload: `GET /pets/fox` as alice, new HTTP connection per request.

Both modes use one release image, one PostgreSQL primary, the same OpenFGA
store/model, session and seeded response fixture. `AUTHZ_CACHE_ENABLED=false`
skips decision caching and revision reads on checks; mutation fencing remains
active in both modes. Default behavior stays cache enabled. This compares the
whole cache mechanism, not just Moka. Normal API scenarios run first; disposable
CI volumes are then recreated before measurement.

All three VMs produced application binary SHA-256
`5a1206aa86213348e6830c4432c851de10bf635edfe639a53fc319d1f8fe1747`.
Dependency digests matched across all three VMs:

- PostgreSQL: `postgres@sha256:665dc7b37ac52a17e298511414c21d23ffa4e6a6ef6225b3e5d59c7afb35e994`
- OpenFGA: `openfga/openfga@sha256:79df38493e1c7bfa3b23a63df96fc6e630922e5c9dc63ad3052eddcaf4da1761`

For a fresh, isolated local lab at the measured commit:

```sh
export COMPOSE_FILE=compose.yaml:compose.cache-ab.yaml
docker compose build app
docker compose up -d --no-build app app-uncached
docker compose exec -T postgres psql -U petlab -d postgres -v ON_ERROR_STOP=1 \
  -c 'CREATE EXTENSION IF NOT EXISTS pg_stat_statements'
AB_SEED=2607 python3 scripts/cache-ab.py
# Disposable lab only: removes its database volumes.
docker compose down -v
```

Use a separate Compose project and fresh volumes, not an existing installation.
For version-exact repetition, pin external images to recorded digests. Source
tags remain mutable; recording their hashes identifies this execution but does
not promise a bit-identical future build or identical VM hardware. The workflow
supports manual dispatch and branch-scoped pushes, not a recurring schedule.

### Raw evidence

The experiment run retains three artifacts until 2026-12-06:

| VM | Artifact ID | ZIP SHA-256 |
|---|---|---|
| 1 | 13535651131 | `796d498cd3343a50593ba6d0da22531108c2533bcb3771d4b872f487d30d3469` |
| 2 | 13534553979 | `6d4f53d9d460b3560606e957970f2b2cb287d3ecdf000090fedbb9c7580f930b` |
| 3 | 13535915748 | `197834ebdafd8c3bbe1133ef21063e285a40ce2b7a2ed2de9671c7784447ab89` |

Each includes raw measured and warmup samples, per-arm counters, host CPU and
pressure snapshots, resolved Compose configuration, source/input and binary
hashes, image metadata, CPU/OS information, logs and PostgreSQL statistics.
The companion downloadable bundle preserves all three original ZIPs plus the
exact harness and an independent analyzer. Raw percentiles, throughput, errors,
counters, fixtures and binary hashes were independently checked after download.

### Limits

This is a tiny, hot, nearly all-hit, read-only workload. It does not characterize
cold caches, large key sets, denial-heavy traffic, mutation-driven invalidation,
fixed-arrival-rate latency, saturation or production SLOs. The Python generator
and all services share each VM. No Kubernetes isolation or dedicated physical
cores were used. p99 estimates and within-run intervals are exploratory. A lower
hit rate or a different deployment topology can change or reverse the result.

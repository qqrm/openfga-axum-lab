# Lakekeeper denial-cache Criterion experiment (manual)

This is a **new experiment**, grounded in Lakekeeper on `main` at
`1c1e48a44be0ae81c6833983a6c0182b6c688e96`. The closed, unmerged
PR #8 is provenance for the methodology, not a performance baseline. Its
previous positive-cache code and `/pets/fox` HTTP fixture are not reused.
The existing `benches/authz_cache.rs` remains a **raw adapter** benchmark.
The new `benches/catalog_cache_ab.rs` measures the full Lakekeeper
`GET /projects/acme/warehouses/analytics/tables/requests` path, including
session lookup, catalog/SQL gates, the denial-only cache, and OpenFGA.

## Arms and guarantees

Both services run the **same application image and binary**, share the same
isolated PostgreSQL database, OpenFGA instance, pinned Lakekeeper model,
fixtures and code revision. Before any measurement, the Rust driver compares
container image IDs and executable SHA-256 hashes. The benchmark client is
built once and executed directly for every arm; its SHA-256 is recorded. Direct execution passes
Criterion's `--bench` explicitly; omitting it only performs a smoke test and
creates no estimates. The driver fails closed when measured files are absent. A
read-only SQL witness fingerprints the catalog rows and checks that the
shared authorization generation is unchanged and `pending=0` **before,
between and after** each comparison pair, and checks the witness against the
shared baseline **across all pairs**. Executable fingerprints are checked
again at the end. The experiment driver creates session tokens before measuring and passes them
unchanged to each Criterion subprocess. Thus session rows are not inserted
between measured arms; session state is shared by both arms. Session inserts
are outside the catalog fingerprint; no application mutations are allowed
between paired measurements.

- `on`: `AUTHZ_DENIAL_CACHE_ENABLED=true` (the default). Only denials may
  be cached for two seconds, keyed by SQL authorization generation.
- `off`: `AUTHZ_DENIAL_CACHE_ENABLED=false`. Every Check reaches OpenFGA.
  **No SQL generation checks, pending-mutation fence, retries, project locks,
  authorization migration gate, fresh allowed Check, or store availability
  validation is disabled.**
- Both modes must deny access after an acknowledged revocation. Every allowed
  response requires a fresh Check and live store. The driver verifies this
  using `miss` (on) or `uncached` (off) counters on every allowed read. A
  `hit_allow` event invalidates every comparison.

The HTTP Criterion cases are `allow_fresh_check` (200, body validated every
iteration) and `deny_cached_or_uncached` (403, exact error JSON, no metadata
leak). Counter deltas cover **warm-up and measurement**: they are not the
number of timed iterations. Denials on the `on` arm should normally be hits,
with occasional TTL misses. On the `off` arm every denial is a miss and an
uncached authoritative Check. The new `uncached` counter is observational.

Cold denial misses are *not* misrepresented as a steady-state Criterion
benchmark: for each of eight rounds before and after paired measurements,
a real grant and acknowledged revoke advance the shared generation. The
first denial on `on` must be a `miss`, the second a `hit_deny`; both requests
on `off` must call OpenFGA. These one-shot cold-path latencies are diagnostic
observations, not a Criterion confidence interval.

The outage gate stops **only** the disposable OpenFGA service, tests that
previously allowed catalog reads fail with HTTP 503 on both arms, and
restarts it. A cached denied read can return 403 during an outage; without a
cached denial it may return 503. Both are fail-closed, never 200. The gate
runs before and after measurement and records statuses.

## Design and statistical interpretation

Criterion 0.8.2 (locked in main by PR #30; no dependency bump in this PR) owns adaptive
iteration counts, 40 samples, one-second warm-up, three-second measurement,
10,000 bootstrap resamples, a 95% within-pair interval and estimates on the
mean-per-iteration ratio. It does **not** produce HTTP request p50 from its
means. The Rust driver handles safety gates, metrics and deterministic
ordering; it does not invent statistically independent samples.

The three stages are A/A-off, A/A-on and A/B, each with four pairs. The A/B
pairs use a reproducibly randomized balanced order, two off/on and two on/off;
stages are also shuffled. Each pair owns a separate Criterion output directory.
`on/off` ratios invert `off/on` estimates *and interval endpoints* when
necessary. A/A reports `second/first`. Within-pair confidence intervals do
not measure between-host noise. Compare several independent hosts/runs only
when authorized, and report A/A variability before interpreting A/B. A
configured 5% noise threshold is not an automatic performance gate. All
historical 25–59% effects are excluded from conclusions about this revision.

No other models, SQL migrations or positive-cache code should be altered.
No scheduled GitHub Actions workflow is included. Never run this against
production infrastructure; the HTTP fixture requires loopback endpoints.

## Prepare on an isolated machine (manual, after approval)

Set fresh non-production `POSTGRES_PASSWORD` and `DEMO_PASSWORD` (minimum 12
bytes) in your local shell; do not store them in a command log or artifact.
Do not reuse existing Compose volumes or ports. Build the **same Docker
image** for both arms. Example setup:

```sh
export COMPOSE_PROJECT_NAME=authz-criterion-$(date +%s)
export POSTGRES_PASSWORD="$(openssl rand -hex 24)"
export DEMO_PASSWORD="$(openssl rand -hex 24)"
export FGA_TRANSPORT=http
export COMPOSE_FILE=compose.yaml:compose.cache-lakekeeper.yaml

docker compose build app
# Bootstrap/migrate through the canonical Lakekeeper app only.
docker compose up -d app
# Wait for http://127.0.0.1:3000/readyz to return 200, then:
docker compose up -d --no-build app-uncached
# Wait for http://127.0.0.1:3001/readyz to return 200.

cargo fmt --all                 # required before verifying the locally prepared patch
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --example catalog_cache_experiment
cargo bench --locked --bench catalog_cache_ab --no-run
# Set to the exact executable path printed by the command above.
export AUTHZ_BENCH_BINARY="$(pwd)/target/release/deps/catalog_cache_ab-REPLACE"
export AUTHZ_EXPERIMENT_OUT=artifacts/criterion-lakekeeper-$(date +%s)
AB_SEED=2606 cargo run --locked --release --example catalog_cache_experiment
```

Preparing or merging the patch does **not** execute benchmarks. The first
successful isolated manual run was [#42613610160](https://github.com/qqrm/openfga-axum-lab/actions/runs/42613610160)
on 9 November 2026. Capture `experiment.json` and each pair's raw Criterion
`sample.json`, `estimates.json`, counter deltas and console logs. The Rust
driver refuses to overwrite an existing output directory. After collecting
results, ensure that the shared authorization generation has no pending
mutations. Optional cleanup must be **explicitly approved** because it
removes disposable volumes:

```sh
# Only after verifying COMPOSE_PROJECT_NAME points to the disposable stand:
docker compose down -v
```

The benchmark itself uses a pooled, one-in-flight HTTP/1.1 client (unlike the
unpooled `tests/performance.rs` diagnostic). Do not compare raw latencies
between those clients, or between this Lakekeeper scenario and Openlane.


## Cross-over and per-request HTTP latency

The manual workflow at `.github/workflows/lakekeeper-criterion.yml` runs two
isolated Compose stacks with the cache mode reversed across the primary and
secondary application process. Build one image; use distinct
`COMPOSE_PROJECT_NAME` and `AUTHZ_EXPERIMENT_OUT` for each assignment.

| Assignment | BENCH_PRIMARY_CACHE_ENABLED | BENCH_SECONDARY_CACHE_ENABLED | AUTHZ_BENCH_SWAP_PORTS |
|---|---|---|---|
| Primary on port 3000 | `true` | `false` | `false` |
| Primary off port 3000 | `false` | `true` | `true` |

The driver refuses to accept wrong mode counters, so an incorrect assignment
fails rather than generating misleading A/B results. Set
`AUTHZ_BENCH_LOAD_PROFILES=true` to record complete per-request HTTP latency
samples at closed-loop concurrency 1 (128 requests), 8 (512 requests) and
32 (1024 requests). Each configuration is measured in two reversed-order
blocks. Workers reuse one HTTP/1.1 connection pool and validate every
response. Results include nearest-rank p50/p95/p99, total throughput,
cache hit/miss counts, and raw sorted per-request nanoseconds. A single
warmup request precedes each measured profile. The `valid` flag is only
true after all correctness gates and unchanged-dataset checks pass.

**Interpretation:** Fixed concurrency is a *closed-loop* workload, not a
fixed arrival rate or an open-loop load test. These are per-request
percentiles, not Criterion iteration-batch percentiles. The crossed
deployments control for mode-to-container assignment on the same host;
they do not provide independent-host uncertainty estimates.

The 9 November baseline indicated ~74x quicker hot denials. A/A allowed
reads ranged from roughly 0.89 to 1.18 (second/first), large enough to
invalidate claims about small allowed-read improvements. Do not claim
production speedups from a single runner. Report independent runs,
A/A drift, crossed-mode results, request p95/p99 and cache hit ratios
before selecting a practical regression threshold.

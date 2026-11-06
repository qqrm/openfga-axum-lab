# Criterion authorization-cache experiment

## Executed result: 2026-11-06

Measured commit: `a3eaac35aeb90d18a000e2e52064c31ca91bded7`.

- [Criterion experiment 42290359295](https://github.com/qqrm/openfga-axum-lab/actions/runs/42290359295): all three jobs succeeded.
- [Standard CI 42290359306](https://github.com/qqrm/openfga-axum-lab/actions/runs/42290359306): succeeded, including formatting, Clippy, nextest, real-stack integration, cross-replica races/faults, dependency checks and formal checks.
- 5,760 Criterion samples, 379,660 measured iterations; server counters cover 546,428 decisions including Criterion warm-up.
- Separate four-client diagnostics: 24,000 measured HTTP requests, zero errors.
- 240 post-revoke checks denied access; both modes returned 503 with OpenFGA stopped on every VM.

This is an integration benchmark of the real service, not a mock-cache microbenchmark or a production capacity claim. No change has been merged by this experiment.

### Criterion mean time

Times below are medians of the four A/B pairs' **Criterion mean-per-iteration estimates**, in milliseconds, cache off → on. Reduction is computed from the geometric mean of paired on/off ratios, not the rounded table cells. These are **not HTTP p50/p95/p99**.

| VM | Allowed read, ms | Mean-time reduction | Denied read, ms | Mean-time reduction |
|---|---:|---:|---:|---:|
| 1 | 1.873 → 1.273 | 32.4% | 1.617 → 0.667 | 58.8% |
| 2 | 2.073 → 1.540 | 25.7% | 1.839 → 0.845 | 54.2% |
| 3 | 2.092 → 1.548 | 26.0% | 1.839 → 0.843 | 54.1% |

All 24 A/B case comparisons (four pairs × two cases × three VMs) had their normalized 95% relative-mean interval wholly below 0.95. Per-VM geometric-mean on/off ratios were 0.6760, 0.7427 and 0.7403 for allowed reads, and 0.4116, 0.4585 and 0.4591 for denied reads. These large effects repeated across the observed pairs and VMs. There is no pooled confidence interval over physical hosts.

For example, VM2's four allowed-read ratio intervals were [0.7264, 0.7588], [0.7407, 0.7558], [0.7302, 0.7492] and [0.7227, 0.7538]. Each interval describes Criterion samples within that pair, not between-runner uncertainty. Original estimates and reverse-order comparisons are retained in the artifacts.

### A/A noise was not negligible

Identical modes produced point ratios from **0.9137 to 1.0956** (−8.6% to +9.6%). Of 48 A/A case comparisons, **11 intervals excluded 1**, and **two were wholly outside [0.95, 1.05]**. Criterion's console reports included one apparent regression on VM1 and one apparent improvement on VM2 despite unchanged code. Other comparisons were classified as no detectable change or within the noise threshold.

| VM | A/A point-ratio range | 95% intervals excluding 1 | Intervals wholly outside ±5% |
|---|---:|---:|---:|
| 1 | 0.9336–1.0956 | 8/16 | 1/16 |
| 2 | 0.9137–1.0150 | 3/16 | 1/16 |
| 3 | 0.9832–1.0170 | 0/16 | 0/16 |

Thus a single Criterion `improved`/`regressed` message is not a reliable CI verdict here. The 5% setting is a practical filter, not an upper bound on environmental noise. The large A/B effects repeated; small effects around 5% are not reliably resolved by this experiment. No multiple-comparison correction or host-population inference is asserted.

### Work counts

A/B cache-hit rates, over the explicitly labelled Criterion warm-up+measurement windows, were 99.9426% for allowed reads and 99.9512–99.9569% for denied reads. PostgreSQL values count executed statements, not packets.

| Per decision | Allowed: off → on | Denied: off → on |
|---|---:|---:|
| Application SQL | 3 → 5 | 2 → 4 |
| OpenFGA SQL | 6 → ≈0.0034 | 6 → ≈0.0026–0.0029 |
| Combined SQL | 9 → ≈5.0034 | 8 → ≈4.0026–4.0029 |
| OpenFGA HTTP | 1 Check → mostly 1 readiness | 1 Check → almost none |
| Generation reads | 0 → 2 | 0 → 2 |

These counts support the mechanism: the coordinated cache adds generation reads but avoids most graph evaluations. Cached denials do not require OpenFGA readiness. The off mode still performs authoritative OpenFGA checks and preserves the mutation fence. The service default remains cache-on.

### Separate HTTP diagnostic

Four closed-loop Tokio workers, 2,000 measured requests per mode/case, 200 warm-ups excluded from each measurement window. Fixed diagnostic order and one pass per case: descriptive only, not the randomized effect estimate or maximum capacity. Every diagnostic had zero errors. Quantiles are nearest-rank from individual HTTP request latencies, not Criterion sample means.

| VM | Case | p50 ms, off → on | p95 ms, off → on | p99 ms, off → on | Achieved RPS, off → on |
|---|---|---:|---:|---:|---:|
| 1 | allow_hot | 6.134 → 1.947 | 28.867 → 2.489 | 44.531 → 3.600 | 412 → 1981 |
| 1 | deny_hot | 4.209 → 0.989 | 21.877 → 1.333 | 32.852 → 1.725 | 576 → 3874 |
| 2 | allow_hot | 5.353 → 2.668 | 24.821 → 3.482 | 39.416 → 4.556 | 480 → 1449 |
| 2 | deny_hot | 4.499 → 1.412 | 20.050 → 1.927 | 30.795 → 2.527 | 583 → 2743 |
| 3 | allow_hot | 5.473 → 2.648 | 25.302 → 3.616 | 40.818 → 4.739 | 466 → 1443 |
| 3 | deny_hot | 4.298 → 1.387 | 18.586 → 1.744 | 32.360 → 1.972 | 626 → 2838 |

### Environment and raw evidence

VM1 used Intel Xeon Platinum 8370C; VM2/VM3 used AMD EPYC 7763. All exposed four virtual CPUs and a Microsoft hypervisor. Independently provisioned VMs do not prove distinct physical hosts. Comparisons are within each VM, not between different CPUs.

All six application instances had executable SHA-256:

`5a1206aa86213348e6830c4432c851de10bf635edfe639a53fc319d1f8fe1747`

Dependency image digests matched across the three runners:

- `postgres@sha256:665dc7b37ac52a17e298511414c21d23ffa4e6a6ef6225b3e5d59c7afb35e994`
- `openfga/openfga@sha256:79df38493e1c7bfa3b23a63df96fc6e630922e5c9dc63ad3052eddcaf4da1761`

Application image IDs differed across builds; the binary hashes matched. Lockfile, model/schema/source hashes, resolved Compose, image metadata, tool versions and CPU information are archived. Source image tags and hosted-runner images are mutable; recording metadata does not pin future infrastructure.

| Artifact | GitHub artifact ID | ZIP SHA-256 |
|---|---:|---|
| criterion-authz-1 | 13539337904 | `60086b3229fa48666f644cada82bf83fd9bd424f76835e56c8d7c1bb981c062d` |
| criterion-authz-2 | 13539528588 | `df6ec0191ad9909839a6212a80d7bd6df605ad336b559d35439c9d171aab51e5` |
| criterion-authz-3 | 13540575805 | `90d633d8c2fcbee298cd5f5200e66710c59daf51271ce880320aca0b5005a990` |

Artifacts expire on 2026-12-06. The companion evidence bundle preserves all three original ZIPs, the numeric summary, this experiment's result report and reproduction instructions. Each ZIP contains original Criterion `sample.json`/`estimates.json`, baseline changes, stdout/stderr, counters, raw HTTP diagnostics and sources. The automated run used `--noplot`: it produced JSON and console reports, **not HTML plots**.

Independent verification recomputed means from each sample's time/iteration count, checked ratio orientation, raw HTTP quantiles and RPS, counter invariants, post-revoke denials, binary equality and ZIP checksums. Confidence intervals came from Criterion, not a replacement custom bootstrap. All A/A comparisons and all observations were retained.

## Method and implementation

`benches/authz_cache.rs` is the native `cargo bench` entry point, with `harness = false`, Criterion 0.8.2 and `async_tokio`. The measured loop is `b.to_async(&runtime).iter(...)`. The async runtime, authentication and PostgreSQL observer are outside the timer. Each timed iteration creates a pooled HTTP/1.1 request, consumes its body and validates the authorization status and expected data. Real Axum/PostgreSQL/OpenFGA are used, not mocks.

Criterion owns timing, warm-up, adaptive iteration counts, bootstrap estimates, baseline comparisons and reports. Settings are 40 samples, one-second warm-up, three-second target measurement, 10,000 resamples, 95% confidence, 5% significance and a 5% noise threshold. The two cases are `authz_http/allow_hot` (Alice, exact seeded fox JSON, 200) and `authz_http/deny_hot` (Dave, empty 403). Each has one in-flight request.

`examples/cache_experiment.rs` orchestrates four time-local paired blocks per A/A-off, A/A-on and A/B stage. A SHA-256-seeded permutation randomizes stage order and balances AB/BA order. Each pair has an isolated output directory. First invocation saves `reference`; second compares against it. Relative mean estimates come from Criterion. Reverse-order A/B ratios and confidence endpoints are inverted to report on/off consistently. A/A ratios are second/first. There is no custom bootstrap in the driver.

Server counter snapshots cover the entire labelled Criterion warm-up+measurement window. `pg_stat_statements` is queried from the maintenance database so observer queries are excluded from the application/OpenFGA DB counts. The separate Rust HTTP diagnostic records individual latency samples and measured-window-only counters, not inferential statistics.

Before measuring, nextest runs real-backend system and cross-replica race/fault suites. The experiment tests unauthenticated access, grants, warm allows, revocation through each writer and denial on both replicas before/after timing. It then requires 503 for an allowed read with OpenFGA stopped. Any unexpected status/body, counter reset, pending bypass or retry invalidates the run. These tests do not prove global serializability.

Rust 1.98.0 is explicitly selected using `RUSTUP_TOOLCHAIN`; Criterion is pinned exactly, and Cargo.lock is committed. Both modes within each runner use one application image and one fresh seeded dataset. Shell is restricted to build/start/inspect/cleanup tasks; the load, orchestration, gates and counters are Rust, with timing and inferential statistics delegated to Criterion.

## Reproduction

The run above used a push to the experiment branch. After the workflow is merged into the default branch, **Actions → Criterion authorization cache experiment → Run workflow** can select a branch. Before merge, changes to the workflow's listed benchmark paths on `experiment/authz-cache-ab` trigger it. No scheduled runs are configured.

For a fresh local stand (ports 3000, 3001 and 55432 must be free):

```sh
export RUSTUP_TOOLCHAIN=1.98.0
export COMPOSE_FILE=compose.yaml:compose.cache-ab.yaml
export COMPOSE_PROJECT_NAME=authz-criterion-local
cargo bench --locked --bench authz_cache --no-run
cargo build --locked --release --example cache_experiment
docker compose build app
docker compose up -d --no-build app app-uncached
docker compose exec -T postgres psql -U petlab -d postgres -v ON_ERROR_STOP=1 -c 'CREATE EXTENSION pg_stat_statements'
AB_SEED=2606 cargo run --locked --release --example cache_experiment
docker compose down -v -t 2
```

Archive `artifacts/criterion-experiment` before another experiment: the driver refuses to silently reuse it. Cleanup deletes the disposable project's volumes. Do not point the benchmark at an existing deployment or reuse valuable volumes.

For a single native Criterion pair on that already started fresh stand:

```sh
AUTHZ_BENCH_MODE=off AUTHZ_BENCH_LABEL=first AUTHZ_BENCH_PAIR_DIR=target/manual-authz \
  cargo bench --locked --bench authz_cache -- --save-baseline reference
AUTHZ_BENCH_MODE=on AUTHZ_BENCH_LABEL=second AUTHZ_BENCH_PAIR_DIR=target/manual-authz \
  cargo bench --locked --bench authz_cache -- --baseline reference
```

## Interpretation limits

Criterion's within-pair interval is not a confidence interval over the population of hosts. Its iteration mean/median is not an HTTP request p50. Inspect A/A and between-pair/runner spread; temporal correlation and multiple comparisons prevent treating one significance message as universal proof.

This is a tiny hot-key workload with pooled HTTP and co-located client/services. Generator headroom was not independently established. Cold caches, large working sets, write-heavy traffic, fixed-arrival-rate latency and production topology need separate experiments. The previous Python client used different connections and concurrency: its results are not a Criterion baseline. This experiment remains report-only, with no automatic performance merge or regression gate.

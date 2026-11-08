# Performance measurement

The Rust nextest binary `tests/performance.rs` measures a complete HTTP read:
token lookup in PostgreSQL, object lookup, the current authorization/cache path
including a fresh OpenFGA Check for every successful access, and data read. It uses closed-loop
clients, disables idle connection pooling and sends `Connection: close`, retaining
one HTTP connection per request. It does not establish a capacity or SLO.

`diagnostic_latency` runs 50 sequential warmups and 400 measured requests at
each concurrency of 1 and 8. It records actual completions, errors, all warmup
and measured samples, percentiles and elapsed time in `artifacts/benchmark.json`.
Any request error or missing observation invalidates the run; latency values
alone never fail a regression gate. The ordinary CI integration job runs this
diagnostic against a release build.

`paired_aa_calibration` is a same-binary A/A calibration. Six randomized,
balanced AB/BA blocks each contain 20 warmup and 120 measured requests per arm
with four closed-loop workers. It retains all samples and computes a paired
within-run bootstrap interval of median-latency ratios from 5,000 resamples.
The fixed seed is 2606, with SplitMix64 used only for ordering/resampling, not
credentials. The raw output is `artifacts/paired-aa.json`.

This is an exploratory noise check, not a regression gate: one runner cannot
estimate between-VM variation, samples within a block are not independent
hosts, and the generator itself may bottleneck. The A/A workflow remains
manual and has no schedule. It requires the real-stack correctness suite to
pass before collecting measurements.

```sh
cargo nextest run --locked --release --profile ci --test system --run-ignored only
cargo nextest run --locked --release --profile ci --test performance \
  --run-ignored only -E 'test(paired_aa_calibration)' --success-output immediate
```

Reports identify the commit, runner, OS/architecture, available parallelism,
Rust toolchain, debug/release configuration, application SHA-256 and benchmark
source SHA-256. The clock is `std::time::Instant`; reporting nanoseconds does
not claim nanosecond hardware resolution. CPU time for the client process is
explicitly `null`/not collected by this portable implementation. Container image
IDs, container state, resource snapshots and Docker engine information are
captured separately. Do not silently substitute wall time for process CPU time.

The Rust client/runtime and native release server differ from the previous
Python-client/Compose-server measurement setup. Historical results are not a
performance baseline for this harness. Establish fresh A/A noise measurements
before comparing revisions, and do not compare across changed generators.

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

## Raw backend diagnostics

`backend_bench` and the Criterion target `authz_cache` exercise concrete OpenFGA
adapters directly. They do not measure the task API, PostgreSQL authorization
scope, migration activation, mutation fencing or the denial cache. Raw
`ListObjects` is used for adapter/model diagnostics; task enumeration
continues to require bounded SQL candidates, per-object Check and revision validation.

Set `FGA_URL`, `FGA_STORE` and `FGA_MODEL` to a disposable backend's endpoint,
store ID and model ID. The diagnostic defaults to HTTP when compiled with that
feature. `BACKEND=grpc` requires the gRPC feature and endpoint. `ITERS` and `CONC`
must be positive integers; every requested iteration completes even when the
division has a remainder or concurrency exceeds the batch size. Invalid options,
backend errors and unexpected decisions fail the run without a timing summary.

```sh
ITERS=101 CONC=8 cargo run --locked --release --bin backend_bench
```

The default Check is Alice's `can_view` on
`task:access-review`. `BENCH_USER`, `BENCH_RELATION` and
`BENCH_OBJECT` select another fixture. `BENCH_EXPECT_ALLOWED` defaults to `true`;
set it explicitly to `false` for an intentional denial workload. `MODE=list`
also requires `BENCH_EXPECTED_OBJECTS` as the exact expected JSON array and uses
`BENCH_TYPE` (default `task`). Unexpected, missing or duplicate
returned objects invalidate the measurement; an explicitly expected empty list
is a distinct workload.

The diagnostic reports individual operation mean/p50/p99 latency separately
from wall-clock throughput. `DELAY_MS` paces each worker before its request;
that delay reduces throughput and is excluded from request latency. It does
not simulate network delay inside the transport. These are diagnostic samples,
not a controlled HTTP-versus-gRPC performance verdict.

The Criterion target uses `FGA_URL_HTTP`/`FGA_URL_GRPC` and the same store/model
and Check fixture variables. Its gRPC connection lives on the runtime used for
the entire benchmark. Every timed Check validates the expected decision, and a
backend error aborts the benchmark. Existing historical results are not evidence
for the current Openlane workload; apply the comparison controls above before
making a new claim.

## Historical calibration, before the Rust harness

The [2026-11-06 GitHub Actions run](https://github.com/qqrm/openfga-axum-lab/actions/runs/42250134346)
recorded six A/A blocks with four closed-loop clients and 120 measured requests
per arm per block. It reported zero errors, a geometric mean of block-level
B/A median-latency ratios of 1.003, and a within-run bootstrap interval of
0.916–1.075. The raw artifact has limited retention. These numbers describe only
that VM and run; they do not establish a regression threshold, cross-run
variability, production capacity, or a baseline for the new Rust client.

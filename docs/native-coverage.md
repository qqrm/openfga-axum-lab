# Native Rust coverage

The mandatory `coverage` job in **Actions → CI → Run workflow** uses the pinned
Rust toolchain, its matching `llvm-tools-preview`, cargo-llvm-cov 0.9.1 and
cargo-nextest 0.9.151. A pull request measurement checks out the exact head SHA;
`environment.txt` records that SHA and tool versions. Coverage is evidence, not
a percentage gate. Choose any future regression threshold after reviewing the
measured source and missing paths.

## Scope and reproduction

With Docker running, execute from the repository root:

```sh
rustup component add llvm-tools-preview
# Install the versions listed above using your normal Cargo tool installer.
cargo llvm-cov clean --workspace
cargo llvm-cov nextest --locked --no-report --profile ci --workspace --lib --bins --tests
cargo llvm-cov nextest --locked --no-report --profile ci \
  --test system --test cache_replicas --run-ignored only --success-output immediate
cargo llvm-cov report --html
cargo llvm-cov report --json --output-path coverage.json
cargo llvm-cov report --lcov --output-path lcov.info
cargo llvm-cov report --show-missing-lines
```

The first run executes ordinary native tests; the second explicitly runs the
ignored system scenarios and the two-replica cache/fault scenario against
real PostgreSQL and OpenFGA. Instrumentation also applies to the ordinary
`CARGO_BIN_EXE_openfga-axum-lab` and `CARGO_BIN_EXE_lab-tool` executables built by
Cargo, not just to test harnesses. `--no-report` accumulates both runs after an
explicit clean. Do not run the instrumented commands inside an independently
configured `cargo llvm-cov show-env` environment.

Reports use cargo-llvm-cov's default source selection: compiled root `src/` and
`tools/` code, including in-file test modules; separate `tests/`, dependencies,
examples and benchmarks are excluded by the tool. Review service and tooling
files separately; their combined percentage is not a server-only metric.

Docker-built application images, OpenFGA/PostgreSQL internals, the separate
browser/WASM package and doctest executions are **outside this measurement**.
The original release-mode integration, Compose image, latency, browser,
formal and security jobs remain required. Instrumented timings are not used as
performance evidence; the Criterion benchmark from PR #8 is not selected.

## Child process and profile integrity

The server registers SIGTERM before announcing readiness and lets Axum finish
in-flight requests before returning normally. Successful native test paths
explicitly stop and reap each child, including restarted processes and both
replicas. A failed exit or shutdown timeout fails the test; forceful cleanup is
only a fallback. `kill_on_drop` still handles failed/abandoned scenarios.

When LLVM coverage is enabled, each Axum child receives a unique profile name
containing a UUID, `%p` (PID) and `%m` (instrumented module). It stays beside the
test profiles in cargo-llvm-cov's target directory. After normal exit, the test
requires that child's profile to exist and be nonempty. Cargo-llvm-cov merges
the profiles with its default strict failure mode. CI also requires executed
functions from main, HTTP, authentication, authorization cache, infrastructure
and use-case source files, so client-only coverage cannot pass.

The `native-rust-coverage` artifact retains HTML, LLVM JSON, LCOV, annotated
source, summary/missing lines, both JUnit reports, environment metadata, raw
profiles, merged profdata and profile hashes. Replaying raw profiles also needs
the matching instrumented executables; the reports themselves are inspectable.
A failed workflow may retain
partial diagnostic reports; it is not an accepted complete measurement.

Stable Rust provides line, region and function coverage here. cargo-llvm-cov's
branch/MC/DC and doctest coverage still require nightly; no numeric branch
coverage claim is made. Inspect zero-count regions in JSON/HTML against the
source to identify untested authorization, recovery and error paths. Rustdoc
warnings and the four executable domain doctests are independently required by
the existing `rust` job (the browser job also executes its two examples).

## Tool documentation

- [cargo-llvm-cov 0.9.1 usage and scope](https://github.com/taiki-e/cargo-llvm-cov/blob/v0.9.1/README.md)
- [Rust instrumentation and profile filenames](https://doc.rust-lang.org/rustc/instrument-coverage.html)
- [Nextest coverage integration](https://nexte.st/docs/integrations/test-coverage/)

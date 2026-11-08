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
ignored system scenarios, including organization/owner transitions, and
the multi-replica cache/fault scenarios against
real PostgreSQL and OpenFGA over the default HTTP adapter. The independent
release integration matrix still verifies HTTP and gRPC; this coverage report
does not claim to measure gRPC execution. Instrumentation also applies to the ordinary
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
The intentional SIGKILL cases in `authorization_migration` stay in the release
integration matrix. They explicitly crash and reap the child while its remote
write is paused; such a process cannot provide a normally flushed profile.
The original release-mode integration, Compose image, latency, browser,
formal and security jobs remain required. Instrumented timings are not used as
performance evidence; the separate Criterion benchmarks are not selected.

## Authorization and failure scenarios

The failure/recovery integration keeps the useful scenarios from PRs #15, #16,
#17, #20 and #21 on the current Openlane data model:

| Original PR | Retained behavioral evidence |
| --- | --- |
| #15 | Edit, deletion and viewer grant/revoke wait for a real PostgreSQL task lock. A coordinated real assigner revocation must then deny the operation without modifying desired SQL state or OpenFGA tuples; restoring authority makes the same request succeed. |
| #16 | Real SQL faults in login accounting, credential lookup, session insertion, authentication and logout fail closed and recover. Missing cache-coordinator state, a rejected pre-write marker and failed post-write finalization preserve intent/fences across replicas and restart. |
| #17 | Failed startup after lost committed provisioning/deletion responses, activation UPDATE or deferred COMMIT errors, deletion UPDATE errors and a missing recovery table must exit normally with a nonzero status before listening. Each failed instrumented child is reaped and must retain its own profile. |
| #20 | With OpenFGA offline, emergency deny mode keeps sessions usable while denying domain access and writes, preserves pending lifecycle data and desired grants, and cannot execute recovery. Every current DenyAll trait operation also has an ordinary unit assertion. |
| #21 | Three real successful Check responses are paused while unrelated writes advance the generation. Exhausting the bounded retry budget returns 503; the unchanged request recovers when churn stops. |

These cases preserve the newer protocol: unresolved intent/debt closes reads
and further mutations; ordinary restart or an acknowledged replay does not prove
that an older request drained. Tests stop all writers and observe captured proxy
writes completing before the explicit operator recovery command. A restored
backend and repeated successful requests are positive controls after each fault.

The marker-failure case separately checks readiness when SQL intent is durable
but the global marker could not commit. The intended contract is 503 during
that unresolved state and 200 after explicit recovery; credentials continue to
work. The failure PR records the baseline and fixed-head execution evidence.

The older requirement that every competing lifecycle handler be admitted is not
restored. Current concurrent lifecycle and row-lock scenarios retain their
no-resurrection and tuple-cleanup checks while allowing the durable fence to
reject an overlapping request with 503.

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
functions from main/server, HTTP/authentication, authorization cache,
infrastructure, task/organization use cases and durable authorization source files, so client-only coverage cannot pass.

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

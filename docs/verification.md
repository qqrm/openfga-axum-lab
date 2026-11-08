# Verification and its limits

Tests are executable evidence for specific scenarios. A test file, a configured
workflow or a skipped Docker test is not evidence that the scenario passed.
Use the CI run for the exact commit and its JUnit/job output.

| Suite | Concrete target |
| --- | --- |
| `model_provenance` | Exact default Lakekeeper model SHA-256 and separate Openlane scenario provenance, pinned commits and preserved licenses/notices |
| `fga_model` | Native real-engine Openlane decisions/mutations/ListObjects through HTTP and gRPC; separate late-owner-write witnesses for both models |
| Domain / HTTP unit tests | Typed identifier grammar, lifecycle transitions, explicit routes/actions, strict DTOs, ambiguous bearer rejection |
| `http_adapter` | Real loopback HTTP with malformed/duplicate decisions, unexpected statuses, redirects, content type/size, stalled body, wrong target, soft-deleted store despite positive Check, pagination and mixed Write |
| `grpc_adapter` | Real loopback gRPC framing, presence/status failures, consistency, target identity and tuple/mixed-write handling |
| `system` | Real Axum + PostgreSQL + OpenFGA: role matrix, sessions, metrics, concurrent idempotency, deletion/grant race, held locks, create/delete recovery, SQL/authentication faults, post-lock revocation, retry exhaustion, persistence and outage |
| `cache_replicas` | Two real replicas: every allow checks live, negative-cache invalidation, delayed decision, Check-only failure, ambiguous grant/revoke, HTTP redirect without forwarding, restart fence |
| `authorization_migration` | Real SQL source and CLI: closed initial gate, role/owner/hierarchy equivalence, source/target rejection, checkpoints, lost response and restart, verification mismatch, late pre-forward batch debt across app/storage restarts, foreign-scenario rejection and post-cutover revocation |
| `compose_deployment` | Actual Dockerfile and provisioning/migration startup, selected runtime transport, authenticated catalog read |
| Browser E2E | Actual API: actor visibility, grant/revoke, escalation denial, owner transfer, role inheritance, scope substitution, moves, session revocation, stale real responses, XSS text rendering, no-store/CSP headers |
| `delivery` | Actual image on two isolated stands; store/session/volume isolation, quiesced two-database dump/restore, dependency failure and deleted store |
| `performance` | Report-only raw request latency and errors; separate manual A/A calibration, no capacity verdict |
| Kani | Finite production lifecycle transition/visibility functions, including hidden `updating`; no SQL/network/distributed-system proof |

The system tests retain six concurrent creates with one key, conflicting
payload rejection, deleted-key tombstones, eight-operation delete/grant racing,
and direct graph inspection after deletion, including reverse hierarchy links.
Persistence tests reuse old sessions after restarting the app and dependencies.
Outage cases first obtain successful decisions, then require closed responses.

The migration compares both allow and deny outcomes, including an outsider,
and exact graph equality before activation. Adapter fault tests cover protocol
shapes that the real server normally would not emit. Real-stack proxy tests
retain actual server state while delaying or replacing a response, which
exercises the uncertain-outcome boundary without inventing a permissive backend.

The native model workload is isolated from catalog SQL, bootstrap configuration
and the live service. Run `bash scripts/test-openlane-model.sh` for its two
transports; it is not a substitute for the catalog integration/browser matrix.
[Native coverage](native-coverage.md) includes gracefully stopped Axum children,
requires measured core source files and reports branch/function/line limits.

## Running the gates

Without Docker:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo nextest run --locked --all-features --profile ci
```

With Docker, separately for HTTP and gRPC:

```sh
cargo nextest run --locked --release --profile ci \
  --no-default-features --features http \
  --test system --test cache_replicas --test authorization_migration \
  --test compose_deployment --test performance \
  --run-ignored only -E 'not test(paired_aa_calibration)' --success-output immediate
```

CI explicitly selects these suites for both transports. Nextest uses no retries
and serializes Docker-heavy tests. Ordinary non-ignored runs cannot establish
end-to-end coverage. The `browser-e2e` job builds a separate real stack and is a
required dependency of `ci-gate`; traces are disabled to avoid persisting bearer
credentials, and only failure screenshots are retained.

Image delivery has its own workflow because it needs a built immutable image
ID. Browser-lab verification exercises the actual devcontainer, not a live
GitHub account login. Live tunnel verification is an explicit manual operation.
Kubernetes is experimental and has no cluster-based CI evidence.

## Tooling and negative controls

Formatting, Clippy, ShellCheck, actionlint, zizmor, cargo-audit, cargo-deny,
cargo-machete, Gitleaks and dependency provenance checks are retained. Several
have deliberate failing fixtures to check instrument sensitivity. They do not
replace semantic authorization tests. The RSA advisory exception requires RSA
to be absent from the resolved dependency graph; it is not a blanket advisory
ignore. The workflow contract also verifies real-stack suite selection.

Kani's mutation control changes a visibility rule and expects proof failure.
That establishes sensitivity of that bounded proof, not coverage of arbitrary
HTTP failures. Structured logs/metrics can aid investigation; they are not an
immutable audit trail or a test of operational incident response.

Every test owns a unique disposable Compose project. Normal exit/panic cleanup
removes only that project's resources. Forced process termination may leave
resources on a developer's Docker engine; inspect the project before cleanup.
Tests and benchmarks must not target the manually shared live laboratory.

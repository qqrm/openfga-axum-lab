# Verification and instrument evidence

| Instrument | Actual target | Negative control | Boundary |
| --- | --- | --- | --- |
| rustfmt | Rust formatting | malformed formatting fixture | Formatting only |
| Clippy | Rust warnings | needless-return fixture | Not semantic correctness |
| ShellCheck | project shell scripts | unquoted expansion fixture (SC2086) | Not runtime correctness |
| cargo-audit | lockfile advisories | known vulnerable RSA fixture | Advisory DB and dependency graph limits |
| cargo-deny | advisories, bans, sources | wildcard dependency fixture in CI | Not a full vulnerability audit |
| cargo-machete | unused dependencies | deliberately unused dependency fixture | Static usage heuristics |
| Gitleaks | Git history secret patterns | synthetically constructed fake AWS key | Pattern coverage is incomplete; two old demo values are narrowly allowlisted |
| actionlint / zizmor | workflow syntax and security patterns | intentionally invalid / unsafe workflow fixtures | Does not prove GitHub settings or authorization |
| Rust Dependabot provenance | signed bot commits and expected head | seven unit tests including bad actor, stale head and missing SHA | Does not prove a live merge occurred |
| Rust bootstrap | model fingerprint, persisted store/model, atomic config publication | changed model, missing identifiers, deleted store | No proof of crash consistency across PostgreSQL and filesystem |
| Kani | pure lifecycle functions | mutate visibility to expose deleting state; proof must fail | No proof of SQL, HTTP, OpenFGA, networking, or concurrency |
| nextest system suite | real API, PostgreSQL and OpenFGA | denied roles, bad credentials, outage, concurrent retries, recovery | Bounded scenarios, not exhaustive |
| nextest cache suite | two actual Axum replicas | delayed allow, concurrent revoke, unavailable health endpoint, lost write response and restart | Not an exhaustive distributed-system proof |
| nextest Compose deployment | actual Dockerfile and Rust bootstrap service | HTTP login and authorized data read after image build | Local deployment smoke test, not production certification |
| nextest workflow contract | tracked sources and explicit CI suite selection | rejection of Python files/runtime references and missing suites | String-level guard; actionlint still validates workflow syntax |

## Behavior retained from the former scripts

`tests/system.rs` covers authorization and sessions, normalized metrics without
credentials, idempotent grants/revokes, six concurrent creates with one key,
conflicting payloads, deleted-key reuse, an eight-operation delete/grant race,
and direct checks that deleted tasks have no remaining OpenFGA tuples.

Persistence stops Axum, OpenFGA and PostgreSQL, then restarts them without
removing volumes. It checks saved data and relations, reuses the **old sessions**,
and verifies that a subsequent revoke takes effect. Separate creation and
deletion recovery tests apply the original SQL fixtures before restarting Axum.
The outage test warms allows first, then requires 503 for both single-task and
list requests after OpenFGA is stopped.

Each test owns a fresh Compose project and temporary configuration. Published
ports are loopback-only and ephemeral; addresses are resolved again after a
restart. The cache and system suites use the same stack support and the same
`lab-tool bootstrap` implementation as the deployment. A separate test builds
and starts the actual Compose application image.

Run unit and policy checks without Docker:

```sh
cargo nextest run --locked --profile ci
```

Run the ordinary full suite with Docker, including explicitly ignored tests:

```sh
cargo nextest run --locked --release --profile ci --run-ignored all \
  -E 'not test(paired_aa_calibration) and not binary(fga_model)' --success-output immediate
```

The CI integration job explicitly selects `system`, `cache_replicas`,
`compose_deployment` and `performance` with `--run-ignored only`. A normal
nextest invocation without that option does **not** establish integration
coverage. A/A calibration remains a separate manual workflow. The CI profile
uses no retries, bounds test duration, serializes Docker-intensive tests and
writes JUnit. RAII cleanup handles normal exits and panic; forced termination
can leave containers/volumes on a local engine. Hosted CI runners are disposable.

## Openlane scenario coverage

`bash scripts/test-openlane-model.sh` runs the pinned native OpenFGA model
matrix and HTTP/gRPC wire tests; CI includes it in the Rust job. Native tests
are excluded from the Docker-only command above because they have their own
binary requirements.

`tests/scenarios/openlane.rs` is included in the real-stack `system` binary.
It covers organization isolation, exact upstream blocked precedence, assignee
limits, direct and inherited grants, role changes, owner-only transfer,
competing transfers, warm caches and lost mutation responses followed by
startup journal replay. The existing HTTP/gRPC integration matrix selects it.

`e2e/tests/` exercises the actual browser UI and API against a disposable
PostgreSQL/OpenFGA stack. CI defines both transport jobs and includes them in
`ci-gate`; merely collecting Playwright tests is not an E2E pass.

Record executed commands and unavailable environments in
[the migration validation report](openlane-migration.md).

## Formal and tooling boundaries

Kani exhaustively explores the finite lifecycle/event choices in the two
harnesses. The service calls the same visibility and transition functions, but
database updates and external side effects are validated separately. Run:

```sh
cargo kani --lib
bash scripts/kani-negative.sh
bash scripts/check-tooling.sh
bash scripts/audit-negative.sh
```

Dependabot auto-merge checks live branch protection before enabling native
auto-merge and otherwise leaves the PR open. The privileged workflow does not
check out or execute PR code. The Rust provenance command runs only in the
read-only workflow. A synthetic policy test is not evidence that a real
Dependabot PR has merged.

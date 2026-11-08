# Openlane migration: scope, review and verification

The initial local review was prepared on `review/openlane-domain` from
`90167709fa65c4c49b9c34d1c15269003df7643c` (8 November 2026). After that review,
the user authorized publication, GitHub checks and merging once checks pass.
Publication integrates newer `main`
`e299e4fc400377caeb273818664706b7bca5b857`: its HTTP default, `ListObjects`,
Criterion dependencies and other tooling changes are retained. Deployment
remains a separate, unauthorized action.

## Implemented scope

The former demonstration is replaced by the real Openlane organization/task
permission graph, reduced to `user`, `organization` and `task`. The upstream
revision is `1c38df0a53530c8c39eb0784862b06eaa3e26f58`. The modular schema 1.2
source is flattened into an executable schema 1.1 JSON model and a readable DSL.
[Provenance, exact differences and permission tables](openlane-model.md), the
full Apache-2.0 license and NOTICE accompany the model and container image.

Axum serves tasks, organization memberships, explicit organization task grants,
task viewer/editor/assignee/blocked grants and owner-only transfer. There is one
organization owner; assigner/organization links on a task are immutable.
Task CRUD preserves idempotency keys, lifecycle tombstones and recovery. Accounts
and organizations are seeded examples; user registration, organization creation,
SSO, suborganizations and the wider compliance domain are deliberately outside
this scenario. Five accounts, two organizations and four initial tasks exercise
isolation and independent paths to the same object.

The browser presents server-confirmed capabilities, current direct task grants,
role changes and transfer without decorative animals. Session/account switches
clear protected data and ignore late responses from the previous session.
The static Pages demo is an explanatory matrix plus a Rust/WASM lifecycle;
it is explicitly not an authorization server.

## Security and consistency changes

Ownership and role replacement use one OpenFGA Write containing writes and
deletes. A PostgreSQL journal and organization revision/fence cover every
permission mutation and task lifecycle projection. All mutation checks run
under organization/task locks. Nonblocking reads revalidate organization
revisions around remote decisions, and the shared cache generation still
invalidates both direct and inherited rights across replicas.

Two additional failure cases were identified during review:

1. HTTP cached allows previously checked only server health. They now also
   require the configured store to exist, matching gRPC. A soft-deleted store
   cannot retain warm authorization through a healthy `/healthz` response.
2. Idempotent replay alone does not order requests. A real OpenFGA experiment
   delayed Alice→Bob until after an acknowledged replay and Bob→Carol, producing
   two owner tuples on the unguarded server. Therefore each outgoing journal
   attempt is durably marked **before** remote I/O. Replaying an attempted job
   can restore reads but leaves a persistent write quarantine. New mutations
   remain 503 until an operator drains all writers/backend/proxy requests and
   runs the explicit recovery command below. A restart or elapsed delay alone
   is never accepted as proof of drain.

The second case is a delivery-order demonstration. OpenFGA SQL writes do
propagate request cancellation; this does not make a client's timeout an
ordering barrier for a separately delayed request. See [the pinned source analysis and counterexample](openfga-write-ordering.md)
and [architecture](architecture.md) for the protocol and its limits. The real-stack tests now include a controlled
before-forward delay, in addition to lost post-commit responses.

Upstream semantics remain intentionally visible: `blocked` denies viewing and
ordinary editing but does not override `can_delete` or its branch in `can_edit`.
An organization `can_edit_task` grant also gives inherited task deletion.
Removing a direct edge does not erase an independent inherited edge. These
are documented/tested properties of the selected source model.

## Safe transition from an existing installation

The four already-applied SQL migrations and `Cargo.lock` are unchanged relative
to the publication base. The newer base's Criterion dependencies are retained. The
new migration creates task/organization/journal tables; it does not drop or
reinterpret existing records. The former routes are removed. Old database
role/database names are retained in Compose to avoid an incidental data migration.

The changed FGA model fingerprint is intentionally incompatible with the old
configuration. Both bootstrap and app startup refuse to use it silently; app
startup checks the fingerprint before database migration or seeding. Never
remove a user's existing volume/configuration to get past this check.

For review, use a **new, disposable** Compose project and credentials:

```bash
export COMPOSE_PROJECT_NAME=openlane-review
export POSTGRES_PASSWORD="$(openssl rand -hex 24)"
export DEMO_PASSWORD="$(openssl rand -hex 24)"
docker compose up -d --build app
```

Keep these generated values for that project. HTTP verification needs a
separate project with `FGA_TRANSPORT=http` and
`FGA_APP_URL=http://openfga:8080`; default gRPC uses 8081. Do not point the
changed app at an existing production store. A retained-data migration would
require a separately reviewed mapping and cutover plan; this change does not
pretend that historical records are compliance tasks.

## Recovery after an uncertain write

Normal startup replays the durable job before serving. A previously attempted
job leaves `authz_uncertain=true` after replay: reads follow the reconciled graph,
mutations are 503, and responses expose `recovery_required=true` with mutation
capabilities disabled. The conservative shared cache marker also survives.
Only identical idempotent requests can still arrive; admitting a newer conflicting
mutation before drain would defeat that property.

To restore writes, an operator must stop **all** application writers/recovery
workers and drain or stop/restart OpenFGA **and any forwarding proxies/queues**.
Confirm that no earlier request can still be delivered. Then run a single
instance with the same database, immutable model and store configuration:

```bash
app --recover-authorization-after-backend-drain
```

For a verified isolated Compose setup, after completing that drain procedure,
`docker compose run --rm --no-deps app --recover-authorization-after-backend-drain`
uses the application's configured credentials. This command is an explicit
operator assertion of drain, not a mechanism that proves it. It does not open
a listener. It reconciles every pending job, checks that none remain, then
clears quarantine/cache debt and increments their revisions in one SQL commit.
Only after success should normal replicas restart. A failure retains the fence.
There is no remote API or ordinary-startup switch that silently clears it.

Legacy or manually inserted lifecycle rows without journal records are treated
as potentially attempted writes. If several such pending/deleting rows belong
to one organization, normal startup can quarantine it after the first replay
and refuse the next recovery mutation. Use the same explicit drained-recovery
procedure to reconcile all rows. This is a conservative availability limit;
the normal API atomically stages one journal and its organization fence, so it
cannot create this set of orphan rows through successful application writes.

## Initial local validation before publication

The initial review source, including durable attempt markers, write quarantine and the
before-forward proxy regression, was checked with Rust/Cargo **1.99.1** on
Linux x86_64. The final local logs are under
`artifacts/openlane-validation/`; the review evidence archive includes the
logs named below. These numbers describe the original local review commit,
before integration of the newer main. That base still used gRPC by default;
the publication branch uses HTTP by default. Subsequent GitHub results are
recorded on the pull request, rather than inferred from these earlier runs.

| Executed check | Observed result | Evidence / boundary |
| --- | --- | --- |
| `cargo test --locked --all-targets` | **45 passed, 0 failed, 24 ignored** | `rust-tests-grpc-complete.log`; default gRPC build |
| `cargo test --locked --all-targets --no-default-features --features http` | **40 passed, 0 failed, 24 ignored** | `rust-tests-http-complete.log`; HTTP-only build |
| `bash scripts/test-openlane-model.sh` with verified native binaries | **3 native tests + 6 HTTP wire tests**, then **3 native tests + 9 gRPC wire tests**, all passed | `native-script-final.log`; wire tests also occur in the ordinary suites above and are not additional unique coverage |
| Native model behavior, each transport | **272 expected decisions**, each checked for HTTP parity; **40 changes plus seed**, including **3 invalid changes rejected** | Real OpenFGA **1.10.3**, memory datastore; actual compiled Rust HTTP/gRPC adapters |
| Readable DSL / deployed JSON parity | Passed in both native invocations | Official fga CLI **0.8.2**; permission rewrites and subject restrictions agree |
| Late-write ordering counterexample | Reproduced on real OpenFGA | Third native test uses a raw HTTP delayed-forward proxy in both feature builds; it is not proof of PostgreSQL quarantine behavior |
| `cargo clippy --locked --all-targets --all-features -- -D warnings` | Passed | `clippy-all-complete.log` |
| `cargo clippy --locked --all-targets --no-default-features --features http -- -D warnings` | Passed | `clippy-http-complete.log` |
| `cargo fmt --all -- --check` and `git diff --cached --check` | Passed | `fmt-complete.log`, `diff-check-complete.log` |
| Static browser demo | Rust/WASM build and `node scripts/check-browser-demo.mjs` passed | Build/contract validation; no browser rendering claim |
| Browser test sources | JavaScript syntax checked; **15 Playwright tests collected** | Collection does not execute browser or API assertions |
| Added shell/workflow configuration | Shell syntax and YAML parse passed | ShellCheck, actionlint and the full remote workflow were not run here |

The 24 ignored tests in each ordinary Cargo invocation consist of three native
tests (subsequently executed by the dedicated script) and **21 Docker-dependent
tests**, including delivery/performance checks and the separate manual A/A
calibration. Those Docker tests remain unexecuted in this environment. No sum
of the ordinary and native-script pass counts should be interpreted as a count
of distinct scenarios.

All final Cargo runs used `CARGO_INCREMENTAL=0`: an earlier native gRPC attempt
encountered zero-byte incremental object files; a non-incremental rebuild and
the final suites passed. Neither dependencies nor `Cargo.lock` were changed to
work around that local cache failure. The full commands are reproducible with
the toolchain above; native binary versions and verified archive hashes are
listed in [the model provenance document](openlane-model.md).

## Remaining validation gates

The local executor has no Docker executable/engine. An explicitly attempted
real PostgreSQL/Axum/OpenFGA system test stopped at process creation with
`Os NotFound`; it did not execute an application assertion. System package
installation also lacks required setgroups/seteuid permissions. The real-stack
Rust tests remain genuine Compose tests and have not been replaced by mocks.

Chromium is not installed. Attempts to install the full browser and headless
shell from the official Playwright CDN returned truncated/invalid ZIP files;
a subsequent connection also timed out. Therefore the 15 Playwright scenarios
were collected and syntax-checked, but browser execution, screenshots and
visual QA are unverified.

The modified CI defines native model validation plus real-stack Rust and
Playwright gates for HTTP and gRPC. No remote workflow had been started when
the initial local evidence above was recorded.
Full PostgreSQL recovery/locking/cache races, container build/deployment,
browser behavior and any existing release/security/formal gates must pass
before merge or release. No production-readiness or performance claim is made.

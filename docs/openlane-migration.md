# Openlane transition: scope, compatibility and verification

PR #26 replaces the fictional shelter with a focused scenario from Openlane.
It incorporates main `609967a2cf56f7070d6edd3fa55163f18752f47b`, which had introduced
a different Lakekeeper catalog example in PR #24. Openlane remains the primary
requested domain. The reconciliation retains and adapts the strict transport,
denial-only cache, durable global fence, verified SQL migration, browser guards,
HTTP default and delivery/workflow checks from that main revision.

This is an explicit domain change. It does not claim the Lakekeeper catalog API
and data model are still served, or silently map catalog records to tasks. The
retired model, provenance and Apache-2.0 license/NOTICE remain archived under
`fga/upstream/`; historical SQL migrations and data are not rewritten or deleted.

## Implemented scope

Only `user`, `organization` and `task` are active model types. Openlane core is
pinned to `1c38df0a53530c8c39eb0784862b06eaa3e26f58`; its modular schema 1.2 source is
flattened to executable schema 1.1 JSON and readable DSL. [Provenance and exact
permission semantics](openlane-model.md), full Apache-2.0 license and NOTICE are
included with the model and container/static distribution.

Axum serves tasks, membership and roles, explicit inherited organization task
permissions, task viewer/editor/assignee/blocked grants, and owner-only transfer.
Task organization/assigner are immutable; organization ownership is singular in
SQL and atomically replaced in OpenFGA. Task CRUD retains idempotency keys,
tombstones and durable recovery. Registration, organization creation, SSO,
suborganizations and the wider compliance domain are outside this small lab.
Five users, two organizations and four tasks exercise isolated and independent
access paths. They are example data, not copied customer data.

The neutral browser UI shows capabilities returned by the server, assignments,
grants, roles and ownership. Account switches clear protected data and discard
late responses. Literal text rendering, no-store/CSP and credential-free request
history are checked against real APIs. The separate static demo is an explanatory
permission matrix plus a Rust/WASM lifecycle, not an authorization server.

## Authorization and recovery guarantees

- Owner and role changes use one mixed OpenFGA Write. Every mutation checks its
  authority while holding organization/task locks; SQL intent and its journal
  commit before remote I/O. A dedicated pool commits attempt markers separately.
- Every allow uses a live HIGHER_CONSISTENCY Check. Only denials may be cached.
  Shared SQL generation and organization revisions discard stale decisions.
- Exact statuses, endpoint binding, response size/deadline and typed JSON/ACK
  checks reject malformed or ambiguous HTTP. gRPC exercises corresponding typed
  protocol behavior. Store health alone is not authorization readiness.
- A global durable pending fence blocks checks, lists and readiness after an
  ambiguous write, including on another replica and after restart. Replaying an
  attempted task/role/owner job or SQL migration batch does not silently clear it.
- The task API uses bounded SQL candidates and per-object Check, returning a
  complete list only within a 256-candidate budget. Larger lists fail closed;
  the lab does not claim production pagination or ListObjects completeness.
- SQL authorization cutover freezes/binds its source, checkpoints import and
  compares exact graph plus independent positive/negative decisions before
  activation. Active reruns never replay old grants over current permissions.
  [The migration contract](migration.md) describes the separate gates and limits.

The [real-server ordering witness](openfga-write-ordering.md) demonstrates why
idempotency alone is insufficient: a buffered original Alice→Bob write arriving
after replay and a newer Bob→Carol transfer can restore Bob alongside Carol.
Tests therefore delay forwarding an actual request as well as losing a response
after commit. Neither a successful replay nor elapsed time is treated as drain.

Upstream semantics stay visible: `blocked` excludes viewing and ordinary editing,
but not `can_delete` or its branch in `can_edit`. An organization `can_edit_task`
grant also inherits deletion. Removing one grant does not cancel another path.
These are documented properties of the selected model, not universal-deny claims.

## Safe transition from an existing installation

Already-applied migration files remain unchanged. New migrations add Openlane
state and a separate SQL authorization journal; they do not delete or reinterpret
retired records. Compose keeps the current main defaults: database/user `authzlab`
and HTTP transport. Older `petlab` volumes are not automatically renamed.

The model fingerprint is incompatible with previous configs. Bootstrap and app
startup validate it before touching application state. Do not delete a user's
volume/config to bypass rejection. Existing Lakekeeper data/binding is refused
by demo seeding/import. Retained-data conversion needs a separate reviewed mapping.

For review, create a **new disposable** Compose project and keep its credentials:

```sh
export COMPOSE_PROJECT_NAME=openlane-review
export POSTGRES_PASSWORD="$(openssl rand -hex 24)"
export DEMO_PASSWORD="$(openssl rand -hex 24)"
docker compose up -d --build app
```

HTTP uses port 8080. A separate gRPC test project sets `FGA_TRANSPORT=grpc` and
`FGA_APP_URL=http://openfga:8081`; bootstrap/migration continue using HTTP.
Bootstrap provisions an empty store/model, then explicit `DEMO_SEED=true` and
`DEMO_MIGRATE=true` derive and verify authorization from SQL. The reference
`fga/legacy.json` is a native model-test fixture, not the production import path.

## Recovery after an uncertain write

Normal startup replays durable jobs before listening. A previously attempted
job leaves organization quarantine and global pending debt. Reads, writes and
`/readyz` remain 503 even if the graph now matches SQL. `/healthz` only reports a
running process. Initial migration replay obeys the same global fence.

To reopen service, stop **all** application writers/recovery workers and drain
or stop/restart OpenFGA **and forwarding proxies/queues**. Establish that no old
request can still be delivered. Then run one process with the same database,
immutable model and store configuration:

```sh
app --recover-authorization-after-backend-drain
```

After that verified drain, an isolated Compose environment can use:

```sh
docker compose run --rm --no-deps app --recover-authorization-after-backend-drain
```

This command asserts the operator precondition; it does not prove it. It opens
no listener, reconciles all jobs, checks none remain, then clears quarantine and
cache debt while incrementing revisions in one SQL commit. Failure retains the
fence. Restart normal replicas only after success. No API or ordinary startup
switch clears the fence. Orphan lifecycle rows are conservatively treated as
previously attempted and require the same explicit recovery.

## Validation evidence and gates

The preceding Openlane-only head `059c6a5b499e6502a5e3ae8fedef5c7329637508`
passed [CI run 42495708913](https://github.com/qqrm/openfga-axum-lab/actions/runs/42495708913):
19 real integration scenarios per HTTP/gRPC transport and 15 real Playwright
scenarios per transport. [Delivery run 42495708905](https://github.com/qqrm/openfga-axum-lab/actions/runs/42495708905)
also passed two isolated stands, restart, quiesced two-database backup/restore,
outages and deleted-store readiness. Those results do **not** validate the later
reconciliation with main; each new PR head must pass its own gates.

The reconciled branch requires rustfmt, Clippy, all-feature and transport builds,
nextest, actual PostgreSQL/OpenFGA integration and migration tests, native model
DSL/JSON/decision parity, both-transport Playwright, image/Compose delivery,
ShellCheck/actionlint/zizmor, dependency/secret checks and bounded Kani proofs.
The PR check list and linked run IDs are the record for the exact tested head.
Wire stubs validate malformed protocol handling; they do not substitute for the
real authorization model or SQL/recovery scenarios.

The local executor lacks Docker and a usable Chromium installation, so local
Rust/native checks and remote container/browser checks are reported separately.
The native OpenFGA 1.10.3 test uses a memory datastore; it is not PostgreSQL
recovery evidence. Finite Kani lifecycle proofs do not prove the distributed
protocol. Long fault campaigns, production scale/pagination, external writers,
SSO, Kubernetes operation and performance/SLO claims remain outside validation.
Publishing/merging the PR does not authorize deployment or a release.

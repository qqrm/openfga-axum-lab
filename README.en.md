# Rust / Axum / OpenFGA — Authorization Lab

A native Rust authorization integration with tenant-scoped resources, roles,
ownership, hierarchy changes, filtered lists, real SQL-to-OpenFGA migration,
and conservative failure behavior.

The complete **Lakekeeper v4.15 authorization model** is preserved unchanged
from [upstream commit b73d5f8083a690ca4f533559e6ef3cbc6fbc13d6](https://github.com/lakekeeper/lakekeeper/blob/b73d5f8083a690ca4f533559e6ef3cbc6fbc13d6/authz/openfga/v4.15/schema.json).
Lakekeeper is a public Rust Apache Iceberg catalog. This lab implements a
subset of its model in a separate application with synthetic accounts and
catalog metadata; its legacy SQL contract is not Lakekeeper's database schema.
See [`fga/provenance.json`](fga/provenance.json), the upstream Apache-2.0 LICENSE
and NOTICE, and the byte-level provenance test.

## Run the live lab

```sh
export POSTGRES_PASSWORD="$(openssl rand -hex 24)"
export DEMO_PASSWORD="$(openssl rand -hex 24)"
docker compose -p authzlab-catalog up -d --build app
```

Open `http://127.0.0.1:3000` with the configured password. Preserve both environment
values for subsequent Compose commands; regenerating them does not rotate an
existing database password. Use a separate project and preserve old volumes
when upgrading from the former domain. Existing migration files are unchanged,
and the model fingerprint prevents implicit reuse of an incompatible store.

| Actor | Initial synthetic access |
| --- | --- |
| Alice | Project administrator and owner of warehouse, namespaces and roles |
| Bob | Nested role-based read, table creation, owner of `errors` |
| Carol | Owner of `requests` |
| Dave | Direct `select` on `latency` |
| Eve | Only the other project, with a same-ID table in another warehouse |

The live console performs actual authenticated requests for Check, grants,
revocations, role assignment, ownership transfer and namespace moves. No
permissions are simulated in the UI. The static Pages site only inspects the
upstream model; its WASM component checks the Rust lifecycle kernel.

## Security and migration

Every allow needs a fresh successful Check with higher consistency and a live
configured store. Both are checked because a soft-deleted store can still answer
Check. Only denials are cached. Any uncertain mutation leaves a durable, replica-wide fence
that blocks authorization and readiness across process restarts. Unknown
fields/actions, ambiguous authorization headers, foreign-scope objects,
malformed responses, redirects and incomplete writes do not produce an allow.

The service repeats authorization under project and object locks. Ownership
and hierarchy replacements persist an intent and use a single mixed OpenFGA
Write. Lists have bounded work and opaque actor/scope-bound expiring cursors.

`lab-tool bootstrap` provisions an empty store and model. The separate
`migrate-authorization` command translates existing SQL roles, grants, owners
and parent links, validates the source, checkpoints idempotent batches and
compares real decisions with a legacy oracle before activating the service.
Compose explicitly opts into synthetic seeding and this real migration;
production startup does neither by default. Active reruns never replay revoked
legacy permissions. A batch is marked before it is sent; resuming an uncertain
attempt preserves a durable fence even after graph verification and activation.

- [Migration contract and operation](docs/migration.md)
- [Application security practices and remaining limits](docs/appsec.md)
- [Architecture and consistency](docs/architecture.md)
- [Verification scenarios and boundaries](docs/verification.md)
- [Delivery](docs/delivery.md), [private browser lab](docs/live-lab.md), [performance](docs/performance.md)

## Independent Openlane scenario

The organization/task model, owner/assignee/assigner rules, inherited access,
blocked behavior and ordered Check/ListObjects cases from PR #26 live under
[`scenarios/openlane`](scenarios/openlane/README.md). Run
`bash scripts/test-openlane-model.sh` to test both adapters against fresh native
OpenFGA stores. This keeps its policy semantics independently reviewable without
changing the catalog API, SQL permission mapper or configured store. Existing
Openlane databases and model bindings are rejected by catalog startup/cutover;
keep them intact and start the catalog with its own database and store.

## Test

```sh
cargo nextest run --locked --all-features --profile ci
cargo nextest run --locked --release --profile ci \
  --no-default-features --features http \
  --test system --test cache_replicas --test authorization_migration \
  --test compose_deployment --test performance \
  --run-ignored only -E 'not test(paired_aa_calibration)' --success-output immediate
```

Repeat the integration command with `--features grpc`. CI also runs Playwright
against real Axum/PostgreSQL/OpenFGA, including escalation attempts, role and
owner changes, revocation, foreign-scope access, stale browser responses and
untrusted metadata. Docker-ignored tests must be selected explicitly; a unit
run alone is not end-to-end evidence. Native coverage includes the Axum child processes and is an independent CI gate;
see [coverage and its limits](docs/native-coverage.md). The separate Cargo browser
package, documented public domain API, doctests and WASM checks remain gated.
Kani covers the finite lifecycle kernel,
not distributed transactions.

HTTP (`reqwest` + `serde`, port 8080) is the default. The optional gRPC adapter
uses 8081 and wins when both features are enabled. Bootstrap and migration use
HTTP with either runtime transport. A gRPC Compose build needs both
`FGA_TRANSPORT=grpc` and `FGA_APP_URL=http://openfga:8081`.

The repository is an isolated engineering lab, not a certified production IAM
system. OpenFGA and PostgreSQL are separate transactional systems; external
writers, hostile infrastructure and unquiesced rollback are outside the
protocol. Security-sensitive changes go through a working branch and PR.

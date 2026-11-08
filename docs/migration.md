# Migrating existing SQL authorization to OpenFGA

The real source is relational data, not a tuple JSON fixture. The lab's source
contract intentionally differs from Lakekeeper's storage schema: only the
unchanged public authorization model is reused.

## Source contract

| Source | Meaning and validation |
| --- | --- |
| `users`, `catalog_memberships` | Known users and project membership |
| `catalog_projects` | Tenant boundary |
| `catalog_warehouses`, `catalog_namespaces` | Owners and parent hierarchy |
| `catalog_tables` | Owner, warehouse/namespace, active lifecycle |
| `catalog_roles` | Project-scoped role and owner |
| `legacy_project_grants` | Subject type/ID and supported project privilege |
| `legacy_resource_grants` | Direct privileges on existing scoped objects |
| `legacy_role_assignments` | User or nested-role membership |

IDs are validated before constructing OpenFGA strings. Owners and role subjects
must belong to the object's project. Unknown objects/users, conflicting scopes,
unsupported relations, cyclic role membership, unresolved lifecycle records and
oversized sources are rejected before writing any tuple. The source supports a
documented flat namespace hierarchy under a warehouse; arbitrary recursive
namespace imports need an explicit extension and equivalence tests.

The generated graph includes project relationships, ownership, role assignees,
resource grants, parent and reverse child links. Synthetic examples include a
nested role and the same table ID in two different warehouses.

## Provision, load and cut over

Prepare the target store/model using a dedicated, empty store. Configuration is
published atomically and later reuse verifies the same model. Normal server
startup applies schema migrations but does not seed or activate the catalog.
Populate the relational source according to the contract above before cutover.

```sh
export FGA_URL=http://127.0.0.1:8080
export FGA_CONFIG=/absolute/private/path/config.json
export FGA_MODEL=fga/model.json
cargo run --locked --bin lab-tool -- bootstrap

# DATABASE_URL must identify the catalog database, not the OpenFGA database.
export DATABASE_URL='postgres://USER:PASSWORD@HOST/authzlab'
export MIGRATIONS_DIR=migrations
cargo run --locked --bin lab-tool -- migrate-authorization
```

The CLI applies schema migrations, but never creates demo users or overwrites
production source data. Configure the application with the same persisted
config and database. `FGA_URL` is HTTP for this CLI even if the runtime adapter
uses gRPC. Demo Compose explicitly opts into both `DEMO_SEED` and `DEMO_MIGRATE`;
these are not default production behavior.

The migration:

1. Validates the pinned model bytes and the live store/model content.
2. Acquires the migration coordinator lock, snapshots/validates SQL under locks
   and binds the journal to source SHA-256 plus target store/model/model hash.
3. Freezes source edits using SQL triggers while cutover is incomplete. Legacy
   ACL tables remain immutable after binding so an active rerun cannot replay a
   modified snapshot.
4. Requires an empty initial target. On resume, accepts only tuples from the
   expected graph; unexpected additions cause a closed failure.
5. Writes batches of at most 100 tuples idempotently. Each acknowledged batch
   advances the durable checkpoint. A lost acknowledgement is retried without
   assuming the earlier write was absent.
6. Requires exact tuple-set equality. Compares every supported relevant action
   over known users plus an outsider against a separate Rust legacy evaluator.
   Both missing access and extra access fail the comparison.
7. Rechecks graph equality and source fingerprint before committing `active`.

The journal progresses through `pending → writing → verifying → active`.
The API and readiness remain 503 until activation matches the running target.
Failure leaves the journal closed and resumable. A retry in `verifying` verifies
again; it cannot activate using a partial earlier comparison. An already active
rerun validates the binding but does not replay old grants over current rights.

Limits are explicit: at most 10,000 source/tuple entries and 100,000 decision
comparisons. Exceeding a bound refuses migration rather than sampling or skipping
verification. This is a small migration demonstrator, not a bulk-scale importer.

## Operational boundaries

Quiesce source updates and administrative writes to the dedicated OpenFGA store
throughout cutover. Application traffic is gated, and SQL triggers protect the
supported source, but OpenFGA administration and PostgreSQL do not share a lock.
A hostile or uncoordinated external writer could race after the final graph
read. Store credentials/network policy must enforce the single-writer boundary.

An incompatible store/model, changed source or extra target tuple is an error,
not permission to delete state and start again. Preserve the journal and resolve
the mismatch explicitly. Back up the catalog DB, OpenFGA DB and model config
while writers are quiesced. Restoring only one database invalidates consistency.

Tests execute the real CLI, Axum, PostgreSQL and OpenFGA. They verify initial
closure, legacy/new permission equivalence, roles/owners/links, repeat-run
behavior, revocations after cutover, failures before activation and resumption
after losing a response and restarting the databases. See
[`tests/authorization_migration.rs`](../tests/authorization_migration.rs).

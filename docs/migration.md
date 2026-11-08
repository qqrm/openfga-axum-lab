# Migrating SQL authorization to the Openlane-derived model

The source is relational data, not a tuple JSON import. The lab adapts the
public Openlane organization/task authorization semantics; it does not claim
to use Openlane's database schema or its complete compliance domain.
[Model provenance and adaptations](openlane-model.md) identify the pinned source.

## Source contract

| Source | Meaning and validation |
| --- | --- |
| `users` | Known application users with valid identifiers |
| `organizations` | Organization and exactly one known owner who is a member |
| `organization_members` | Known users with the `member` or `admin` role |
| `organization_permissions` | Explicit `can_view_task`, `can_edit_task`, `can_delete_task` grants |
| `tasks` | Immutable organization and assigner, validated task/lifecycle state |
| `task_relations` | Known users with `viewer`, `editor`, `assignee` or `blocked` relationships |

IDs are validated before constructing graph strings. Unknown subjects, missing
parents, owners outside membership, unsupported relations, unresolved lifecycle
or authorization jobs, and oversized sources fail before tuple writes. External
users may receive explicit task grants; membership removal does not erase
independent task permissions. Those are intentional Openlane semantics.

The exact expected graph contains organization owner/member/admin edges,
explicit organization permissions, task assigner/parent_context and direct task
relationships. Five demo users, two organizations and four tasks are merely a
small initial SQL dataset; the importer derives the graph from the current SQL
source rather than hard-coding those accounts.

## Provision, load and cut over

Use a dedicated empty target store. Bootstrap publishes configuration atomically
and verifies the same pinned model on reuse. It imports no `legacy.json` tuples.
Normal server startup applies schema migrations without implicitly seeding or
activating authorization. Load the source according to the contract above.

```sh
export FGA_URL=http://127.0.0.1:8080
export FGA_CONFIG=/absolute/private/path/config.json
export FGA_MODEL=fga/model.json
cargo run --locked --bin lab-tool -- bootstrap

# The application database, distinct from OpenFGA's own database.
export DATABASE_URL='postgres://USER:PASSWORD@HOST/authzlab'
export MIGRATIONS_DIR=migrations
cargo run --locked --bin lab-tool -- migrate-authorization
```

The CLI applies schema migrations but does not create demo users or overwrite
source data. The application must use the same persisted config/database.
`FGA_URL` is HTTP for bootstrap/migration even when the runtime uses gRPC;
`FGA_HTTP_URL` supplies the migration/model-verification endpoint at startup.
Demo Compose explicitly enables `DEMO_SEED` and `DEMO_MIGRATE`. `AUTHZ_MODE=deny`
skips optional OpenFGA migration/recovery and does not become ready.

The migration preserves these gates from the previous implementation on main:

1. Validate the pinned model bytes and live store/model contents.
2. Lock, snapshot and validate SQL; bind the migration journal to the source
   fingerprint and the target store, model and model hash.
3. Freeze supported source edits while cutover is incomplete using SQL triggers.
4. Require an empty initial target. A resume accepts only tuples from the
   expected graph; additional target authority is an error.
5. Commit the remote-attempt marker before each batch of at most 100 tuples.
   Advance the checkpoint only after a complete valid acknowledgement.
6. Compare exact tuple sets and every supported relevant action for each known
   user plus an outsider against an independent Rust evaluator of the SQL source.
   Both missing access and extra access prevent activation.
7. Recheck the graph and source fingerprint before committing `active`.

The Openlane journal is separate from the retained historical catalog journal.
It progresses through `pending → writing → verifying → active`. The API requires
an active binding matching the running target. Readiness also requires a live
backend/model and zero durable pending debt. An active rerun checks the binding
without replaying the old source graph over grants changed after cutover.

A replayed attempted batch may still have an older request buffered in a proxy.
It therefore adds durable global pending debt before retrying. Graph verification
may complete, but **activation alone does not reopen access**: reads, mutations
and readiness remain closed until the explicit backend-drained recovery procedure
in [the transition guide](openlane-migration.md). A restart or successful Check
is not proof that the old request cannot arrive later.

Bounds are explicit: at most 10,000 source/tuple entries and 100,000 decision
comparisons. Exceeding them refuses migration rather than sampling verification.
The API's smaller 256-candidate enumeration budget is a separate runtime limit.

## Existing installations and operational boundaries

Historical shelter/catalog SQL migrations, records and license material remain
intact. The new model fingerprint deliberately rejects their old configuration.
No catalog objects are guessed to be tasks, no store is reset, and no old database
role is renamed automatically. Existing Lakekeeper data or a bound catalog
migration is rejected by demo setup/import; use a separate Openlane database
and store, or supply a separately reviewed source mapping and cutover plan.

Quiesce source updates and administrative writes to the dedicated OpenFGA store
throughout cutover. SQL triggers protect the supported source, but PostgreSQL and
OpenFGA administration do not share a lock. Restrict direct tuple writers so they
cannot race after verification. Back up the application DB, OpenFGA DB and model
config together with writers stopped; restoring only one invalidates consistency.

An incompatible binding, changed source or extra target tuple is a closed error,
not permission to erase state. Preserve the journal and investigate the mismatch.
The real [migration suite](../tests/authorization_migration.rs) runs the CLI,
Axum, PostgreSQL and OpenFGA to cover initial closure, exact graph and positive/
negative decisions, source validation/freezing, active reruns after revocation,
lost-response restart recovery and target substitution. Wire tests separately
exercise malformed acknowledgements and protocol responses.

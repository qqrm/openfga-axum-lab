# Architecture and consistency

`domain.rs` contains typed IDs, scope, subjects, actions, privileges and the pure
lifecycle. Serde implementations live at the application boundary so the domain
also compiles directly to WASM and under bounded Kani checks. Axum maps explicit
routes to application operations. `use_cases.rs` owns SQL scope checks, locks,
lifecycle transitions and OpenFGA mutations; the `Authorizer` trait isolates the
HTTP and gRPC adapters. SQLx is used directly because transaction boundaries
are part of the authorization contract.

## Authority and routing

A bearer session resolves a server-side user ID. Client JSON never supplies the
acting principal. `ProjectId`, `WarehouseId`, `TableId`, `NamespaceId`, `RoleId`
and `UserId` have separate types and a restricted grammar. Table objects are
`lakekeeper_table:{warehouse}/{id}`. SQL lookups also require project and
warehouse, so an authorized object in one tenant cannot authorize the same ID
in another. Role subjects serialize only as `role:{id}#assignee` after membership
and project checks.

The router uses a closed mapping. Unknown routes/methods cannot skip a guard.
Single-table reads run the actual service check before returning metadata;
mutations recheck inside the service. All app-owned security mutations first
lock the project row, then the table where applicable. Reads take a project
shared lock. This intentionally serializes project security writes in a small
lab; it is not a claim of high write throughput or coordination with arbitrary
external OpenFGA writers.

## Lifecycle and uncertainty

Only `active` rows are visible. Creation requires an actor/scope-bound
idempotency key; a different payload conflicts, and a deleted result remains a
410 tombstone. The original creator is preserved when ownership changes.
Creation commits `pending`, provisions ownership/parent/child tuples, then
activates the row. Deletion commits `deleting`, removes direct tuples and the
namespace backlink, then marks `deleted`. IDs are not reused.

Ownership transfer and namespace move commit `updating` with an exact durable
write/delete intent before touching OpenFGA. Replacement uses one mixed Write,
not separate grant and revoke calls. The row stays hidden until OpenFGA confirms
success and SQL metadata is committed. Startup replays unresolved intents
idempotently only after the target-bound migration gate is active.

These lifecycle records are separate from the durable authorization fence.
Replaying an intent does **not** erase an older uncertain mutation marker.

## Denial cache and replica fence

Moka stores only denials (capacity 10,000, TTL two seconds). Every allow requires
a fresh Check, bounded to two seconds and requesting `HIGHER_CONSISTENCY`.
Readiness or GetStore is never substituted for a Check. Errors are not cached.

Each decision reads a revision from the same PostgreSQL primary. Before a
mutation, the wrapper commits a revision increment and a pending count; after a
confirmed success it increments the revision again and clears only that
operation's marker. A nonzero pending count rejects all decisions and readiness
before using a cached denial or calling OpenFGA. This is intentionally global:
a role change can affect descendants across a graph.

After a Check, the wrapper reads the revision/fence again. A change discards
the response and retries, up to a finite bound. Thus a delayed pre-revoke allow
cannot become a new post-revoke cached permission. A request whose authorization
already completed may still finish; revocation cannot recall previously sent
bytes or cancel arbitrary in-flight work.

If a write times out, returns an unexpected response or is cancelled, the
marker persists across replica and process restarts. Subsequent successful
writes cannot clear it. Operator recovery requires stopping all writers,
draining/stopping remote writes, reconciling the complete authoritative graph
and SQL intents, then explicitly resetting the fence and advancing the revision
while writers are still stopped. Blindly setting `pending=0`, restarting, or
re-running bootstrap is not recovery. The repository does not automate that
unsafe shortcut.

## HTTP boundary

The HTTP adapter disables redirects and validates the final endpoint, exact
operation status, JSON content type, response size and full-body deadline.
Check uses a strict bool DTO and rejects duplicate/unknown decision fields.
Write requires the protocol's acknowledgement object. Tuple deletion rereads
the first page after each removal and rejects an empty nonterminal page.
Provisioning and migration apply corresponding strict protocol validation.

Configuration binds store ID, model ID and SHA-256 to the compiled model.
Migration activation binds those values and the source snapshot in PostgreSQL.
Both runtime requests and readiness enforce the binding.

## Lists and directory

Table pages accept limits 1–100 (default 50), scan at most 500 candidates in
batches of 100 and issue at most eight concurrent checks. A failed check fails
the whole page, never exposing the unverified remainder. Cursors are random
stored capabilities bound to actor/project/warehouse, expire after 15 minutes,
and do not encode a hidden object's ID. Pages reauthorize on every use.
The directory itself filters warehouses, namespaces and roles using their
upstream actions; membership enumeration needs assignment-reading permission.
Directory work is capped rather than allowing an unbounded graph traversal.

## Authentication, browser and deployment

Passwords use Argon2id. Random 12-hour bearer tokens are stored as SHA-256 hashes;
logout revokes them. Password verification uses bounded blocking workers and
per-username database throttling. Ambiguous Authorization headers are rejected.
The browser stores a token only in memory, renders metadata as text and discards
responses from an older session or scope. Responses use no-store, nosniff,
no-referrer and a restrictive self-hosted CSP.

Local Compose explicitly enables synthetic seeding and migration. Production
startup leaves an unactivated catalog closed. Internal plaintext networking,
demo passwords, broad lab database privileges and local-only OpenFGA access are
lab boundaries. Public deployment requires real identity lifecycle, service
credentials, TLS, scoped accounts, operational session cleanup, abuse controls,
backup/restore policy and an independent threat-model review.

PostgreSQL and OpenFGA do not share a transaction. The invariants above provide
conservative failure behavior under app-controlled writes, not a distributed
transaction or a proof against hostile administrators, direct graph mutations,
DB rollback or all possible interleavings.

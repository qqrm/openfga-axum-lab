# Architecture and consistency

The functional domain in `src/domain.rs` describes valid pet identifiers,
inputs and lifecycle transitions. `use_cases.rs` coordinates operations,
`authentication.rs` handles session use cases, `infrastructure.rs` adapts
OpenFGA and bootstrapping, `http.rs` maps Axum requests and errors, and
`main.rs` composes them. This is intentionally small: application use cases
use SQLx directly because PostgreSQL row locks and transaction boundaries are
part of this implementation. It is not a database-independent domain layer.

## Invariants

- Only active pets may be returned. Unknown HTTP method/route mappings fail
  closed. OpenFGA errors return 503 rather than permitting access.
- POST requires a key scoped to the owner. The same key and payload return the
  same ID; the same key with different content returns 409. A deleted result
  returns 410. The record of the key remains with the tombstone.
- Create writes a pending database row, serializes on its row lock, writes the
  owner and shelter tuples idempotently, and activates the row. A crash leaves
  pending state which startup recovery processes under the same row lock.
- Delete locks the active pet, rechecks management permission, persists a
  deleting tombstone, and then removes all direct tuples for the object. It
  becomes deleted only after cleanup. Startup retries unfinished deletion.
  The actor that began deletion can retry. IDs are not reused.
- Edit and relation changes lock the pet and repeat authorization while holding
  the lock, serializing these writes with deletion and each other through this
  application. External writers bypassing this contract are not supported.

The OpenFGA read/delete loop repeats the first page after each deletion; it
does not assume pagination is stable while deleting. It has a finite safety
bound. A network timeout has an unknown outcome; idempotent writes and
persistent lifecycle states permit retries rather than assuming success.

The DB and OpenFGA are separate transactional systems. These mechanisms narrow
failure cases, but they do not prove global serializability, protect against
direct external writes, or provide a distributed transaction. Permission checks
for ordinary reads can race with a concurrent revocation; the system does not
claim a stronger authorization consistency guarantee.

## Authentication and demo boundary

Passwords are Argon2id hashes; random opaque 12-hour tokens are stored as
SHA-256 hashes and revoked on logout. Password verification runs in the blocking
pool with bounded concurrent workers; per-username attempts are rate limited
in PostgreSQL. The browser keeps its token only in memory. Demo seeding is
explicitly enabled by the local Compose environment.

The Compose account, password, test users and unencrypted internal transport
are for an isolated local lab. A public service needs real identity lifecycle,
per-client and distributed abuse controls, operational session revocation and
cleanup, secret management, scoped database/service accounts, transport
security, backup/restore and independent security review.

## Authorization cache

Each replica caches successful allow and deny decisions with Moka (10,000
entries, two-second TTL). The key includes a generation read from the same
PostgreSQL primary by every replica. A permission change commits a generation
increase and a pending marker before sending the OpenFGA write. After OpenFGA
acknowledges success, a second commit advances the generation and removes that
operation's marker. While any marker remains, all replicas bypass their caches.
The generation applies to the whole graph, including inherited permissions.

Both cache hits and live checks re-read the generation before returning. A
change triggers a fresh check; repeated contention returns 503. If a cache
insert races with a revoke after the final read, it can only populate a key
for the old generation. After a successful revoke, a new check on any replica
cannot reuse a pre-revoke allow. A request authorized before the final commit
can still finish; revoking access does not cancel work already authorized.

Cached allows require a live OpenFGA readiness response. A health check is
cheaper in intent than evaluating the authorization graph, but still uses a
network request, and readiness cannot detect every individual Check failure.
Errors are not cached. Every decision also costs PostgreSQL reads. Measure
the tradeoff before claiming a latency gain. Cache metrics distinguish hits,
misses, retries and pending bypasses without recording subjects or objects.

If the response to a mutation is lost, the pending marker persists across
process restarts. Subsequent successes only clear their own markers. A
nonzero `authz_cache_state.pending` after mutation traffic is drained is an
operator signal: stop all writers and drain or stop OpenFGA before verifying
the authoritative relationships and reconciling any uncertain writes. Only
then, while writers are still stopped, clear the marker count and increment
the generation in one committed SQL update. Until recovery, requests perform
live checks. They can still see the authoritative result of a late remote
write; the cache does not provide distributed transactions.

All replicas must use the same durable PostgreSQL primary and the same
OpenFGA store/model. Upgrade all writers before enabling the cache on that
store. Direct OpenFGA writes and restores to older snapshots fall outside
the protocol; quiesce the service and clear process caches for maintenance.

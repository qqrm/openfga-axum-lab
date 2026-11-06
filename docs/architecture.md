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

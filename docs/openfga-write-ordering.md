# Why ambiguous writes keep authorization closed

An atomic OpenFGA Write and an idempotent retry do not establish ordering
between separate requests. A successful replay cannot prove that an older
request has stopped or will never arrive. The laboratory therefore preserves
a durable global authorization fence after replaying an attempted job.

## Verified upstream contract

These sources are pinned to OpenFGA **1.10.3**, commit
`293893b22a53d8a130a5f7c97cab4bc8e91bcaa4`, matching the Compose image:

- [Write command](https://github.com/openfga/openfga/blob/293893b22a53d8a130a5f7c97cab4bc8e91bcaa4/pkg/server/commands/write.go):
  the request specifies writes, deletes and behavior for existing/missing
  tuples. It has no operation epoch or compare-and-swap precondition against
  the application's ownership version.
- [SQL writer](https://github.com/openfga/openfga/blob/293893b22a53d8a130a5f7c97cab4bc8e91bcaa4/pkg/storage/sqlcommon/sqlcommon.go):
  one request uses a READ COMMITTED transaction, locks relevant existing tuple
  rows, then deletes, inserts and commits. The ignore options are based on
  current tuple existence; they do not remember earlier application operations.
- [PostgreSQL adapter](https://github.com/openfga/openfga/blob/293893b22a53d8a130a5f7c97cab4bc8e91bcaa4/pkg/storage/postgres/postgres.go),
  [timeout middleware](https://github.com/openfga/openfga/blob/293893b22a53d8a130a5f7c97cab4bc8e91bcaa4/pkg/middleware/timeout.go),
  and [context wrapper](https://github.com/openfga/openfga/blob/293893b22a53d8a130a5f7c97cab4bc8e91bcaa4/pkg/storage/storagewrappers/context.go):
  Write preserves the request context through `BeginTx` and `ExecContext`.
  The cancellation-stripping wrapper overrides tuple-read methods, not Write.
  This analysis does not claim PostgreSQL writes intentionally ignore request
  cancellation. Client cancellation nevertheless supplies no application-level
  certificate that every queued or forwarded request has been drained.

`HIGHER_CONSISTENCY` checks and a graph that currently matches desired state
cannot establish that certificate either.

## Real-server ordering witness

A disposable test used the official OpenFGA 1.10.3 binary with its memory
datastore and a local proxy that delayed forwarding the first request. The
client timed out; the proxy later delivered that already-accepted request.

| Event | OpenFGA owner tuples afterward |
| --- | --- |
| Initial state | Alice |
| A replay of Alice → Bob is acknowledged | Bob |
| A newer Bob → Carol transfer is acknowledged | Carol |
| The original delayed Alice → Bob request arrives | **Bob and Carol** |

The late request ignores the missing Alice tuple and inserts Bob again. It
never mentions Carol, so Carol remains an owner. Bob also regains inherited
task access. A PostgreSQL `owner_id` check on administrative endpoints alone
would not protect reads authorized through this stale inherited path.

The committed native regression
`native_idempotency_does_not_order_late_writes` in
[`tests/fga_model.rs`](../tests/fga_model.rs) verifies the same delivery order
against the real server, including inherited task visibility. It is a witness
to the upstream ordering boundary, not an assertion that the application should
permit the sequence. It does not simulate cancellation of a PostgreSQL
transaction. The actual delayed-proxy run is recorded separately in
`artifacts/openlane-validation/late-write-repro-results.json`.

## Application policy

Before polling an authorization mutation, `apply` durably marks its job as
attempted through a separate coordination pool. The main transaction retains
the organization row lock. Consequently, process cancellation or a lost
acknowledgement cannot erase the evidence that a remote write may exist.

Recovery can replay that same durable, idempotent operation. Once acknowledged,
the organization journal is finalized, but `authz_uncertain` and global cache
debt remain. Every check, list, mutation and readiness probe stays closed until
backend-drained recovery. Replaying an attempted job explicitly adds global
debt even if the original process died before the cache's own marker committed.
The same rule applies to a previously attempted SQL migration import batch.
This stronger policy preserves the global fail-closed behavior introduced on
main and prevents a later conflicting grant/revoke from crossing the fence.

A reconstructed legacy lifecycle job has no durable evidence that it was never
attempted. It must be treated conservatively as a replay as well.

Normal startup must not clear this quarantine merely because a replay succeeds,
enough time has elapsed, or current tuples match PostgreSQL. Release requires
the explicit operator procedure in
[`openlane-migration.md`](openlane-migration.md), including stopping all writers
and draining backend/proxy requests. That operational precondition cannot be
inferred from a local database flag.

Fully automatic recovery without that operational boundary would require a
larger protocol, such as immutable versioned authorization graphs. Versioning
only organization objects is insufficient if late writes can restore old
`parent_context` links on stable task objects. The quarantine policy keeps this
small laboratory's uncertainty explicit instead of claiming that ordinary
idempotency solves request ordering.

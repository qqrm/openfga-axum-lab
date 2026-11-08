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

The committed `native_idempotency_does_not_order_late_writes` test in
[`tests/fga_model.rs`](../tests/fga_model.rs) sends an explicit delivery sequence
to the official OpenFGA 1.10.3 binary with a memory datastore. It repeats the
sequence in fresh stores with the exact default Lakekeeper model and the
independent Openlane subset.

| Event | OpenFGA owner tuples afterward |
| --- | --- |
| Initial state | Alice |
| A replay of Alice → Bob is acknowledged | Bob |
| A newer Bob → Carol transfer is acknowledged | Carol |
| The original delayed Alice → Bob request arrives | **Bob and Carol** |

The late request ignores the missing Alice tuple and inserts Bob again. It
never mentions Carol, so Carol remains an owner. Under Lakekeeper, Bob regains
`ownership` and `can_read_data` on the table. Under Openlane, Bob regains
organization `owner` and inherited task `can_view`. All witness checks request
`HIGHER_CONSISTENCY`; the stale write has become actual current graph state.

This test controls request delivery order directly. It does not simulate
cancellation of a PostgreSQL transaction or exercise the application's recovery
fence. Pre-forward proxy and SQL integration tests must independently establish
that the service stays closed during that situation. The witness shows why
checking a local `owner_id`, retrying idempotently or observing the current graph
is insufficient to dismiss an outstanding remote write.

An initial SQL import has the same ordering risk: replaying its old ownership
tuple after a later successful owner transfer can recreate the previous owner.
Migration batches therefore need the same durable evidence of attempted writes
as normal catalog mutations.

## Application policy

Before sending a catalog authorization mutation or import batch, record durable
evidence that the request may reach OpenFGA. This evidence must survive rollback
or cancellation of the transaction that was waiting for the response. An
ambiguous write closes authorization across replicas, including checks, lists,
mutations and readiness.

Recovery may replay the original operation to reconcile local work, but a
successful replay cannot establish that no older request remains in a proxy,
queue or backend. The global uncertainty fence must therefore survive ordinary
startup and replay. A reconstructed legacy job has no evidence that it was never
attempted and requires the same conservative treatment.

Release requires an explicit operator boundary: stop all writers/recovery
workers and establish that old backend and forwarding requests can no longer
arrive, then reconcile outstanding work in isolation before reopening service.
Elapsed time, a healthy backend and equality of the current graph to SQL do not
prove this precondition. See the primary
[migration and recovery contract](migration.md) for the supported procedure.

This requirement is separate from the Openlane workload. That workload has no
application writer, SQL importer or recovery endpoint. Its purpose here is to
preserve an independently attributed policy example while sharing protocol
checks with the primary catalog.

Fully automatic recovery without a drain boundary would require a larger
protocol, such as immutable versioned authorization graphs. Versioning only a
parent object is insufficient if late writes can restore old parent links on
stable child objects. The laboratory keeps this uncertainty explicit rather
than treating ordinary Write idempotency as an ordering guarantee.

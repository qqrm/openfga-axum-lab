# Architecture and consistency

The functional core in `src/domain.rs` validates identifiers/task input and
lifecycle transitions. `use_cases.rs` owns tasks, `organizations.rs` owns
membership/ownership, and `authorization.rs` journals remote graph changes.
`authentication.rs` handles sessions, `infrastructure.rs` contains seeding and
the HTTP adapter, `infrastructure/openfga_grpc.rs` the gRPC adapter, `http/`
maps requests/errors, and `server.rs` composes the application. SQLx is used
directly: PostgreSQL locks and transaction boundaries are part of the contract.

The three-type authorization graph is a users-only subset of the real Openlane
organization/task model. See [source provenance and semantics](openlane-model.md).
A task has an immutable organization and assigner. Ownership belongs to the
organization. The graph does not invent a task owner or a universal deny policy.

## Invariants and ordering

- Only lifecycle `active` tasks are returned. Task business status (`todo`,
  `in_progress`, `done`) is separate. Unknown HTTP route/method mappings fail
  closed; authentication, database and OpenFGA errors never become an allow.
- Creation keys are scoped by organization and assigner. The same key/payload
  yields one ID; conflicting content yields 409; a deleted result yields 410.
  Tombstones keep their keys. A matching, already-authorized pending creation
  can resume its exact durable projection; a fresh request cannot bypass a fence.
- All mutations lock the organization first and the task second, then check
  OpenFGA permission while holding those locks. Organization role changes
  cannot interleave between a task mutation's check and staging/commit.
  This is deliberately coarse serialization for a small lab, not a throughput claim.
- Organization owner changes require both the current SQL owner and an OpenFGA
  `owner` check, an exact expected owner, and an existing member as the target.
  The replacement is one OpenFGA Write containing old-owner deletion and
  new-owner insertion. Generic member/grant routes cannot write `owner`.
- The current owner's membership cannot be removed. Changing/removing another
  membership affects only that role. Direct task grants, assigner rights and
  independent organization task permissions remain independent, as in Openlane.
- Task grant management is a documented local API policy requiring `can_delete`.
  Direct editors and assignees cannot delegate access. Only the four writable
  relations viewer/editor/assignee/blocked are accepted; parent/assigner/computed
  permissions cannot be written through this endpoint.

## Durable projection and recovery

A transaction stages the desired SQL rows, an `authorization_jobs` record and
`organizations.authz_pending=true`, incrementing `authorization_version`.
It commits **before** any remote write. An apply transaction locks that
organization. A separate coordinator connection first commits the job
`attempted` marker, then the complete idempotent OpenFGA change is sent and its
full response awaited. The separate pool prevents exhausted application-pool
connections from deadlocking this independent commit. Only then does it activate/delete the lifecycle row where applicable,
remove the journal, clear the fence, increment the version and commit together.

If the request is cancelled, the remote response is lost, or the final SQL commit
fails, the durable journal/fence survives. A pending organization's public task
and organization operations return 503, even when OpenFGA may already have
applied the change. Startup replays jobs before serving requests. Acknowledged
replay restores reads, but an already-attempted job sets a persistent
`authz_uncertain` write quarantine. Duplicate writes/missing deletes make that
one operation idempotent; they do not order a late original request against a
newer change. Every further mutation stays 503 until an explicit backend-drain
recovery. Responses disable mutation capabilities and expose `recovery_required`.
Historical pending/deleting lifecycle fixtures are also converted to projections.
Multiple orphan lifecycle rows in the same organization can stop ordinary
startup after the first replay establishes quarantine. The explicit drained
recovery command reconciles the remaining rows; ordinary startup does not
relax quarantine to improve availability. Normal API transactions stage the
journal and organization fence together and do not produce these orphans.

A creation inserts a pending task and projects assigner/parent_context tuples
before activation. Deletion first persists a deleting row with its actor, then
removes every direct tuple for the object, then finalizes a deleted tombstone.
The initiating actor can resume its pending deletion. Deleted IDs are not reused.
The first OpenFGA Read page is reread after each deletion batch; pagination is
not assumed stable during mutation. Cleanup has a finite safety bound.

A failed generic grant/role/owner mutation is replayed by restarting against
the same store/database, restoring read access but retaining write quarantine.
The explicit one-shot `app --recover-authorization-after-backend-drain` command
is required to restore writes after all application writers, OpenFGA and proxy
queues have been drained. It asserts that operator precondition rather than
proving it; no fixed sleep or successful replay is treated as a drain barrier.
Do not clear flags manually or re-seed. See the [recovery procedure](openlane-migration.md).
A new process refuses to listen if replay itself fails.

## Reads and concurrent revocation

Task lists obtain candidate object IDs through OpenFGA `ListObjects`, then
read active SQL rows and evaluate each returned task's view/edit/delete
capabilities. Candidate selection is inside the same organization revision
snapshot as the final response; an empty candidate list does not bypass the
final version/fence check.

Ordinary reads do not hold SQL locks over network calls, allowing a revoke to
complete while an old check response is delayed. They capture the organization's
version/fence with task data, evaluate permissions and revalidate the version
before returning. A changed version retries; a pending fence or repeated
contention returns 503. Task/organization lists also validate the complete
organization version snapshot, so they cannot combine pre/post-change access
from different checks. A list may return 503 when any scanned organization is
pending; a direct read of an unaffected organization's task remains available.

A request can finish after a revoke if it already passed the final revision
check before that revoke began. Revocation does not cancel authorized in-flight
responses. These protocols cover application writers sharing one PostgreSQL
primary and one immutable OpenFGA store/model; direct external graph/SQL writes,
manual ownership tuples, old-snapshot restores and multiple independent primaries
are outside the contract. This is a durable projection with an availability
fence, not a distributed transaction or an exhaustive concurrency proof.

## Authorization cache

Each replica uses Moka (10,000 entries, two-second TTL). Keys include a generation
from the shared PostgreSQL primary. Before a remote mutation, the cache coordinator
commits a generation increment and pending count; acknowledgement commits a second
increment and removes that operation's count. Both positive/negative hits and
live checks re-read the generation; changed generations retry. A delayed fill
can populate only its old generation's key. Graph-wide invalidation includes
organization permissions inherited by tasks.

Cached allows still require live backend/store availability. HTTP probes health
and the configured store; gRPC probes the store. Readiness additionally checks
the configured immutable model. A healthy server with a soft-deleted store
cannot validate a cached allow. Errors are never cached. This costs network and
SQL round trips; no latency improvement is claimed without measurement.

The cache pending counter is separate from the durable organization job. An
ambiguous remote response leaves its conservative cache marker across restarts;
a successfully replayed job clears the organization fence but not an older
uncertain cache count. Until reconciliation all decisions use live checks, and the organization
quarantine prohibits newer conflicting mutations. The explicit one-shot recovery
command clears both quarantine and cache debt only after pending jobs are
reconciled and the operator has confirmed backend/proxy drain. Organization
revisions and cache generation are incremented in the same committed reset.
Do not reset counters merely because an app restarted.

## Authentication and demo boundary

Passwords use Argon2id; random opaque twelve-hour sessions are stored as SHA-256
hashes and revoked on logout. Password work runs in the blocking pool with bounded
concurrency; per-username attempts are rate limited in PostgreSQL. Browser tokens
stay in memory, and account changes clear protected content immediately.

Seeding runs only with explicit DEMO_SEED and creates initial rows without
reinstating revoked membership/grant metadata on restart. Existing credentials,
owner transfers, role changes and lifecycle tombstones are preserved. Startup
and bootstrap both verify the model fingerprint before using the new scenario.
Applied migration files remain unchanged, and previous-domain data are retained
without old routes or implicit data conversion. Use an isolated new store for
this incompatible scenario change; see [migration guide](openlane-migration.md).

The local accounts, unencrypted private service transport and Compose setup are
an isolated engineering lab. Real deployments need an identity lifecycle,
transport security, abuse controls, secret management, operational revocation,
backup/restore and independent review appropriate to their environment.

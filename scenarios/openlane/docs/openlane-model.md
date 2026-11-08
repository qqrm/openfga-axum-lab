# Openlane organization and task authorization

This independent scenario uses a deliberately small, modified subset of the real
[Openlane core authorization model](https://github.com/theopenlane/core/tree/1c38df0a53530c8c39eb0784862b06eaa3e26f58/fga/model).
The source snapshot is **`1c38df0a53530c8c39eb0784862b06eaa3e26f58`** (observed on 2026-11-08).
The main application continues to use Lakekeeper. This scenario is not the
complete Openlane application and does not claim affiliation with Openlane. The sample users, organizations and task records are fictional local
fixtures; the authorization rules come from the public project.

## Source and license

| Upstream file at the pinned commit | Used here |
| --- | --- |
| [`base/organization.fga`](https://github.com/theopenlane/core/blob/1c38df0a53530c8c39eb0784862b06eaa3e26f58/fga/model/base/organization.fga) | Organization permissions and `full_access` |
| [`roles/roles.fga`](https://github.com/theopenlane/core/blob/1c38df0a53530c8c39eb0784862b06eaa3e26f58/fga/model/roles/roles.fga) | Organization owner, admin and member roles |
| [`generated/crud.fga`](https://github.com/theopenlane/core/blob/1c38df0a53530c8c39eb0784862b06eaa3e26f58/fga/model/generated/crud.fga) | Organization `can_*_task` permissions |
| [`base/task.fga`](https://github.com/theopenlane/core/blob/1c38df0a53530c8c39eb0784862b06eaa3e26f58/fga/model/base/task.fga) | Task assigner, assignee, direct grants, blocking and inheritance |
| [`tests/tests.yaml`](https://github.com/theopenlane/core/blob/1c38df0a53530c8c39eb0784862b06eaa3e26f58/fga/tests/tests.yaml) and [`tests/tuples/tasks.yaml`](https://github.com/theopenlane/core/blob/1c38df0a53530c8c39eb0784862b06eaa3e26f58/fga/tests/tuples/tasks.yaml) | Reference scenarios for assignment, explicit sharing and denial |
| [`fga.mod`](https://github.com/theopenlane/core/blob/1c38df0a53530c8c39eb0784862b06eaa3e26f58/fga/model/fga.mod) | Source modular schema version 1.2 |
| [`LICENSE`](https://github.com/theopenlane/core/blob/1c38df0a53530c8c39eb0784862b06eaa3e26f58/LICENSE) | Apache-2.0, Copyright 2026 theopenlane, Inc. |

The upstream license is included verbatim in
[LICENSES/Apache-2.0-Openlane.txt](../LICENSES/Apache-2.0-Openlane.txt).
[NOTICE](../NOTICE) records attribution and modifications. No upstream
`NOTICE` file was present at this snapshot. The modified readable source
[fga/model.fga](../fga/model.fga) carries the copyright, license and change
notice; the corresponding [fga/model.json](../fga/model.json) points to that
source through OpenFGA `source_info` metadata.

## Adaptation to this laboratory

Openlane composes a schema **1.2** model from multiple modules. This laboratory
uses the schema **1.1 JSON API representation**, obtained by flattening only the
organization and task relations below. Copying the original modules directly
would not produce a usable model here.

The subset has three types: `user`, `organization` and `task`. It omits
services, groups, specialized and super-admin roles, conditions, task templates,
organization hierarchy, audit logs, and all other compliance objects. It also
omits task-to-task parents. Every fixture task has one organization through `parent_context`. This supplies
a real inheritance path, but the model alone does not enforce one-parent or
one-owner cardinality. No permissions are added to organization
members or admins to compensate for the omitted roles.

The JSON model and DSL express the same reduced rules. In particular:

- `organization.full_access = owner`. Admin is not an owner.
- `organization.member` includes owners and admins, as well as direct members.
- `organization.can_edit` includes admins and owners.
- `organization.can_create_task` includes organization editors and holders of
  `can_edit_task`.
- Organization `can_view_task`, `can_edit_task` and `can_delete_task` allow
  direct user grants; higher task permissions include lower ones and owners
  hold all three.
- Task `parent_viewer`, `parent_editor` and `parent_deleter` follow those
  permissions through the task's `parent_context`.

A member can view their organization, but membership alone does **not** reveal
its tasks. An admin can create a task, but admin status alone does **not** reveal
or edit every task in the organization. In the fixture, Bob has a separate explicit
`can_view_task` grant so users can distinguish that grant from his admin role.

### Task permissions

| Effective role or relationship | View | Edit | Delete |
| --- | --- | --- | --- |
| Assigner (task creator) | Yes | Yes | Yes |
| Assignee | Yes | Yes | No |
| Direct editor | Yes | Yes | No |
| Direct viewer | Yes | No | No |
| Organization owner | Yes | Yes | Yes |
| Organization `can_view_task` grant | Yes | No | No |
| Organization `can_edit_task` or `can_delete_task` grant | Yes | Yes | Yes |
| Organization admin/member alone | No | No | No |
| Unrelated user without a grant | No | No | No |

This table assumes no `blocked` tuple and no additional independent grant.
An organization's `can_edit_task` granting task deletion may be surprising,
but it is upstream behavior: task `deleter` includes `parent_editor`.

### The exact meaning of blocked

The upstream precedence is deliberately preserved:

```text
can_view   = viewer but not blocked
can_edit   = (editor but not blocked) or can_delete
can_delete = assigner or parent_editor or parent_deleter
```

Consequently:

| Blocked user's other access | View | Edit | Delete |
| --- | --- | --- | --- |
| Viewer, editor or assignee only | No | No | No |
| Assigner or inherited deleter, including organization owner | No | Yes | Yes |

`blocked` is not a universal emergency revocation in this model. It denies
viewing and ordinary editing while retaining the privileged edit/delete path.
The ordered fixture exercises that distinction. A complete access
revocation requires removing **all** independent grants or roles that still
supply the relevant permission. Removing an explicit viewer grant does not
remove inherited organization access; removing an assignee tuple does not remove
an independent editor or assigner relationship.

## Model behavior and service boundaries

OpenFGA evaluates the tuple graph. It does not enforce a single owner, immutable
assigner, an existing-member transfer target, or authority to call the raw Write
API. A real service must enforce those invariants in its domain and mutation
protocol; the native scenario deliberately makes no task-service claim.

- Organization ownership is separate from task assignment. The model defines
  task `assigner` and `assignee`, with no `task.owner` relationship.
- The fixture transfers ownership using one atomic Write that deletes the old
  owner and adds the new one, then checks the remaining independent paths. That
  proves the graph rewrite's behavior, not authorization of an API caller.
- Derived relationships and invalid parent types are rejected by the server.
  This is schema validation, not proof that a generic tuple-write endpoint is
  safe to expose to application users.
- Cross-organization sharing is explicit: Dave's membership of the second
  organization grants nothing in Acme, but his direct viewer tuple on
  `vendor-review` grants access. Membership revocation alone cannot remove all
  such independent grants.
- A role removal, assignment change or explicit denial changes the relevant
  graph paths. Application cache coordination, SQL recovery and error handling
  are separately exercised by the primary catalog's tests.

## Demonstration fixtures

[fga/legacy.json](../fga/legacy.json) is a synthetic native-test seed. Its
historical filename is retained, but the application bootstrap and SQL importer
do not read it.

| Organization | Direct owner | Other role and permission tuples |
| --- | --- | --- |
| `acme` | Alice | Alice member; Bob admin plus `can_view_task`; Carol member |
| `other` | Erin | Erin member; Dave member |

| Task | Organization | Assigner | Additional direct grants |
| --- | --- | --- | --- |
| `access-review` | `acme` | Carol | — |
| `vendor-review` | `acme` | Alice | Dave viewer |
| `incident-response` | `acme` | Bob | Carol assignee |
| `other-review` | `other` | Erin | — |

The scenario has a different model fingerprint and object namespace from the
catalog. Tests install it into a fresh isolated store. There is no model switch
in the main application and no conversion of existing permissions. See
[the migration boundary](openlane-migration.md).

## Verification expectations

The permission tables describe expected behavior. The committed native fixture
checks successful atomic transfer/replay, role downgrade, assignment,
independent and inherited grant/revoke, cross-organization sharing, privileged
blocking and whole-request rejection of an invalid compound write. The fixture
does not claim API-level owner cardinality, last-owner protection or validated
transfer targets. It also does not prove SQL durability or failure recovery.

### Native model and adapter gate

`tests/fga_model.rs` at the repository root runs this explicitly selected model against an actual **OpenFGA
1.10.3** process without Docker. It creates an isolated in-memory datastore,
binds HTTP/gRPC to ephemeral loopback ports, seeds the model, and stops the
process on completion or failure. The ordered fixture in
`scenarios/openlane/tests/fixtures/openlane-model-cases.json` contains 272 expected decisions and
40 mutations after the initial seed, including three rejected invalid writes.
Each adapter decision is also compared with a direct HTTP Check. The DSL parity
test uses the official CLI to compare permission rewrites and direct subject
type restrictions; source-location metadata is intentionally not compared.

Run `bash scripts/test-openlane-model.sh` from the repository root. It uses the
pinned checksummed official binaries below (or explicitly supplied local paths),
then tests both transport builds.

The tests are explicitly ignored by default because they execute supplied
external binaries. They do not download tools automatically. With verified
native binaries already available, run:

```bash
OPENFGA_TEST_BINARY=/path/to/openfga FGA_CLI_TEST_BINARY=/path/to/fga \
  cargo test --locked --no-default-features --features http \
  --test fga_model --test http_adapter -- --include-ignored --nocapture

OPENFGA_TEST_BINARY=/path/to/openfga FGA_CLI_TEST_BINARY=/path/to/fga \
  cargo test --locked --no-default-features --features grpc \
  --test fga_model --test grpc_adapter -- --include-ignored --nocapture
```

The native test rejects a server version other than 1.10.3, matching the
repository's Compose pin. When both transport features are compiled, the
native matrix selects gRPC, so run the two feature configurations above to
exercise both adapters. The normal wire tests separately verify a single
atomic ownership Write, both idempotency options, no automatic mutation retry,
and fail-closed response deadlines. A separate raw-server ordering witness
shows why idempotent writes cannot replace application quarantine; see
[write ordering and recovery](../../../docs/openfga-write-ordering.md). The HTTP
wire tests also distinguish a healthy server from an unavailable configured store.

The runner downloads official release assets and verifies these archive
SHA-256 digests before extraction:

| Official artifact | Verified archive SHA-256 |
| --- | --- |
| [OpenFGA 1.10.3 linux amd64](https://github.com/openfga/openfga/releases/download/v1.10.3/openfga_1.10.3_linux_amd64.tar.gz), [manifest](https://github.com/openfga/openfga/releases/download/v1.10.3/checksums.txt) | `cbf0ddaddd9580dc0b9d6af70265853c53b4847ff1ac91a9fd06b97ed6f97efb` |
| [fga CLI 0.8.2 linux amd64](https://github.com/openfga/cli/releases/download/v0.8.2/fga_0.8.2_linux_amd64.tar.gz), [manifest](https://github.com/openfga/cli/releases/download/v0.8.2/checksums.txt) | `e48c4a73cf68031fe9fc0827f1462a9b2d6981e3d2e9b82f1e1c2e120c293443` |

The server reported build commit
`293893b22a53d8a130a5f7c97cab4bc8e91bcaa4`; the CLI reported
`f2015b8e6d55a7fbc77813791e21f8c7cded26f3`.

This finite gate verifies the fixture's expected behavior on the real server and
exercises the compiled HTTP/gRPC adapters. It does **not** cover PostgreSQL persistence,
Axum endpoint authorization, transaction recovery, cross-replica cache races,
Compose networking, or browser behavior. Those remain distinct integration and
end-to-end gates.

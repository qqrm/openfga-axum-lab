# Openlane organization and task authorization

This laboratory uses a deliberately small, modified subset of the real
[Openlane core authorization model](https://github.com/theopenlane/core/tree/1c38df0a53530c8c39eb0784862b06eaa3e26f58/fga/model).
The source snapshot is **`1c38df0a53530c8c39eb0784862b06eaa3e26f58`** (observed on 2026-11-08).
It is not the complete Openlane application and does not claim affiliation with
Openlane. The sample users, organizations and task records are fictional local
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
omits task-to-task parents. Every task has exactly one organization through
`parent_context`; this preserves a useful real inheritance path without
introducing another hierarchy. No permissions are added to organization
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
or edit every task in the organization. In the demo, Bob has a separate explicit
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
The UI and tests must describe and exercise that distinction. A complete access
revocation requires removing **all** independent grants or roles that still
supply the relevant permission. Removing an explicit viewer grant does not
remove inherited organization access; removing an assignee tuple does not remove
an independent editor or assigner relationship.

## Application invariants and endpoint policies

OpenFGA decides whether a tuple graph grants a permission; it does not by itself
enforce the application's ownership and consistency invariants.

- Organization ownership is separate from task assignment. Tasks use
  `assigner` and `assignee`; they do not acquire an invented `task.owner`
  relation. The creator is the immutable assigner.
- The organization has one direct owner. Only that owner may transfer ownership
  or change the organization's roles and task-wide grants. Admins cannot promote
  themselves or take ownership.
- An ownership transfer replaces the owner tuple atomically, targets an existing
  organization member, and preserves unrelated direct memberships and task
  grants. The old owner's continued access must be evaluated from those
  remaining relationships, rather than assumed to disappear.
- The laboratory authorizes task grant/revoke operations using `can_delete`.
  This is an explicit API policy layered on the imported model; Openlane's FGA
  task model does not define a separate `can_manage` permission.
- Grant mutations only accept the intended direct relationships, and must not
  let users write derived permissions or change `assigner` or
  `parent_context` through a generic tuple API.
- Cross-organization sharing is explicit: Dave's membership of the second
  organization grants nothing in Acme, but his direct viewer tuple on
  `vendor-review` does.
- A successful parent role/grant change invalidates cached child decisions.
  Ambiguous authorization writes, failed restoration, or an unavailable
  authorization service must not produce a successful unauthorized mutation.
  SQL/OpenFGA recovery and replica cache coordination remain service concerns.

## Demonstration fixtures

[fga/legacy.json](../fga/legacy.json) is the bootstrap seed filename retained for
compatibility with the existing loader. It contains only the new domain;
there is no old-model compatibility mode.

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

The bootstrap model hash changes with this migration. Existing persisted
OpenFGA configuration intentionally fails the model-hash check; it must not
silently accept the old graph or reset user data. Start a separate disposable
Compose project for this demonstration or follow an explicit migration plan.
Never remove existing volumes merely to make the new demo start.

## Verification expectations

The permission tables are the behavioral specification, not a statement that
tests have run. Integration tests must exercise the actual PostgreSQL/OpenFGA
stack, and the HTTP and gRPC adapters where supported. Cover successful and
rejected transfers, last-owner protection, roles, assignment, direct and
inherited grant/revoke, cross-organization isolation, and the privileged
`blocked` exception. Mutation failure, recovery and cross-replica warm-cache
tests remain necessary because a static model cannot prove those properties.
See the delivered verification report for the checks actually executed and
any environment limitations.

### Native model and adapter gate

`tests/fga_model.rs` can run the committed model against an actual **OpenFGA
1.10.3** process without Docker. It creates an isolated in-memory datastore,
binds HTTP/gRPC to ephemeral loopback ports, seeds the model, and stops the
process on completion or failure. The ordered fixture in
`tests/fixtures/openlane-model-cases.json` contains 272 expected decisions and
40 mutations after the initial seed, including three rejected invalid writes.
Each adapter decision is also compared with a direct HTTP Check. The DSL parity
test uses the official CLI to compare permission rewrites and direct subject
type restrictions; source-location metadata is intentionally not compared.

The CI Rust job runs `bash scripts/test-openlane-model.sh`, which uses the pinned
checksummed official binaries below (or explicitly supplied local paths), then
tests both transport builds.

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
[write ordering and recovery](openfga-write-ordering.md). The HTTP wire tests also distinguish a
healthy server from an unavailable configured store.

Tools used for the 2026-11-08 verification were downloaded from the official
release assets. Archive SHA-256 digests were checked against each release's
checksum manifest before extraction:

| Official artifact | Verified archive SHA-256 |
| --- | --- |
| [OpenFGA 1.10.3 linux amd64](https://github.com/openfga/openfga/releases/download/v1.10.3/openfga_1.10.3_linux_amd64.tar.gz), [manifest](https://github.com/openfga/openfga/releases/download/v1.10.3/checksums.txt) | `cbf0ddaddd9580dc0b9d6af70265853c53b4847ff1ac91a9fd06b97ed6f97efb` |
| [fga CLI 0.8.2 linux amd64](https://github.com/openfga/cli/releases/download/v0.8.2/fga_0.8.2_linux_amd64.tar.gz), [manifest](https://github.com/openfga/cli/releases/download/v0.8.2/checksums.txt) | `e48c4a73cf68031fe9fc0827f1462a9b2d6981e3d2e9b82f1e1c2e120c293443` |

The server reported build commit
`293893b22a53d8a130a5f7c97cab4bc8e91bcaa4`; the CLI reported
`f2015b8e6d55a7fbc77813791e21f8c7cded26f3`.

This gate proves the reduced model's behavior on the real server and exercises
the compiled HTTP/gRPC adapters. It does **not** cover PostgreSQL persistence,
Axum endpoint authorization, transaction recovery, cross-replica cache races,
Compose networking, or browser behavior. Those remain distinct integration and
end-to-end gates.

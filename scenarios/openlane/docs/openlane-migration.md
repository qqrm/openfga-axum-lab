# Openlane scenario isolation and migration boundary

The primary application uses the pinned, unchanged Lakekeeper catalog model.
The Openlane model lives under `scenarios/openlane` and runs only when its native
model tests explicitly select it. Neither its model nor its seed is loaded by
the catalog bootstrap or SQL permission migration.

## What the scenario owns

The runner starts a fresh OpenFGA process with an in-memory datastore, installs
the scenario model in a new store and inserts the synthetic tuples from
`fga/legacy.json` relative to the scenario root. It binds local ephemeral ports
and terminates that process afterward. It does not consume the normal app
configuration, connect to PostgreSQL, run SQL migrations or bind the catalog's
store/model ID. HTTP and gRPC builds run the same workload separately.

The model JSON, readable DSL, initial seed and ordered fixture are preserved.
Their upstream provenance and Apache-2.0 attribution stay alongside them. The
scenario demonstrates organization/task permissions; it provides no task API,
SQL schema conversion, browser application or alternate production mode.

## Existing data is not a model fixture

Catalog objects such as `project`, `namespace` and `lakekeeper_table` have no
lossless automatic mapping to Openlane `organization` and `task`. Table ownership
is also different from an organization owner or a task assigner. In particular,
explicit cross-organization task grants and the privileged `blocked` exception
must not be translated into catalog isolation or emergency-revocation guarantees.

An installation already bound to an Openlane store and SQL task data needs a
separately designed migration before it can run the catalog application. Keep
its original database, model configuration, authorization store and backups.
Changing the configured model ID, replacing a model fingerprint or dropping data
does not migrate permissions. There is no such conversion in this scenario.

Likewise, the catalog's existing SQL migration must continue to derive and verify
catalog grants using its own binding, frozen source and recovery protocol. The
scenario's `legacy.json` filename does not make it an input to that migration.
See the primary [migration contract](../../../docs/migration.md) for current
catalog operation and [write ordering](../../../docs/openfga-write-ordering.md)
for why an uncertain import cannot be made safe merely by replaying it.

## Evidence boundary

The original Openlane branch's integration/browser results remain evidence for
that branch and its former application. They do not certify this catalog
integration. The native scenario proves only the committed finite decision and
mutation matrix, DSL/JSON parity and shared adapter behavior against a real
OpenFGA memory server. The integrated catalog must pass its own PostgreSQL,
recovery, protocol and browser checks at the actual reviewed commit.

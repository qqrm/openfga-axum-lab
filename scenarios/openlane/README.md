# Independent Openlane model workload

This directory preserves the attributed, modified users-only Openlane
organization/task model and its ordered behavioral fixtures. It is an independent
workload for the shared HTTP and gRPC OpenFGA adapters. The application, catalog
API, SQL migration and browser demonstration use the unchanged Lakekeeper model
in [`../../fga/model.json`](../../fga/model.json).

## Run the workload

From the repository root:

```sh
bash scripts/test-openlane-model.sh
```

The script obtains the pinned, checksummed official OpenFGA 1.10.3 and fga CLI
0.8.2 Linux amd64 binaries when their paths are not supplied. To use binaries
already available on your platform:

```sh
OPENFGA_TEST_BINARY=/absolute/path/to/openfga \
FGA_CLI_TEST_BINARY=/absolute/path/to/fga \
  bash scripts/test-openlane-model.sh
```

The runner builds HTTP and gRPC separately. Each native test starts its own
OpenFGA process with a memory datastore and ephemeral loopback ports. It creates
a fresh store, loads this directory's model explicitly and never reads or
changes the application's configured store, SQL database or bootstrap config.
The tests are ignored by ordinary `cargo test`; this script enables them.

The workload retains 272 expected Check decisions, 40 ordered mutations
(including three rejected writes), six exact seed ListObjects results and 52
ListObjects membership comparisons during mutation. Each Check is also compared
with a direct HTTP Check. The official CLI transforms the readable DSL, and the
test compares the resulting relation rewrites and directly allowed subject types
with the scenario JSON. Shared wire-adapter tests run alongside the workload.

A separate real-server test in the same binary demonstrates a late stale owner
write against **both** this Openlane subset and the exact default Lakekeeper
model, in separate stores. See [the ordering boundary](../../docs/openfga-write-ordering.md).
These are native model/adapter checks; SQL persistence, catalog mutations,
recovery, HTTP endpoints and browser behavior have their own integration tests.

## Contents and boundaries

| File | Purpose |
| --- | --- |
| [`fga/model.fga`](fga/model.fga) | Readable adapted Openlane rules |
| [`fga/model.json`](fga/model.json) | Exact scenario model installed by the native tests |
| [`fga/provenance.json`](fga/provenance.json) | Pinned upstream revision, adaptation and model digest |
| [`fga/legacy.json`](fga/legacy.json) | Synthetic native-test seed; the historical filename is retained |
| [`tests/fixtures/openlane-model-cases.json`](tests/fixtures/openlane-model-cases.json) | Ordered decisions and graph mutations |
| [`docs/openlane-model.md`](docs/openlane-model.md) | Preserved semantics, including privileged `blocked` behavior |
| [`docs/openlane-migration.md`](docs/openlane-migration.md) | Isolation and migration boundary |
| [`NOTICE`](NOTICE) and [`LICENSES/Apache-2.0-Openlane.txt`](LICENSES/Apache-2.0-Openlane.txt) | Attribution and upstream license |

`source_info.file`, `license_path` and `notice_path` in scenario artifacts are
relative to this directory. The fixture and model bytes are preserved from the
Openlane implementation; moving them does not alter their permission rules.

This scenario supplies no task API, task UI, production server mode or automatic
catalog-to-task conversion. It does not install its model through application
bootstrap. Organization ownership and task assignment are useful independent
policy examples, but their semantics do not replace catalog ownership, projects,
roles or namespace hierarchy.

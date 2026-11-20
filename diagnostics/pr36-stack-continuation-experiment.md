# PR #36: full application with two OpenFGA datastores

Declared November 20, 2026, before the paired browser execution.

## Question and existing evidence

The real OpenFGA memory component passed two direct-ownership cycles in
Chromium. The previous full PostgreSQL-backed stack got farther than a simple
service-start failure: Axum's process started, but its first authorization
migration Write returned deadline Code(4004), HTTP 500, before HTTP readiness.
The observed request length was 3,419 bytes. The pinned synthetic seed and
migration implementation reconstruct a matching 40-tuple request; the actual
request body was not logged, so this is source-derived evidence.

The successful one-tuple component writes do not exercise that migration,
computed relations, SQL readiness or the application's full acceptance flow.
This experiment therefore preserves the original full application workload.

## Two sequential fresh VMs on one runner

1. **Memory variant:** OpenFGA's real memory datastore, while PostgreSQL still
   runs and serves the real Rust application's SQL data. Skip only the OpenFGA
   SQL migration command. Keep both existing database-creation commands.
2. **PostgreSQL variant:** the original OpenFGA PostgreSQL configuration and
   migration command, with the same observation additions as the memory case.

Each case uses a fresh Chromium VM with two guest CPUs and 512 MiB. The first
checker must exit and retain complete, valid source capture before restoring
the four mutable served files for the second case. No guest process, store,
SQL cluster or VM state is carried into the next VM. The host and verified
downloaded runtime bytes are shared.

The second case can follow either successful application acceptance or a
captured functional failure of the first case. Setup, instrumentation or
incomplete-capture failures do not authorize the second VM in this workflow.
There are no VM retries and no browser matrix in this experiment.

## Frozen inputs and changes

- Runtime source: `444b069daf16e9606a0bbf1ea0aea7f4d61b97d8`.
- Idle-worker GC emulator: `6f35f24b7ca345cf47c873becee6a41ef94e65a9`.
- Same original rootfs, Linux kernel, PostgreSQL, Rust/Axum, lab-tool, separately
  verified bootstrap, `.10` OpenFGA executable and pinned Lakekeeper model.
- All seventeen native producer inputs remain unchanged. No native or emulator
  rebuild is part of this comparison.
- Only served `entrypoint.sh`, `guest.sh` and the necessary startup/derived
  manifest hashes change. Exact source hashes, unique transformation anchors
  and inverse transformations check the intended shell changes.
- Both variants add concise stage and exit-reason observations. They do not
  print command arguments or credentials. Failure reason and guest time precede
  the existing bounded optional diagnostic dump.
- No additional model/tuple warmup request is inserted before application
  migration. No acceptance implementation, model semantics, application batch
  size, authorization deadline or service-readiness deadline changes.

The root filesystem and original executables are the same in both cases;
the memory variant changes the OpenFGA datastore, not the application's SQL
backend. Source and manifest metadata must say which variant actually ran.

## Acceptance and budgets

Use the exact production browser checker with only the previously reviewed
single-engine selection and explicit sandboxed system-Chrome launch changes.
The unchanged guest runs `/usr/local/bin/lab-tool acceptance`; the host requires
two distinct indexed executions of `403 -> 204 -> 200 -> 204 -> 403`, valid
readiness, service liveness, complete capture and no page error.

This exercises actual application migrations, model binding, computed
permissions, authentication, grants/revokes, cleanup and session handling.
An OpenFGA health response, listening port, single Check or `LAB|ready` marker
alone cannot pass application acceptance.

The job has a 65-minute outer limit. The first checker may launch only within
eight minutes of the actual Actions job start. Before the second checker,
at least 28 minutes must remain, allowing its 25-minute step and retention.
Each original checker retains its 20-minute first wait and three-minute
second wait. These are host bounds; the guest/application/RPC deadlines are
unchanged. Capture and browser teardown must finish before served bytes change.

## Interpretation fixed before execution

- Success in either variant demonstrates the two original application flows
  for that configuration in this Chromium execution.
- A memory success does not prove the original PostgreSQL-backed OpenFGA
  configuration works. A PostgreSQL success does not independently validate
  Firefox, the model-explorer build, sustained reliability or Pages publication.
- If both fail at the same migration Write, PostgreSQL storage for OpenFGA is
  not necessary for that failure in the observed memory case. Other load and
  scheduling effects remain possible.
- If only the PostgreSQL variant fails, its additional storage work becomes a
  narrower investigation target. This is not proof of a specific SQL defect.
- A failure before the target migration cannot be interpreted as a failure of
  that migration. Missing/corrupt capture is an instrumentation result.
- The fixed order and changing host/cache/load state prevent causal speed
  comparisons, even though both VMs use one runner. This is a functional
  investigation, not a benchmark or a reliability estimate.

Results, raw source capture and input identities will be retained separately.
The production PR branch, browser acceptance requirements and deployment are
not changed by this diagnostic workflow.

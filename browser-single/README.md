# Experimental single-container browser laboratory

This is **one Linux image** containing three **real** processes:

- PostgreSQL 17 (SQL source, sessions, and the OpenFGA datastore in separate databases)
- OpenFGA 1.10.3 (checksummed official release in the native image; the browser
  startup adaptation is documented below)
- The existing Rust/Axum application and `lab-tool`, built with the HTTP adapter

It does **not** contain Docker, a fake authorization engine, JavaScript
reimplementations, or an alternative FGA model. The pinned, unchanged Lakekeeper
model remains the default. It does not modify the normal Compose/Pages deployment.

## Current browser result

[Run 43476279151](https://github.com/qqrm/openfga-axum-lab/actions/runs/43476279151)
completed with **zero of two acceptance passes in both engines**. The new
VirtIO RNG configuration passed all source, kernel and runtime preparation
checks. Firefox's complete console records CRNG readiness before the guest
startup script; initdb and PostgreSQL subsequently complete. Its first real
bootstrap deny Check still exceeds OpenFGA's unchanged three-second deadline.
Chromium reaches the original application's migration, where a real Check
also exceeds that deadline.

[Fresh native verification of the structural-path candidate](https://github.com/qqrm/openfga-axum-lab/actions/runs/43478795296)
completed successfully, including the real native backend proof and two original
acceptance sequences. That candidate preserves real tuple reads and authorization
decisions; browser performance and acceptance remain unverified. The exact
sources, hashes, evidence and limitations are in
[the startup review](../docs/browser-startup-review.md). Pages remains gated on
two original acceptance sequences in each real browser.

## Native proof before emulation

Run these commands at the repository root on an x86_64 Docker host:

```sh
docker build --platform linux/amd64 -f browser-single/Dockerfile -t authz-browser-one:local .
docker run --rm -d --name authz-browser-one --shm-size=128m \
  -e BROWSER_LAB_EPHEMERAL=yes -p 127.0.0.1:3000:3000 authz-browser-one:local
docker exec authz-browser-one /usr/local/bin/lab-tool probe
docker exec -e SMOKE_ALLOW_MUTATIONS=true authz-browser-one /usr/local/bin/lab-tool acceptance
```

The entrypoint starts PostgreSQL first, migrates its OpenFGA database, starts
OpenFGA, verifies/binds the exact FGA model, seeds the catalog via actual SQL
migration, then starts Axum. Each process runs as the unprivileged `postgres`
OS user. PostgreSQL and OpenFGA restrict their TCP listeners to guest loopback;
OpenFGA uses PostgreSQL's private Unix socket at `/tmp/browser-lab`. Only Axum is
exposed on container port 3000. The native smoke test asserts actual access
denial, grant, allow, revocation and denial again.

For an interactive local session, read the generated password **locally** with
`docker exec authz-browser-one cat /tmp/browser-lab/demo-password`, open
`http://127.0.0.1:3000`, and log in as one of the demo identities.
Use `docker logs authz-browser-one` for startup diagnostics and
`docker rm -f authz-browser-one` to stop it.

## Size and conversion

`.github/workflows/browser-single.yml` builds and runs the actual one-image
stack on PRs affecting this experiment; its native smoke records the image
size. Conversion can be requested through *Browser single-image feasibility*
with `convert=true`; it also runs for PR updates from the dedicated
`experiment/browser-one-container` branch. The workflow only converts *after*
the native integration test succeeds. It checks the c2w v0.8.5 release
checksums, wraps the tested native image in `Dockerfile.wasm` to set the required
ephemeral-only boot flag, then calls
`c2w --to-js authz-browser-one:wasm browser-output/`.

The complete generated bundle is uploaded as `browser-one-converted` after
successful conversion and size reporting, without a size-based upload filter.
The former 250 MiB cutoff has been removed: the measured 633,649,004-byte
bundle must not be silently discarded. Size remains diagnostic information,
not a reason to skip saving the executable files. A missing bundle or upload
failure fails the job; the size report is still uploaded after an upload failure.
The executable artifact is retained for one day, and `browser-one-report` for
seven days. This does not bypass GitHub's own storage limits or quotas, and
never publishes a converted executable to Pages automatically.

The original converter kernel lacked System V IPC. The separately rebuilt,
checksummed kernel enables `SYSVIPC` and `IPC_NS`; an actual Chromium run reached
PostgreSQL startup. **Not yet established by that run:** whether all three daemons
can boot and complete a Check/Write within browser memory and CPU limits; how to bridge
browser Fetch requests into port 3000 without an external server; and whether
the result works in Firefox/Chrome. A successful `c2w` conversion alone is
**not** a successful browser deployment.

## Deliberate security and durability limits

- `BROWSER_LAB_EPHEMERAL=yes` must be set explicitly. The DB uses trust
  authentication on guest loopback and its private local Unix socket. Never connect this image to
  public networks or production data.
- Passwords are generated locally on startup, not baked into image layers;
  the generated demo password is stored in the unprivileged user's private
  temporary directory. The browser-side login bridge is **not implemented**.
- Data live under `/tmp` and are not backed up; restart/restore and
  multi-session isolation are **not guaranteed**. This is not a production image.
- The inherited PostgreSQL base image has its own declared volume. This
  experiment overrides `PGDATA` to a separate temporary location.
- Do not infer full-service verification from the existing scalar WASM demo on
  Pages; that demo checks lifecycle rules, not OpenFGA decisions.

Related prior experiment: [docs/browser-feasibility.md](../docs/browser-feasibility.md).

## Pages startup adaptation and acceptance gate

The Pages experiment retains the verified `99f00cc` source artifact.
Its filesystem still supplies PostgreSQL, Axum, `lab-tool`, and the pinned
Lakekeeper model. Changing this directory's Dockerfile alone cannot update that
artifact. `prepare-pages.sh` supplies exactly four verified startup files through
the converter's read-only host mount: the guest wrapper, current entrypoint,
adapted OpenFGA executable, and a separately built bootstrap executable.
The last executable performs initial model provisioning and the opt-in, real
empty-store deny probe described below. Probe and both acceptance executions
continue to use the original absolute `/usr/local/bin/lab-tool` from the
retained rootfs.

Before starting QEMU, the launcher checks all four payload sizes and SHA-256
digests and requires the reviewed bootstrap source, native-test result and
binary identity. The materializer also verifies the bootstrap source/test,
Dockerfile, Cargo lockfile and native-test-log hashes.
`runtime/startup-manifest.json` records these separate build identities; the
original source archive and application filesystem keep their existing identity.

The [QEMU adaptation](qemu/README.md) rebuilds only the emulator JS/WASM pair
with separate early eviction and total live-instance limits. It releases half
of a CPU's retained compiled instances at 2048, while an atomic reservation
limits live instances, including those awaiting browser GC, to 8192 for Firefox
and the original 15000 backstop for other engines, including Chromium. The
launcher configures one shared C value before main starts; the backend freezes
it at its first initialization. Invalid or late changes are rejected. The
returned value is recorded in VM evidence and each worker's diagnostics.
Earlier lower-cap Chromium runs blocked compilation for long intervals while
awaiting GC. Firefox previously reached OpenFGA with 8192, but the current
combination still needs acceptance. Early detachment remains at 2048 per CPU.
The derived data, loader,
arguments and separately reviewed kernel are checked before and after the
emulator overlay.
The derived manifest records this separate emulator build; it does not rewrite
the identity of the original source ZIP. This memory-budget candidate still
requires real browser acceptance.

The candidate kernel enables ACPI and `X86_PM_TIMER` while retaining SYSVIPC,
IPC namespaces and `HPET_TIMER`. Its build records the full resolved configuration
difference, including dependencies selected by ACPI. The launcher removes the
exact Linux `acpi=off` token before the init-argument separator so Linux can
discover the advertised ACPI timers. It retains two sockets with one CPU each,
512 MiB and the existing MTTCG configuration. No clocksource is forced.

A separate `acpi=noirq` experiment booted Linux but did not establish backend
readiness: [43458558432](https://github.com/qqrm/openfga-axum-lab/actions/runs/43458558432)
recorded zero acceptance passes in both engines, with their last confirmed
service stage at PostgreSQL initialization. That option is not included in
this source. See [the evidence and its limits](../docs/browser-startup-review.md).

The guest verifies `online=0-1` before starting services; requested QEMU arguments
alone do not prove that Linux discovered both CPUs.
Before downloading the VM, the launcher requires a valid browser report of at
least two logical CPUs and records that report in VM evidence. This pinned
backend sizes per-CPU vectors from that value, so a smaller report cannot
safely accommodate the two requested vCPUs.
The guest also verifies `Cpus_allowed_list=0-1` before starting any service.
Ordinary scheduling across both CPUs is preserved. The previous candidate
pinned all services and clients to CPU 0, but
[run 43358719645](https://github.com/qqrm/openfga-axum-lab/actions/runs/43358719645)
still failed in both browsers: Chromium's first tuple Write returned
`validation_error`, and Firefox ran out of memory compiling a WASM module during
PostgreSQL initialization. That pinning is removed. Successive CI runs are not
a controlled performance comparison.

The [OpenFGA adaptation](openfga/README.md) changes HTTP gateway startup
self-dial from three seconds to a bounded 60 seconds. A separate bounded
startup phase exercises the codecs and fully validates the exact pinned
model once under one total 60-second budget, before opening listeners.
The validity of that conditionless model body is retained. A real model
write compares every protobuf field, including unknown fields, except the
generated ID; a match reuses the already established schema validity and still
writes PostgreSQL. In browser build `.8`, one fully validated TypeSystem is
retained privately for a single matching resolver cache miss. The resolver must
first obtain the actual model through its existing datastore accessor; the
claim requires nonempty store/resolved model IDs, equality of the resolved ID
with the returned model's ID, and the complete pinned body.
An atomic claim gives one caller exclusive ownership, and the real model ID
replaces the warmup ID before the object can escape. Cancellation discards a
claimed object. Other callers retain the existing validation/construction path;
no graph is shared between separately constructed TypeSystems. The upstream
store/model resolver cache remains. Its accessor can use an existing model
cache, so this does not promise a physical SQL read for every request.
Real tuple operations, datastore writes and Check decisions still run.
The successful startup also prepares a private JSON-to-protobuf decoder entry.
Only the exact pinned JSON body reuses an independent decoded request; the
original streaming parser and all mismatching inputs keep the default behavior.
The URL still supplies the real store ID, and the real gRPC/validation/auth
and PostgreSQL write path remains mandatory.

The `.8` HTTP encoder can retain one successful serialization, at most 1 MiB,
of a real `ReadAuthorizationModelResponse` for the pinned conditionless policy.
A hit requires full `proto.Equal` equality with the newly returned response,
including its actual model ID and unknown fields, and the exact intended
JSONPb options. The private deep key and private byte copies prevent aliasing.
The real RPC/model retrieval still runs; errors, other IDs, models, response
types and options keep the original path. Warmup's `NewEncoder` path cannot
populate this entry. Encoding duration and hit status are observed separately;
neither this reuse nor the TypeSystem transfer has established browser success.

Tuple Write now passes the TypeSystem already resolved and authorized for that
request into the write command. It reuses it only for the exact nonempty
store/model binding, preserving the schema-version guard and every tuple
validation and datastore write. Other bindings take the original read/build
path. This removes a second graph construction within the same request; it does
not cache Check results or raise request deadlines.

Compact HTTP and gRPC transport traces record request boundaries and timing
without reading request bodies or changing the handler's context, writer,
response or error. Fixed labels distinguish an expired/canceled RPC context and
the exact upstream resolver error that maps cancellation to `validation_error`;
other error messages are not logged. The browser build omits the upstream unary raw-payload
logger, which otherwise serializes payloads even when INFO is disabled.
The unused Prometheus exporter is disabled; authentication, validation,
authorization and timeout middleware remain in place.

PostgreSQL initialization and OpenFGA migration remain sequential. `initdb`
uses the same 60 connections and 16 MiB shared buffers as the running server;
its actual IPC probes and durability settings remain enabled. A bounded
180-second listener wait includes the additional OpenFGA startup phase and
schedules bootstrap. Only its real `SERVING` health, unchanged model and
successful operations permit Axum startup.

`BROWSER_BOOTSTRAP_MODEL_TIMEOUT_SECONDS` defaults to 5 and accepts only ASCII
integers from 5 through 30. Invalid settings fail before provisioning. The
browser wrapper opts in to 30 for its child entrypoint. Only the model POST and
model-verification GET use that budget, applied both to the individual reqwest
request and its enclosing Tokio timeout, including response-body consumption.
Health, store creation/verification and tuple reads retain five-second limits.
The application authorization client remains at two seconds, OpenFGA requests
at three seconds, and each acceptance request at five seconds; migration
behavior and its limits are unchanged.

The bootstrap still requires exact response status and URL, bounded JSON
responses, the pinned model and matching readback, and a confirmed empty new
store before publishing its configuration. An uncertain write is not retried.
A failed verification neither resets an existing binding nor publishes an
unverified one; atomic configuration publication remains unchanged.

The separate browser bootstrap also opts into
`BROWSER_BOOTSTRAP_EMPTY_STORE_CHECK=yes`. The only accepted explicit value is
`yes`; absence preserves the previous behavior and any other value fails
before network activity. On a newly provisioned binding only, after the actual
model readback and empty higher-consistency tuple Read, it makes one genuine
`HIGHER_CONSISTENCY` Check for `user:browser-bootstrap-probe`, relation
`ownership`, object `lakekeeper_table:browser-bootstrap-probe`, using the real
store and model IDs. HTTP 200 with a required Boolean `allowed:false` is the
only acceptable result. Failure, malformed data, `allowed:true` or timeout
prevents configuration publication. No tuple is written or response supplied
by the bootstrap; there is no retry. Existing bindings keep their previous
verification path.

This executes the real first model resolution and direct Check path before
Axum migration. It can populate OpenFGA's existing store/model resolver cache;
it does not cache an authorization answer or remove any migration or acceptance
check. Its client and outer body deadline stay at five seconds and the OpenFGA
server deadline stays at three. The ordinary repository CLI sources and the
archived probe/acceptance executable remain unchanged. Native success proves
these semantics, while its effect on browser startup requires the browser gate.

The guest's 900-second startup budget includes payload staging. On failure,
a bounded diagnostic snapshot records logs, memory and PostgreSQL process state
without reading command lines or environment variables.

The browser checker requires two sequential, indexed executions of the existing
native `lab-tool acceptance` in each Chromium and Firefox. Both executions must
return `403 → 204 → 200 → 204 → 403`. Duplicate, malformed or out-of-order serial
results cannot satisfy the gate. A service exit clears the live accepted state.
Uncaught browser/worker errors and unhandled rejections also clear readiness and
acceptance. The checker stops waiting when a page error or crash is observed.
Both browsers are attempted independently, including Firefox after a Chromium
failure. Any failed browser keeps the combined gate failed; neither a skipped
browser nor a single successful execution satisfies it.

The checker subscribes to each original serial/print log chunk before starting
the VM. One pump sends batches of up to 1024 original chunks; a JSONL batch
records its first source sequence, host-receipt time and every original text.
This reconstructs the exact ordered stream independently of the UI's
120000-character tail. A sequence gap, failed delivery or the host capture
limits of 16 MiB/1000000 source chunks prevent a passing gate. The browser queue,
including its in-flight batch, is bounded at 4 MiB/65536 chunks. A host timer
bounds cutoff and drain to ten seconds; a delivery failure retains the guest
snapshot taken at cutoff and still fails the gate.

The earlier two-CPU ACPI diagnostic,
[run 43424228941](https://github.com/qqrm/openfga-axum-lab/actions/runs/43424228941),
started PostgreSQL and OpenFGA in both Chromium and Firefox, then canceled the
initial model POST at the existing five-second client limit. Both browsers
completed **zero acceptance passes**. The diagnostic workflow's successful
completion records observations; it is not a passing backend acceptance gate.

The [data-only reader 43427570912](https://github.com/qqrm/openfga-axum-lab/actions/runs/43427570912)
verified both complete console byte streams against their captures. Chromium
reported the outer `Elapsed(())` timeout; Firefox reported reqwest `TimedOut`.
These observations establish the client boundary reached in that run, without
proving the cause of its elapsed time.

The [native proof 43433122340](https://github.com/qqrm/openfga-axum-lab/actions/runs/43433122340)
and the versioned [Node producer verification 43435460160](https://github.com/qqrm/openfga-axum-lab/actions/runs/43435460160)
both passed the 19 bootstrap Rust tests, default five-second failures while
genuine model responses remained withheld, opt-in 30-second provisioning with
six-second delays on both model operations, and two real native acceptance
sequences. The Node producer also passed all seven startup-payload tests and
the production payload validator. It exported the executable from that tested
image: 7415568 bytes, SHA-256
`d47425c10ebc1d2c1501e86dd7a1f3e8e5b6d2c9a6e4cf8123f20bd5d8c1866f`.
Its bytes match the bootstrap used by
[Chromium/Firefox run 43434436745](https://github.com/qqrm/openfga-axum-lab/actions/runs/43434436745).

That completed run used OpenFGA `.7`; **both browsers passed zero of the two
required acceptance executions**. Chromium completed model POST/GET and two
real Read calls, then failed the first tuple Write with RPC Code 2000,
`context_state=deadline` and `error_kind=model_resolution_deadline`. The RPC
observer reported about 3300 ms; HTTP reported 3814 ms. Firefox completed model
provisioning and started Axum's startup path, then the application's separate
model GET exceeded its unchanged five-second client budget: the
`ReadAuthorizationModel` RPC succeeded in 56 ms, while HTTP ended canceled
after 5644 ms. Both captures were complete and neither page recorded an error.
The interval after Firefox's RPC includes gateway protobuf decoding, JSON
encoding, writing and scheduling; these observations do not isolate one cause.
Guest-observer durations are not a controlled host-time comparison.

The `.8` candidate addresses the first resolver construction and repeated exact
model-response encoding without changing those deadlines. Its Go and race
checks passed in [43440821229](https://github.com/qqrm/openfga-axum-lab/actions/runs/43440821229).
That run then stopped before the native backend proof because the service
could not access the mounted OpenFGA executable. The producer now stages the
already verified executable in its own traversable directory and checks its
hash again, keeping the service user, startup guard and original input intact.

[Native verification 43445129977](https://github.com/qqrm/openfga-axum-lab/actions/runs/43445129977)
reused that exact Go-tested `.8` binary and passed seven payload tests,
19 Rust tests, both default-five-second causal failure cases, opt-in 30-second
provisioning with genuine six-second-delayed model responses, and two real
native acceptance sequences. A deliberately inaccessible input-directory
fixture remained unchanged while the staged copy worked. These native results
do not establish browser acceptance. The current PR's **Browser preview**
checks remain authoritative and must confirm two genuine consecutive
`403 → 204 → 200 → 204 → 403` executions in each engine before publication.
See the [retained timing and provenance](../docs/browser-startup-review.md).

The original full-log implementation failed the Chromium check in
[run 43373341199](https://github.com/qqrm/openfga-axum-lab/actions/runs/43373341199):
10558 of its 10567 retained chunks were single characters, and one serial host
call per character overflowed its 4096-chunk queue. Batching addresses that
observed transport defect. Firefox's complete 79419-byte capture reached Linux
boot and PostgreSQL initialization, then recorded a Module OOM. At the exact
throw, one worker alone had 8895 registered instances without a delivered GC
callback; early detach and GC were active. The Firefox-only 8192 cap prevents
that admission under the same history, but is not a native-memory byte bound
or a guarantee of successful startup. Neither browser completed acceptance.

Full capture was introduced after an observed evidence loss in
[run 43364708449](https://github.com/qqrm/openfga-axum-lab/actions/runs/43364708449):
both browsers timed out after 1200 seconds; Chromium had not emitted the guest
boot marker, and Firefox emitted it but never became ready. Firefox's UI tail
contained only emulator counters, overwriting the earlier service logs. Its
retained counters show continued registration, detachment and GC callbacks;
they do not establish absent GC, OOM, or the service phase at timeout.
The existing kernel `loglevel=0` token is replaced in place by `loglevel=7`,
before the init-argument separator, so ordinary boot diagnostics remain visible.
This log level is included in the observed VM configuration.

The current QEMU candidate also corrects eight operand-normalization calls in
the signed/unsigned 32-bit multiply emitters. Extracted upstream C emits real
Wasm for the regression tests: the original fails 42 of 96 arithmetic cases,
and the correction passes all 96, including signed negative operands, dirty
upper bits and register aliases. This separately demonstrated instruction
defect is not established as the cause of the preceding browser timeouts, and
the actual corrected build still encountered the Firefox OOM recorded above.
See the [emulator source and proof](qemu/README.md).

The compiled QEMU memory-barrier path also emitted a `nop` where its interpreter
used a full barrier. The correction emits Wasm `atomic.fence` and preserves the
interpreter path. Extracted C-to-Wasm tests and tiny actual Chromium/Firefox
fixtures verify the encoding and stack/shared-memory behavior. This separate
correctness repair can add execution cost; it does not establish the cause of
the model POST delay or satisfy browser acceptance.

The versioned `bootstrap/build.sh` consumes the already verified OpenFGA build,
builds one native service image from the fixed Rust source, and requires the
Rust tests, real delayed-response proof and two native acceptance sequences
before exporting its executable. It writes exactly four files under
`artifacts/browser-bootstrap`: `bootstrap`, `build.json`, `native-tests.log`
and `SOURCE.txt`. Source, fixture, tested image, executable, OpenFGA input and
proof hashes are recorded; no unverified executable fallback exists.

Prepare and check locally from this source revision on an x86_64 Docker/Buildx
host with the pinned Rust toolchain, Git, Node, ShellCheck and Playwright.
Use fresh worktree paths and build-output directories: the producer refuses
existing proof/output directories and existing containers with its reserved names.
The fixed source and packaging fixture must remain unmodified. Keep the nested
`browser-bootstrap-source` checkout until `prepare-pages.sh` finishes: the
materializer verifies the new overlay against those exact source/test bytes,
while separately requiring the ordinary repository CLI sources to remain at
their reviewed pre-probe identities.

```sh
git fetch origin 207d15fab685d3adb9c5d51a048982c2236ae75c 4b7a1e1933083232fb8305a60a47bdd3c32e6689
git worktree add --detach browser-bootstrap-source 207d15fab685d3adb9c5d51a048982c2236ae75c
git worktree add --detach ../openfga-bootstrap-fixture 4b7a1e1933083232fb8305a60a47bdd3c32e6689
```

Run the producer after OpenFGA and before runtime preparation:

```sh
bash scripts/build-browser-demo.sh
docker buildx build -f browser-single/OpenFGA.Dockerfile \
  --output type=local,dest=artifacts/browser-openfga .
bash browser-single/bootstrap/build.sh browser-bootstrap-source ../openfga-bootstrap-fixture
bash browser-single/qemu/build.sh
bash browser-single/prepare-kernel.sh
# Optional: set BROWSER_RUNTIME_ARCHIVE to a retained original ZIP.
# Its pinned checksum is still mandatory.
bash browser-single/prepare-pages.sh
npm --prefix e2e ci --ignore-scripts
(cd e2e && npx --no-install playwright install chromium firefox)
node browser-single/check-pages.mjs
```

`.github/workflows/pages.yml` (Browser preview) is the browser acceptance gate;
Browser lab verification exercises a native Dev Container instead. Browser
evidence and prepared OpenFGA, bootstrap and emulator build records must be
retained even on failure.
Pages deployment requires a separate manual run on `main` with `publish=true`,
and still depends on successful browser acceptance. Ordinary pushes, PR
validation and the default manual run do not publish Pages.

See [the review and observed limitations](../docs/browser-startup-review.md).

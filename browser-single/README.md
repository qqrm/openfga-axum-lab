# Experimental single-container browser laboratory

This is **one Linux image** containing three **real** processes:

- PostgreSQL 17 (SQL source, sessions, and the OpenFGA datastore in separate databases)
- OpenFGA 1.10.3 (checksummed official release in the native image; the browser
  startup adaptation is documented below)
- The existing Rust/Axum application and `lab-tool`, built with the HTTP adapter

It does **not** contain Docker, a fake authorization engine, JavaScript
reimplementations, or an alternative FGA model. The pinned, unchanged Lakekeeper
model remains the default. It does not modify the normal Compose/Pages deployment.

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
OS user. Both PostgreSQL and OpenFGA bind to guest loopback only. Only Axum is
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
  authentication *only on the guest loopback*. Never connect this image to
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
artifact. `prepare-pages.sh` supplies the current entrypoint and a separately
built OpenFGA executable through the converter's read-only host mount. The
launcher checks every payload's size and SHA-256 before starting QEMU;
`runtime/startup-manifest.json` identifies the source, patch, tests and binary.

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
The original data, loader,
arguments and reviewed SYSVIPC kernel are checked before and after that overlay.
The derived manifest records this separate emulator build; it does not rewrite
the identity of the original source ZIP. This memory-budget candidate still
requires real browser acceptance.

The launcher requests two sockets with one CPU each. With this kernel's ACPI
disabled, SeaBIOS's legacy MP table otherwise exposes only one CPU from a
two-CPU single socket. The guest verifies `online=0-1` before starting services;
requested QEMU arguments alone do not prove that Linux discovered both CPUs.
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
Only the validity of that conditionless model body is retained. A real model
write compares every protobuf field, including unknown fields, except the
generated ID; a match reuses the already established schema validity and still
writes PostgreSQL. On resolver loads, matching models get a fresh TypeSystem
with their real ID; graphs and mutable state are not shared by this cache.
Every other model uses the original complete validator. The original bootstrap,
datastore reads/writes, tuple operations and Check decisions still run. This
optimization remains a candidate until real browser acceptance passes.
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
schedules the existing bootstrap. Only its real `SERVING` health, unchanged
model and successful operations permit Axum startup. Application, bootstrap
and acceptance request deadlines are unchanged. The guest's 900-second
startup budget includes payload staging. On failure, a bounded diagnostic
snapshot records logs, memory and PostgreSQL process state without reading
command lines or environment variables.

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

Prepare and check locally (Docker, pinned Rust, Node and Playwright required):

```sh
bash scripts/build-browser-demo.sh
docker buildx build -f browser-single/OpenFGA.Dockerfile \
  --output type=local,dest=artifacts/browser-openfga .
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
evidence and both adapted binary/build records are retained even on failure.
Pages deployment requires a separate manual run on `main` with `publish=true`,
and still depends on successful browser acceptance. Ordinary pushes, PR
validation and the default manual run do not publish Pages.

See [the review and observed limitations](../docs/browser-startup-review.md).

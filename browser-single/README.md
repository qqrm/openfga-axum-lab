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

The Pages experiment retains the verified `99f00cc` emulator/rootfs artifact.
Its filesystem still supplies PostgreSQL, Axum, `lab-tool`, and the pinned
Lakekeeper model. Changing this directory's Dockerfile alone cannot update that
artifact. `prepare-pages.sh` supplies the current entrypoint and a separately
built OpenFGA executable through the converter's read-only host mount. The
launcher checks every payload's size and SHA-256 before starting QEMU;
`runtime/startup-manifest.json` identifies the source, patch, tests and binary.

The [OpenFGA adaptation](openfga/README.md) changes HTTP gateway startup
self-dial from three seconds to a bounded 60 seconds. Compact HTTP and gRPC
transport traces record request boundaries and timing without reading request
bodies or changing the handler's context, writer, response or error.
PostgreSQL initialization
and OpenFGA migration remain sequential. A lightweight, bounded listener wait
schedules the existing bootstrap, which still requires real `SERVING` health,
the unchanged model, and successful operations before Axum starts. Application,
bootstrap and acceptance request deadlines are unchanged. The guest's total
startup budget includes payload staging, and service phases are streamed to
the diagnostic console.

The browser checker requires two sequential, indexed executions of the existing
native `lab-tool acceptance` in each Chromium and Firefox. Both executions must
return `403 → 204 → 200 → 204 → 403`. Duplicate, malformed or out-of-order serial
results cannot satisfy the gate. A service exit clears the live accepted state.
Chromium failure stops further VM work and records Firefox as skipped; skipped
is never accepted as a passing browser result.

Prepare and check locally (Docker, pinned Rust, Node and Playwright required):

```sh
bash scripts/build-browser-demo.sh
docker buildx build -f browser-single/OpenFGA.Dockerfile \
  --output type=local,dest=artifacts/browser-openfga .
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
evidence and the adapted binary/build record are retained even on failure.
Pages deployment requires a separate manual run on `main` with `publish=true`,
and still depends on successful browser acceptance. Ordinary pushes, PR
validation and the default manual run do not publish Pages.

See [the review and observed limitations](../docs/browser-startup-review.md).

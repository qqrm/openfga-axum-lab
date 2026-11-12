# Browser execution findings — 2026-11-12

## Result

The existing artifact runs real QEMU/WASM and starts the Linux guest in Chromium.
It does **not** establish a working three-service browser application. A controlled
cold boot exposes a fatal PostgreSQL failure: the packaged kernel lacks System V
IPC, and PostgreSQL fails its `shmget` call. OpenFGA and Axum are not reached in
that run. Browser-to-Axum HTTP routing and authorization are not verified.

Both browser captures were performed locally without rebuilding or converting
an image, running Actions, or deploying the application. No native application
backend or static Lakekeeper authorization explorer was used.

## Artifact and source identity

- Repository: `qqrm/openfga-axum-lab`.
- Artifact source: `99f00cc170ca611ff4dc4c4ae3bbe98bb10cff58`.
- Inspected main: `95ae17f86bfc71695957dbfecfed29e8b6009888`; its tree has no
  differences from the artifact source tree.
- Workflow run: `43015497393`; artifact ID: `13854011214`.
- Original archive: `browser-one-converted.zip`, 226,186,469 bytes.
- Archive SHA-256:
  `b2f2b46ee95398a4f716c5f15561dd36176841ad91881f93bc5a95babf5e03bc`.
- Five uncompressed files total 633,997,052 bytes. They are `arg-module.js`,
  `load.js`, `out.js`, `qemu-system-x86_64.data`, and
  `qemu-system-x86_64.wasm`.
- All five hashes were rechecked after both runs and remained unchanged.
- The downloaded archive was preserved independently of the Actions retention.

The `.data` file includes the kernel, root filesystem and VM snapshot. The
package contains no HTML frontend or browser HTTP bridge. The small probe supplies
only a loader, serial terminal and a static file server.

## Two distinct real-browser runs

Both runs used Chrome Headless Shell **157.0.8103.78**, launched by Playwright.
The browser reported `crossOriginIsolated=true` and `SharedArrayBuffer` support.
The static server provided COOP `same-origin` and COEP `require-corp`.

| Run | Effective configuration | Observation | Capture result |
| --- | --- | --- | --- |
| Original | Unchanged arguments, migration snapshot, 128 MiB guest RAM | QEMU initialized; serial reached `[browser-single] initializing isolated PostgreSQL 17` | Capture timed out after 180.641 seconds; the cause of the silent period was not established |
| Cold diagnostic | Same five artifact files; removed `-incoming file:/pack/vm.state`; changed `-m 128M` to `-m 512M` | Linux booted, the original entrypoint ran, PostgreSQL `initdb` failed fatally | Failure appeared about 57 seconds after capture started; capture ended at 63.908 seconds |

The timings include browser setup, asset loading and compilation. They are
diagnostic observations from this environment, not performance benchmarks.

After capture, the runner's failure classification was tightened to require an
explicit fatal indicator, and uncaught browser errors were separated from a
guest timeout. The VM loading path and default arguments were preserved. Final
JavaScript/Python syntax, patch whitespace and the recorded fatal evidence were
checked; these small diagnostic changes were not followed by another VM run.

The subsequent CI repair replaces the original static server with a dependency-free
Node.js server and uses the runner's Node.js executable to launch it. HTTP and
runner checks cover the replacement with synthetic artifact files. The historical
captures and artifact hashes above are retained; this maintenance check is not a
new VM boot or authorization result.

The cold-run serial output contains:

```text
running bootstrap script ... 2026-11-12 05:01:43.770 UTC [61] FATAL:  could not create shared memory segment: Function not implemented
2026-11-12 05:01:43.770 UTC [61] DETAIL:  Failed system call was shmget(key=31, size=56, 03600).
child process exited with exit code 1
initdb: removing data directory "/tmp/browser-lab/postgres"
```

The reported allocation is **56 bytes**, not a large database buffer. Raising
guest RAM to 512 MiB did not supply the missing system call. The two modified
arguments mean this run must not be described as an unchanged-snapshot result.

## Independent kernel evidence

The extraction helper reads the `/pack/bzImage` offsets from `load.js`, extracts
the gzip-compressed ELF kernel and decodes its embedded IKCONFIG. It does not
infer the build configuration merely from upstream source.

```text
# CONFIG_SYSVIPC is not set
CONFIG_SHMEM=y
CONFIG_FUTEX=y
CONFIG_IKCONFIG=y
CONFIG_IKCONFIG_PROC=y
```

- Packaged kernel SHA-256:
  `9f5d77352db90112a0ea3851b004de7dd7ef459304f039bcc5c3271f76f12737`.
- Extracted configuration SHA-256:
  `e68b1d9d9bb4d75361d67e71debe930ecf1faf770d978a62ec6d3a765cc5dd6d`.

PostgreSQL 17 requires a small System V shared-memory control segment even when
the larger shared-memory region uses `mmap`. Its dynamic POSIX/mmap setting does
not eliminate that requirement. The actual missing kernel feature and the actual
browser `shmget` failure therefore agree.

Sources:

- [PostgreSQL 17 kernel resources](https://www.postgresql.org/docs/17/kernel-resources.html).
- [PostgreSQL shared-memory implementation](https://github.com/postgres/postgres/blob/REL_17_STABLE/src/backend/port/sysv_shmem.c).
- [Pinned c2w QEMU kernel configuration](https://github.com/container2wasm/container2wasm/blob/v0.8.5/config/qemu/linux_x86_config).

## Networking and authorization

Both browser runs attempted `ws://127.0.0.1:8888/` and received
`net::ERR_CONNECTION_REFUSED`. This matches the generated QEMU network arguments:

```text
-netdev socket,id=vmnic,connect=127.0.0.1:8888
-device virtio-net-pci,netdev=vmnic
```

The supplied package has no matching network service or browser Fetch-to-guest
port-3000 bridge. The probe does not add one. Loading the probe over host HTTP
is not routing HTTP to Axum. No HTTP authorization request was sent to the guest.

The unmodified entrypoint initializes PostgreSQL before migrating and starting
OpenFGA, then bootstraps the model before starting Axum. In the cold diagnostic,
PostgreSQL failed and the entrypoint exited before those later stages. This does
not establish any separate incompatibility in OpenFGA or Axum.

The acceptance sequence that remains unverified is the real
`403 → 204 → 200 → 204 → 403` flow from `tools/delivery.rs`, including bearer
sessions, a real OpenFGA grant and revoke, and the JSON body on the DELETE request.
A future bridge must preserve those methods, bodies, headers and status codes.

## Next repair to evaluate, not yet executed

Enable `CONFIG_SYSVIPC=y` in the pinned c2w x86 kernel configuration and build
that kernel. For the first experiment, the existing emulator WASM and rootfs can
be reused: load the replacement `bzImage` and cold boot without the old VM
snapshot. This avoids repeating the complete Rust/OpenFGA/PostgreSQL conversion
merely to test the missing kernel feature. The replacement-kernel path itself
has not been built or validated here.

Then verify PostgreSQL, OpenFGA and Axum readiness, and implement/test the browser
HTTP bridge and real authorization scenario. A successful kernel repair alone
will not prove the complete application. The adequacy of 128 MiB, the original
snapshot's delay, later application timeouts, Firefox compatibility and final
interactive UX remain unverified.

The Emscripten module separately reserves 3,000 MiB of shared linear memory;
that is not the guest's RAM budget or a measurement of physical memory use.

## Evidence and publication boundary

The reproduction archive supplied separately to the repository owner contains
the complete probe and the evidence paths below. These evidence directories and
the executable artifact were not included in the original seven-file probe change:

- `evidence/original/`: original serial, event log and structured result.
- `evidence/cold-512/`: cold-run serial, event log and structured result.
- `evidence/analysis/`: extraction helper, full kernel configuration and JSON.
- `evidence/verification.json`: hashes and exact argument-difference checks.
- `evidence/artifact-metadata.json`: retained GitHub artifact metadata.

Opening a PR that adds `browser-single/probe/**` automatically starts existing
CI, CodeQL, Delivery verification, Secret scan,
Workflow Security, Browser lab verification and the native single-image build.
A draft PR does not suppress them. A new branch other than
`experiment/browser-one-container` does not trigger WASM conversion, and these
paths do not trigger Pages deployment. The user authorized publication of the
probe and its automatic CI checks; a new conversion, kernel build or deployment
is outside this change.

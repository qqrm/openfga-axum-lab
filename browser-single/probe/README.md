# Browser VM probe

This local probe loads the **existing** `browser-one-converted` artifact in a
real Chromium browser. It does not build an image, convert an image, call Actions,
publish a site, or start a native application backend. The Python server serves
static files on loopback. PostgreSQL, OpenFGA and Axum can run only inside the
supplied QEMU/WASM guest.

## Artifact identity

- Repository: `qqrm/openfga-axum-lab`
- Workflow run: `43015497393`
- Artifact: `13854011214`, `browser-one-converted`
- Source commit: `99f00cc170ca611ff4dc4c4ae3bbe98bb10cff58`
- ZIP size: `226186469` bytes
- ZIP SHA-256:
  `b2f2b46ee95398a4f716c5f15561dd36176841ad91881f93bc5a95babf5e03bc`

The artifact's five extracted files have independent hashes in
`artifact.SHA256SUMS`. Keep the archive outside the repository; its original
Actions retention was one day. An expired download is not a reason to silently
rerun conversion.

## Reproduce manually

Extract the saved ZIP to a local directory, then verify its contents:

```sh
# Set both paths to your local checkout and extracted artifact.
repo=/path/to/openfga-axum-lab
artifact=/path/to/browser-one-converted
(cd "$artifact" && sha256sum -c "$repo/browser-single/probe/artifact.SHA256SUMS")
python3 "$repo/browser-single/probe/serve.py" --artifact "$artifact" --port 8765
```

Open <http://127.0.0.1:8765/> in Chromium. The server supplies
`Cross-Origin-Opener-Policy: same-origin` and
`Cross-Origin-Embedder-Policy: require-corp`, enabling the shared WebAssembly
memory required by this artifact. No CDN is needed. The page shows guest serial
output separately from browser/QEMU diagnostics and can download a JSON report.

The default URL preserves the artifact's original QEMU arguments, including its
128 MiB guest memory and migration snapshot. The Emscripten module separately
requests 3000 MiB of shared linear memory. These are different memory budgets;
neither the ZIP size nor the linear-memory reservation measures actual RAM use.

## Capture with Playwright

Use an existing local Node.js installation with `playwright` available and an
installed Chromium browser. If Playwright is in a separate module directory,
make it discoverable through `NODE_PATH`.

```sh
node "$repo/browser-single/probe/run.mjs" \
  --artifact "$artifact" \
  --chromium /usr/bin/chromium \
  --out /tmp/browser-vm-original \
  --timeout 180
```

`--artifact` starts the static server as a child process, checks its HTTP
readiness and isolation headers, runs Chromium in the same execution environment,
and stops the owned server on completion. The default port is assigned by the OS.
This is useful when separate tool commands have isolated network namespaces.
Use `--python /path/to/python3` or `--port 8765` when needed. To connect to an
already running manual server, replace `--artifact` with
`--url http://127.0.0.1:8765/`.

The runner writes `serial.log`, `events.jsonl`, and `result.json`. The JSON records
the browser version, original and effective arguments, isolation support, elapsed
time, guest serial output and browser errors. It waits for the time limit or
stops seven seconds after an observed terminal failure so the surrounding error
message can finish. Stopping capture does not make a guest result successful.
Terminal guest indications include `initdb: error`, explicit `FATAL:`, a kernel
panic, or a nonzero runtime exit. Unsupported-syscall text alone remains an
observation because a program may recover from a capability probe. Browser
uncaught errors, unhandled rejections, and worker errors are reported as runtime
failures; a WebSocket console error alone does not terminate the observation.
Browser launch failures are also written to the result, including the failing
phase and the actual launch error. The exit code is `2` for infrastructure or
browser-runtime failure, `3` for an observed guest failure, `4` for a capture
timeout and `130` for interruption. A normal runtime exit has code `0`, which
still does not establish authorization. `authorizationVerified` stays `false`
because this boot probe does not perform the authorization scenario.

The runner blocks HTTP requests outside the probe origin. The unchanged artifact
may attempt its configured WebSocket network connection; those attempts and
errors are recorded separately. This probe does not implement a browser-to-guest
HTTP bridge and does not equate loading the static page with routing to Axum.

## Diagnostic variants

Only after capturing the original behavior, a cold boot can test whether the
migration snapshot changes a failure:

```text
http://127.0.0.1:8765/?boot=cold
http://127.0.0.1:8765/?boot=cold&memory=512M
```

With the owned server, pass `--query 'boot=cold&memory=512M'` to `run.mjs`.

Cold boot removes the `-incoming` snapshot argument in memory; an optional memory
override changes `-m`. The five artifact files remain untouched. Capture each
variant in its own output directory and report its effective arguments. A
successful altered configuration would not prove the original snapshot works.

## What would establish authorization

QEMU initialization and guest boot are preliminary evidence. Full success needs
the real guest PostgreSQL and OpenFGA services, Axum `/readyz`, working HTTP
routing, and the actual deny/grant/allow/revoke/deny scenario. The existing
`lab-tool acceptance` documents the application scenario; executing it against a
native backend does not validate this VM. No static Lakekeeper WASM explorer is
loaded by this probe.

## Loader provenance

The module setup follows container2wasm v0.8.5's
[official minimal Emscripten example](https://github.com/container2wasm/container2wasm/blob/v0.8.5/examples/emscripten-simple/htdocs/index.html):
`load.js`, `arg-module.js`, `out.js`, `Module.pty`,
`mainScriptUrlOrBlob`, `/pack/info` with guest time, and the TTY poll callback.
The small raw PTY adapter records serial bytes without xterm or a CDN dependency.
It does not emulate an authorization engine, a network stack, or guest syscalls.

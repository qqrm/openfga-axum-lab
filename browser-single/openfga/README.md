# Browser-only OpenFGA startup adaptation

Upstream is OpenFGA v1.10.3 at
[`293893b22a53d8a130a5f7c97cab4bc8e91bcaa4`](https://github.com/openfga/openfga/tree/293893b22a53d8a130a5f7c97cab4bc8e91bcaa4).
Its HTTP gateway starts the gRPC server, then makes a blocking connection to it
with a hardcoded three-second deadline in `cmd/run/run.go`. Slow Linux/WASM
emulation can exceed that startup deadline. This failure was observed in the
local QEMU WASM experiment; its effect on Chromium must be tested separately.

There is no setting for that self-dial timeout in this release.
`OPENFGA_HTTP_UPSTREAM_TIMEOUT` and `OPENFGA_REQUEST_TIMEOUT` govern requests
and do not change it. Disabling HTTP would also disable the existing bootstrap
and HTTP adapter, requiring a larger migration.

The patch moves this connection into a small helper with a **60-second
startup budget** and parent cancellation. It retains `grpc.WithBlock()`, TLS
options and fatal startup failure. No authorization RPC timeout, OpenFGA policy,
PostgreSQL data or Axum behavior changes. The separate browser startup supervisor
must wait for real OpenFGA readiness before invoking bootstrap; increasing this
budget alone cannot extend bootstrap's own 60-second readiness wait.

The browser build also emits short transport diagnostics through a separate
standard Go logger on stderr, with UTC timestamps. HTTP start/end events contain
the method, path without query string, content length, elapsed time and context
error. Unary RPC start/end events contain the method, status code and elapsed
time. It does not log request/response bodies or headers. These events remain
available when OpenFGA's normal logger is set to `warn`, without enabling its
verbose raw payload logs.

The HTTP observer passes the original writer, request, body and context to the
handler. The unary observer runs before the existing logging middleware and
passes through the original context, request, response and error. Validation,
authentication, authorization, timeout and recovery middleware stay in place.
Panics continue to upstream recovery and do not emit a false completion event.
Diagnostic logging itself adds work; it is not a performance improvement or a
replacement for successful browser acceptance.

## Build and check

Run after approval for any paid CI use, from the repository root:

```sh
docker buildx build --platform linux/amd64 \
  -f browser-single/OpenFGA.Dockerfile \
  --output type=local,dest=artifacts/browser-openfga .
```

The recipe pins the upstream commit, Go **1.24.6**, and upstream `go.mod`/`go.sum`.
It verifies the source preimage before applying the zero-context unified patch,
runs the targeted tests, and builds a stripped static Linux/amd64 executable.
The exact source checksum guard is mandatory for this zero-context patch; both
patch checking and application explicitly use `git apply --unidiff-zero`. Module checksums are
verified and dependency/config changes fail the build. The compiler image uses
an exact version tag, and the compiler reports that same version before use.

Tests invoke the same helper called by the patched server:

- A real gRPC server delays its HTTP/2 handshake for four seconds. The original
  three-second call fails; the adapted call connects and completes a health RPC.
- An unavailable server still fails at the actual 60-second startup deadline.
- Parent deadline and cancellation still terminate startup.
- OpenFGA request/list deadlines and HTTP gateway timeout defaults are unchanged.
- Transport observers call the original handlers exactly once, preserving
  inputs, HTTP status/headers/body, RPC results/errors, cancellation and panics.
- Diagnostic output contains only boundary metadata, with no request/response
  bodies, query strings, headers or error details.

The tests include a deliberate 60-second wait to verify the production bound.
They do not substitute for the real PostgreSQL/OpenFGA/Axum browser acceptance.
Both Chromium and Firefox must still pass the actual sequence twice, in order:
`403 → 204 → 200 → 204 → 403`.

Output contains `openfga`, `build.json`, `dial-tests.log`, `LICENSE`, `SOURCE.txt`,
the exact source patch and helper. `build.json` is created only after passing
tests and records the source commit, compiler version, canonical UTC commit
date, patch/helper/test digests, metadata-only diagnostics, dependency checksum
and executable SHA-256. The build identifies itself as
`v1.10.3-browser-startup.1`, rather than claiming to be an untouched release.

The same `build.sh` can run directly from a clean checkout of the pinned upstream
commit when Go 1.24.6 is available on `PATH`; pass the output directory as its
first argument. It applies the same source, dependency, test and build gates as
the Docker recipe. Syntax checks and patch applicability alone are insufficient.
A successful native build verifies startup dial and transport observer tests, while real browser
acceptance remains a separate required check.

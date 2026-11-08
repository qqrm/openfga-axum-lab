# Native Rust tooling — goal #11, first implementation slice

This increment implements the documentation/browser/toolchain portion of
[#11](https://github.com/qqrm/openfga-axum-lab/issues/11). It does not complete
the domain-type, dispatch, allocation, coverage or sanitizer work.

## Toolchain and update policy

Development, explicit CI Rust installations, Pages builds and the Docker
builder and devcontainer use Rust **1.99.1**. Nextest installations use **0.9.151**. The upstream
release announcements and official Docker image definition were checked on
2026-11-07:

- <https://blog.rust-lang.org/releases/>
- <https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.151>
- <https://github.com/docker-library/official-images/blob/master/library/rust>

The installer is SHA-pinned to `taiki-e/install-action` **2.90.2**, which supports
the selected nextest release; checksum verification remains enabled and source
build fallback remains disabled. The previous pinned installer refused this
version rather than silently installing a different one.

Rustfmt and Clippy come from that Rust toolchain. Kani keeps its existing
pinned release and its own supported compiler; it is not a stable-Rust lane.
Other existing pinned audit/security tools are not upgraded by this increment.
Pinning a Docker version tag does not pin its image digest.

Treat toolchain updates as a coordinated change to `rust-toolchain.toml`, the
Docker builder and workflow installation inputs, then execute all affected
checks. Existing Dependabot coverage is not a promise that it updates every
one of these inputs: do not accept a partial version bump as a coordinated
upgrade. Automated synchronized upgrades and an independently verified MSRV
remain open items in #11; no second dependency-management service is added.

## Rust documentation is executable

`src/lib.rs` contains four examples covering lifecycle progression, rejection
of invalid transitions, identifier boundaries and UTF-8 byte limits. Public
pure-domain items and the scalar browser ABI require documentation. Server
modules remain visible for the real integration and benchmark harnesses; this
increment does not pretend all server interfaces have complete API docs. CI builds rustdoc with warnings denied
and executes examples separately from nextest:

```sh
RUSTDOCFLAGS='-D warnings' cargo doc --locked --no-deps --document-private-items
RUSTDOCFLAGS='-D warnings' cargo test --locked --doc
```

The HTML is retained as `rust-api-docs`. Passing these examples is not proof of
HTTP, SQL or distributed authorization behavior; the existing integration and
Kani checks remain required.

## The browser wrapper is a Cargo package

`browser-demo/Cargo.toml` is a small, dependency-free standalone package. It
reuses `src/domain.rs` by path as before, preserving one domain implementation
without dragging the server dependency graph into WebAssembly. A separate
manifest and dependency-free lockfile keep the existing server package, its
lockfile, formal harnesses and runtime build unchanged.

The browser package is deliberately not a member of a new root workspace.
Therefore root-only Cargo commands do not claim to check it: the mandatory
`browser-demo` CI job explicitly formats, lints and tests this package on the
host, lints the WASM target, builds its docs, executes two ABI examples, builds
WASM through Cargo and runs the existing JavaScript/WASM checks.

Its nextest profile lives in `browser-demo/.config/nextest.toml`. It retains
bounded test duration, zero retries and JUnit, without importing the server
workspace's Docker binary filters. The root nextest configuration and its
strict filter validation remain unchanged.

```sh
rustup target add wasm32-unknown-unknown
cargo fmt --manifest-path browser-demo/Cargo.toml --all -- --check
cargo clippy --locked --offline --manifest-path browser-demo/Cargo.toml --all-targets -- -D warnings
cargo clippy --locked --offline --manifest-path browser-demo/Cargo.toml --target wasm32-unknown-unknown --lib -- -D warnings
cargo nextest run --locked --offline --manifest-path browser-demo/Cargo.toml --config-file browser-demo/.config/nextest.toml --profile ci
RUSTDOCFLAGS='-D warnings' cargo doc --locked --offline --manifest-path browser-demo/Cargo.toml --no-deps
RUSTDOCFLAGS='-D warnings' cargo test --locked --offline --manifest-path browser-demo/Cargo.toml --doc
bash scripts/build-browser-demo.sh
node scripts/check-browser-demo.mjs
```

Three new native tests check the scalar ABI's complete valid transition table,
visibility and rejection of out-of-range values. The shared source's two unit
tests also run in this package. These are in addition to, not replacements for,
the real server scenarios. The build script preserves `domain.wasm` and the
existing JS interface; the size-oriented settings are expressed in Cargo's
`wasm` profile rather than a hand-written rustc invocation.

CI uploads browser JUnit and HTML as `browser-cargo-results`. It retains the
normal `rust`, `integration`, `shell`, `formal`, `browser-demo`, `browser-e2e`, `coverage` dependency list
of `ci-gate` and the `workflow_dispatch` entry point. A/A remains separately
manual. Neither Pages publication nor release publication is run by this PR.

## Scope boundaries

No authorization decisions, wire/storage formats, cache consistency protocol,
cryptography, SQL behavior or runtime abstraction have been changed here.
[Native Axum child-process coverage](native-coverage.md) is implemented in a
separate required job. Property/mutation tests, applicable sanitizers, further
semantic newtypes, dispatch/allocation changes and custom harness simplification
still require separate implementation and execution evidence.
No performance improvement is asserted by adding Cargo or documentation checks.

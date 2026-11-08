# Openlane Tasks — object authorization with Rust/Axum

A focused Rust/Axum, PostgreSQL and OpenFGA laboratory adapted from the real
[Openlane organization/task model](docs/openlane-model.md), with pinned
Apache-2.0 provenance. Demo accounts and organizations are synthetic.
Organization owners, administrators, assigners, assignees and direct/inherited
grants demonstrate object authorization, transfer and failure recovery.

## Important model behavior

Owners inherit task permissions; admins can create tasks but need explicit
grants to read others’ tasks. An assignee can edit, not delete or delegate.
Upstream `blocked` excludes reads and ordinary edits, but does not override
`can_delete` or its `can_edit` branch. Removing one direct grant leaves any
independent inherited path intact. Ownership transfer is owner-only, requires
a current member and atomically replaces one owner tuple. A durable PostgreSQL
journal fences pending changes. An uncertain replay restores reads but keeps
writes quarantined until explicit backend-drain recovery; replay alone does not
order a delayed old request against a newer transfer.

The old schema/data are retained without exposing the former API. Existing
OpenFGA configuration deliberately fails the changed model fingerprint check;
use an isolated Compose project or plan an explicit migration. No reset is
automatic. [Migration and validation](docs/openlane-migration.md).

## Quick start

Requirements: Docker, Docker Compose, OpenSSL and `just`. Tests additionally
need Rust 1.99 and cargo-nextest.

```sh
git clone --recurse-submodules https://github.com/qqrm/openfga-axum-lab.git
cd openfga-axum-lab
export POSTGRES_PASSWORD="$(openssl rand -hex 24)"
export DEMO_PASSWORD="$(openssl rand -hex 24)"
just start
just test
```

Open http://localhost:3000. `just stop` keeps volumes; `just clean` removes all
data. Keep and reuse the same exported passwords when restarting existing
volumes; changing the variables does not rotate existing database or user credentials.

## Just commands

Install [just](https://github.com/casey/just), then:

```sh
just build        # build the Rust application
just check        # format, clippy and unit tests
just fmt          # format code
just start        # start the local stand
just logs         # application logs
just test         # e2e integration tests
just cache-test   # replica cache scenarios
just stop         # stop without removing data
just clean        # stop and remove volumes
```

The optional Playwright suite (`just e2e`) uses the exported `DEMO_PASSWORD`
for the running isolated lab. CI now defines a browser-e2e matrix for both
HTTP and gRPC, using actual Axum/PostgreSQL/OpenFGA containers. A configuration
is not evidence of a completed run; see [current validation](docs/openlane-migration.md).

Russian version: [README.md](README.md).
## Kubernetes: experimental path

The manifests in `k8s/` use the application image and Rust `lab-tool` for
bootstrap. They have not been deployed or validated in a cluster by this CI.
Use Compose or the private Codespaces lab for the verified installation path.

## OpenFGA transport

The default `http` feature uses port 8080. Build the gRPC adapter with
`cargo build --locked --no-default-features --features grpc` and use port 8081.
When both features are enabled, the application selects gRPC. Bootstrap always
uses HTTP independently of the application's transport.

Both adapters use `HIGHER_CONSISTENCY`, idempotent grants/revocations and
two-second request deadlines. CI exercises the real stack, cache races and
recovery scenarios separately for both transports. `/readyz` probes PostgreSQL,
the selected transport, the store and the configured model without cached decisions.

Compose builds gRPC by default. For HTTP, set both the build feature and endpoint:

```sh
FGA_TRANSPORT=http FGA_APP_URL=http://openfga:8080 docker compose up --build
```

The earlier timing comparison used different consistency settings and cannot
support performance claims. See the [diagnostic benchmark instructions](README.md#backend-diagnostic-benchmark)
for rerunning the corrected tool, which aborts on authorization errors or denials.

## Verified delivery and private browser labs

The [delivery guide](docs/delivery.md) describes image-only installation,
isolated environments, acceptance checks, recovery and release promotion.
The [browser lab guide](docs/live-lab.md) covers private Codespaces and bounded
Actions sessions. Delivery uses gRPC images by default; an explicitly built HTTP
image also needs `FGA_APP_URL=http://openfga:8080` in its Compose environment.
Bootstrap always uses OpenFGA's HTTP API on port 8080.

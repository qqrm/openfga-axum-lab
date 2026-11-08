# Paws — shelter and object authorization

Small pet project with a real Rust/Axum API, PostgreSQL and OpenFGA. Animals,
accounts and data are fictional. The project demonstrates an engineering
approach to authorization and failures; it is not a production-ready public
service.

## Quick start

Requirements: Docker and Docker Compose.

```sh
git clone --recurse-submodules https://github.com/qqrm/openfga-axum-lab.git
cd openfga-axum-lab
just start
just test
```

Open http://localhost:3000. `just stop` keeps volumes; `just clean` removes all
data.

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

Russian version: [README.md](README.md).
## Local Kubernetes

Manifests are in . Resource requests and limits are set for small clusters.

```sh
just k8s-start
just k8s-port-forward
just k8s-stop
```

## Local Kubernetes

Manifests are in k8s/. Resource requests and limits are set for small clusters.

`
just k8s-start
just k8s-port-forward
just k8s-stop
`

## OpenFGA transport

The default `grpc` feature uses port 8081. Build the HTTP adapter with
`cargo build --locked --no-default-features --features http` and use port 8080.
When both features are enabled, the application selects gRPC. Bootstrap always
uses HTTP independently of the application's transport.

Both adapters use `HIGHER_CONSISTENCY`, idempotent grants/revocations and
two-second request deadlines. CI exercises the real stack, cache races and
recovery scenarios separately for both transports.

Compose builds gRPC by default. For HTTP, set both the build feature and endpoint:

```sh
FGA_TRANSPORT=http FGA_APP_URL=http://openfga:8080 docker compose up --build
```

The earlier timing comparison used different consistency settings and cannot
support performance claims. See the [diagnostic benchmark instructions](README.md#backend-diagnostic-benchmark)
for rerunning the corrected tool, which aborts on authorization errors or denials.

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

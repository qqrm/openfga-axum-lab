# OpenFGA + Axum lab

A self-contained Rust authorization lab inspired by a vacancy. It is not an
integration with the employer's unnamed platform. The lab contains a small
observability-style dashboard API, an authorization trait, an OpenFGA HTTP
client, Axum route middleware, relationship writes, a sample bootstrap
migration, filtered lists, a feature switch and containerized tests.

**Demo only. Do not expose this service or use it to secure real data.**
The `x-demo-user` header is intentionally unauthenticated and can be forged.
The OpenFGA instance is in-memory, unauthenticated and reachable within the
Compose network. A real integration needs authenticated identity, secure
service transport, a persistent store, audited policy and production review.

## Run

Requirements: Docker and Docker Compose, with network access for image builds.

```sh
docker compose up -d --build
docker compose --profile test run --rm test
curl -H 'x-demo-user: bob' http://127.0.0.1:3000/dashboards
docker compose down -v
```

The endpoint is bound to loopback. The bootstrap container creates a new
in-memory store and model, imports `fga/legacy.json`, and writes its IDs into
the Compose volume. It is a fixture migration, not a database migration or
rollback tool. Recreating the test stand resets its state.

## Policy and interface

- Alice is an organization admin; Bob is a member. They can view dashboards
  inherited from the organization; Alice can manage relationships.
- Carol owns a private dashboard; Dave has direct viewer access to dashboard
  two; Eve begins with no access.
- `GET /dashboards` filters the sample list by individual checks.
  `GET /dashboards/{id}` checks `can_view`.
- `PUT`/`DELETE /dashboards/{id}/relations/{relation}/{subject}` changes
  viewer, editor, owner or parent tuples after `can_manage`. For the lab,
  the only accepted parent organization is `acme`.
- `PUT`/`DELETE /organizations/acme/members/{subject}` requires the
  organization admin relation. Unsupported actions are denied.
- `AUTH_BACKEND=fga` is the default. `AUTH_BACKEND=legacy` selects a fixed,
  read-only demo legacy policy rather than allowing everything. It is not a
  safe production rollback: changes made in OpenFGA are not synchronized back.
- Failed checks and writes return 503, denied checks return 403, and missing or
  malformed demo identities return 401. The client has a two-second timeout.

The e2e script exercises inherited and direct access, filtering, role changes,
owner changes, hierarchy, membership, denied mutations and revocation. CI also
stops OpenFGA and checks fail-closed behavior.

## Performance

`python3 scripts/benchmark.py` runs after the stand and correctness tests. It
warms up with 50 sequential requests and records 500 single-client requests,
raw latency, status errors, timer, host and commit metadata in
`artifacts/benchmark.json`. The GitHub Actions job uploads this file.
This is **diagnostic only**: a shared runner, one process, one workload and no
paired baseline do not establish regression, capacity, SLO or production
performance. Compare changes only with a separately designed same-job,
balanced paired experiment, correctness controls and an uncertainty estimate.

## Integration boundaries

Real systems require an explicit object/route inventory, a policy model
approved by the product owner, reliable identity and object lookup, tenant
isolation, scalable list filtering, durable and idempotent tuple/data updates,
retry and reconciliation semantics, a resumable migration and rollback, and
tests against the real data model. Those are not established by this lab.
No assumptions about the employer's API or existing access model are made.

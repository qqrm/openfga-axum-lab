# Browser acceptance tests

These tests drive the served Axum UI and the real HTTP API. They require a fresh,
disposable PostgreSQL/OpenFGA lab seeded with the Openlane scenario. They do not
stub authorization decisions. A session-race test delays an unchanged response
fetched from the real server to verify that an old session cannot repopulate the
new session's view. The organization tests temporarily transfer Acme
ownership and change roles; their cleanup restores the original seed state.

Set `BASE_URL` to the running application and provide its configured
`DEMO_PASSWORD` through the environment, then run:

```sh
npm ci --ignore-scripts --prefix e2e
node e2e/node_modules/playwright/cli.js install chromium
npm test --prefix e2e
```

Run the suite separately against stacks configured with `FGA_TRANSPORT=http`
and `FGA_TRANSPORT=grpc`. Setting that variable only for the Playwright command
labels its JUnit report; the running application must actually use that
transport. Browser requests always go to Axum over HTTP.

For an isolated Compose run, provide separate `DEMO_PASSWORD` and
`POSTGRES_PASSWORD` environment variables and run:

```sh
FGA_TRANSPORT=http bash scripts/browser-e2e.sh
FGA_TRANSPORT=grpc bash scripts/browser-e2e.sh
```

The script creates its own Compose project, selects the matching OpenFGA port,
publishes an ephemeral loopback port, waits for readiness, runs the suite, then
collects service logs and removes only that disposable project's resources.
Both transports use this same script in CI.

Reports are written to `artifacts/browser-e2e-<transport>.xml`. Screenshots of
failed tests are kept under `e2e/test-results/`. Traces containing request
headers and session tokens are deliberately not recorded.
The locked Playwright version is pinned exactly, and browser request traces are
explicitly disabled.

The suite covers role-specific task lists, task creation/editing/deletion,
external viewers, assignment, direct grant/revoke, `blocked`, independent
inheritance paths, role promotion/demotion, owner transfer and transfer back,
forbidden requests made directly to the API, session revocation and immediate
warm-cache invalidation. Revocation tests use the same authenticated session
without waiting for a cache TTL. The 18 scenarios also cover an old response
arriving after a new identity has signed in, untrusted task metadata rendered as
literal text, public Openlane provenance, and no-store/CSP headers on successful
and rejected requests. Dedicated Rust integration tests cover
backend outages, restart recovery and two-replica races.

`npm test --prefix e2e -- --list` validates test collection only; it does not run
browser or backend assertions. The static `browser-demo` page is not used as a
substitute for this suite.

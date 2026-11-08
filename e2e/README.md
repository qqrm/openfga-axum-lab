# Browser acceptance tests

These tests drive the served Axum UI and the real HTTP API. They require a fresh,
disposable PostgreSQL/OpenFGA lab seeded with the Openlane scenario. They do not
stub authorization responses. The organization tests temporarily transfer Acme
ownership and change roles; their cleanup restores the original seed state.

Set `BASE_URL` to the running application and provide its configured
`DEMO_PASSWORD` through the environment, then run:

```sh
npm ci --prefix e2e
node e2e/node_modules/playwright/cli.js install chromium
npm test --prefix e2e
```

Run the suite separately against stacks configured with `FGA_TRANSPORT=http`
and `FGA_TRANSPORT=grpc`. Setting that variable only for the Playwright command
labels its JUnit report; the running application must actually use that
transport. Browser requests always go to Axum over HTTP.

Reports are written to `artifacts/browser-e2e-<transport>.xml`. Screenshots of
failed tests are kept under `e2e/test-results/`. Traces containing request
headers and session tokens are deliberately not recorded.

The suite covers role-specific task lists, task creation/editing/deletion,
external viewers, assignment, direct grant/revoke, `blocked`, independent
inheritance paths, role promotion/demotion, owner transfer and transfer back,
forbidden requests made directly to the API, session revocation and immediate
warm-cache invalidation. Revocation tests use the same authenticated session
without waiting for a cache TTL. Dedicated Rust integration tests cover
backend outages, restart recovery and two-replica races.

`npm test --prefix e2e -- --list` validates test collection only; it does not run
browser or backend assertions. The static `browser-demo` page is not used as a
substitute for this suite.

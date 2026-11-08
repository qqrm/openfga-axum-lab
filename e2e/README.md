# Browser acceptance tests

These tests drive the served Axum UI and the real HTTP API. They require a fresh,
disposable PostgreSQL/OpenFGA lab seeded with the Lakekeeper catalog. They do not
stub authorization decisions. A session-race test delays an unchanged response
fetched from the real server to verify that an old session cannot repopulate the
new session's view. Ownership, role and grant tests restore their changed
fixtures during cleanup; creation tests delete the tables they created.

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

The nine scenarios cover actor-specific catalog lists, table creation, editing
and deletion, grant/revoke through the same session without waiting for a TTL,
reader escalation rejection, ownership transfer and transfer back, nested role
assignment/removal, cross-project substitutions and forbidden namespace moves.
They also verify session revocation, an unchanged real response arriving after a
new identity signs in, untrusted metadata rendered literally, public Lakekeeper
provenance, and no-store/CSP headers on successful and rejected requests.
Dedicated Rust integration tests cover cursor binding, same-ID warehouse
isolation, backend outages, initial migration, restart recovery and two-replica
races. The independent Openlane model workload under `scenarios/openlane` runs
against native OpenFGA through both adapters; it is not a second browser/API
application.

`npm test --prefix e2e -- --list` validates test collection only; it does not run
browser or backend assertions. The static `browser-demo` page is not used as a
substitute for this suite.

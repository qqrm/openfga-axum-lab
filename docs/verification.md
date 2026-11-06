# Verification and instrument evidence

| Instrument | Actual target | Negative control | Boundary |
| --- | --- | --- | --- |
| rustfmt | Rust formatting | malformed formatting fixture | Formatting only |
| Clippy | Rust warnings | needless-return fixture | Not semantic correctness |
| ShellCheck | project shell scripts | unquoted expansion fixture (SC2086) | Not runtime correctness |
| cargo-audit | lockfile advisories | known vulnerable RSA fixture | Advisory DB and dependency graph limits |
| cargo-deny | advisories, bans, sources | wildcard dependency fixture in CI | Not a full vulnerability audit |
| cargo-machete | unused dependencies | deliberately unused dependency fixture | Static usage heuristics |
| Gitleaks | Git history secret patterns | synthetically constructed fake AWS key | Pattern coverage is incomplete; two old demo values are narrowly allowlisted |
| actionlint / zizmor | workflow syntax and security patterns | intentionally invalid / unsafe workflow fixtures | Does not prove GitHub settings or authorization |
| Dependabot provenance | signed bot commits and expected head | six unit tests including bad actor and stale head | Does not prove a live merge occurred |
| Kani | pure lifecycle functions | mutate visibility to expose deleting state; proof must fail | No proof of SQL, HTTP, OpenFGA, networking, or concurrency |
| Compose end-to-end | API, PostgreSQL and OpenFGA | denied roles, bad credentials, outage, concurrent retries, recovery | Bounded scenarios, not exhaustive |

Kani exhaustively explores the finite lifecycle/event choices in the two
harnesses. The service calls the same visibility and transition functions, but
database updates and external side effects are validated separately. Run:

```sh
cargo kani --lib
bash scripts/kani-negative.sh
bash scripts/check-tooling.sh
bash scripts/audit-negative.sh
```

The GitHub branch-protection API currently returned HTTP 403 for this private
repository. Dependabot auto-merge checks the live protection before enabling
native auto-merge and otherwise leaves the PR open. A synthetic policy test is
not evidence that a real Dependabot PR has merged.

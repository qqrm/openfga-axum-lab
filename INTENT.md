# Intent: first-class engineering for a focused authorization laboratory

Build an understandable organization/task service adapted from the real
Openlane OpenFGA model. Demonstrate Rust engineering with object authorization,
ownership transfer, assignment, inherited permissions, persistence and failures.
Retain the upstream provenance/license and describe local application policies.
The example accounts, organizations and local credentials are demonstration
data. The goal is credible, reviewable engineering, not production certification.

## What the project should demonstrate

- Idiomatic Rust and Axum, explicit domain rules and small, useful boundaries.
  Keep pure decisions separate from HTTP, databases, clocks and network I/O;
  avoid layers or abstractions that do not clarify a real invariant.
- Real PostgreSQL and OpenFGA integration, migrations, recovery, access control,
  tests of faults and restarts, and honest handling of partial failures.
- All relevant formatters, linters, dependency and workflow checks, and useful
  automations from the current personal Rust project template. Add ShellCheck
  for shell scripts. Treat checks as instruments: exercise meaningful negative
  controls and report what they detect and what they cannot detect.
- Apply formal methods to explicit, bounded, valuable invariants, preferably
  to production domain functions. Report assumptions and proof limits; models
  are not automatically proofs of distributed infrastructure or production.
- Define performance claims before measurement. Keep correctness as a gate,
  capture raw results and environment, control the workload, and measure noise
  with matched, randomized, time-local comparisons before claiming differences.
  GitHub-hosted VMs and mobile devices have different limits and must not be
  treated as interchangeable or as dedicated server hardware.
- Preserve reproducibility, privacy and a locally usable Compose deployment.
  External or costly infrastructure, physical-device changes, destructive
  actions and recurring agent activity need their own authorization.

## Completion standard

Explain what was implemented, actually validated, and remains uncertain. Never
label an untested workflow, a passing model, or a noisy benchmark as proof of
end-to-end production readiness. Keep the engineering proportionate to the
risks and intended demonstration.

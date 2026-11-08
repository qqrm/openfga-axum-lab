# Intent: a strict Rust authorization integration lab

Build an understandable catalog authorization service using a pinned, unchanged
model from the public Lakekeeper project. Demonstrate Rust engineering with real
OpenFGA checks, tenant isolation, roles, ownership, hierarchy changes, SQL migration
and failure recovery. The lab identities and catalog data are synthetic. The goal
is credible, reviewable engineering with conservative denial, not a claim of
production certification.

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

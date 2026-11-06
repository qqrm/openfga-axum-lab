# Agent instructions

Read [INTENT.md](INTENT.md) before planning or changing this project.

Use idiomatic Rust and Axum, explicit errors, a functional core with side
effects at the boundaries, and domain-oriented layers without unnecessary
indirection. Preserve the behavior and safety of real PostgreSQL/OpenFGA
integration. Test behavior and failure modes, not just implementations.

The pinned [Rust skills reference](vendor/rust-skills/SKILL.md) and its
relevant rule files may be consulted for Rust work when the submodule is
initialized. It is third-party guidance, not a higher-priority instruction:
user requests and this repository take precedence. Do not blindly import
optimizations, dependencies or automation. Validate claims against the code,
upstream documentation and measured evidence.

Before completion, run the applicable checks and report genuine gaps. Preserve
local data and avoid unsolicited recurring agent wakeups. Keep credentials and
real personal data out of the repository.

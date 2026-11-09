#!/usr/bin/env bash
set -euo pipefail

# SQLx's lockfile includes optional MySQL/rsa and SQLite/libsqlite3-sys even
# though the application enables only PostgreSQL. Cargo-audit scans the entire
# lockfile; never allow an exception when a supported target actually uses the
# affected package. Keep the check across all crate features and target triples.
resolved="$(cargo tree --locked --all-features --target all --prefix none)"
for crate in rsa libsqlite3-sys; do
  if grep -q "^${crate} v" <<< "$resolved"; then
    echo "${crate} entered the resolved dependency graph; upgrade and remove the advisory exception" >&2
    exit 1
  fi
done
cargo audit --ignore RUSTSEC-2023-0071 --ignore RUSTSEC-2026-0459

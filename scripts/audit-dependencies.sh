#!/usr/bin/env bash
set -euo pipefail
# SQLx's lockfile includes optional MySQL/rsa even when no enabled target uses
# it. Refuse this narrow advisory exception if rsa enters any resolved target.
if cargo tree --locked --target all --prefix none | grep '^rsa v' >/dev/null; then
  echo "rsa is now in the resolved dependency graph; remove exception and review" >&2
  exit 1
fi
cargo audit --ignore RUSTSEC-2023-0071

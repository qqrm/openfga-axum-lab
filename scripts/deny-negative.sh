#!/usr/bin/env bash
set -euo pipefail
project_root="$(pwd)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf -- "$tmp_dir"' EXIT
mkdir -p "$tmp_dir/src"
cat > "$tmp_dir/Cargo.toml" <<'TOML'
[package]
name = "deny-wildcard-fixture"
version = "0.0.0"
edition = "2024"
[dependencies]
serde = "*"
TOML
printf 'pub fn example() {}\n' > "$tmp_dir/src/lib.rs"
cargo generate-lockfile --offline --manifest-path "$tmp_dir/Cargo.toml"
if cargo deny --manifest-path "$tmp_dir/Cargo.toml" --config "$project_root/deny.toml" check bans >"$tmp_dir/report" 2>&1; then
  echo 'cargo-deny allowed a forbidden wildcard' >&2
  exit 1
fi
if ! grep -qi 'wildcard' "$tmp_dir/report"; then
  cat "$tmp_dir/report" >&2
  echo 'cargo-deny failed for the wrong reason' >&2
  exit 1
fi
echo 'cargo-deny rejected a wildcard dependency'

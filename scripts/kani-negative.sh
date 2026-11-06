#!/usr/bin/env bash
# Mutation must trigger an assertion failure, not merely fail to compile.
set -euo pipefail
tmp_dir="$(mktemp -d)"
trap 'rm -rf -- "$tmp_dir"' EXIT
mkdir -p "$tmp_dir/src"
cat > "$tmp_dir/Cargo.toml" <<'TOML'
[package]
name = "kani-negative-control"
version = "0.0.0"
edition = "2024"
TOML
cp src/domain.rs "$tmp_dir/src/lib.rs"
python3 - "$tmp_dir/src/lib.rs" <<'PY'
import pathlib, sys
path = pathlib.Path(sys.argv[1])
text = path.read_text()
old = "matches!(state, Lifecycle::Active)"
assert text.count(old) == 1
path.write_text(text.replace(old, "matches!(state, Lifecycle::Active | Lifecycle::Deleting)"))
PY
if cargo kani --manifest-path "$tmp_dir/Cargo.toml" --harness deletion_cannot_resurrect_or_be_visible >"$tmp_dir/output" 2>&1; then
    echo 'Kani accepted the deliberately unsafe lifecycle' >&2
    exit 1
fi
if ! grep -q 'VERIFICATION:- FAILED' "$tmp_dir/output"; then
    cat "$tmp_dir/output" >&2
    echo 'Kani negative control failed for the wrong reason' >&2
    exit 1
fi
echo 'Kani rejected deliberately unsafe visibility'

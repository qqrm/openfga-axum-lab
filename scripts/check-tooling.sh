#!/usr/bin/env bash
# Negative controls demonstrate that format and shell lint failures are caught.
set -euo pipefail
tmp_dir="$(mktemp -d)"
trap 'rm -rf -- "$tmp_dir"' EXIT
cat > "$tmp_dir/bad.rs" <<'RUST'
fn main(){println!("bad formatting");}
RUST
if rustfmt --check "$tmp_dir/bad.rs" >/dev/null 2>&1; then
    echo 'rustfmt negative control unexpectedly passed' >&2
    exit 1
fi
cat > "$tmp_dir/bad.sh" <<'SHELL'
#!/usr/bin/env bash
echo $unquoted_variable
SHELL
if shellcheck "$tmp_dir/bad.sh" >"$tmp_dir/shellcheck.txt" 2>&1; then
    echo 'ShellCheck negative control unexpectedly passed' >&2
    exit 1
fi
if ! grep -q 'SC2086' "$tmp_dir/shellcheck.txt"; then
    echo 'ShellCheck failed for the wrong reason' >&2
    exit 1
fi
echo 'Formatter and ShellCheck negative controls passed'
mkdir -p "$tmp_dir/clippy/src"
cat > "$tmp_dir/clippy/Cargo.toml" <<'TOML'
[package]
name = "clippy-negative-control"
version = "0.0.0"
edition = "2024"
TOML
cat > "$tmp_dir/clippy/src/lib.rs" <<'RUST'
pub fn answer() -> i32 { return 42; }
RUST
if cargo clippy --offline --manifest-path "$tmp_dir/clippy/Cargo.toml" -- -D warnings >"$tmp_dir/clippy.txt" 2>&1; then
    echo 'Clippy negative control unexpectedly passed' >&2
    exit 1
fi
if ! grep -q 'needless_return' "$tmp_dir/clippy.txt"; then
    cat "$tmp_dir/clippy.txt" >&2
    echo 'Clippy failed for the wrong reason' >&2
    exit 1
fi
echo 'Clippy negative control passed'

#!/usr/bin/env bash
set -euo pipefail
tmp_dir="$(mktemp -d)"
trap 'rm -rf -- "$tmp_dir"' EXIT
mkdir -p "$tmp_dir/.github/workflows"
cat > "$tmp_dir/.github/workflows/invalid.yml" <<'YAML'
name: invalid
on: push
jobs:
  job:
    runs-onn: ubuntu-latest
    steps:
      - run: echo test
YAML
if actionlint "$tmp_dir/.github/workflows/invalid.yml" >"$tmp_dir/actionlint.txt" 2>&1; then
  echo 'actionlint negative control unexpectedly passed' >&2
  exit 1
fi
if ! grep -q 'runs-onn' "$tmp_dir/actionlint.txt"; then
  echo 'actionlint failed for the wrong reason' >&2
  exit 1
fi
cat > "$tmp_dir/.github/workflows/insecure.yml" <<'YAML'
name: insecure
on: pull_request_target
permissions: write-all
jobs:
  unsafe:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          ref: ${{ github.event.pull_request.head.sha }}
      - run: npm install
YAML
if zizmor --offline "$tmp_dir/.github/workflows/insecure.yml" >"$tmp_dir/zizmor.txt" 2>&1; then
  echo 'zizmor negative control unexpectedly passed' >&2
  exit 1
fi
if ! grep -Eq 'dangerous-triggers|artipacked|unpinned-uses|excessive-permissions' "$tmp_dir/zizmor.txt"; then
  cat "$tmp_dir/zizmor.txt" >&2
  echo 'zizmor failed for the wrong reason' >&2
  exit 1
fi
echo 'actionlint and zizmor rejected deliberately bad workflows'

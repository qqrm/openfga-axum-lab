#!/usr/bin/env bash
set -euo pipefail
tmp_dir="$(mktemp -d)"
trap 'rm -rf -- "$tmp_dir"' EXIT
# Syntactically valid but deliberately fictional value assembled at runtime.
printf 'aws_access_key_id = "%s%s"\n' 'AKIA' '7F63MQL2Z42RNH6S' > "$tmp_dir/fixture.txt"
if gitleaks dir "$tmp_dir" --redact --no-banner --report-path="$tmp_dir/report.json" --report-format=json >/dev/null 2>&1; then
  echo 'Gitleaks negative control unexpectedly passed' >&2
  exit 1
fi
if ! grep -q '"RuleID": "aws-access-token"' "$tmp_dir/report.json"; then
  echo 'Gitleaks failed for the wrong reason' >&2
  exit 1
fi
echo 'Gitleaks detected a synthetic credential'

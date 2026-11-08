#!/usr/bin/env bash
# Execute the browser against an isolated real Rust/PostgreSQL/OpenFGA stack.
set -euo pipefail
: "${DEMO_PASSWORD:?set DEMO_PASSWORD for the disposable test stand}"
: "${POSTGRES_PASSWORD:?set POSTGRES_PASSWORD for the disposable test stand}"
export FGA_TRANSPORT="${FGA_TRANSPORT:-http}"
case "$FGA_TRANSPORT" in
  http) expected_fga_url=http://openfga:8080 ;;
  grpc) expected_fga_url=http://openfga:8081 ;;
  *) echo 'FGA_TRANSPORT must be http or grpc' >&2; exit 1 ;;
esac
if [[ "${FGA_APP_URL:-$expected_fga_url}" != "$expected_fga_url" ]]; then
  echo 'FGA_APP_URL must match the selected transport of the isolated OpenFGA service' >&2
  exit 1
fi
export FGA_APP_URL="$expected_fga_url"
# Always allocate a separate project and an ephemeral loopback port. A user's
# existing Compose project and its database volumes must not be reused.
export APP_PORT=0
project="openlane-browser-${FGA_TRANSPORT}-${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-1}-$BASHPID"
repo_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
artifact_dir="$repo_dir/artifacts"
mkdir -p "$artifact_dir"
compose=(docker compose -p "$project" -f "$repo_dir/compose.yaml")
cleanup() {
  local result=$?
  trap - EXIT
  "${compose[@]}" logs --no-color > "$artifact_dir/browser-stack-${FGA_TRANSPORT}.log" 2>&1 || true
  if ! "${compose[@]}" down --volumes --remove-orphans --timeout 2; then
    result=1
  fi
  exit "$result"
}
trap cleanup EXIT
"${compose[@]}" up -d --build app
address=$("${compose[@]}" port app 3000)
[[ "$address" == 127.0.0.1:* ]] || { echo 'Browser test API must bind to loopback'; exit 1; }
export BASE_URL="http://$address"
ready=false
for ((attempt=0; attempt<180; attempt++)); do
  if curl --fail --silent --max-time 3 "$BASE_URL/readyz" | jq -e '.ready == true' > /dev/null; then
    ready=true
    break
  fi
  sleep 1
done
[[ "$ready" == true ]] || { echo 'Real stack did not pass readiness'; exit 1; }
cd "$repo_dir/e2e"
npm test

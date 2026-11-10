#!/usr/bin/env bash
set -euo pipefail
: "${COMPOSE_PROJECT_NAME:?select a disposable project}"
: "${AUTHZ_EXPERIMENT_OUT:?select a unique output directory}"
: "${AUTHZ_BENCH_BINARY:?compile a benchmark binary}"
: "${POSTGRES_PASSWORD:?set isolated credentials}"
: "${DEMO_PASSWORD:?set isolated credentials}"
[[ "$COMPOSE_PROJECT_NAME" == authz-criterion-* ]] || { echo 'Refusing non-disposable project' >&2; exit 1; }
[[ ! -e "$AUTHZ_EXPERIMENT_OUT" ]] || { echo 'Refusing existing results directory' >&2; exit 1; }
case "${AUTHZ_BENCH_SWAP_PORTS:-false}" in
  false)
    [[ "${BENCH_PRIMARY_CACHE_ENABLED:-true}" == true && "${BENCH_SECONDARY_CACHE_ENABLED:-false}" == false ]] ||
      { echo 'Cache mapping does not match normal port order' >&2; exit 1; }
    ;;
  true)
    [[ "${BENCH_PRIMARY_CACHE_ENABLED:-true}" == false && "${BENCH_SECONDARY_CACHE_ENABLED:-false}" == true ]] ||
      { echo 'Cache mapping does not match swapped port order' >&2; exit 1; }
    ;;
  *) echo 'Invalid AUTHZ_BENCH_SWAP_PORTS' >&2; exit 1 ;;
esac
export COMPOSE_FILE=compose.yaml:compose.cache-lakekeeper.yaml
cleanup() {
  local status=$?
  if (( status != 0 )); then
    mkdir -p "$AUTHZ_EXPERIMENT_OUT"
    docker compose ps > "$AUTHZ_EXPERIMENT_OUT/compose-ps.txt" 2>&1 || true
    docker compose logs --no-color --tail=150 app app-uncached openfga postgres \
      > "$AUTHZ_EXPERIMENT_OUT/compose-logs.txt" 2>&1 || true
  fi
  docker compose down -v --remove-orphans >/dev/null 2>&1 || true
  exit "$status"
}
trap cleanup EXIT
docker compose up -d --no-build app
for _ in $(seq 1 180); do
  if curl -fsS --max-time 2 http://127.0.0.1:3000/readyz >/dev/null 2>&1; then break; fi
  sleep 2
done
curl -fsS http://127.0.0.1:3000/readyz >/dev/null
docker compose up -d --no-build app-uncached
for _ in $(seq 1 180); do
  if curl -fsS --max-time 2 http://127.0.0.1:3001/readyz >/dev/null 2>&1; then break; fi
  sleep 2
done
curl -fsS http://127.0.0.1:3001/readyz >/dev/null
./target/release/examples/catalog_cache_experiment
test -s "$AUTHZ_EXPERIMENT_OUT/experiment.json"

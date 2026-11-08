#!/usr/bin/env bash
# Infrastructure only: acceptance and application behavior are implemented in Rust.
set -euo pipefail
set +x
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
export LAB_STATE_DIR="$root/.lab/codespaces"
export APP_PORT=3000
project=authzlab-codespaces
compose=(docker compose -p "$project" --env-file "$LAB_STATE_DIR/environment.env" -f compose.release.yaml)

verify() {
  # Use the running application's lab-tool, not an old image from the first env file.
  "${compose[@]}" exec -T -e SMOKE_ALLOW_MUTATIONS=true app lab-tool acceptance
}

case "${1:-start}" in
  start)
    umask 077
    mkdir -p .lab
    # Docker-in-Docker owns its own daemon; no host socket is mounted.
    for attempt in {1..60}; do
      if docker info >/dev/null 2>&1; then break; fi
      if test "$attempt" = 60; then echo 'Docker-in-Docker did not become ready' >&2; exit 1; fi
      sleep 2
    done
    # Trust only this explicit mounted workspace, not every directory on the host.
    revision=$(git -c "safe.directory=$root" rev-parse HEAD)
    docker build --build-arg "APP_REVISION=$revision" -t authzlab-codespaces:local .
    export LAB_IMAGE
    LAB_IMAGE=$(docker image inspect authzlab-codespaces:local --format '{{.Id}}')
    fresh=false
    if ! test -e "$LAB_STATE_DIR"; then
      docker run --rm --user "$(id -u):$(id -g)" \
        -v "$root:$root" -w "$root" --entrypoint /usr/local/bin/lab-tool \
        "$LAB_IMAGE" init-env "$LAB_STATE_DIR" "$LAB_IMAGE"
      fresh=true
    fi
    test -f "$LAB_STATE_DIR/environment.env"
    if test "$fresh" = false; then
      # A restart can rerun one-shot migrations. Quiesce this lab first;
      # remove only its completed job containers, never its persistent volumes.
      "${compose[@]}" stop -t 5 app openfga
      "${compose[@]}" rm -f fga-migrate bootstrap
    fi
    "${compose[@]}" up -d --wait --wait-timeout 120 app
    if test "$fresh" = true; then verify; fi
    curl --fail --silent --show-error --max-time 5 http://127.0.0.1:3000/readyz
    printf '\nReal lab ready: open private port 3000 in the Ports panel.\n'
    printf 'Demo users: alice, bob, carol, dave, eve. Read the password in your own terminal:\ncat .lab/codespaces/secrets/demo-password\n'
    ;;
  private)
    if test "${CODESPACES:-false}" = true; then
      : "${CODESPACE_NAME:?Codespace identity is required}"
      # Fail visibly rather than silently turning a failed privacy update into success.
      gh codespace ports visibility 3000:private --codespace "$CODESPACE_NAME"
      printf 'Port 3000 is private. Open it from the Ports panel.\n'
    fi
    ;;
  verify)
    verify
    ;;
  status)
    "${compose[@]}" ps
    curl --fail --silent --show-error --max-time 5 http://127.0.0.1:3000/version
    printf '\n'
    ;;
  stop)
    "${compose[@]}" down
    echo 'Stopped this Codespace lab; data volumes and passwords are preserved.'
    ;;
  *) echo 'usage: codespace-lab.sh start|private|verify|status|stop' >&2; exit 2 ;;
esac

#!/usr/bin/env bash
# Browser-only wrapper: real binaries and original acceptance, no ACL simulation.
set -uo pipefail
umask 077
export BROWSER_LAB_EPHEMERAL=yes
export DEMO_PASSWORD_FILE=/tmp/browser-lab/demo-password
export SMOKE_BASE_URL=http://127.0.0.1:3000
export SMOKE_ALLOW_MUTATIONS=true
# Bound only the setup CLI connection and avoid a timezone-directory search.
export PGCONNECT_TIMEOUT=10 TZ=UTC
printf 'LAB|boot\n'
/usr/local/bin/browser-lab-entrypoint >/tmp/browser-lab-startup.log 2>&1 &
lab_pid=$!
trap 'kill -TERM "$lab_pid" 2>/dev/null || true' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
ready=false
deadline=$((SECONDS + 360))
while (( SECONDS < deadline )); do
    if /usr/local/bin/lab-tool probe >/dev/null 2>&1; then
        ready=true
        break
    fi
    if ! kill -0 "$lab_pid" 2>/dev/null; then break; fi
    sleep 2
done
if [[ "$ready" != true ]]; then
    tail -c 8000 /tmp/browser-lab-startup.log
    for diagnostic in /tmp/browser-lab/initdb.log /tmp/browser-lab/postgres.log; do
        if [[ -f "$diagnostic" ]]; then
            printf '\nGuest diagnostic: %s\n' "$diagnostic"
            tail -c 8000 "$diagnostic"
        fi
    done
    printf '\nLAB|boot-failed\n'
    exit 1
fi
printf 'LAB|ready\n'
verify() {
    printf 'LAB|verification-start\n'
    if /usr/local/bin/lab-tool acceptance >/tmp/browser-lab-result.json 2>/tmp/browser-lab-test-error.log; then
        printf 'LAB|result|'
        cat /tmp/browser-lab-result.json
    else
        tail -c 4000 /tmp/browser-lab-test-error.log
        printf '\nLAB|verification-failed\n'
        exit 1
    fi
}
verify
while IFS= read -r command; do
    case "$command" in
        verify) verify ;;
        *) printf 'LAB|unknown-command\n' ;;
    esac
done

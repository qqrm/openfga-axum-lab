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
# Compact browser transport traces identify failed startup requests without
# enabling full model/Check payload logs from the upstream INFO interceptor.
export OPENFGA_LOG_LEVEL=warn
printf '\nLAB|boot\n'
deadline=$((SECONDS + 900))
# The archived rootfs keeps PostgreSQL, Axum, lab-tool and the model unchanged.
# Copy the verified browser-only OpenFGA build out of the read-only host mount.
export BROWSER_LAB_OPENFGA_BINARY=/tmp/browser-lab-runtime/openfga
printf '[browser-guest] staging the verified OpenFGA executable\n'
if ! mkdir -m 0700 /tmp/browser-lab-runtime || \
    ! timeout --kill-after=5 900 cp /browser-host/openfga "$BROWSER_LAB_OPENFGA_BINARY" || \
    ! chmod 0500 "$BROWSER_LAB_OPENFGA_BINARY"; then
    printf 'LAB|boot-failed\n'
    exit 1
fi
printf '[browser-guest] executable staged; starting the real services\n'
/bin/bash /browser-host/entrypoint.sh > >(tee /tmp/browser-lab-startup.log) 2>&1 &
lab_pid=$!
trap 'kill -TERM "$lab_pid" 2>/dev/null || true' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
ready=false
while (( SECONDS < deadline )); do
    if grep -Fq 'Listening on ' /tmp/browser-lab-startup.log && \
        /usr/local/bin/lab-tool probe >/dev/null 2>&1; then
        ready=true
        break
    fi
    if ! kill -0 "$lab_pid" 2>/dev/null; then break; fi
    sleep 3
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
postgres_pid=
if ! read -r postgres_pid </tmp/browser-lab/postgres/postmaster.pid || \
    [[ ! "$postgres_pid" =~ ^[1-9][0-9]*$ ]]; then
    printf '\nLAB|boot-failed\n'
    exit 1
fi
printf 'LAB|ready\n'
verification_id=0
verify() {
    verification_id=$((verification_id + 1))
    printf 'LAB|verification-start|%s\n' "$verification_id"
    if /usr/local/bin/lab-tool acceptance >/tmp/browser-lab-result.json 2>/tmp/browser-lab-test-error.log; then
        printf 'LAB|result|%s|' "$verification_id"
        cat /tmp/browser-lab-result.json
    else
        tail -c 4000 /tmp/browser-lab-test-error.log
        printf '\nLAB|verification-failed\n'
        exit 1
    fi
}
verify
while true; do
    if ! kill -0 "$lab_pid" 2>/dev/null || ! kill -0 "$postgres_pid" 2>/dev/null; then
        tail -c 8000 /tmp/browser-lab-startup.log
        printf '\nLAB|service-failed\n'
        exit 1
    fi
    if IFS= read -r -t 3 command; then
        case "$command" in
            verify) verify ;;
            *) printf 'LAB|unknown-command\n' ;;
        esac
    else
        status=$?
        # A timed read returns >128. EOF/errors terminate the guest instead of
        # spinning or continuing to display a live, accepted backend.
        if (( status <= 128 )); then
            printf '\nLAB|service-failed\n'
            exit 1
        fi
    fi
done

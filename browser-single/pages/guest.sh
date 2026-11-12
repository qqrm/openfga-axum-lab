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
# Compact browser transport traces identify failed startup requests.
export OPENFGA_LOG_LEVEL=warn
# The browser demo does not use the Prometheus exporter.
export OPENFGA_METRICS_ENABLED=false
printf '\nLAB|boot\n'
deadline=$((SECONDS + 900))
# Do not confuse QEMU's requested topology with CPUs Linux actually discovered.
online_cpus=
if ! read -r online_cpus </sys/devices/system/cpu/online || [[ "$online_cpus" != '0-1' ]]; then
    printf '[browser-guest] expected online CPUs 0-1, got: %s\n' "$online_cpus"
    printf 'LAB|boot-failed\n'
    exit 1
fi
printf '[browser-guest] confirmed online CPUs: %s\n' "$online_cpus"
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
    # One failure snapshot: 8 seconds plus 1 second to force termination.
    # Keep the failure marker outside diagnostics, including their timeout.
    timeout --kill-after=1s 8s /bin/bash <<'DIAGNOSTICS' || true
for file in /tmp/browser-lab-startup.log \
            /tmp/browser-lab/initdb.log \
            /tmp/browser-lab/postgres.log; do
    if [[ -f "$file" ]]; then
        printf '\nGuest diagnostic: %s\n' "$file"
        tail -c 8000 "$file"
    fi
done

for file in /proc/meminfo /sys/devices/system/cpu/online /proc/uptime; do
    printf '\nGuest diagnostic: %s\n' "$file"
    cat "$file" 2>/dev/null || true
done

count=0
for proc in /proc/[0-9]*; do
    IFS=$'\t' read -r key name 2>/dev/null <"$proc/status" || continue
    case "$name" in initdb|postgres) ;; *) continue ;; esac

    printf '\nGuest process: %s\n' "${proc##*/}"
    while IFS=: read -r key value; do
        case "$key" in
            Name|State|Pid|PPid|VmRSS|VmSwap|Threads)
                printf '%s:%s\n' "$key" "$value" ;;
        esac
    done 2>/dev/null <"$proc/status"

    printf 'wchan: '
    cat "$proc/wchan" 2>/dev/null || true
    printf '\n'
    if IFS= read -r stat 2>/dev/null <"$proc/stat"; then
        # Fields 14/15 follow the pid and parenthesized comm: cumulative ticks.
        read -r -a fields <<<"${stat##*) }"
        printf 'utime_ticks: %s\nstime_ticks: %s\n' \
            "${fields[11]:-unavailable}" "${fields[12]:-unavailable}"
    fi

    count=$((count + 1))
    if (( count >= 16 )); then
        printf 'Process snapshot stopped at limit=16\n'
        break
    fi
done
DIAGNOSTICS
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

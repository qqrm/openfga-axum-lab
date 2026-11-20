#!/usr/bin/env bash
# A separate real authorization scenario inside the retained Linux guest.
set -Eeuo pipefail
umask 077

fail() {
    printf 'CONCEPT|failed|{"message":"%s"}\n' "$1"
    exit 1
}

printf '\nCONCEPT|phase|{"phase":"linux-booted","detail":"Linux is running inside this tab."}\n'
online_cpus=
if ! read -r online_cpus </sys/devices/system/cpu/online || [[ "$online_cpus" != 0-1 ]]; then
    fail 'The guest did not discover the required two CPUs.'
fi
allowed_cpus=
while IFS=: read -r key value; do
    if [[ "$key" == Cpus_allowed_list ]]; then
        allowed_cpus=${value//[[:space:]]/}
        break
    fi
done </proc/self/status
[[ "$allowed_cpus" == 0-1 ]] || fail 'The services cannot use both guest CPUs.'
printf '[concept-guest] online CPUs: %s; service CPUs: %s\n' "$online_cpus" "$allowed_cpus"
[[ "$(id -un)" == postgres ]] || fail 'The guest must use its unprivileged service account.'

export BROWSER_CONCEPT_EPHEMERAL=yes
export TZ=UTC PGCONNECT_TIMEOUT=10
export CONCEPT_RUNTIME_DIR=/tmp/browser-concept-runtime
printf 'CONCEPT|phase|{"phase":"staging","detail":"Preparing the verified Rust and OpenFGA executables."}\n'
if ! mkdir -m 0700 "$CONCEPT_RUNTIME_DIR" || \
    ! timeout --kill-after=5 180 cp /browser-host/concept /browser-host/openfga "$CONCEPT_RUNTIME_DIR/" || \
    ! chmod 0500 "$CONCEPT_RUNTIME_DIR/concept" "$CONCEPT_RUNTIME_DIR/openfga"; then
    fail 'The verified concept executables could not be staged.'
fi
exec /bin/bash /browser-host/entrypoint.sh

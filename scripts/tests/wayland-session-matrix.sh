#!/usr/bin/env bash
# Test matrix for scripts/flamewm-wayland-session.
#
# Drives the real wrapper against a stub compositor (FLAMEWM_WAYLAND_BINARY), a stub services bridge,
# a stub dbus-run-session / dbus-update-activation-environment and a stub Xwayland. Nothing here
# touches a seat, a compositor, Xwayland, cargo or the network: every case runs in its own temporary
# HOME and XDG_RUNTIME_DIR and asserts on the wrapper exit code, the stub's argv/environment record
# and the session log.
#
# Usage: bash scripts/tests/wayland-session-matrix.sh [-v]
# Exit: 0 when every check passes, 1 otherwise. Machine-readable tail: `SUMMARY cases=<n> passed=<n>
# failed=<n>`.
#
# What is covered (one or more checks per case):
#   default-backend, inherited-display-drm, nested-flag-wins, flag-stripped, drm-flag-explicit,
#   capability-gate-missing, capability-unsupported, probe-advisory, runtime-missing,
#   runtime-insecure, missing-binary-127, not-executable-126, sigterm-forward,
#   sigterm-escalate, log-mode-rotation, socket-xwayland-activation, invalid-backend,
#   desktop-exec-contract.
set -uo pipefail

SELF_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(cd -- "$SELF_DIR/../.." && pwd)
WRAPPER="$ROOT/scripts/flamewm-wayland-session"

BASE=$(mktemp -d "${TMPDIR:-/tmp}/flamewm-wayland-session-matrix.XXXXXX") || exit 1
cleanup() { rm -rf "$BASE"; }
trap cleanup EXIT

PASSES=0
FAILS=0
CASES=0
FAILED_CASES=()

ok() { PASSES=$((PASSES + 1)); printf 'ok   %s: %s\n' "$CASE_NAME" "$1"; }
bad() { FAILS=$((FAILS + 1)); FAILED_CASES+=("$CASE_NAME/$1"); printf 'FAIL %s: %s: %s\n' "$CASE_NAME" "$1" "$2"; }

check_eq() { # name expected actual
    if [[ "$2" == "$3" ]]; then ok "$1"; else bad "$1" "expected [$2] got [$3]"; fi
}
check_ne() { # name unexpected actual
    if [[ "$2" != "$3" ]]; then ok "$1"; else bad "$1" "value must differ from [$2]"; fi
}
check_file_has() { # name file needle
    if [[ -f "$2" ]] && grep -Fq -- "$3" "$2"; then ok "$1"; else bad "$1" "$2 lacks [$3]"; fi
}
check_file_lacks() { # name file needle
    if [[ -f "$2" ]] && grep -Fq -- "$3" "$2"; then bad "$1" "$2 unexpectedly contains [$3]"; else ok "$1"; fi
}
check_file_lacks_line() { # name file line (exact whole line; the record also holds paths that embed the needle)
    if [[ -f "$2" ]] && grep -Fxq -- "$3" "$2"; then bad "$1" "$2 unexpectedly contains line [$3]"; else ok "$1"; fi
}
check_mode() { # name file expected-mode
    local actual
    actual="$(stat -c %a "$2" 2>/dev/null || printf 'missing')"
    if [[ "$actual" == "$3" ]]; then ok "$1"; else bad "$1" "mode of $2: expected $3 got $actual"; fi
}

STUB_COMPOSITOR='#!/bin/sh
# Stub flamewm-wayland: records argv/environment, then behaves per STUB_* variables.
set -u
record="${STUB_RECORD:?STUB_RECORD unset}"
printf "invoke args=[%s] backend=%s display=%s wayland_display=%s\n" \
    "$*" "${FLAMEWM_WAYLAND_BACKEND:-<unset>}" "${DISPLAY:-<unset>}" "${WAYLAND_DISPLAY:-<unset>}" >> "$record"
for arg in "$@"; do
    case "$arg" in
        --capabilities)
            printf "capabilities\n" >> "$record"
            case "${STUB_CAP_MODE:-ok}" in
                ok) printf "backend=%s\ndrm_compiled=%s\n" "${STUB_CAP_BACKEND:-native}" "${STUB_CAP_DRM:-true}"; exit 0 ;;
                empty) exit 0 ;;
                fail) printf "flamewm-wayland: unknown argument --capabilities\n" >&2; exit 64 ;;
                *) exit 64 ;;
            esac
            ;;
        --probe)
            printf "probe\n" >> "$record"
            printf "[ok] fixture prerequisite present\n"
            printf "[FAIL] fixture failure: no fixture seat\n"
            exit "${STUB_PROBE_RC:-0}"
            ;;
    esac
done
printf "run\n" >> "$record"
case "${STUB_RUN:-exit0}" in
    exit0) exit "${STUB_RUN_RC:-0}" ;;
    socket)
        if [ -n "${XDG_RUNTIME_DIR:-}" ]; then : > "$XDG_RUNTIME_DIR/${STUB_SOCKET:-wayland-flame}"; fi
        printf "flamewm-wayland: XWayland ready on DISPLAY=:97\n"
        sleep "${STUB_RUN_SLEEP:-0.4}"
        exit "${STUB_RUN_RC:-0}"
        ;;
    term)
        trap "printf \"got TERM\\n\" >> \"\$record\"; printf \"flamewm-wayland: terminated by TERM\\n\"; exit 143" TERM
        trap "printf \"got INT\\n\" >> \"\$record\"; exit 130" INT
        trap "printf \"got HUP\\n\" >> \"\$record\"; exit 129" HUP
        printf "flamewm-wayland: running\n"
        while :; do sleep 0.05; done
        ;;
    ignore-term)
        trap "printf \"got TERM\\n\" >> \"\$record\"" TERM
        trap "" INT HUP
        printf "flamewm-wayland: running\n"
        while :; do sleep 0.05; done
        ;;
    *) exit 0 ;;
esac
'

STUB_BRIDGE='#!/bin/sh
record="${STUB_RECORD:?}"
printf "bridge start args=[%s]\n" "$*" >> "$record"
trap "printf \"bridge TERM\\n\" >> \"\$record\"; exit 0" TERM
while :; do sleep 0.05; done
'

STUB_DBUS_RUN_SESSION='#!/bin/sh
printf "dbus-run-session args=[%s]\n" "$*" >> "${STUB_RECORD:-/dev/null}"
while [ "${1:-}" = "--" ]; do shift; done
exec "$@"
'

STUB_DBUS_UPDATE='#!/bin/sh
{
    printf "dbus-update args=[%s]\n" "$*"
    printf "dbus-update wayland_display=%s display=%s\n" "${WAYLAND_DISPLAY:-<unset>}" "${DISPLAY:-<unset>}"
} >> "${STUB_RECORD:-/dev/null}"
exit "${STUB_DBUS_UPDATE_RC:-0}"
'

STUB_XWAYLAND='#!/bin/sh
printf "Xwayland invoked args=[%s]\n" "$*" >> "${STUB_RECORD:-/dev/null}"
exit 0
'

install_stub() { # path content
    printf '%s' "$2" > "$1"
    chmod 0755 "$1"
}

start_case() { # name
    CASE_NAME="$1"
    CASES=$((CASES + 1))
    CASE="$BASE/$1"
    HOME_DIR="$CASE/home"
    RT="$CASE/runtime"
    BIN="$CASE/bin"
    RECORD="$CASE/record"
    OUT="$CASE/wrapper.out"
    mkdir -p "$HOME_DIR" "$RT" "$BIN"
    chmod 700 "$RT"
    : > "$RECORD"
    install_stub "$BIN/flamewm-wayland" "$STUB_COMPOSITOR"
    install_stub "$BIN/flamewm-desktop-services" "$STUB_BRIDGE"
    install_stub "$BIN/dbus-run-session" "$STUB_DBUS_RUN_SESSION"
    install_stub "$BIN/dbus-update-activation-environment" "$STUB_DBUS_UPDATE"
    install_stub "$BIN/Xwayland" "$STUB_XWAYLAND"
    LOG="$HOME_DIR/.cache/flamewm/wayland-session.log"
    # Fully controlled environment: no inherited DISPLAY/WAYLAND_DISPLAY/backend/D-Bus leaks in.
    CASE_ENV=(
        "HOME=$HOME_DIR"
        "PATH=$BIN:/usr/bin:/bin:/usr/sbin:/sbin"
        "XDG_RUNTIME_DIR=$RT"
        "FLAMEWM_WAYLAND_BINARY=$BIN/flamewm-wayland"
        "FLAMEWM_SERVICES_BINARY=$CASE/no-such-bridge"
        "FLAMEWM_WAYLAND_STARTUP_TIMEOUT_MS=300"
        "FLAMEWM_WAYLAND_SHUTDOWN_TIMEOUT_MS=300"
        "STUB_RECORD=$RECORD"
        "STUB_CAP_MODE=ok"
        "STUB_CAP_BACKEND=native"
        "STUB_CAP_DRM=true"
        "STUB_RUN=exit0"
        "STUB_PROBE_RC=0"
    )
    printf '\n== case %s\n' "$CASE_NAME"
}

add_env() { CASE_ENV+=("$1=$2"); }

run_wrapper() { # [wrapper args...] -> $RUN_RC, output in $OUT
    timeout 30 env -i "${CASE_ENV[@]}" "$WRAPPER" "$@" >"$OUT" 2>&1
    RUN_RC=$?
}

run_wrapper_signalled() { # signal, [wrapper args...] -> $RUN_RC (runs in background, sends signal)
    local sig="$1"; shift
    env -i "${CASE_ENV[@]}" "$WRAPPER" "$@" >"$OUT" 2>&1 &
    local wrapper_pid=$!
    sleep 0.6
    kill -s "$sig" "$wrapper_pid" 2>/dev/null || :
    wait "$wrapper_pid" 2>/dev/null
    RUN_RC=$?
}

record_line() { grep -m 1 -F -- "$1" "$RECORD" 2>/dev/null; }

no_leftover_process() { # name pattern
    ps -eo pid=,args= > "$CASE/ps.txt" 2>/dev/null || : > "$CASE/ps.txt"
    if grep -Fq -- "$2" "$CASE/ps.txt"; then
        bad "$1" "process still running: $(grep -F -- "$2" "$CASE/ps.txt" | head -n 2 | tr '\n' ';')"
    else
        ok "$1"
    fi
}

# --- 1. No DISPLAY, no flag: the default backend is drm (no inference at all).
start_case default-backend
run_wrapper
check_eq "exit" 0 "$RUN_RC"
check_file_has "backend-drm" "$RECORD" "backend=drm"
check_file_has "session-stage" "$LOG" "FLAMEWM-WAYLAND-STARTUP stage=session status=ok"
check_file_has "drm-socket-name" "$LOG" "stage=socket"
check_file_has "log-stage" "$LOG" "stage=log status=ok"
check_file_has "no-dbus-address" "$LOG" "dbus_session_bus=absent"

# --- 2. Inherited DISPLAY/WAYLAND_DISPLAY (a greeter leak) must still select drm.
start_case inherited-display-drm
add_env DISPLAY ":0"
add_env WAYLAND_DISPLAY "wayland-0"
run_wrapper
check_eq "exit" 0 "$RUN_RC"
check_file_has "backend-drm" "$RECORD" "backend=drm"
check_file_lacks "no-winit" "$RECORD" "backend=winit"
check_file_has "inherited-display-dropped" "$RECORD" "display=<unset>"
check_file_has "startup-detail" "$LOG" "inherited_display=dropped"

# --- 3. Explicit --backend=nested beats both the environment and an inherited DISPLAY.
start_case nested-flag-wins
add_env DISPLAY ":0"
add_env FLAMEWM_WAYLAND_BACKEND "drm"
run_wrapper --backend=nested
check_eq "exit" 0 "$RUN_RC"
check_file_has "backend-winit" "$RECORD" "backend=winit"
check_file_has "nested-gate-skipped" "$LOG" "stage=capabilities status=skip"
check_file_has "nested-keeps-display" "$RECORD" "display=:0"

# --- 4. The backend flag never reaches the compositor; other arguments are preserved.
start_case flag-stripped
run_wrapper --backend=winit --fixture-keep=1 --fixture-flag
check_eq "exit" 0 "$RUN_RC"
check_file_lacks "flag-not-forwarded" "$RECORD" "--backend"
check_file_has "other-args-forwarded" "$RECORD" "args=[--fixture-keep=1 --fixture-flag]"
check_file_has "env-value" "$RECORD" "backend=winit"

# --- 5. The desktop `Exec` form `--backend=drm` (and the split `--backend drm`) is accepted, selects
#        drm, keeps the capability gate and never reaches the compositor argv.
start_case drm-flag-explicit
add_env DISPLAY ":0"
run_wrapper --backend=drm
check_eq "exit-equals-form" 0 "$RUN_RC"
check_file_has "backend-drm-equals" "$RECORD" "backend=drm"
check_file_has "gate-ran" "$LOG" "stage=capabilities status=ok"
check_file_has "inherited-display-dropped" "$LOG" "inherited_display=dropped"
run_wrapper --backend drm
check_eq "exit-split-form" 0 "$RUN_RC"
check_eq "drm-invocations" 2 "$(grep -c -F 'invoke args=[] backend=drm' "$RECORD")"
check_file_lacks "flag-not-forwarded" "$RECORD" "--backend"
check_file_has "argv-preserved-split" "$RECORD" "invoke args=[]"

# --- 6. drm_compiled=false blocks the drm session with exit 2 and the documented line.
start_case capability-gate-missing
add_env STUB_CAP_DRM "false"
run_wrapper
check_eq "exit" 2 "$RUN_RC"
check_file_has "gate-line" "$LOG" "FLAMEWM-WAYLAND-STARTUP stage=capabilities status=fail code=2 detail=drm_feature_missing"
check_file_lacks_line "compositor-not-started" "$RECORD" "run"
check_file_has "exit-stage" "$LOG" "stage=exit status=fail code=2"

# --- 6. A build that does not answer --capabilities also fails the gate (exit 2).
start_case capability-unsupported
add_env STUB_CAP_MODE "fail"
run_wrapper
check_eq "exit" 2 "$RUN_RC"
check_file_has "unsupported-line" "$LOG" "stage=capabilities status=fail code=2 detail=capabilities_unsupported"
check_file_lacks_line "compositor-not-started" "$RECORD" "run"

# --- 7. A failing probe is recorded but never changes the session exit code.
start_case probe-advisory
add_env STUB_PROBE_RC "3"
run_wrapper
check_eq "exit" 0 "$RUN_RC"
check_file_has "probe-exit" "$LOG" "probe exit=3"
check_file_has "probe-fail-lines" "$LOG" "[FAIL] fixture failure: no fixture seat"
check_file_has "probe-stage" "$LOG" "stage=probe status=fail code=3 detail=advisory,continuing"

# --- 8. A missing XDG_RUNTIME_DIR warns with an actionable line and the session continues.
start_case runtime-missing
add_env XDG_RUNTIME_DIR "$CASE/absent-runtime"
run_wrapper
check_eq "exit" 0 "$RUN_RC"
check_file_has "runtime-stage" "$LOG" "stage=runtime status=fail code=0"
check_file_has "runtime-action" "$LOG" "ACTION: XDG_RUNTIME_DIR must be an existing directory"
check_file_has "runtime-detail" "$LOG" "detail=not_a_directory,continuing"

# --- 9. A world-readable runtime dir warns and continues.
start_case runtime-insecure
chmod 0755 "$RT"
run_wrapper
check_eq "exit" 0 "$RUN_RC"
check_file_has "insecure-mode" "$LOG" "detail=insecure_mode=755,expected_700,continuing"

# --- 10. Missing compositor binary: 127.
start_case missing-binary-127
add_env FLAMEWM_WAYLAND_BINARY "$CASE/does-not-exist"
run_wrapper
check_eq "exit" 127 "$RUN_RC"
check_file_has "exit-stage" "$LOG" "stage=exit status=fail code=127 detail=binary_missing"

# --- 11. Non-executable compositor: 126.
start_case not-executable-126
printf '#!/bin/sh\nexit 0\n' > "$CASE/not-executable"
chmod 0644 "$CASE/not-executable"
add_env FLAMEWM_WAYLAND_BINARY "$CASE/not-executable"
run_wrapper
check_eq "exit" 126 "$RUN_RC"
check_file_has "exit-stage" "$LOG" "stage=exit status=fail code=126 detail=binary_not_executable"

# --- 12. SIGTERM is forwarded as TERM, the bridge is stopped, and the code is propagated.
start_case sigterm-forward
add_env FLAMEWM_SERVICES_BINARY "$BIN/flamewm-desktop-services"
add_env STUB_RUN "term"
add_env DBUS_SESSION_BUS_ADDRESS "unix:path=$CASE/fixture-bus"
run_wrapper_signalled TERM
check_eq "exit" 143 "$RUN_RC"
check_file_has "got-term" "$RECORD" "got TERM"
check_file_lacks "no-int" "$RECORD" "got INT"
check_file_lacks "no-hup" "$RECORD" "got HUP"
check_file_has "forwarded" "$LOG" "forwarding SIGTERM to flamewm-wayland pid="
check_file_has "bridge-stopped" "$RECORD" "bridge TERM"
check_file_has "exit-stage" "$LOG" "stage=exit status=fail code=143"
no_leftover_process "no-zombie-compositor" "$BIN/flamewm-wayland"
no_leftover_process "no-zombie-bridge" "$BIN/flamewm-desktop-services"

# --- 13. A compositor that ignores TERM is killed with SIGKILL after the bounded grace period.
start_case sigterm-escalate
add_env FLAMEWM_SERVICES_BINARY "$CASE/no-such-bridge"
add_env STUB_RUN "ignore-term"
add_env DBUS_SESSION_BUS_ADDRESS "unix:path=$CASE/fixture-bus"
run_wrapper_signalled TERM
check_eq "exit" 137 "$RUN_RC"
check_file_has "got-term" "$RECORD" "got TERM"
check_file_has "escalated" "$LOG" "sending SIGKILL"
no_leftover_process "no-zombie-compositor" "$BIN/flamewm-wayland"

# --- 14. The log is 0600 and exactly one .1 is kept.
start_case log-mode-rotation
run_wrapper
check_eq "first-exit" 0 "$RUN_RC"
check_mode "log-0600" "$LOG" "600"
first_session_line="$(grep -m 1 -F 'stage=session status=ok' "$LOG")"
check_ne "first-session-line-captured" "" "$first_session_line"
run_wrapper
check_eq "second-exit" 0 "$RUN_RC"
check_mode "log-0600-after-rotation" "$LOG" "600"
check_mode "rotated-0600" "$LOG.1" "600"
check_file_has "rotated-previous-run" "$LOG.1" "$first_session_line"
log_files="$(find "$(dirname "$LOG")" -maxdepth 1 -name 'wayland-session.log*' | wc -l)"
check_eq "one-rotation-only" 2 "$log_files"

# --- 15. Socket and XWayland DISPLAY exist before the activation environment is synced; the D-Bus
#        address never reaches the log.
start_case socket-xwayland-activation
add_env STUB_RUN "socket"
add_env DBUS_SESSION_BUS_ADDRESS "unix:path=$CASE/fixture-bus-do-not-log"
run_wrapper
check_eq "exit" 0 "$RUN_RC"
check_file_has "socket-stage" "$LOG" "stage=socket status=ok"
check_file_has "xwayland-stage" "$LOG" "stage=xwayland status=ok"
check_file_has "ready-stage" "$LOG" "stage=ready status=ok"
check_file_has "activation-wayland-display" "$RECORD" "dbus-update wayland_display=wayland-flame"
check_file_has "activation-xwayland-display" "$RECORD" "display=:97"
check_file_has "activation-var-list" "$RECORD" "args=[--systemd XDG_SESSION_TYPE XDG_CURRENT_DESKTOP XDG_MENU_PREFIX WAYLAND_DISPLAY DISPLAY]"
check_file_lacks "dbus-address-not-logged" "$LOG" "fixture-bus-do-not-log"
check_file_lacks "xwayland-not-spawned-by-wrapper" "$RECORD" "Xwayland invoked"
check_file_has "dbus-run-session-not-used" "$LOG" "dbus_session_bus=present"

# --- 16. An unsupported --backend value is rejected before anything runs (exit 2).
start_case invalid-backend
run_wrapper --backend=surfaceless
check_eq "exit" 2 "$RUN_RC"
if grep -Fq "detail=invalid_backend,source=flag" "$OUT"; then ok "invalid-backend-line"; else bad "invalid-backend-line" "missing structured line in $OUT"; fi
if grep -Fq "invoke" "$RECORD"; then bad "nothing-executed" "the compositor was executed"; else ok "nothing-executed"; fi

# --- 17. The packaging `Exec` line must start this wrapper and every argument it passes must be
#        accepted as-is and still yield a deterministic native (drm) login.
start_case desktop-exec-contract
DESKTOP_FILE="$ROOT/packaging/flamewm-wayland.desktop"
exec_line="$(sed -n 's/^Exec=//p' "$DESKTOP_FILE" 2>/dev/null | head -n 1)"
check_ne "desktop-exec-present" "" "$exec_line"
if [[ -n "$exec_line" ]]; then printf '%s\n' "$exec_line" > "$CASE/desktop-exec.txt"; ok "desktop-exec-captured"; else bad "desktop-exec-captured" "$DESKTOP_FILE has no Exec= line"; fi
read -r -a exec_argv <<< "$exec_line"
check_eq "desktop-exec-wrapper" "/usr/libexec/flamewm/flamewm-wayland-session" "${exec_argv[0]:-}"
add_env DISPLAY ":0"
if (( ${#exec_argv[@]} > 1 )); then
    run_wrapper "${exec_argv[@]:1}"
else
    run_wrapper
fi
check_eq "desktop-exec-exit" 0 "$RUN_RC"
check_file_has "desktop-exec-backend-drm" "$RECORD" "backend=drm"
check_file_has "desktop-exec-gate-ran" "$LOG" "stage=capabilities status=ok"
check_file_lacks "desktop-exec-flag-not-forwarded" "$RECORD" "--backend"

printf '\nSUMMARY cases=%d passed=%d failed=%d\n' "$CASES" "$PASSES" "$FAILS"
if (( FAILS > 0 )); then
    printf 'FAILED: %s\n' "${FAILED_CASES[*]}"
    exit 1
fi
printf 'OK: wrapper matrix clean\n'
exit 0

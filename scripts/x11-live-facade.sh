#!/usr/bin/env bash
# Typed facade for the live X11 harness (scripts/x11-startup-faults). Source-only.
#
# Sourced by:
#   scripts/x11-startup-faults       (start_xvfb display resolution)
#   scripts/test-x11-live-harness    (pure-shell tests, no X server)
#
# The canonical display-name rule is the single-helper contract ported from
# .vendor/xfwm4-master/common/xfwm-common.c:191-224 (substitute_screen_number ->
# xfwm_make_display_name): one typed function owns the display string, callers
# never build it by concatenation. No vendor code is linked, loaded or run.
# The free-display rule follows the local socket+lock precedent in
# scripts/break/lib.sh:23-29 (a socket-only check misses stale server locks).
set -uo pipefail

# Hermetic roots: tests override these so no host X socket or lock is touched.
X11_LIVE_SOCKET_DIR="${X11_LIVE_SOCKET_DIR:-/tmp/.X11-unix}"
X11_LIVE_LOCK_DIR="${X11_LIVE_LOCK_DIR:-/tmp}"

# x11_live_display_name NUM -> canonical ":NUM". rc 2 on a non-numeric NUM.
# Single owner of the DISPLAY value; callers must not concatenate it by hand.
x11_live_display_name() {
  local num="${1:-}"
  [[ "$num" =~ ^[0-9]+$ ]] || return 2
  printf ':%s\n' "$num"
}

# x11_live_pick_display FIRST LAST HINT -> first free display number in
# [FIRST, LAST], scanned from HINT and wrapping back to FIRST. A number is free
# only when neither its transport socket (X<n>) nor its server lock
# (.X<n>-lock) exists. rc 1 when all are taken, rc 2 on malformed arguments.
x11_live_pick_display() {
  local first="${1:-}" last="${2:-}" hint="${3:-}" n
  [[ -z "$hint" ]] && hint="$first"
  [[ "$first" =~ ^[0-9]+$ && "$last" =~ ^[0-9]+$ && "$hint" =~ ^[0-9]+$ ]] || return 2
  (( first <= last && hint >= first && hint <= last )) || return 2
  for ((n = hint; n <= last; n++)); do
    if [[ ! -e "$X11_LIVE_SOCKET_DIR/X$n" && ! -e "$X11_LIVE_LOCK_DIR/.X$n-lock" ]]; then
      printf '%s\n' "$n"
      return 0
    fi
  done
  for ((n = first; n < hint; n++)); do
    if [[ ! -e "$X11_LIVE_SOCKET_DIR/X$n" && ! -e "$X11_LIVE_LOCK_DIR/.X$n-lock" ]]; then
      printf '%s\n' "$n"
      return 0
    fi
  done
  return 1
}

#!/usr/bin/env bash
# Runtime proof for Wayland Alt+Tab previews (SwitcherEntry.thumb, docs/backlog/remaining-audit.md).
# Private Xvfb + private dbus-run-session + nested flamewm-wayland (winit) + Wayland xdg clients.
# Holds Alt+Tab open with XTest, then reads the WindowSwitcherChanged signals the compositor
# publishes on the private bus: a signal entry carrying a `thumb` dict with `rgba` pixels is a
# non-None SwitcherEntry.thumb. Client frame callbacks give the frame-time sample.
# Usage: ALTTAB_CLIENT_BIN=/path/to/wayland-client scripts/break/alttab-thumb.sh
# Env: FLAMEWM_BIN_DIR (default: ${CARGO_TARGET_DIR:-$ROOT/.target}/release), CLIENTS (default 3),
#      HOLD (seconds the switcher stays open, default 3).
# Evidence: .agents/runtime/break/alttab-thumb-<stamp>/{signals.log,comp.log,client-N.log,verdict.txt}
# Exit: 0 unless the verdict is FAIL. Never touches the host session bus or the real display.
set -uo pipefail
ROOT="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/../.." && pwd -P)"
SELF="$(readlink -f "${BASH_SOURCE[0]}")"
HERE="$(dirname "$SELF")"

if [[ "${1:-}" == --inner ]]; then
  # Runs inside the private dbus-run-session; every input comes from the exported variables.
  unset WAYLAND_DISPLAY
  export DISPLAY=":$XDISP" XDG_RUNTIME_DIR="$XDG" HOME="$RUN/home" XDG_CONFIG_HOME="$RUN/home/.config"
  SOCK="wl-alttab-$$"
  dbus-monitor "type='signal',interface='com.arkflame.FlameWM1',member='WindowSwitcherChanged'" \
    >"$RUN/signals.log" 2>&1 &
  MON=$!
  FLAMEWM_WAYLAND_BACKEND=winit FLAMEWM_WAYLAND_SOCKET="$SOCK" FLAMEWM_WAYLAND_AUTOSTART='' \
    FLAMEWM_WAYLAND_WIDTH=1350 FLAMEWM_WAYLAND_HEIGHT=641 RUST_BACKTRACE=1 \
    "$BIN_DIR/flamewm-wayland" >"$RUN/comp.log" 2>&1 &
  COMP=$!
  for _ in $(seq 1 150); do [[ -S "$XDG/$SOCK" ]] && break; sleep 0.2; done
  kids=()
  for ((i = 1; i <= CLIENTS; i++)); do
    WAYLAND_DISPLAY="$SOCK" "$ALTTAB_CLIENT_BIN" >"$RUN/client-$i.log" 2>&1 &
    kids+=($!)
    sleep 0.5
  done
  sleep 4
  python3 -I "$HERE/alt_hold.py" 300 300 "$HOLD" >"$RUN/altab.log" 2>&1
  hold_rc=$?
  sleep 2
  alive=$(kill -0 "$COMP" 2>/dev/null && echo yes || echo no)
  kill "${kids[@]}" "$COMP" "$MON" 2>/dev/null
  wait 2>/dev/null
  echo "inner: alt_hold_rc=$hold_rc compositor_alive_after_hold=$alive" >"$RUN/inner.txt"
  exit 0
fi

BIN_DIR="${FLAMEWM_BIN_DIR:-${CARGO_TARGET_DIR:-$ROOT/.target}/release}"
[[ "$BIN_DIR" == /* ]] || BIN_DIR="$ROOT/$BIN_DIR"
CLIENTS="${CLIENTS:-3}"
HOLD="${HOLD:-3}"
for need in "$BIN_DIR/flamewm-wayland" "${ALTTAB_CLIENT_BIN:-/nonexistent}" "$HERE/alt_hold.py"; do
  [[ -e "$need" ]] || { echo "ALTTAB-THUMB: missing $need (build flamewm-wayland; set ALTTAB_CLIENT_BIN)" >&2; exit 2; }
done
for tool in Xvfb dbus-run-session dbus-monitor python3; do
  command -v "$tool" >/dev/null || { echo "ALTTAB-THUMB: $tool required" >&2; exit 2; }
done

STAMP="$(date +%Y%m%d-%H%M%S)"
RUN="$ROOT/.agents/runtime/break/alttab-thumb-$STAMP"
mkdir -p "$RUN/home" "$RUN/xdg"
chmod 700 "$RUN/xdg"
export RUN BIN_DIR CLIENTS HOLD ALTTAB_CLIENT_BIN XDG="$RUN/xdg"
XDISP=""
for n in $(seq 170 199); do
  [[ -e "/tmp/.X11-unix/X$n" || -e "/tmp/.X$n-lock" ]] || { XDISP=$n; break; }
done
[[ -n "$XDISP" ]] || { echo "ALTTAB-THUMB: no free X display in :170-:199" >&2; exit 2; }
export XDISP

Xvfb ":$XDISP" -screen 0 1600x900x24 -nolisten tcp -noreset >"$RUN/xvfb.log" 2>&1 &
XVFB=$!
trap 'kill "$XVFB" 2>/dev/null' EXIT
for _ in $(seq 1 50); do [[ -S "/tmp/.X11-unix/X$XDISP" ]] && break; sleep 0.2; done
sleep 1

env -i PATH="$PATH" LANG=C.UTF-8 RUN="$RUN" BIN_DIR="$BIN_DIR" CLIENTS="$CLIENTS" HOLD="$HOLD" \
  ALTTAB_CLIENT_BIN="$ALTTAB_CLIENT_BIN" XDG="$XDG" XDISP="$XDISP" \
  dbus-run-session -- "$SELF" --inner
rc=$?

signals="$RUN/signals.log"
opens=$(grep -c 'member=WindowSwitcherChanged' "$signals" 2>/dev/null || true)
thumbs=$(grep -c 'string "rgba"' "$signals" 2>/dev/null || true)
panics=$(grep -c 'panicked' "$RUN/comp.log" 2>/dev/null || true)
frames=$(for f in "$RUN"/client-*.log; do
    awk '$1=="FRAME_CB"{ if (p) print $2 - p; p = $2; n++ } END { print "count", n + 0 > "/dev/stderr" }' "$f"
  done 2>"$RUN/frame-counts.txt" | sort -n | awk '
  { g[NR] = $1 } END {
    if (NR < 1) { print "gap_median_ms=na gap_max_ms=na"; exit }
    print "gap_median_ms=" g[int((NR + 1) / 2)] " gap_max_ms=" g[NR]
  }')
frames="$frames counts=$(awk '{ s = s " " $2 } END { print s }' "$RUN/frame-counts.txt" 2>/dev/null)"
verdict=FAIL
if [[ "$rc" == 0 && "$thumbs" -gt 0 && "$panics" == 0 ]] && grep -q 'compositor_alive_after_hold=yes' "$RUN/inner.txt" 2>/dev/null; then
  verdict=PASS
fi
{
  echo "ALTTAB-THUMB: $verdict switcher_opens_signals=$opens entries_with_thumb_pixels=$thumbs panics=$panics $frames"
  echo "run=$RUN inner_rc=$rc"
} | tee "$RUN/verdict.txt"
[[ "$verdict" == FAIL ]] && exit 1
exit 0

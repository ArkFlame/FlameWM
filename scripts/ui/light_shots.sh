#!/usr/bin/env bash
# Reproduce a Light (or Dark) FlameWM session on a private Xvfb and capture the chrome.
# Usage: scripts/ui/light_shots.sh [light|dark] [OUT_DIR]
# Own processes only: Xvfb, the nested session process group (setsid) and the apps this
# script starts are killed by PID on exit. Never kill by process name.
set -euo pipefail

MODE="${1:-light}"
case "$MODE" in light|dark) ;; *) echo "usage: $0 [light|dark] [OUT_DIR]" >&2; exit 2 ;; esac
ROOT="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/../.." && pwd -P)"
OUT="${2:-${ROOT}/.target/ui-shots/${MODE}}"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/.target}"
HOST_DISPLAY="${FLAMEWM_SHOTS_XVFB_DISPLAY:-:91}"
NEST_DISPLAY="${FLAMEWM_SHOTS_NEST_DISPLAY:-:90}"
GEOMETRY="${FLAMEWM_SHOTS_GEOMETRY:-1350x641}"
for bin in Xvfb xdpyinfo import dbus-run-session; do
  command -v "$bin" >/dev/null || { echo "missing: $bin" >&2; exit 3; }
done
for bin in flamewm flamewm-desktop flamewm-shell flamewm-task-manager flamewm-explorer flamewm-settings; do
  [[ -x "$TARGET_DIR/release/$bin" ]] || { echo "missing release binary: $TARGET_DIR/release/$bin (cargo build --release first)" >&2; exit 3; }
done

CFG="$OUT/cfg"
mkdir -p "$CFG" "$OUT/session"
printf 'revision=6\nappearance=%s\nwallpaper=\nfontFamily=IBM Plex Sans\ntaskbarHeight=44\n' "$MODE" >"$CFG/settings.toml"

XVFB_PID=''; SESSION_PID=''; APP_PIDS=()
cleanup() {
  for pid in "${APP_PIDS[@]:-}"; do [[ -n "$pid" ]] && kill "$pid" 2>/dev/null || true; done
  if [[ -n "$SESSION_PID" ]]; then kill -- "-$SESSION_PID" 2>/dev/null || true; fi
  [[ -n "$XVFB_PID" ]] && kill "$XVFB_PID" 2>/dev/null || true
}
trap cleanup EXIT

Xvfb "$HOST_DISPLAY" -screen 0 1600x1000x24 >"$OUT/xvfb.log" 2>&1 & XVFB_PID=$!
for _ in $(seq 1 50); do DISPLAY="$HOST_DISPLAY" xdpyinfo >/dev/null 2>&1 && break; sleep 0.2; done

# setsid makes the nested stack its own process group so cleanup kills only it.
DISPLAY="$HOST_DISPLAY" FLAMEWM_XEPHYR_DISPLAY="$NEST_DISPLAY" FLAMEWM_XEPHYR_BUILD=0 \
  FLAMEWM_CONFIG_DIR="$CFG" FLAMEWM_XEPHYR_OUTPUT_DIR="$OUT/session" \
  FLAMEWM_XEPHYR_EVIDENCE_ROOT="$OUT/perf" FLAMEWM_XEPHYR_DEBUG_ROOT="$OUT/debug" \
  CARGO_TARGET_DIR="$TARGET_DIR" \
  setsid dbus-run-session -- bash -c 'export FLAMEWM_XEPHYR_PRIVATE_DBUS_INNER=1; exec "$0"' "$ROOT/scripts/xephyr" \
  >"$OUT/xephyr.log" 2>&1 & SESSION_PID=$!
for _ in $(seq 1 150); do DISPLAY="$NEST_DISPLAY" xdpyinfo >/dev/null 2>&1 && break; sleep 0.2; done
DISPLAY="$NEST_DISPLAY" xdpyinfo >/dev/null 2>&1 || { echo "nested session did not start; see $OUT/xephyr.log" >&2; exit 4; }
sleep 3

# Apps inherit the nested session bus from the running WM.
WM_PID="$(pgrep -x flamewm | head -n1 || true)"
[[ -n "$WM_PID" ]] && BUS="$(tr '\0' '\n' <"/proc/$WM_PID/environ" | sed -n 's/^DBUS_SESSION_BUS_ADDRESS=//p')"
export DBUS_SESSION_BUS_ADDRESS="${BUS:-}"
for app in flamewm-task-manager flamewm-explorer flamewm-settings; do
  DISPLAY="$NEST_DISPLAY" "$TARGET_DIR/release/$app" >"$OUT/$app.log" 2>&1 & APP_PIDS+=("$!")
  sleep 4
done
DISPLAY="$NEST_DISPLAY" import -window root "$OUT/${MODE}-apps.png"
DISPLAY="$NEST_DISPLAY" import -window root -crop 1350x44+0+597 +repage "$OUT/${MODE}-taskbar.png" 2>/dev/null || true
echo "wrote $OUT/${MODE}-apps.png (appearance=$MODE)"

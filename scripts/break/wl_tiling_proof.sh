#!/usr/bin/env bash
# Runtime proof of Wayland tiling/snap parity with X11 (scripts/break/x11_tiling_proof.py), nested:
# private Xvfb + private dbus-run-session + nested flamewm-wayland (winit window, no autostart) +
# examples/test_client. The driver (wl_tiling_checks.py) injects Super+arrow, titlebar double-click and
# drag through XTest on the private Xvfb and asserts on the client's configure log (size, maximized)
# and on the client's pixel bounding box (position). Never touches the host display or session bus.
# Usage: scripts/break/wl_tiling_proof.sh
# Env: FLAMEWM_BIN_DIR (default: ${CARGO_TARGET_DIR:-$ROOT/.target}/debug, needs flamewm-wayland and examples/test_client).
# Evidence: .agents/runtime/break/wl-tiling-<stamp>/{comp.log,client.log,configure.log,checks.log}
# Exit: 0 only when every check passes (PASS lines), 1 on any FAIL, 2 on missing prerequisites.
set -uo pipefail
ROOT="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/../.." && pwd -P)"
SELF="$(readlink -f "${BASH_SOURCE[0]}")"
PY="$ROOT/scripts/break/wl_tiling_checks.py"

if [[ "${1:-}" == --inner ]]; then
  unset WAYLAND_DISPLAY
  export DISPLAY=":$XDISP" XDG_RUNTIME_DIR="$XDG" HOME="$RUN/home" XDG_CONFIG_HOME="$RUN/home/.config"
  SOCK="wl-tile-$$"
  setsid env FLAMEWM_WAYLAND_BACKEND=winit FLAMEWM_WAYLAND_SOCKET="$SOCK" FLAMEWM_WAYLAND_AUTOSTART='' \
    FLAMEWM_WAYLAND_WIDTH="$W" FLAMEWM_WAYLAND_HEIGHT="$H" RUST_BACKTRACE=1 \
    timeout 240 "$BIN_DIR/flamewm-wayland" >"$RUN/comp.log" 2>&1 &
  COMP=$!
  for _ in $(seq 1 150); do [[ -S "$XDG/$SOCK" ]] && break; sleep 0.2; done
  [[ -S "$XDG/$SOCK" ]] || { echo "WL_TILING: compositor socket missing" >&2; kill -KILL -- "-$COMP" 2>/dev/null; exit 3; }
  sleep 2
  env WAYLAND_DISPLAY="$SOCK" FLAMEWM_TEST_HOLD_MS=120000 FLAMEWM_TEST_SIZE=320x200 FLAMEWM_TEST_COLOR=E0A020 \
    FLAMEWM_TEST_CONFIGURE_LOG="$RUN/configure.log" "$BIN_DIR/examples/test_client" >"$RUN/client.log" 2>&1 &
  CLIENT=$!
  export SOCK
  python3 "$PY" >"$RUN/checks.log" 2>&1
  rc=$?
  cat "$RUN/checks.log"
  kill -TERM "$CLIENT" 2>/dev/null; wait "$CLIENT" 2>/dev/null
  kill -TERM -- "-$COMP" 2>/dev/null; sleep 1; kill -KILL -- "-$COMP" 2>/dev/null; wait "$COMP" 2>/dev/null
  exit "$rc"
fi

BIN_DIR="${FLAMEWM_BIN_DIR:-${CARGO_TARGET_DIR:-$ROOT/.target}/debug}"
[[ "$BIN_DIR" == /* ]] || BIN_DIR="$ROOT/$BIN_DIR"
[[ -x "$BIN_DIR/flamewm-wayland" && -x "$BIN_DIR/examples/test_client" ]] || {
  echo "WL_TILING: missing flamewm-wayland or examples/test_client under $BIN_DIR" >&2; exit 2; }
for tool in Xvfb dbus-run-session python3 import; do
  command -v "$tool" >/dev/null || { echo "WL_TILING: $tool required" >&2; exit 2; }
done
XLIB_PATH="$(python3 -c 'import Xlib,os;print(os.path.dirname(os.path.dirname(Xlib.__file__)))' 2>/dev/null)" || true
[[ -n "$XLIB_PATH" ]] || { echo "WL_TILING: python3 Xlib required" >&2; exit 2; }

STAMP="$(date +%Y%m%d-%H%M%S)"
RUN="$ROOT/.agents/runtime/break/wl-tiling-$STAMP"
mkdir -p "$RUN/home"
XDG="$(mktemp -d "${TMPDIR:-/tmp}/flamewm-wt.XXXXXX")"
chmod 700 "$XDG"
W=1024 H=640
XDISP=""
for n in $(seq 500 599); do
  [[ -e "/tmp/.X11-unix/X$n" || -e "/tmp/.X$n-lock" ]] || { XDISP=$n; break; }
done
[[ -n "$XDISP" ]] || { echo "WL_TILING: no free X display in :500-:599" >&2; exit 2; }
export RUN BIN_DIR XDG W H XDISP
Xvfb ":$XDISP" -screen 0 1280x800x24 -nolisten tcp -noreset >"$RUN/xvfb.log" 2>&1 &
XVFB=$!
trap 'kill "$XVFB" 2>/dev/null; rm -rf "$XDG"' EXIT
for _ in $(seq 1 50); do [[ -S "/tmp/.X11-unix/X$XDISP" ]] && break; sleep 0.2; done
sleep 1

env -i PATH="$PATH" LANG=C.UTF-8 RUN="$RUN" BIN_DIR="$BIN_DIR" XDG="$XDG" XDISP="$XDISP" W="$W" H="$H" \
  PYTHONPATH="$XLIB_PATH" \
  dbus-run-session -- "$SELF" --inner
rc=$?
if [[ $rc -eq 0 ]]; then echo "WL_TILING: PASS exit=0 evidence=$RUN"; else echo "WL_TILING: FAIL exit=$rc evidence=$RUN"; fi
exit "$rc"

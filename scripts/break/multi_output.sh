#!/usr/bin/env bash
# Runtime proof of the per-output work area and window migration on output removal, nested:
# private Xvfb + private dbus-run-session + nested flamewm-wayland (winit window, no autostart) with
# debug virtual outputs + examples/test_client. The primary nested output is W x H at (0,0); one
# virtual output VW x VH sits at (W,0). The driver (multi_output_checks.py) maximises the client on the
# virtual output (its work area bounds the configure), then writes the removal flag file and checks
# the client migrated inside the primary output (configure fits, pixels drawn inside the nested window).
# Never touches the host display or session bus. Never kills by name: only the PIDs it started.
# Usage: scripts/break/multi_output.sh
# Env: FLAMEWM_BIN_DIR (default: ${CARGO_TARGET_DIR:-$ROOT/.target}/debug; needs flamewm-wayland and examples/test_client).
# Evidence: .agents/runtime/break/multi-output-<stamp>/{comp.log,client.log,configure.log,checks.log}
# Exit: 0 only when every check passes, 1 on any FAIL, 2 on missing prerequisites, 3 on startup failure.
set -uo pipefail
ROOT="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/../.." && pwd -P)"
SELF="$(readlink -f "${BASH_SOURCE[0]}")"
PY="$ROOT/scripts/break/multi_output_checks.py"
W=1024 H=640 VW=800 VH=600

if [[ "${1:-}" == --inner ]]; then
  unset WAYLAND_DISPLAY
  export DISPLAY=":$XDISP" XDG_RUNTIME_DIR="$XDG" HOME="$RUN/home" XDG_CONFIG_HOME="$RUN/home/.config"
  SOCK="multi-out-$$"
  FLAG="$RUN/remove-output.flag"
  rm -f "$FLAG"
  setsid env FLAMEWM_WAYLAND_BACKEND=winit FLAMEWM_WAYLAND_SOCKET="$SOCK" FLAMEWM_WAYLAND_AUTOSTART='' \
    FLAMEWM_WAYLAND_WIDTH="$W" FLAMEWM_WAYLAND_HEIGHT="$H" RUST_BACKTRACE=1 \
    FLAMEWM_DEBUG_VIRTUAL_OUTPUTS="${VW}x${VH}@${W},0" FLAMEWM_DEBUG_REMOVE_OUTPUT_FLAG="$FLAG" \
    timeout 240 "$BIN_DIR/flamewm-wayland" >"$RUN/comp.log" 2>&1 &
  COMP=$!
  for _ in $(seq 1 150); do [[ -S "$XDG/$SOCK" ]] && break; sleep 0.2; done
  [[ -S "$XDG/$SOCK" ]] || { echo "MULTI_OUTPUT: compositor socket missing" >&2; kill -KILL -- "-$COMP" 2>/dev/null; exit 3; }
  sleep 2
  env WAYLAND_DISPLAY="$SOCK" FLAMEWM_TEST_HOLD_MS=120000 FLAMEWM_TEST_SIZE=320x200 FLAMEWM_TEST_COLOR=E0A020 \
    FLAMEWM_TEST_CONFIGURE_LOG="$RUN/configure.log" "$BIN_DIR/examples/test_client" >"$RUN/client.log" 2>&1 &
  CLIENT=$!
  export FLAG
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
  echo "MULTI_OUTPUT: missing flamewm-wayland or examples/test_client under $BIN_DIR" >&2; exit 2; }
for tool in Xvfb dbus-run-session python3 import; do
  command -v "$tool" >/dev/null || { echo "MULTI_OUTPUT: $tool required" >&2; exit 2; }
done
XLIB_PATH="$(python3 -c 'import Xlib,os;print(os.path.dirname(os.path.dirname(Xlib.__file__)))' 2>/dev/null)" || true
[[ -n "$XLIB_PATH" ]] || { echo "MULTI_OUTPUT: python3 Xlib required" >&2; exit 2; }

STAMP="$(date +%Y%m%d-%H%M%S)"
RUN="$ROOT/.agents/runtime/break/multi-output-$STAMP"
mkdir -p "$RUN/home"
XDG="$(mktemp -d "${TMPDIR:-/tmp}/flamewm-mo.XXXXXX")"
chmod 700 "$XDG"
XDISP=""
for n in $(seq 600 699); do
  [[ -e "/tmp/.X11-unix/X$n" || -e "/tmp/.X$n-lock" ]] || { XDISP=$n; break; }
done
[[ -n "$XDISP" ]] || { echo "MULTI_OUTPUT: no free X display in :600-:699" >&2; exit 2; }
export RUN BIN_DIR XDG W H VW VH XDISP
Xvfb ":$XDISP" -screen 0 1280x800x24 -nolisten tcp -noreset >"$RUN/xvfb.log" 2>&1 &
XVFB=$!
trap 'kill "$XVFB" 2>/dev/null; rm -rf "$XDG"' EXIT
for _ in $(seq 1 50); do [[ -S "/tmp/.X11-unix/X$XDISP" ]] && break; sleep 0.2; done
sleep 1

env -i PATH="$PATH" LANG=C.UTF-8 RUN="$RUN" BIN_DIR="$BIN_DIR" XDG="$XDG" XDISP="$XDISP" W="$W" H="$H" \
  VW="$VW" VH="$VH" PYTHONPATH="$XLIB_PATH" \
  dbus-run-session -- "$SELF" --inner
rc=$?
if [[ $rc -eq 0 ]]; then echo "MULTI_OUTPUT: PASS exit=0 evidence=$RUN"; else echo "MULTI_OUTPUT: FAIL exit=$rc evidence=$RUN"; fi
exit "$rc"

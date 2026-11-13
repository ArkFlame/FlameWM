#!/usr/bin/env bash
# Runtime proof that a Wayland clipboard text survives its owner's exit (nested winit compositor).
# Private Xvfb + private dbus-run-session + nested flamewm-wayland + examples/clipboard_client:
#   1. writer offers a text selection, the compositor reads and keeps it, then the writer exits;
#   2. a second client (reader) receives the text after the owner is gone -> PASS "persist";
#   3. an oversize selection (> 1 MiB) is writer-exited and not kept -> reader sees no text -> PASS "cap".
# Usage: scripts/break/clipboard_persist.sh
# Env: FLAMEWM_BIN_DIR (default: ${CARGO_TARGET_DIR:-$ROOT/.target}/debug); binaries from
#      cargo build -p flamewm-wm-wayland --bin flamewm-wayland --example clipboard_client
# Evidence: .agents/runtime/break/clipboard-persist-<stamp>/ (comp.log, writer*.log, reader*.log, verdict.txt)
# Exit: 0 only on PASS. Never touches the host session bus or the real display.
set -uo pipefail
ROOT="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/../.." && pwd -P)"
SELF="$(readlink -f "${BASH_SOURCE[0]}")"

if [[ "${1:-}" == --inner ]]; then
  # Runs inside the private dbus-run-session; every input comes from the exported variables.
  unset WAYLAND_DISPLAY
  export DISPLAY=":$XDISP" XDG_RUNTIME_DIR="$XDG" HOME="$RUN/home" XDG_CONFIG_HOME="$RUN/home/.config"
  SOCK="wl-clip-$$"
  setsid env FLAMEWM_WAYLAND_BACKEND=winit FLAMEWM_WAYLAND_SOCKET="$SOCK" FLAMEWM_WAYLAND_AUTOSTART='' \
    FLAMEWM_WAYLAND_WIDTH=640 FLAMEWM_WAYLAND_HEIGHT=360 RUST_BACKTRACE=1 \
    timeout 200 "$BIN_DIR/flamewm-wayland" >"$RUN/comp.log" 2>&1 &
  COMP=$!
  for _ in $(seq 1 150); do [[ -S "$XDG/$SOCK" ]] && break; sleep 0.2; done
  export WAYLAND_DISPLAY="$SOCK"
  CLIENT="$BIN_DIR/examples/clipboard_client"

  # Persist: the owner writes, the compositor keeps the text, the owner's writer exits on cancel.
  printf 'flamewm persist proof' >"$RUN/small.txt"
  "$CLIENT" writer "@$RUN/small.txt" 15000 >"$RUN/writer1.log" 2>&1 &
  W1=$!
  for _ in $(seq 1 100); do grep -q 'done cancelled=true' "$RUN/writer1.log" 2>/dev/null && break; sleep 0.2; done
  wait "$W1"; echo "writer1_exit=$?" >>"$RUN/verdict.txt"
  "$CLIENT" reader 10000 >"$RUN/reader1.log" 2>&1
  echo "reader1_exit=$?" >>"$RUN/verdict.txt"

  # Cap: an oversize selection is dropped; the reader then finds no text.
  head -c 1100000 /dev/zero | tr '\0' a >"$RUN/big.txt"
  "$CLIENT" writer "@$RUN/big.txt" 15000 >"$RUN/writer2.log" 2>&1 &
  W2=$!
  for _ in $(seq 1 100); do grep -q 'done cancelled=' "$RUN/writer2.log" 2>/dev/null && break; sleep 0.2; done
  kill -TERM "$W2" 2>/dev/null; wait "$W2" 2>/dev/null
  "$CLIENT" reader 4000 >"$RUN/reader2.log" 2>&1
  echo "reader2_exit=$?" >>"$RUN/verdict.txt"

  kill -TERM -- "-$COMP" 2>/dev/null; sleep 1; kill -KILL -- "-$COMP" 2>/dev/null; wait "$COMP" 2>/dev/null
  exit 0
fi

BIN_DIR="${FLAMEWM_BIN_DIR:-${CARGO_TARGET_DIR:-$ROOT/.target}/debug}"
[[ "$BIN_DIR" == /* ]] || BIN_DIR="$ROOT/$BIN_DIR"
[[ -x "$BIN_DIR/flamewm-wayland" && -x "$BIN_DIR/examples/clipboard_client" ]] || {
  echo "CLIPBOARD_PERSIST: missing flamewm-wayland or examples/clipboard_client under $BIN_DIR" >&2; exit 2; }
for tool in Xvfb dbus-run-session xvfb-run; do
  command -v "$tool" >/dev/null || { echo "CLIPBOARD_PERSIST: $tool required" >&2; exit 2; }
done

STAMP="$(date +%Y%m%d-%H%M%S)"
RUN="$ROOT/.agents/runtime/break/clipboard-persist-$STAMP"
mkdir -p "$RUN/home/.config" "$RUN/xdg" && chmod 700 "$RUN/xdg"
XDISP=$((99 + RANDOM % 300))
XDG="$RUN/xdg"
Xvfb ":$XDISP" -screen 0 1280x800x24 -nolisten tcp >"$RUN/xvfb.log" 2>&1 &
XVFB=$!
sleep 1
export XDISP XDG RUN BIN_DIR
env -i PATH="$PATH" XDISP="$XDISP" XDG="$XDG" RUN="$RUN" BIN_DIR="$BIN_DIR" \
  timeout 300 dbus-run-session -- "$SELF" --inner
kill -TERM "$XVFB" 2>/dev/null; wait "$XVFB" 2>/dev/null

verdict=FAIL
if grep -q '^writer1_exit=0' "$RUN/verdict.txt" 2>/dev/null \
  && grep -q '^CLIPBOARD-READ flamewm persist proof$' "$RUN/reader1.log" 2>/dev/null \
  && grep -q '^reader2_exit=' "$RUN/verdict.txt" && ! grep -q 'CLIPBOARD-READ flamewm\|^CLIPBOARD-READ a' "$RUN/reader2.log" 2>/dev/null \
  && ! grep -qi 'panicked' "$RUN/comp.log"; then
  verdict=PASS
fi
echo "$verdict" >>"$RUN/verdict.txt"
printf 'CLIPBOARD_PERSIST: %s evidence=%s\n' "$verdict" "$RUN"
[[ "$verdict" == PASS ]]

#!/usr/bin/env bash
# Runtime proof of the Wayland clipboard history over the Flame control bus (nested winit compositor).
# Private Xvfb + private dbus-run-session + nested flamewm-wayland + examples/clipboard_client:
#   1. three writers each copy one text and exit; the compositor keeps each as a history entry;
#   2. ListClipboardHistory lists three previews newest first ("three", "two", "one");
#   3. SelectClipboardHistory(id of "two") makes it the selection; a reader pastes "two" -> PASS "select";
#   4. ClearClipboardHistory empties the list -> PASS "clear";
#   5. the compositor log never contains the clipboard texts -> PASS "nolog".
# Usage: scripts/break/clipboard_history_proof.sh
# Env: FLAMEWM_BIN_DIR (default: ${CARGO_TARGET_DIR:-$ROOT/.target}/debug); binaries from
#      cargo build -p flamewm-wm-wayland --bin flamewm-wayland --example clipboard_client
# Evidence: .agents/runtime/break/clipboard-history-<stamp>/ (comp.log, writer*.log, list*.txt, reader.log, verdict.txt)
# Exit: 0 only on PASS. Never touches the host session bus or the real display.
set -uo pipefail
ROOT="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/../.." && pwd -P)"
SELF="$(readlink -f "${BASH_SOURCE[0]}")"

if [[ "${1:-}" == --inner ]]; then
  # Runs inside the private dbus-run-session; every input comes from the exported variables.
  unset WAYLAND_DISPLAY
  export DISPLAY=":$XDISP" XDG_RUNTIME_DIR="$XDG" HOME="$RUN/home" XDG_CONFIG_HOME="$RUN/home/.config"
  SOCK="wl-hist-$$"
  setsid env FLAMEWM_WAYLAND_BACKEND=winit FLAMEWM_WAYLAND_SOCKET="$SOCK" FLAMEWM_WAYLAND_AUTOSTART='' \
    FLAMEWM_WAYLAND_WIDTH=640 FLAMEWM_WAYLAND_HEIGHT=360 RUST_BACKTRACE=1 \
    timeout 200 "$BIN_DIR/flamewm-wayland" >"$RUN/comp.log" 2>&1 &
  COMP=$!
  for _ in $(seq 1 150); do [[ -S "$XDG/$SOCK" ]] && break; sleep 0.2; done
  export WAYLAND_DISPLAY="$SOCK"
  CLIENT="$BIN_DIR/examples/clipboard_client"
  DEST=com.arkflame.FlameWM1
  PATHOBJ=/com/arkflame/FlameWM1
  IFACE=com.arkflame.FlameWM1.Clipboard
  # Waits until the bus name is owned (the control service is up) before the first call.
  for _ in $(seq 1 100); do
    dbus-send --session --print-reply --dest="$DEST" "$PATHOBJ" org.freedesktop.DBus.Peer.Ping >/dev/null 2>&1 && break
    sleep 0.2
  done

  # 1. Copy three texts. Each writer holds its selection long enough for the compositor to read it.
  for word in one two three; do
    printf 'flamewm history %s' "$word" >"$RUN/$word.txt"
    "$CLIENT" writer "@$RUN/$word.txt" 2500 >"$RUN/writer-$word.log" 2>&1
    echo "writer_$word=$?" >>"$RUN/verdict.txt"
  done

  # 2. List over the bus: ids and previews only.
  dbus-send --session --print-reply --dest="$DEST" "$PATHOBJ" "$IFACE.List" >"$RUN/list1.txt" 2>&1
  echo "list1_exit=$?" >>"$RUN/verdict.txt"
  ID_TWO="$(python3 -I -c '
import re,sys
text=open(sys.argv[1]).read()
ids=[int(m) for m in re.findall(r"string \"id\"\s+variant\s+uint64 (\d+)", text)]
previews=re.findall(r"string \"preview\"\s+variant\s+string \"([^\"]*)\"", text)
pairs=list(zip(ids,previews))
print(";".join(f"{i}:{p}" for i,p in pairs))
' "$RUN/list1.txt")"
  echo "list1_entries=$ID_TWO" >>"$RUN/verdict.txt"
  TWO_ID="$(printf '%s' "$ID_TWO" | tr ';' '\n' | sed -n 's/^\([0-9]*\):flamewm history two$/\1/p')"

  # 3. Select the second entry and paste it.
  if [[ -n "$TWO_ID" ]]; then
    dbus-send --session --print-reply --dest="$DEST" "$PATHOBJ" "$IFACE.Select" uint64:"$TWO_ID" >"$RUN/select.txt" 2>&1
    echo "select_exit=$?" >>"$RUN/verdict.txt"
  fi
  "$CLIENT" reader 6000 >"$RUN/reader.log" 2>&1
  echo "reader_exit=$?" >>"$RUN/verdict.txt"

  # 4. Clear, then list is empty.
  dbus-send --session --print-reply --dest="$DEST" "$PATHOBJ" "$IFACE.Clear" >"$RUN/clear.txt" 2>&1
  echo "clear_exit=$?" >>"$RUN/verdict.txt"
  dbus-send --session --print-reply --dest="$DEST" "$PATHOBJ" "$IFACE.List" >"$RUN/list2.txt" 2>&1

  kill -TERM -- "-$COMP" 2>/dev/null; sleep 1; kill -KILL -- "-$COMP" 2>/dev/null; wait "$COMP" 2>/dev/null
  exit 0
fi

BIN_DIR="${FLAMEWM_BIN_DIR:-${CARGO_TARGET_DIR:-$ROOT/.target}/debug}"
[[ "$BIN_DIR" == /* ]] || BIN_DIR="$ROOT/$BIN_DIR"
[[ -x "$BIN_DIR/flamewm-wayland" && -x "$BIN_DIR/examples/clipboard_client" ]] || {
  echo "CLIPBOARD_HISTORY: missing flamewm-wayland or examples/clipboard_client under $BIN_DIR" >&2; exit 2; }
for tool in Xvfb dbus-run-session dbus-send python3; do
  command -v "$tool" >/dev/null || { echo "CLIPBOARD_HISTORY: $tool required" >&2; exit 2; }
done

STAMP="$(date +%Y%m%d-%H%M%S)"
RUN="$ROOT/.agents/runtime/break/clipboard-history-$STAMP"
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
python3 -I - "$RUN" <<'PY' && verdict=PASS
import re, sys, pathlib
run = pathlib.Path(sys.argv[1])
def read(name):
    p = run / name
    return p.read_text(errors="replace") if p.exists() else ""
v = read("verdict.txt")
ok = all(f"writer_{w}=0" in v for w in ("one", "two", "three"))
list1 = read("list1.txt")
previews = re.findall(r'string "preview"\s+variant\s+string "([^"]*)"', list1)
ok = ok and "list1_exit=0" in v and previews == ["flamewm history three", "flamewm history two", "flamewm history one"]
reader = read("reader.log")
ok = ok and "select_exit=0" in v and "CLIPBOARD-READ flamewm history two" in reader
ok = ok and "clear_exit=0" in v and not re.findall(r'string "preview"', read("list2.txt"))
comp = read("comp.log")
ok = ok and "panicked" not in comp and not any(t in comp for t in ("flamewm history one", "flamewm history two", "flamewm history three"))
sys.exit(0 if ok else 1)
PY
echo "$verdict" >>"$RUN/verdict.txt"
printf 'CLIPBOARD_HISTORY: %s evidence=%s\n' "$verdict" "$RUN"
[[ "$verdict" == PASS ]]

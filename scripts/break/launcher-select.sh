#!/usr/bin/env bash
# Launcher selected-row and backdrop measurement (wsh-launcher-select) on a private Xvfb, X11 session.
# Usage: [BREAK_LAUNCHER_SESSION=wayland] scripts/break/launcher-select.sh [default|accent|backdrop]...
#   (no args: all three). Default session is X11. wayland runs the nested winit compositor (scripts/wayland
#   env, private dbus-run-session) inside the private Xvfb, so the nested window is the only graphical client.
# Per config: settings.toml overrides, Super+Space via XTest, pointer hover on row 1 (selects it),
# import screenshots, PIL RGB medians. Evidence: .agents/runtime/break/<stamp>/<config>/ and results.tsv.
# Exit: 0 when every requested config measured, 1 otherwise. Sampling boxes are for the 1280x800 sandbox.
set -uo pipefail
# shellcheck disable=SC1091
source "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/lib.sh"
KEY="$BREAK_DIR/key.py"
POINTER="$BREAK_DIR/pointer.py"
SESSION="${BREAK_LAUNCHER_SESSION:-x11}"
# Pointer position on row 1 (hover selects it). X11 sandbox: 1280x800 layout. Nested: set after the open shot.
HOVER_X="${BREAK_HOVER_X:-600}"
HOVER_Y="${BREAK_HOVER_Y:-224}"

write_settings() { # config: writes the settings.toml overrides the session reads at start
  case "$1" in
    default) : ;;
    accent) printf 'accentCustom=1\naccent=#E53935\n' ;;
    backdrop) printf 'surfaceCustom=1\nsurfaceColor=#405060\nsurfaceOpacity=60\n' ;;
    *) return 1 ;;
  esac
}

measure() { # closed_shot open_shot hover_shot -> one line of RGB medians
  python3 -I - "$1" "$2" "$3" <<'PY'
import statistics, sys
from PIL import Image

def med(path, box):
    im = Image.open(path).convert("RGB").crop(box)
    px = list(im.getdata())
    return tuple(int(statistics.median(p[i] for p in px)) for i in range(3))

closed, opened, hover = sys.argv[1], sys.argv[2], sys.argv[3]
sel = med(hover, (800, 208, 921, 240))      # row 1 (Ark), selected by hover
unsel = med(hover, (800, 248, 921, 280))    # row 2 (Audacity), unselected
surf = med(opened, (905, 380, 931, 396))    # surface margin, no row content
wall = med(closed, (905, 380, 931, 396))    # wallpaper under the same spot, launcher closed
print(f"selected={sel} unselected={unsel} surface={surf} wallpaper_under_surface={wall}")
PY
}

start_nested() { # nested winit FlameWM inside the private Xvfb, as scripts/wayland sets it up
  cp "$BREAK_BIN_SRC/flamewm-wayland" "$S/bin/" || return 1
  setsid env -u WAYLAND_DISPLAY -u SSH_AUTH_SOCK HOME="$S/home" XDG_CONFIG_HOME="$S/home/.config" \
    XDG_STATE_HOME="$S/home/.local/state" XDG_DATA_HOME="$S/home/.local/share" XDG_RUNTIME_DIR="$S/tmp" \
    TMPDIR="$S/tmp" XDG_CURRENT_DESKTOP=FlameWM DISPLAY=":$DISP" PATH="$S/bin:/usr/bin:/bin" LANG=C.UTF-8 \
    XDG_MENU_PREFIX=flamewm- RWR_UI_FONT="IBM Plex Sans" FLAMEWM_WAYLAND_BACKEND=winit \
    FLAMEWM_WAYLAND_WIDTH=1350 FLAMEWM_WAYLAND_HEIGHT=641 \
    FLAMEWM_WAYLAND_AUTOSTART="$S/bin/flamewm-desktop:$S/bin/flamewm-shell" \
    dbus-run-session -- "$S/bin/flamewm-wayland" >"$S/session.out" 2>&1 &
  # shellcheck disable=SC2034 # read by teardown in lib.sh
  SPID=$!
  for _ in $(seq 1 300); do DISPLAY=":$DISP" xwininfo -root -children 2>/dev/null | grep -q '1350x641' && break; sleep 0.2; done
  local wid
  wid="$(DISPLAY=":$DISP" xwininfo -root -children 2>/dev/null | awk '/1350x641/{print $1; exit}')"
  [ -n "$wid" ] || return 1
  # No WM on the Xvfb: give the nested window X input focus so XTest keys reach the compositor.
  python3 -I - "$DISP" "$wid" <<'PY'
import ctypes, sys
x11 = ctypes.CDLL("libX11.so.6")
x11.XOpenDisplay.restype = ctypes.c_void_p
x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
x11.XSetInputFocus.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong]
x11.XFlush.argtypes = [ctypes.c_void_p]
dpy = x11.XOpenDisplay(f":{sys.argv[1]}".encode())
x11.XSetInputFocus(dpy, int(sys.argv[2], 16), 1, 0)
x11.XFlush(dpy)
PY
  sleep 2
  return 0
}

run_config() { # config
  local cfg=$1 out
  sandbox_new "$cfg" || return 1
  mkdir -p "$S/home/.config/flamewm"
  write_settings "$cfg" > "$S/home/.config/flamewm/settings.toml" || return 1
  if [ "$SESSION" = wayland ]; then
    start_nested || { log "$cfg: nested window not up"; teardown; return 1; }
    # No WM on the Xvfb: keys go to the window under the pointer, so park the pointer on the nested window.
    python3 -I "$POINTER" sweep 320 600 600 3 1 >/dev/null
  else
    start_session bus
    wait_until 30 wm_ready || { log "$cfg: WM not ready"; teardown; return 1; }
  fi
  sleep 12
  shot closed >/dev/null
  python3 -I "$KEY" Super_L+space >/dev/null && sleep 4
  shot open >/dev/null
  python3 -I "$POINTER" sweep "$HOVER_Y" "$HOVER_X" "$HOVER_X" 5 1 >/dev/null && sleep 1
  shot hover >/dev/null
  out="$(measure "$S/shot-closed.png" "$S/shot-open.png" "$S/shot-hover.png" 2>"$S/measure.err")" || { log "$cfg: measure failed"; teardown; return 1; }
  record "launcher-select:$SESSION:$cfg" "MEASURED" "$out settings_keys=$(grep -c . "$S/home/.config/flamewm/settings.toml") session=$S"
  teardown
}

configs=("$@")
[ ${#configs[@]} -eq 0 ] && configs=(default accent backdrop)
rc=0
for c in "${configs[@]}"; do run_config "$c" || { record "launcher-select:$SESSION:$c" "FAIL" "see $RUN"; rc=1; }; done
exit $rc

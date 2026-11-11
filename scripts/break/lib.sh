#!/usr/bin/env bash
# Shared helpers for scripts/break. Source only; see break.sh.
# Every scenario gets a private Xvfb display, private HOME/XDG tree and copies of the fast binaries.
# Nothing here touches the user's real display or session bus.
set -uo pipefail
BREAK_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
REPO="$(cd "$BREAK_DIR/../.." && pwd -P)"
BIN_SRC="${BREAK_BIN_SRC:-/run/media/linsaftw/SDD/gate-targets/break-fast/fast}"
RUN="${BREAK_RUN:-$REPO/.agents/runtime/break/$(date +%Y%m%d-%H%M%S)}"
RESULTS="$RUN/results.tsv"
CLK_TCK="$(getconf CLK_TCK)"
mkdir -p "$RUN" && touch "$RESULTS"

log() { printf '[break] %s\n' "$*" >&2; }
record() { printf '%s\t%s\t%s\n' "$1" "$2" "$3" >>"$RESULTS"; log "$1 $2 $3"; }

wait_until() { # timeout_s cmd...
  local t=$1 i; shift
  for ((i = 0; i < t * 5; i++)); do "$@" && return 0; sleep 0.2; done
  return 1
}

pick_display() {
  local n
  for n in $(seq 150 199); do
    [ -e "/tmp/.X11-unix/X$n" ] || [ -e "/tmp/.X$n-lock" ] || { echo "$n"; return 0; }
  done
  return 1
}

sandbox_new() { # name: sets S (dir), DISP, XPID
  S="$RUN/$1"; rm -rf "$S"
  mkdir -p "$S/bin" "$S/home/.config" "$S/home/.local/state" "$S/home/.local/share" "$S/tmp"
  local b
  for b in flamewm flamewm-session flamewm-shell flamewm-desktop; do
    [ -x "$BIN_SRC/$b" ] || { log "missing binary $BIN_SRC/$b (build: cargo build --profile fast -p flamewm-wm -p flamewm-shell -p flamewm-desktop)"; return 1; }
    cp "$BIN_SRC/$b" "$S/bin/$b"
  done
  DISP="$(pick_display)" || { log "no free display in :150-:199"; return 1; }
  # -noreset: without it the server resets when the last client disconnects (the xprop probe
  # above), and a client connecting during that reset gets EOF in x11rb setup (break-b2).
  Xvfb ":$DISP" -screen 0 1280x800x24 -nolisten tcp -noreset +extension RANDR >"$S/xvfb.log" 2>&1 &
  XPID=$!
  # The socket file appears before the server accepts setup; gate on a real X round-trip.
  export DISPLAY=":$DISP"
  wait_until 10 xprop -root >/dev/null 2>&1 || { log "Xvfb :$DISP did not accept clients"; return 1; }
}

start_session() { # bus|nobus
  local envs=(env -u WAYLAND_DISPLAY -u SSH_AUTH_SOCK
    HOME="$S/home" XDG_CONFIG_HOME="$S/home/.config" XDG_STATE_HOME="$S/home/.local/state"
    XDG_DATA_HOME="$S/home/.local/share" XDG_RUNTIME_DIR="$S/tmp" TMPDIR="$S/tmp"
    XDG_CURRENT_DESKTOP=FlameWM DISPLAY=":$DISP" PATH="$S/bin:/usr/bin:/bin" LANG=C.UTF-8)
  if [ "$1" = bus ]; then
    setsid dbus-run-session -- "${envs[@]}" "$S/bin/flamewm-session" >"$S/session.out" 2>&1 &
  else
    setsid "${envs[@]}" DBUS_SESSION_BUS_ADDRESS="unix:path=$S/tmp/absent.sock" \
      "$S/bin/flamewm-session" >"$S/session.out" 2>&1 &
  fi
  SPID=$!
}

pids_named() { # comm -> pids on this sandbox display
  local p
  for p in $(pgrep -x "$1" 2>/dev/null); do
    tr '\0' '\n' 2>/dev/null <"/proc/$p/environ" | grep -qx "DISPLAY=:$DISP" && echo "$p"
  done
  return 0
}
have_proc() { [ -n "$(pids_named "$1")" ]; }
have_new_proc() { [ -n "$(pids_named "$1" | grep -vx "$2")" ]; }
new_pid() { pids_named "$1" | grep -vx "$2" | head -1; }
rss_kb() { awk '/VmRSS/{print $2}' "/proc/$1/status" 2>/dev/null; }
cpu_ticks() { awk '{print $14+$15}' "/proc/$1/stat" 2>/dev/null; }
wm_ready() { DISPLAY=":$DISP" xprop -root _NET_SUPPORTING_WM_CHECK 2>/dev/null | grep -q window; }
window_count() { DISPLAY=":$DISP" xwininfo -root -children 2>/dev/null | grep -c '^ *0x'; }
active_window() { DISPLAY=":$DISP" xprop -root _NET_ACTIVE_WINDOW 2>/dev/null | awk -F'# ' '{print $2}'; }
# panel_geometry root_w root_h: one line per window owning a non-zero _NET_WM_STRUT_PARTIAL, then
# "panels=N bad=M". Reads the real X geometry (xwininfo -id) and checks the panel sits on its strut
# edge with the root's full width (top/bottom) or height (left/right) and thickness == strut value.
panel_geometry() {
  local rw=$1 rh=$2 id p g
  for id in $(DISPLAY=":$DISP" xwininfo -root -tree 2>/dev/null | grep -o '0x[0-9a-f]*' | sort -u); do
    p="$(DISPLAY=":$DISP" xprop -id "$id" _NET_WM_STRUT_PARTIAL 2>/dev/null | sed -n 's/.*= //p')"
    [ -n "$p" ] || continue
    g="$(DISPLAY=":$DISP" xwininfo -id "$id" 2>/dev/null | awk -F': *' '/Absolute upper-left X/{x=$2} /Absolute upper-left Y/{y=$2} /^ *Width:/{w=$2} /^ *Height:/{h=$2} END{print x, y, w, h}')"
    echo "$id $(echo "$p" | tr -d ,) $g"
  done | awk -v rw="$rw" -v rh="$rh" '
    { n++; s[0]=$2; s[1]=$3; s[2]=$4; s[3]=$5; x=$14; y=$15; w=$16; h=$17; edge="none"; ok=1
      if (s[3]>0)      { edge="bottom"; t=s[3]; ok=(y+h==rh && h==t && x==0 && w==rw) }
      else if (s[2]>0) { edge="top";    t=s[2]; ok=(y==0 && h==t && x==0 && w==rw) }
      else if (s[0]>0) { edge="left";   t=s[0]; ok=(x==0 && w==t && y==0 && h==rh) }
      else if (s[1]>0) { edge="right";  t=s[1]; ok=(x+w==rw && w==t && y==0 && h==rh) }
      if (edge=="none") { ok=1 }
      if (!ok) bad++
      printf "PANEL id=%s edge=%s abs=(%s,%s) size=%sx%s strut_thickness=%s want_span=%s strut_lrtb=%s,%s,%s,%s verdict=%s\n", $1, edge, x, y, w, h, t, (edge=="left"||edge=="right")?rh:rw, s[0], s[1], s[2], s[3], ok?"OK":"MISPLACED"
    }
    END { printf "panels=%d bad=%d\n", n+0, bad+0 }'
}
shot() { DISPLAY=":$DISP" import -window root "$S/shot-$1.png" 2>/dev/null; echo "$S/shot-$1.png"; }
panic_count() { grep -rhc 'panicked at' "$S/session.out" "$S/home" 2>/dev/null | awk '{s+=$1} END {print s+0}'; }
logs_grep() { grep -rhi "$1" "$S/session.out" "$S/home" 2>/dev/null | head -"${2:-5}"; }

reap_display() { # kill every process bound to this sandbox display only
  local p
  for p in /proc/[0-9]*; do
    p=${p#/proc/}
    tr '\0' '\n' 2>/dev/null <"/proc/$p/environ" | grep -qx "DISPLAY=:$DISP" && kill -KILL "$p" 2>/dev/null
  done
  return 0
}

teardown() {
  [ -n "${SPID:-}" ] && kill -TERM -- "-$SPID" 2>/dev/null
  sleep 1
  [ -n "${DISP:-}" ] && reap_display
  [ -n "${XPID:-}" ] && kill "$XPID" 2>/dev/null
  wait "${XPID:-}" 2>/dev/null
  SPID="" XPID="" DISP=""
  unset DISPLAY
}

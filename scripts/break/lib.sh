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
  Xvfb ":$DISP" -screen 0 1280x800x24 -nolisten tcp +extension RANDR >"$S/xvfb.log" 2>&1 &
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

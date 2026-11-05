#!/usr/bin/env bash
# settings_session.sh: boots a nested FlameWM session with flamewm-settings autostarted and runs
# settings_scenarios.py (slider drag, tab switch) against it. Same bootstrap as fm.sh.
# usage: settings_session.sh <bin-dir|target-dir> <out-dir> --slider x0,x1,y --tabs x,y[;x,y...] [--steps N]
set -u
[ $# -ge 2 ] || { echo "usage: settings_session.sh <bin-dir|target-dir> <out-dir> --slider x0,x1,y --tabs x,y[;...] [--steps N]" >&2; exit 2; }
T=$(readlink -f "$1"); OUT=$2; shift 2
HERE=$(cd "$(dirname "$0")" && pwd -P)

BIN=""
for c in "$T" "$T/release" "$T/fast" "$T/debug"; do
  if [ -x "$c/flamewm-wayland" ]; then BIN=$c; break; fi
done
[ -n "$BIN" ] || { echo "no flamewm-wayland under $T" >&2; exit 1; }
for b in flamewm-desktop flamewm-shell flamewm-settings; do
  [ -x "$BIN/$b" ] || { echo "missing $BIN/$b" >&2; exit 1; }
done

RUN=$OUT/$(date +%Y%m%d-%H%M%S)-$$
mkdir -p "$RUN/home" "$RUN/rt"; chmod 700 "$RUN/rt"
HOSTD=:$((300 + RANDOM % 200))
while [ -e /tmp/.X11-unix/X${HOSTD#:} ]; do HOSTD=:$((300 + RANDOM % 200)); done
XV=""; PG=""; WM=""
cleanup(){
  [ -n "$PG" ] && kill -TERM -- -"$PG" 2>/dev/null
  sleep 1
  [ -n "$PG" ] && kill -KILL -- -"$PG" 2>/dev/null
  [ -n "$XV" ] && kill "$XV" 2>/dev/null
}
trap cleanup EXIT

Xvfb $HOSTD -screen 0 1280x800x24 -nolisten tcp -noreset -ac >"$RUN/xvfb.log" 2>&1 & XV=$!
for _ in $(seq 1 40); do DISPLAY=$HOSTD xdpyinfo >/dev/null 2>&1 && break; sleep .25; done
env -u WAYLAND_DISPLAY -u XAUTHORITY DISPLAY=$HOSTD HOME=$RUN/home XDG_RUNTIME_DIR=$RUN/rt \
  XDG_CONFIG_HOME=$RUN/home/.config XDG_STATE_HOME=$RUN/home/.local/state XDG_CACHE_HOME=$RUN/home/.cache \
  XDG_DATA_HOME=$RUN/home/.local/share FLAMEWM_WAYLAND_BACKEND=winit \
  FLAMEWM_WAYLAND_AUTOSTART=$BIN/flamewm-desktop:$BIN/flamewm-shell:$BIN/flamewm-settings PATH=$BIN:/usr/bin:/bin \
  setsid dbus-run-session -- $BIN/flamewm-wayland >"$RUN/compositor.log" 2>&1 & WM=$!
sleep 1
PG=$(ps -o pgid= -p $WM 2>/dev/null | tr -d ' ')
if [ "$PG" != "$WM" ]; then echo "refusing to signal group: pgid=$PG pid=$WM" >&2; PG=""; exit 1; fi
for _ in $(seq 1 60); do
  pgrep -g $PG -x flamewm-shell >/dev/null && pgrep -g $PG -x flamewm-settings >/dev/null && break
  sleep .25
done
sleep 3
python3 "$HERE/settings_scenarios.py" "$PG" "$HOSTD" "$RUN/shot" "$@" > "$RUN/result.json"
RC=$?
echo "run=$RUN rc=$RC" >&2
exit $RC

#!/usr/bin/env bash
# Session-lock, focus/urgency and media measurement for wsh-lock-x11 on a private Xvfb (X11 session).
# Usage: scripts/break/lock-x11.sh
# Lock: Super+L (default ACTION_SESSION_LOCK) via XTest; full-screen lock window present; Super+Space
#   while locked must not open the launcher (pixel diff of the locked screen); SIGKILL of flamewm-lock
#   must respawn it. Focus/urgency: unsolicited _NET_ACTIVE_WINDOW from an app with no user input must not
#   take focus and should set _NET_WM_STATE_DEMANDS_ATTENTION. Media: checks for an audio backend first.
# Evidence: .agents/runtime/break/<stamp>/lock-x11/ and results.tsv. Exit 0 when no FAIL.
set -uo pipefail
# shellcheck disable=SC1091
source "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/lib.sh"
KEY="$BREAK_DIR/key.py"
XCLIENT="$BREAK_DIR/xclient.py"
rc=0

diff_pct() { # shot_a shot_b -> percent of pixels that differ by more than 16 in any channel
  python3 -I - "$1" "$2" <<'PY'
import sys
from PIL import Image, ImageChops
a = Image.open(sys.argv[1]).convert("RGB")
b = Image.open(sys.argv[2]).convert("RGB")
d = ImageChops.difference(a, b).convert("L").point(lambda v: 255 if v > 16 else 0)
hist = d.histogram()
changed = hist[255]
print(f"{100.0 * changed / (a.size[0] * a.size[1]):.2f}")
PY
}

cover_geom() { # 1280x800+0+0 window present on root children -> yes|no
  DISPLAY=":$DISP" xwininfo -root -children 2>/dev/null | grep -q '1280x800+0+0' && echo yes || echo no
}

sandbox_new lock-x11 || exit 1
# The WM spawns flamewm-lock from its own directory, so the lock binary must sit beside flamewm.
cp "$BREAK_BIN_SRC/flamewm-lock" "$S/bin/flamewm-lock" || { log "missing flamewm-lock"; teardown; exit 1; }
start_session bus
if ! wait_until 30 wm_ready; then
  record lock-x11 FAIL "WM not ready; see $S/session.out"; teardown; exit 1
fi
sleep 8

# Focus and urgency: unsolicited activation request from an app with no recent user input. A baseline
# client takes focus first so the probe's map does not hit the no-active-window case.
focus_probe() { # idle|input mode(request|none) -> one focus-urgency row; none is the map-only control
  local prelude=$1 mode=$2 win active_before active_after state focus urg XB XC
  python3 -I "$XCLIENT" 30 >"$S/xclient-base-$mode.out" 2>&1 &
  XB=$!
  sleep 4
  active_before="$(active_window)"
  # input: one Shift tap just before the probe maps, so recent user input is set when it maps and requests.
  [ "$prelude" = input ] && { python3 -I "$KEY" Shift_L >/dev/null; }
  if [ "$mode" = request ]; then python3 -I "$XCLIENT" 12 >"$S/xclient-$mode.out" 2>"$S/xclient-$mode.err" &
  else python3 -I "$XCLIENT" 12 none >"$S/xclient-$mode.out" 2>"$S/xclient-$mode.err" &
  fi
  XC=$!
  sleep 4
  win="$(sed -n 's/^window=\(0x[0-9a-f]*\) .*$/\1/p' "$S/xclient-$mode.out")"
  active_after="$(active_window)"
  state="$(DISPLAY=":$DISP" xprop -id "$win" _NET_WM_STATE 2>/dev/null | sed -n 's/.*= //p')"
  if [ -n "$win" ]; then
    focus=$([ "$(echo "$active_after" | tr -d ' ')" = "$win" ] && echo taken || echo refused)
    urg=$(echo "$state" | grep -q DEMANDS_ATTENTION && echo set || echo unset)
    record "focus-urgency:$prelude:$mode" MEASURED "probe=$win focus=$focus demands_attention=$urg active_before=[$active_before] active_after=[$active_after] state=[$state]"
  else
    record "focus-urgency:$prelude:$mode" FAIL "probe window not created; see $S/xclient-$mode.err"; rc=1
  fi
  wait "$XC" 2>/dev/null
  kill "$XB" 2>/dev/null; wait "$XB" 2>/dev/null
}
focus_probe idle none
focus_probe idle request
focus_probe input none
focus_probe input request

# Media: the Xvfb keymap has the XF86 keysyms. Press the volume-up key in the live session and record the
# audio-related lines the session logs after it, plus whether a PulseAudio sink answers.
media_mark=$(wc -l <"$S/session.out")
python3 -I "$KEY" XF86AudioRaiseVolume >/dev/null
sleep 3
media_new="$(tail -n +"$((media_mark + 1))" "$S/session.out")"
media_lines="$(printf '%s\n' "$media_new" | grep -ci 'audio\|volume\|pulse\|wpctl\|pactl' || true)"
media_sample="$(printf '%s\n' "$media_new" | grep -i 'audio\|volume\|pulse\|wpctl\|pactl' | head -2 | cut -c1-140 | tr '\n' ' ')"
sink="$(command -v pactl >/dev/null && pactl info >/dev/null 2>&1 && echo yes || echo no)"
record media-x11 MEASURED "XF86AudioRaiseVolume pressed; audio_log_lines_after_press=$media_lines sample=[$media_sample] pulse_sink_reachable=$sink"

# Lock: Super+L, then coverage and input gating.
lock_before="$(pids_named flamewm-lock | head -1)"
python3 -I "$KEY" Super_L+l >/dev/null && sleep 5
lock_pid="$(pids_named flamewm-lock | head -1)"
shot locked >/dev/null
if [ -z "$lock_pid" ]; then
  record lock-x11 FAIL "flamewm-lock not running after Super+L (before=[$lock_before]); see $S/session.out"
  teardown; exit 1
fi
cover="$(cover_geom)"
python3 -I "$KEY" Super_L+space >/dev/null && sleep 3
shot locked-after-keys >/dev/null
gate_diff="$(diff_pct "$S/shot-locked.png" "$S/shot-locked-after-keys.png")"
gate_launcher="$(DISPLAY=":$DISP" xwininfo -root -children 2>/dev/null | grep -c 'flamewm' || true)"
record lock-x11 MEASURED "lock_pid=$lock_pid full_cover_1280x800=$cover pixels_changed_after_super_space=${gate_diff}% root_children_named_flamewm=$gate_launcher"

# Crash supervision: SIGKILL the locker (drops grabs) and time its respawn.
kill -KILL "$lock_pid" 2>/dev/null
t0=$(date +%s%N)
respawn_pid=""
for _ in $(seq 1 75); do
  respawn_pid="$(pids_named flamewm-lock | grep -vx "$lock_pid" | head -1)"
  [ -n "$respawn_pid" ] && break
  sleep 0.2
done
if [ -n "$respawn_pid" ]; then
  ms=$(( ($(date +%s%N) - t0) / 1000000 ))
  sleep 2
  shot respawned >/dev/null
  record lock-x11 MEASURED "respawn killed=$lock_pid new=$respawn_pid after_ms=$ms cover_after=$(cover_geom)"
else
  record lock-x11 FAIL "no respawn of flamewm-lock within 15s after SIGKILL $lock_pid"; rc=1
fi

teardown
exit $rc

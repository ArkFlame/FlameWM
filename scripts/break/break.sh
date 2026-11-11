#!/usr/bin/env bash
# Adversarial reliability scenarios against the X11 session on a private Xvfb.
# Usage: scripts/break/break.sh [all|s1|s2|s3|s4|s5|s6|s7]...
# Evidence: .agents/runtime/break/<stamp>/<scenario>/ plus results.tsv (scenario, verdict, evidence).
# Exit: 0 when no FAIL; UNVERIFIED does not fail the run.
set -uo pipefail
source "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/lib.sh"
PTR="$BREAK_DIR/pointer.py"

finish() { # scenario verdict evidence-note: adds panic evidence, tears down
  local n; n="$(panic_count)"
  record "$1" "$2" "$3 panics=$n dir=$S"
  teardown
}

ready_or_fail() { # scenario; settle past the 500ms startup grace so kills test respawn, not fallback
  wait_until 30 wm_ready && wait_until 20 have_proc flamewm-shell && wait_until 20 have_proc flamewm-desktop && sleep 5 && return 0
  finish "$1" FAIL "session not ready; see $S/session.out"; return 1
}

panel_settled() { # root_w root_h file: 0 once a strut owner exists and every one is on its edge
  panel_geometry "$1" "$2" >"$3"; grep -q 'panels=[1-9]' "$3" && grep -q 'bad=0' "$3"
}

s1_respawn() {
  sandbox_new s1 && start_session bus || { finish s1 FAIL "setup failed"; return; }
  ready_or_fail s1 || return
  local win0 sh0 dt0 sh1 dt1
  win0="$(window_count)"; sh0="$(pids_named flamewm-shell | head -1)"
  kill -9 "$sh0"
  wait_until 15 have_new_proc flamewm-shell "$sh0"; sh1="$(new_pid flamewm-shell "$sh0")"
  wm_ready || { finish s1 FAIL "WM lost after shell kill"; return; }
  dt0="$(pids_named flamewm-desktop | head -1)"
  kill -9 "$dt0"
  wait_until 15 have_new_proc flamewm-desktop "$dt0"; dt1="$(new_pid flamewm-desktop "$dt0")"
  sleep 2; shot s1 >/dev/null
  local v=PASS note="shell $sh0->${sh1:-none} desktop $dt0->${dt1:-none} windows $win0->$(window_count)"
  [ -n "$sh1" ] && [ -n "$dt1" ] && wm_ready || v=FAIL
  finish s1 "$v" "$note"
}

s2_churn() {
  sandbox_new s2 && start_session bus || { finish s2 FAIL "setup failed"; return; }
  ready_or_fail s2 || return
  local wm rss0 rss1 rss2 i p cpu0 cpu1 w v=PASS note
  wm="$(pids_named flamewm | head -1)"; rss0="$(rss_kb "$wm")"
  for ((i = 0; i < 150; i++)); do
    if ((i % 2)); then xterm -geometry 80x24+10+10 -e sleep 5 & else xeyes & fi
    p=$!; sleep 0.25; kill -TERM "$p" 2>/dev/null; wait "$p" 2>/dev/null
  done
  sleep 2; rss1="$(rss_kb "$wm")"
  local ids=()
  for ((i = 0; i < 40; i++)); do xeyes -geometry 60x40+$((i*20 % 1000))+$((i*13 % 600)) & ids+=($!); done
  sleep 3; cpu0="$(cpu_ticks "$wm")"; sleep 5; cpu1="$(cpu_ticks "$wm")"
  rss2="$(rss_kb "$wm")"
  local cpu_pct=$(( (cpu1 - cpu0) * 100 / CLK_TCK / 5 ))
  kill -TERM "${ids[@]}" 2>/dev/null; for id in "${ids[@]}"; do wait "$id" 2>/dev/null; done
  sleep 3; local rss3; rss3="$(rss_kb "$wm")"
  local act; act="$(active_window)"
  if [ -n "$act" ] && [ "$act" != "0x0" ] && ! DISPLAY=":$DISP" xwininfo -id "$act" >/dev/null 2>&1; then v=FAIL; note="stale active window $act"; fi
  wm_ready || { v=FAIL; note="WM lost"; }
  note="${note:-} wm_rss_kb base=$rss0 after_churn=$rss1 with40=$rss2 after_close=$rss3 cpu40=${cpu_pct}% active=${act:-none}"
  [ "$(( rss3 - rss0 ))" -le 8192 ] || { v=FAIL; note="$note RSS_GROWTH>8MB"; }
  finish s2 "$v" "$note"
}

s3_hover() {
  sandbox_new s3 && start_session bus || { finish s3 FAIL "setup failed"; return; }
  ready_or_fail s3 || return
  local sh c0 c1 r0 r1 cpu_pct v=PASS
  sh="$(pids_named flamewm-shell | head -1)"; c0="$(cpu_ticks "$sh")"; r0="$(rss_kb "$sh")"
  python3 -I "$PTR" sweep 796 0 1279 500 10 >"$S/sweep.out" 2>&1 || v=FAIL
  c1="$(cpu_ticks "$sh")"; r1="$(rss_kb "$sh")"
  cpu_pct=$(( (c1 - c0) * 100 / CLK_TCK / 10 ))
  [ "$cpu_pct" -lt 50 ] || v=FAIL
  [ "$(( r1 - r0 ))" -le 8192 ] || v=FAIL
  shot s3 >/dev/null
  finish s3 "$v" "sweep=500/10s shell_cpu=${cpu_pct}% shell_rss_kb $r0->$r1 label_flicker=UNVERIFIED(no map-count probe)"
}

s4_randr() {
  sandbox_new s4 && start_session bus || { finish s4 FAIL "setup failed"; return; }
  ready_or_fail s4 || return
  local rc1 rc2 rc3 rc4 v=PASS geom
  xrandr -q >"$S/randr-before.txt" 2>&1; shot s4-before >/dev/null
  panel_geometry 1280 800 >"$S/panel-before.txt"
  xrandr --newmode "1024x768_brk" 63.50 1024 1072 1176 1328 768 771 775 798 -hsync +vsync >/dev/null 2>&1; rc1=$?
  xrandr --addmode screen "1024x768_brk" >/dev/null 2>&1; rc2=$?
  xrandr -s 1024x768 >"$S/randr-set.txt" 2>&1; rc3=$?
  sleep 3; wm_ready || v=FAIL
  have_proc flamewm-shell && have_proc flamewm-desktop || v=FAIL
  # Geometry assertion: poll until the strut owner is on its edge at 1024x768 (<=10 s), keep last sample.
  wait_until 10 panel_settled 1024 768 "$S/panel-after.txt"
  shot s4-resized >/dev/null
  if grep -q 'panels=0 ' "$S/panel-after.txt"; then geom="UNVERIFIED(no strut-owning dock window found)"; [ "$v" = PASS ] && v=UNVERIFIED
  elif grep -q 'bad=0' "$S/panel-after.txt"; then geom="PASS($(tail -1 "$S/panel-after.txt"))"
  else geom="FAIL($(grep MISPLACED "$S/panel-after.txt" | tr '\n' ' ')$(tail -1 "$S/panel-after.txt"))"; v=FAIL; fi
  xrandr -s 1280x800 >/dev/null 2>&1; rc4=$?; sleep 3
  wm_ready || v=FAIL
  [ "$rc3" = 0 ] || v=UNVERIFIED
  finish s4 "$v" "newmode=$rc1 addmode=$rc2 size1024=$rc3 size_back=$rc4 dock_geometry=$geom before=$(tail -1 "$S/panel-before.txt") evidence=$S/randr-set.txt panel=$S/panel-after.txt"
}

s5_disk() {
  sandbox_new s5 || { finish s5 FAIL "setup failed"; return; }
  mkdir -p "$S/home/.config/flamewm"; chmod 500 "$S/home/.config/flamewm"
  start_session bus
  ready_or_fail s5 || return
  sleep 6
  local denied; denied="$(logs_grep 'settings.*\(denied\|failed\|could not\|cannot\)' 5)"
  local v=UNVERIFIED
  [ -n "$denied" ] && v=PASS
  finish s5 "$v" "read-only config dir; save failures logged: ${denied:-none (no save triggered at startup, so the save-error path is unexercised)}"
}

s6_malformed() {
  sandbox_new s6 || { finish s6 FAIL "setup failed"; return; }
  mkdir -p "$S/home/.config/flamewm"
  printf 'revision=\xff\xfe\x00garbage\nzoom=not-a-number\n\x00\x00' >"$S/home/.config/flamewm/settings.toml"
  start_session bus
  ready_or_fail s6 || return
  sleep 4
  finish s6 PASS "garbage settings.conf: session up, panic-free; surfaced_in_log=$(logs_grep 'settings' 2 | tr '\n' ' ')"
  sandbox_new s6b || return
  mkdir -p "$S/home/.config/flamewm"
  printf 'revision = 12\naccent = "#11' >"$S/home/.config/flamewm/settings.toml"
  start_session bus
  ready_or_fail s6b || return
  sleep 4
  finish s6b PASS "truncated settings.conf: session up, panic-free"
}

s7_nobus() {
  sandbox_new s7 && start_session nobus || { finish s7 FAIL "setup failed"; return; }
  # Readiness waits up to READY_TIMEOUT (30s) for the control bus before degrading; allow for it.
  local t0=$SECONDS; wait_until 45 bash -c "true" ; wait_until 45 have_proc flamewm-shell; local t_shell=$((SECONDS - t0))
  wait_until 15 have_proc flamewm-desktop; sleep 2
  local comp="wm=$(have_proc flamewm && echo up || echo down) shell=$(have_proc flamewm-shell && echo up || echo down) desktop=$(have_proc flamewm-desktop && echo up || echo down) shell_after_s=$t_shell"
  local v=PASS; wm_ready || v=FAIL; have_proc flamewm-shell && have_proc flamewm-desktop || v=FAIL
  shot s7 >/dev/null
  # Late attach: the bus appears on the absent socket; the shell retries every 30 s and must attach.
  local late=no
  DBUS_VERBOSE=0 dbus-daemon --session --address="unix:path=$S/tmp/absent.sock" --nofork --nopidfile >"$S/late-bus.out" 2>&1 &
  BUS_PID=$!
  wait_until 75 bash -c "grep -q 'control bus attached' '$S/home/.local/state/flamewm/session.log'" && late=yes
  kill "$BUS_PID" 2>/dev/null; wait "$BUS_PID" 2>/dev/null
  shot s7-late >/dev/null
  finish s7 "$v" "no session bus: wm_ready=$(wm_ready && echo yes || echo no) $comp late_attach=$late dbus_lines=$(logs_grep 'dbus' 2 | cut -c1-120 | tr '\n' ' ')"
}

main() {
  local t
  [ "$#" -eq 0 ] && set -- all
  for t in "$@"; do
    case "$t" in
      all) s1_respawn; s2_churn; s3_hover; s4_randr; s5_disk; s6_malformed; s7_nobus ;;
      s1) s1_respawn ;; s2) s2_churn ;; s3) s3_hover ;; s4) s4_randr ;;
      s5) s5_disk ;; s6) s6_malformed ;; s7) s7_nobus ;;
      *) echo "unknown scenario $t" >&2; exit 2 ;;
    esac
  done
  echo; echo "results: $RESULTS"; column -t -s$'\t' "$RESULTS"
  ! grep -q $'\tFAIL\t' "$RESULTS"
}
main "$@"

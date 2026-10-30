#!/usr/bin/env bash
# Print per-process RSS and PSS for FlameWM processes running on one X display.
#
# usage: BENCH_DISPLAY=:166 tools/bench_mem.sh [label]
#
# Selects processes whose DISPLAY environment equals BENCH_DISPLAY (or whose
# command line is the Xephyr server on that display), then reads
# /proc/<pid>/smaps_rollup. RSS includes shared pages; PSS divides shared pages
# between the processes that map them, so PSS totals are the honest footprint.
# Run it after the session has been idle for a few seconds. It does not sample CPU.
set -euo pipefail

label=${1:-idle}
display=${BENCH_DISPLAY:?set BENCH_DISPLAY, for example BENCH_DISPLAY=:166}
names="flamewm-session flamewm flamewm-desktop flamewm-shell flamewm-wm flamewm-audio-helper flamewm-terminal flamewm-settings flamewm-explorer Xephyr"

# pgrep -x matches the kernel comm name, which is cut to 15 characters.
pids=""
for name in $names; do
  for pid in $(pgrep -x "${name:0:15}" || true); do
    env_match=$(tr '\0' '\n' <"/proc/$pid/environ" 2>/dev/null | grep -cx "DISPLAY=$display" || true)
    cmd_match=$(tr '\0' ' ' <"/proc/$pid/cmdline" 2>/dev/null | grep -c -- "$display" || true)
    if [[ $env_match -gt 0 || $cmd_match -gt 0 ]]; then
      pids="$pids $pid"
    fi
  done
done

echo "## $label (display $display, $(date -u +%Y-%m-%dT%H:%M:%SZ))"
echo "| process | args | RSS MiB | PSS MiB |"
echo "|---|---|---|---|"
total_rss=0
total_pss=0
for pid in $pids; do
  rollup="/proc/$pid/smaps_rollup"
  [[ -r $rollup ]] || continue
  comm=$(cat "/proc/$pid/comm" 2>/dev/null || echo "?")
  args=$(tr '\0' ' ' <"/proc/$pid/cmdline" 2>/dev/null | cut -c1-60)
  rss=$(awk '/^Rss:/{print $2}' "$rollup")
  pss=$(awk '/^Pss:/{print $2}' "$rollup")
  total_rss=$((total_rss + rss))
  total_pss=$((total_pss + pss))
  awk -v c="$comm" -v a="$args" -v r="$rss" -v p="$pss" \
    'BEGIN{printf "| %s | %s | %.1f | %.1f |\n", c, a, r/1024, p/1024}'
done
awk -v r="$total_rss" -v p="$total_pss" \
  'BEGIN{printf "| **Total** | | **%.1f** | **%.1f** |\n", r/1024, p/1024}'

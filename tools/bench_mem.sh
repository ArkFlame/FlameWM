#!/usr/bin/env bash
# Per-process RSS and PSS for FlameWM processes on one X display.
#
# Two modes. The legacy listing is the default and its output/contract is
# unchanged; session mode is the whole-session measurement.
#
#   BENCH_DISPLAY=:166 tools/bench_mem.sh [label]        # per-process listing
#   BENCH_DISPLAY=:166 tools/bench_mem.sh --session [...] # whole installed session
#
# LEGACY MODE (default) selects processes whose DISPLAY equals BENCH_DISPLAY
# (or whose command line names the display), then prints RSS and PSS per
# process from /proc/<pid>/smaps_rollup. RSS includes shared pages; PSS divides
# shared pages between the processes that map them, so PSS totals are the
# honest footprint. Run it after the session has been idle for a few seconds.
# It does not sample CPU. Its known gaps are listed in SESSION_MODE_NOTES below
# and are what session mode exists to close.
#
# SESSION MODE (--session) measures the complete session at a declared
# resolution with a declared number of open applications:
#   * every FlameWM process on the display, including flamewm-desktop-services
#     (the Python session companion) and flamewm-wayland when the session uses it;
#   * the X server that actually serves the display (Xorg, Xephyr or Xwayland),
#     matched by owning the /tmp/.X11-unix/X<n> socket, so server-owned pixmaps
#     are inside the total;
#   * PSS and RSS per process plus core/app/FlameWM/server/SESSION totals;
#   * a `measured=`/`excluded=` manifest naming everything not included and why;
#   * an XFCE or IceWM baseline measured the same way when those binaries exist,
#     otherwise an explicit SKIPPED line naming the missing dependency.
# A missing optional process or baseline never aborts the run.
set -euo pipefail

ROOT="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/.." && pwd -P)"
SESSION_TOOL="$ROOT/tools/bench_flamewm_session_memory.py"

# What session mode adds to the legacy listing, and what stays unmeasured even
# there. Printed by --explain so the numbers can never be misread.
SESSION_MODE_NOTES=$(cat <<'EOF'
session-mode coverage:
  included: flamewm(-desktop|-shell|-settings|-xsettings|-desktop-services|-session|-wayland)
            flamewm-audio-helper, flamewm-terminal, flamewm-explorer, flamewm-lock,
            flamewm-screenshot, flamewm-task-manager, flamewm-finder
  included: the X server owning the display socket (Xorg, Xephyr or Xwayland)
  included: PSS and RSS per process, plus core/app/FlameWM/server/session totals
  excluded: foreign applications that belong to no FlameWM role
  excluded: kernel-mapped framebuffer/KMS driver memory outside any process
  excluded: swap written to disk once pages left RSS/PSS
  not proven: the declared --resolution is not queried from the server
legacy-mode gaps that session mode closes:
  - Xephyr was listed but the server owning the display is included explicitly
  - flamewm-desktop-services was absent from the process-name list
  - flamewm-xsettings was absent from the process-name list
  - flamewm-task-manager was absent, and pgrep -x truncation hides long names
  - no total, no resolution, no application set and no baseline comparison
EOF
)

usage(){
  cat <<'EOF'
Usage:
  BENCH_DISPLAY=:N tools/bench_mem.sh [LABEL]
      Per-process RSS/PSS listing for FlameWM processes on DISPLAY (legacy, unchanged).
  BENCH_DISPLAY=:N tools/bench_mem.sh --session [options]
      Whole-session measurement: every FlameWM process, the X server, a session total,
      a measured=/excluded= manifest and an XFCE/IceWM baseline when installed.
      Options are passed through to tools/bench_flamewm_session_memory.py:
        --display :N            session display (default: $BENCH_DISPLAY or $DISPLAY)
        --resolution WxH        declared session resolution (default 1920x1080)
        --apps N                applications to expect/open (default 3)
        --launch                start the applications before measuring
        --settle SECONDS        wait after launching (default 5)
        --baseline auto|xfce|icewm|none
        --baseline-display :N   measure an already running baseline session
        --json PATH             write the machine-readable report
  tools/bench_mem.sh --explain
      Print exactly what session mode includes and excludes.
EOF
}

case "${1:-}" in
  -h|--help) usage; exit 0 ;;
  --explain) printf '%s\n' "$SESSION_MODE_NOTES"; exit 0 ;;
  --session)
    shift
    [[ -f $SESSION_TOOL ]] || { echo "missing $SESSION_TOOL" >&2; exit 2; }
    exec python3 -I "$SESSION_TOOL" "$@"
    ;;
esac

label=${1:-idle}
display=${BENCH_DISPLAY:?set BENCH_DISPLAY, for example BENCH_DISPLAY=:166}
names="flamewm-session flamewm flamewm-desktop flamewm-shell flamewm-wm flamewm-audio-helper flamewm-terminal flamewm-settings flamewm-explorer Xephyr"
# Session-only processes (the Python companion, the X settings owner, and the
# other servers a real session can run on). Listed here so the legacy table
# does not omit them when they exist; session mode reports them unconditionally.
extra_names="flamewm-desktop-services flamewm-xsettings Xorg Xwayland"

# pgrep -x matches the kernel comm name, which is cut to 15 characters, and the
# kernel truncates the executable name the same way. Measured on Linux 7.0:
#   pgrep -x flamewm-desktop-services -> zero matches + "pattern ... longer than
#   15 characters will result in zero matches"; the installed bridge runs as a
#   python3 interpreter, so its comm is "python3" and no name pattern finds it.
# Candidates therefore come from one whole-cmdline scan, and identity is then
# verified from /proc/<pid>/exe (and argv for the Python bridge) rather than
# trusted from a truncated name.
bridge_arg(){
  # Prints the FlameWM bridge script name when argv[0..1] names it.
  local pid="$1" arg
  while IFS= read -r arg; do
    case ${arg##*/} in
      flamewm-desktop-services|flamewm_desktop_services.py) printf '%s\n' "${arg##*/}"; return 0 ;;
    esac
  done < <(tr '\0' '\n' <"/proc/$pid/cmdline" 2>/dev/null | head -2)
  return 1
}
exe_is_known(){
  # True when /proc/<pid>/exe is one of the measured executables.
  local pid="$1" exe
  exe=$(readlink "/proc/$pid/exe" 2>/dev/null || true)
  exe=${exe% (deleted)}
  exe=${exe##*/}
  [[ -n $exe ]] || return 1
  case " $names $extra_names " in
    *" $exe "*) return 0 ;;
  esac
  return 1
}
candidates=""
for pattern in flamewm Xorg Xephyr Xwayland; do
  for pid in $(pgrep -f -- "$pattern" 2>/dev/null || true); do
    candidates="$candidates $pid"
  done
done
pids=""
seen=""
for pid in $candidates; do
  case " $seen " in *" $pid "*) continue ;; esac
  seen="$seen $pid"
  # Exact identity: a command line mentioning "flamewm" is not enough, and the
  # installed bridge is a python3 process whose argv names the script.
  if ! exe_is_known "$pid"; then
    bridge_arg "$pid" >/dev/null || continue
  fi
  env_match=$(tr '\0' '\n' <"/proc/$pid/environ" 2>/dev/null | grep -cx "DISPLAY=$display" || true)
  cmd_match=$(tr '\0' ' ' <"/proc/$pid/cmdline" 2>/dev/null | grep -c -- "$display" || true)
  if [[ $env_match -gt 0 || $cmd_match -gt 0 ]]; then
    pids="$pids $pid"
  fi
done

# The Python session companion must never be silently absent from the report.
services_pid=""
for pid in $pids; do
  if bridge_arg "$pid" >/dev/null; then services_pid=$pid; fi
done

echo "## $label (display $display, $(date -u +%Y-%m-%dT%H:%M:%SZ))"
echo "scope=display-listing only; no X-server ownership match, no resolution, no app set, no baseline"
echo "use '$0 --session' for the whole-session total, or '$0 --explain' for the exclusions"
echo "| process | args | RSS MiB | PSS MiB |"
echo "|---|---|---|---|"
total_rss=0
total_pss=0
seen=""
for pid in $pids; do
  case " $seen " in *" $pid "*) continue ;; esac
  seen="$seen $pid"
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
if [[ -n "$services_pid" ]]; then
  echo "desktop_services=present pid=$services_pid"
else
  echo "desktop_services=ABSENT (flamewm-desktop-services is not running on $display; the Total above omits the Python session services)"
fi
echo "excluded=X-server-owned-pixmaps unless the server itself listed above; excluded=resolution; excluded=app-set; excluded=baseline"

#!/usr/bin/env bash
# Runtime proof for a live Settings output scale on the nested winit output (sync_output_scale).
# Private Xvfb + private dbus-run-session + nested flamewm-wayland (winit). The settings file starts at
# scale.wayland-0=100, is rewritten to 150 and back to 100 the way Settings persists it (scale.<output>
# key, revision unchanged). Each change must be picked up by the compositor loop with no input event
# (a loop turn). The applied logical output size comes from the compositor's "output scale" log line.
# Wake: "idle" when applied with no input; "input" when applied after one harmless XTest pointer move
# (an idle compositor has no timer, so a settings write alone does not wake the loop).
# PASS needs both changes applied, the physical/logical ratio within 1% of the scale, and no panic.
# Usage: scripts/break/scale_live.sh
# Env: FLAMEWM_BIN_DIR (default: ${CARGO_TARGET_DIR:-$ROOT/.target}/release)
# Evidence: .agents/runtime/break/scale-live-<stamp>/{settings.toml, wm.log, steps.tsv, verdict.txt}
# Exit: 0 only on PASS. Never touches the host session bus or the real display.
set -uo pipefail
ROOT="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/../.." && pwd -P)"
SELF="$(readlink -f "${BASH_SOURCE[0]}")"
HERE="$(dirname "$SELF")"
PHYS_W=640 PHYS_H=360

if [[ "${1:-}" == --inner ]]; then
  # Runs inside the private dbus-run-session; every input comes from the exported variables.
  unset WAYLAND_DISPLAY
  export DISPLAY=":$XDISP" XDG_RUNTIME_DIR="$XDG" HOME="$RUN/home" XDG_CONFIG_HOME="$RUN/home/.config"
  SET="$RUN/settings.toml"
  origin() { xwininfo -root -children 2>/dev/null | awk "/ ${PHYS_W}x${PHYS_H}\\+/" | grep -oE '\+-?[0-9]+\+-?[0-9]+' | tail -1 | sed -E 's/^\+//; s/\+/ /'; }
  # Writes a complete new file (rename), the way a settings save replaces it.
  write_scale() { printf 'revision=2\nscale.wayland-0=%s\n' "$1" >"$SET.tmp" && mv "$SET.tmp" "$SET"; }
  now() { date +%s%3N; }
  # Sets LAT (ms from the write to the log line) and WAKE: idle (applied with no input), input
  # (applied only after a pointer move), or none. Waits 12 s idle before it nudges the pointer.
  step() {
    sleep 1.1
    local t; t=$(now); write_scale "$1"; LAT=""; WAKE=none
    while (( $(now) < t + 12000 )); do
      if grep -q "output scale $1% " "$RUN/wm.log"; then LAT=$(( $(now) - t )); WAKE=idle; return; fi
      sleep 0.1
    done
    read -r ox oy <<<"$(origin)"; python3 -I "$PTR" at $((ox + 320)) $((oy + 180))
    while (( $(now) < t + 22000 )); do
      if grep -q "output scale $1% " "$RUN/wm.log"; then LAT=$(( $(now) - t )); WAKE=input; return; fi
      sleep 0.1
    done
  }
  logical() { grep -o "output scale $1% (logical [0-9]*x[0-9]*)" "$RUN/wm.log" | tail -1 | grep -oE '[0-9]+x[0-9]+' ; }

  mkdir -p "$RUN/home"
  write_scale 100
  cp "$SET" "$RUN/settings.initial"
  setsid env FLAMEWM_SETTINGS_PATH="$SET" FLAMEWM_WAYLAND_BACKEND=winit \
    FLAMEWM_WAYLAND_SOCKET="wl-scale-$$" FLAMEWM_WAYLAND_WIDTH="$PHYS_W" FLAMEWM_WAYLAND_HEIGHT="$PHYS_H" \
    RUST_BACKTRACE=1 timeout 300 "$BIN_DIR/flamewm-wayland" >"$RUN/wm.log" 2>&1 &
  WM=$!
  o=""
  for _ in $(seq 1 150); do o="$(origin)"; [[ -n "$o" ]] && break; sleep 0.2; done
  sleep 3
  printf 'step\tpercent\tlat_ms\twake\tlogical\tratio\n' >"$RUN/steps.tsv"
  fail=0
  if [[ -z "$o" ]]; then
    echo "window=missing" >>"$RUN/steps.tsv"; fail=1
  else
    for pct in 150 100; do
      step "$pct"
      lg="$(logical "$pct")"
      ratio="none"
      if [[ -n "$lg" ]]; then
        ratio="$(python3 -I -c 'import sys; w, h = map(int, sys.argv[1].split("x")); p = int(sys.argv[2]) / 100
ok = abs(640 / w - p) <= 0.01 and abs(360 / h - p) <= 0.01
print(f"{640 / w:.4f}x{360 / h:.4f}", "ok" if ok else "bad")' "$lg" "$pct")"
        [[ "$ratio" == *" ok" ]] || fail=1
      fi
      printf 'step\t%s\t%s\t%s\t%s\t%s\n' "$pct" "${LAT:-none}" "$WAKE" "${lg:-none}" "$ratio" >>"$RUN/steps.tsv"
      [[ "$WAKE" != none && -n "$lg" ]] || fail=1
    done
  fi
  kill -0 "$WM" 2>/dev/null || { echo "compositor exited" >>"$RUN/steps.tsv"; fail=1; }
  kill -TERM -- "-$WM" 2>/dev/null; sleep 1; kill -KILL -- "-$WM" 2>/dev/null; wait "$WM" 2>/dev/null
  grep -q 'panicked' "$RUN/wm.log" && { echo "panic in wm.log" >>"$RUN/steps.tsv"; fail=1; }
  cat "$RUN/steps.tsv"
  exit "$fail"
fi

BIN_DIR="${FLAMEWM_BIN_DIR:-${CARGO_TARGET_DIR:-$ROOT/.target}/release}"
[[ "$BIN_DIR" == /* ]] || BIN_DIR="$ROOT/$BIN_DIR"
[[ -x "$BIN_DIR/flamewm-wayland" ]] || { echo "SCALELIVE: missing $BIN_DIR/flamewm-wayland" >&2; exit 2; }
for tool in Xvfb dbus-run-session python3 xwininfo setsid; do
  command -v "$tool" >/dev/null || { echo "SCALELIVE: $tool required" >&2; exit 2; }
done

STAMP="$(date +%Y%m%d-%H%M%S)"
RUN="$ROOT/.agents/runtime/break/scale-live-$STAMP"
XDG="$(mktemp -d "${TMPDIR:-/tmp}/flamewm-sl.XXXXXX")"
chmod 700 "$XDG"
mkdir -p "$RUN/home"
PTR="$HERE/pointer.py"
export RUN BIN_DIR XDG PTR
XDISP=""
for n in $(seq 400 499); do
  [[ -e "/tmp/.X11-unix/X$n" || -e "/tmp/.X$n-lock" ]] || { XDISP=$n; break; }
done
[[ -n "$XDISP" ]] || { echo "SCALELIVE: no free X display in :400-:499" >&2; exit 2; }
export XDISP

Xvfb ":$XDISP" -screen 0 1600x900x24 -nolisten tcp -noreset >"$RUN/xvfb.log" 2>&1 &
XVFB=$!
trap 'kill "$XVFB" 2>/dev/null; rm -rf "$XDG"' EXIT
for _ in $(seq 1 50); do [[ -S "/tmp/.X11-unix/X$XDISP" ]] && break; sleep 0.2; done
sleep 1

env -i PATH="$PATH" LANG=C.UTF-8 RUN="$RUN" BIN_DIR="$BIN_DIR" XDG="$XDG" XDISP="$XDISP" PTR="$PTR" \
  dbus-run-session -- "$SELF" --inner
rc=$?
if [[ $rc -eq 0 ]]; then echo "SCALELIVE PASS" | tee "$RUN/verdict.txt"; else echo "SCALELIVE FAIL rc=$rc" | tee "$RUN/verdict.txt"; fi
exit "$rc"

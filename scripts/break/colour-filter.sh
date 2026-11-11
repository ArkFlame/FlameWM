#!/usr/bin/env bash
# Runtime pixel proof for the compositor colour filter (winit path, docs/backlog/remaining-audit.md a11y).
# Private Xvfb + private dbus-run-session + nested flamewm-wayland (winit). For each mode in
# none, grayscale, invert the compositor renders one frame, the PPM dump (FLAMEWM_WAYLAND_DUMP)
# reads back the filtered pixels, and the script asserts grayscale and invert each differ from none.
# Usage: scripts/break/colour-filter.sh
# Env: FLAMEWM_BIN_DIR (default: ${CARGO_TARGET_DIR:-$ROOT/.target}/release), DUMP_AFTER_MS (default 3000).
# Evidence: .agents/runtime/break/colour-filter-<stamp>/{<mode>.ppm,<mode>.log,verdict.txt}
# Exit: 0 unless the verdict is FAIL. Never touches the host session bus or the real display.
set -uo pipefail
ROOT="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/../.." && pwd -P)"
SELF="$(readlink -f "${BASH_SOURCE[0]}")"

if [[ "${1:-}" == --inner ]]; then
  # Runs inside the private dbus-run-session; every input comes from the exported variables.
  unset WAYLAND_DISPLAY
  export DISPLAY=":$XDISP" XDG_RUNTIME_DIR="$XDG" HOME="$RUN/home" XDG_CONFIG_HOME="$RUN/home/.config"
  for mode in none grayscale invert; do
    mkdir -p "$RUN/$mode"
    if [[ "$mode" == none ]]; then : >"$RUN/$mode/settings.toml"; else
      printf 'colourFilter = "%s"\n' "$mode" >"$RUN/$mode/settings.toml"
    fi
    SOCK="wl-cf-$mode-$$"
    FLAMEWM_SETTINGS_PATH="$RUN/$mode/settings.toml" \
      FLAMEWM_WAYLAND_BACKEND=winit FLAMEWM_WAYLAND_SOCKET="$SOCK" FLAMEWM_WAYLAND_AUTOSTART='' \
      FLAMEWM_WAYLAND_WIDTH=640 FLAMEWM_WAYLAND_HEIGHT=360 RUST_BACKTRACE=1 \
      FLAMEWM_WAYLAND_DUMP="$RUN/$mode.ppm" FLAMEWM_WAYLAND_EXIT_AFTER_DUMP=1 \
      FLAMEWM_WAYLAND_DUMP_AFTER_MS="${DUMP_AFTER_MS:-3000}" \
      timeout 60 "$BIN_DIR/flamewm-wayland" >"$RUN/$mode.log" 2>&1
    echo "$mode rc=$?" >>"$RUN/inner.txt"
  done
  exit 0
fi

BIN_DIR="${FLAMEWM_BIN_DIR:-${CARGO_TARGET_DIR:-$ROOT/.target}/release}"
[[ "$BIN_DIR" == /* ]] || BIN_DIR="$ROOT/$BIN_DIR"
[[ -x "$BIN_DIR/flamewm-wayland" ]] || { echo "COLOUR-FILTER: missing $BIN_DIR/flamewm-wayland (cargo build --release -p flamewm-wm-wayland)" >&2; exit 2; }
for tool in Xvfb dbus-run-session python3; do
  command -v "$tool" >/dev/null || { echo "COLOUR-FILTER: $tool required" >&2; exit 2; }
done

STAMP="$(date +%Y%m%d-%H%M%S)"
RUN="$ROOT/.agents/runtime/break/colour-filter-$STAMP"
# The Wayland socket path must stay under SUN_LEN (108 bytes), so the runtime dir is a short temp dir.
XDG="$(mktemp -d "${TMPDIR:-/tmp}/flamewm-cf.XXXXXX")"
chmod 700 "$XDG"
mkdir -p "$RUN/home"
export RUN BIN_DIR XDG
XDISP=""
for n in $(seq 170 199); do
  [[ -e "/tmp/.X11-unix/X$n" || -e "/tmp/.X$n-lock" ]] || { XDISP=$n; break; }
done
[[ -n "$XDISP" ]] || { echo "COLOUR-FILTER: no free X display in :170-:199" >&2; exit 2; }
export XDISP

Xvfb ":$XDISP" -screen 0 1600x900x24 -nolisten tcp -noreset >"$RUN/xvfb.log" 2>&1 &
XVFB=$!
trap 'kill "$XVFB" 2>/dev/null; rm -rf "$XDG"' EXIT
for _ in $(seq 1 50); do [[ -S "/tmp/.X11-unix/X$XDISP" ]] && break; sleep 0.2; done
sleep 1

env -i PATH="$PATH" LANG=C.UTF-8 RUN="$RUN" BIN_DIR="$BIN_DIR" XDG="$XDG" XDISP="$XDISP" \
  ${DUMP_AFTER_MS:+DUMP_AFTER_MS="$DUMP_AFTER_MS"} \
  dbus-run-session -- "$SELF" --inner
rc=$?

verdict=FAIL
detail=""
if python3 -I - "$RUN" <<'PY'
import sys, os
run = sys.argv[1]
def read_ppm(path):
    with open(path, "rb") as f:
        data = f.read()
    parts, i = [], 0
    while len(parts) < 4:
        while data[i:i+1].isspace(): i += 1
        j = i
        while not data[j:j+1].isspace(): j += 1
        parts.append(data[i:j]); i = j
    i += 1
    if parts[0] != b"P6": raise SystemExit("not a P6 PPM")
    return int(parts[1]), int(parts[2]), data[i:]
def diff(a, b):
    if len(a) != len(b): return -1
    return sum(1 for x, y in zip(a, b) if x != y)
try:
    w0, h0, base = read_ppm(os.path.join(run, "none.ppm"))
    out = []
    ok = True
    for mode in ("grayscale", "invert"):
        w, h, px = read_ppm(os.path.join(run, mode + ".ppm"))
        d = diff(base, px) if (w, h) == (w0, h0) else -1
        out.append(f"{mode}_bytes_differ_from_none={d}")
        ok = ok and d > 0
    print(" ".join(out))
    sys.exit(0 if ok else 1)
except (OSError, SystemExit) as e:
    print(f"proof_error={e}")
    sys.exit(1)
PY
then verdict=PASS; fi
panics=$(grep -c 'panicked' "$RUN"/*.log 2>/dev/null | awk -F: '{s+=$2} END {print s+0}')
[[ "$panics" == 0 ]] || verdict=FAIL
{
  echo "COLOUR-FILTER: $verdict panics=$panics inner_rc=$rc $(cat "$RUN/inner.txt" 2>/dev/null | tr '\n' ' ')"
  echo "run=$RUN"
} | tee "$RUN/verdict.txt"
[[ "$verdict" == FAIL ]] && exit 1
exit 0

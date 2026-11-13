#!/usr/bin/env bash
# Runtime proof for wlr-layer-shell placement on the single nested output (winit path).
# Private Xvfb + private dbus-run-session + nested flamewm-wayland (winit, 640x360) + the
# examples/layer_client wlr-layer-shell client (bottom edge, 40 px, exclusive zone 40, output None).
# Captures the Xvfb root before the client maps and after it commits, then asserts:
#   - the bottom 40 rows of the nested window turn red (layer surface placed, visible, full width);
#   - the layer's configure is 640x40 (the client log);
#   - nothing above the band changed (no stray placement, same red count as the baseline);
#   - the compositor logged no panic.
# Usage: scripts/break/layer_place.sh
# Env: FLAMEWM_BIN_DIR (default: ${CARGO_TARGET_DIR:-$ROOT/.target}/release); the client is read from
#      $FLAMEWM_BIN_DIR/examples/layer_client (cargo build --release -p flamewm-wm-wayland --bin flamewm-wayland --example layer_client).
# Evidence: .agents/runtime/break/layer-place-<stamp>/{before,after}.ppm, comp.log, client.log, verdict.txt
# Exit: 0 only on PASS. Never touches the host session bus or the real display.
set -uo pipefail
ROOT="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/../.." && pwd -P)"
SELF="$(readlink -f "${BASH_SOURCE[0]}")"

if [[ "${1:-}" == --inner ]]; then
  # Runs inside the private dbus-run-session; every input comes from the exported variables.
  unset WAYLAND_DISPLAY
  export DISPLAY=":$XDISP" XDG_RUNTIME_DIR="$XDG" HOME="$RUN/home" XDG_CONFIG_HOME="$RUN/home/.config"
  SOCK="wl-layer-$$"
  setsid env FLAMEWM_WAYLAND_BACKEND=winit FLAMEWM_WAYLAND_SOCKET="$SOCK" FLAMEWM_WAYLAND_AUTOSTART='' \
    FLAMEWM_WAYLAND_WIDTH=640 FLAMEWM_WAYLAND_HEIGHT=360 RUST_BACKTRACE=1 \
    timeout 120 "$BIN_DIR/flamewm-wayland" >"$RUN/comp.log" 2>&1 &
  COMP=$!
  for _ in $(seq 1 150); do [[ -S "$XDG/$SOCK" ]] && break; sleep 0.2; done
  # Window origin of the nested 640x360 winit window on the root ("X Y").
  origin() { xwininfo -root -children 2>/dev/null | awk '/ 640x360\+/' | grep -oE '\+-?[0-9]+\+-?[0-9]+' | tail -1 | sed -E 's/^\+//; s/\+/ /'; }
  o=""
  for _ in $(seq 1 150); do o="$(origin)"; [[ -n "$o" ]] && break; sleep 0.2; done
  echo "origin=${o:-missing}" >"$RUN/inner.txt"
  if [[ -z "$o" ]]; then kill -TERM -- "-$COMP" 2>/dev/null; exit 3; fi
  sleep 3
  import -depth 8 -window root "ppm:$RUN/before.ppm" 2>/dev/null
  WAYLAND_DISPLAY="$SOCK" FLAMEWM_TEST_HOLD_MS=8000 FLAMEWM_LAYER_HEIGHT=40 FLAMEWM_TEST_COLOR=FF0000 \
    "$BIN_DIR/examples/layer_client" >"$RUN/client.log" 2>&1 &
  CLIENT=$!
  sleep 3
  import -depth 8 -window root "ppm:$RUN/after.ppm" 2>/dev/null
  kill -TERM "$CLIENT" 2>/dev/null
  wait "$CLIENT" 2>/dev/null
  kill -TERM -- "-$COMP" 2>/dev/null
  sleep 1
  kill -KILL -- "-$COMP" 2>/dev/null
  wait "$COMP" 2>/dev/null
  exit 0
fi

BIN_DIR="${FLAMEWM_BIN_DIR:-${CARGO_TARGET_DIR:-$ROOT/.target}/release}"
[[ "$BIN_DIR" == /* ]] || BIN_DIR="$ROOT/$BIN_DIR"
[[ -x "$BIN_DIR/flamewm-wayland" && -x "$BIN_DIR/examples/layer_client" ]] || {
  echo "LAYER_PLACE: missing flamewm-wayland or examples/layer_client under $BIN_DIR" >&2; exit 2; }
for tool in Xvfb dbus-run-session python3 xwininfo import; do
  command -v "$tool" >/dev/null || { echo "LAYER_PLACE: $tool required" >&2; exit 2; }
done

STAMP="$(date +%Y%m%d-%H%M%S)"
RUN="$ROOT/.agents/runtime/break/layer-place-$STAMP"
mkdir -p "$RUN/home"
# The Wayland socket path must stay under SUN_LEN (108 bytes), so the runtime dir is a short temp dir.
XDG="$(mktemp -d "${TMPDIR:-/tmp}/flamewm-lp.XXXXXX")"
chmod 700 "$XDG"
export RUN BIN_DIR XDG
XDISP=""
for n in $(seq 400 499); do
  [[ -e "/tmp/.X11-unix/X$n" || -e "/tmp/.X$n-lock" ]] || { XDISP=$n; break; }
done
[[ -n "$XDISP" ]] || { echo "LAYER_PLACE: no free X display in :400-:499" >&2; exit 2; }
export XDISP

Xvfb ":$XDISP" -screen 0 1600x900x24 -nolisten tcp -noreset >"$RUN/xvfb.log" 2>&1 &
XVFB=$!
trap 'kill "$XVFB" 2>/dev/null; rm -rf "$XDG"' EXIT
for _ in $(seq 1 50); do [[ -S "/tmp/.X11-unix/X$XDISP" ]] && break; sleep 0.2; done
sleep 1

env -i PATH="$PATH" LANG=C.UTF-8 RUN="$RUN" BIN_DIR="$BIN_DIR" XDG="$XDG" XDISP="$XDISP" \
  dbus-run-session -- "$SELF" --inner
rc=$?

verdict=FAIL
if python3 - "$RUN" <<'PY'
import sys, os

run = sys.argv[1]
W, H, BAND, MIN_FRAC = 640, 360, 40, 0.99


def read_ppm(path):
    with open(path, "rb") as f:
        data = f.read()
    parts, i = [], 0
    while len(parts) < 4:
        while data[i:i + 1].isspace():
            i += 1
        j = i
        while not data[j:j + 1].isspace():
            j += 1
        parts.append(data[i:j])
        i = j
    if parts[0] != b"P6":
        raise SystemExit("not a P6 PPM")
    return int(parts[1]), int(parts[2]), data[i + 1:]


def is_red(img, stride, x, y):
    i = (y * stride + x) * 3
    return img[i] >= 240 and img[i + 1] <= 20 and img[i + 2] <= 20


def red_count(img, stride, xs, ys):
    return sum(1 for y in ys for x in xs if is_red(img, stride, x, y))


origin = open(os.path.join(run, "inner.txt")).read().split("=", 1)[1].split()
ox, oy = int(origin[0]), int(origin[1])
sw, _, before = read_ppm(os.path.join(run, "before.ppm"))
aw, _, after = read_ppm(os.path.join(run, "after.ppm"))
assert sw == aw
xs = range(ox, ox + W)
band_ys = range(oy + H - BAND, oy + H)
above_ys = range(oy, oy + H - BAND)
total = W * BAND
band_after = red_count(after, sw, xs, band_ys)
band_before = red_count(before, sw, xs, band_ys)
above_after = red_count(after, sw, xs, above_ys)
above_before = red_count(before, sw, xs, above_ys)
# First row (top to bottom) that holds red at the window's centre column: the layer's top edge.
edge = next((y for y in range(oy + H - BAND - 1, oy - 1, -1) if not is_red(after, sw, ox + W // 2, y)), None)
edge_row = None if edge is None else edge + 1 - oy
client = open(os.path.join(run, "client.log")).read()
configured = "layer-client: configured 640x40" in client
comp_log = open(os.path.join(run, "comp.log")).read(4000) if os.path.exists(os.path.join(run, "comp.log")) else ""
panics = comp_log.count("panicked")
frac = band_after / total
ok = (
    configured
    and frac >= MIN_FRAC
    and band_before / total <= 0.01
    and above_after == above_before
    and panics == 0
)
print(f"origin={ox},{oy} window=640x360 band=rows {oy + H - BAND}..{oy + H - 1} (40 px)")
print(f"band_red_after={band_after}/{total} frac={frac:.4f} band_red_before={band_before}/{total}")
print(f"above_red_after={above_after} above_red_before={above_before}")
print(f"layer_top_row_in_window={edge_row} expected=320")
print(f"client_configured_640x40={configured} comp_panics={panics}")
sys.exit(0 if ok else 1)
PY
then verdict=PASS; fi
{
  echo "LAYER_PLACE: $verdict inner_rc=$rc $(tr '\n' ' ' <"$RUN/inner.txt" 2>/dev/null)"
  echo "run=$RUN"
} | tee "$RUN/verdict.txt"
[[ "$verdict" == PASS ]] || exit 1
exit 0

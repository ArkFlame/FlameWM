#!/usr/bin/env bash
# Runtime proof for the magnifier follow modes (zoomFollowMode proportional, centred, push) on the
# nested winit path. Private Xvfb + private dbus-run-session + nested flamewm-wayland (winit).
# Per mode: zoomLevel=1, the pointer rests at the centre, the zoom-in shortcut (Super+Alt+equal) is
# pressed twice (zoom 2.0), then the pointer visits six window-local points with XTest. After each
# move the Xvfb root is captured. The viewport origin is recovered by matching the 2x frame, sampled
# every second pixel, against the unzoomed frame (normalised cross-correlation over all shifts).
# PASS needs every origin within 8 px of the follow-mode model and a match score of at least 0.9.
# Usage: scripts/break/zoomfollow.sh
# Env: FLAMEWM_BIN_DIR (default: ${CARGO_TARGET_DIR:-$ROOT/.target}/release), ZOOM_CLIENT (default flamewm-terminal).
# Evidence: .agents/runtime/break/zoomfollow-<stamp>/{<mode>/s<n>.{ppm,png}, steps.tsv, verdict.txt}
# Exit: 0 unless the verdict is FAIL. Never touches the host session bus or the real display.
set -uo pipefail
ROOT="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/../.." && pwd -P)"
SELF="$(readlink -f "${BASH_SOURCE[0]}")"
HERE="$(dirname "$SELF")"

if [[ "${1:-}" == --inner ]]; then
  # Runs inside the private dbus-run-session; every input comes from the exported variables.
  unset WAYLAND_DISPLAY
  export DISPLAY=":$XDISP" XDG_RUNTIME_DIR="$XDG" HOME="$RUN/home" XDG_CONFIG_HOME="$RUN/home/.config"
  # Window origin of the nested 640x360 winit window on the root ("X Y").
  origin() { xwininfo -root -children 2>/dev/null | awk '/ 640x360\+/' | grep -oE '\+-?[0-9]+\+-?[0-9]+' | tail -1 | sed -E 's/^\+//; s/\+/ /'; }
  # Pointer to a window-local point, through XTest.
  local_move() { read -r ox oy <<<"$(origin)"; python3 -I "$PTR" at $((ox + $1)) $((oy + $2)); }
  for mode in proportional centred push; do
    D="$RUN/$mode"
    mkdir -p "$D"
    printf 'revision=2\nzoomLevel=1\nzoomFollowMode=%s\n' "$mode" >"$D/settings.toml"
    setsid env FLAMEWM_SETTINGS_PATH="$D/settings.toml" FLAMEWM_WAYLAND_BACKEND=winit \
      FLAMEWM_WAYLAND_SOCKET="wl-zf-$mode-$$" FLAMEWM_WAYLAND_AUTOSTART="${ZOOM_CLIENT-flamewm-terminal}" \
      FLAMEWM_WAYLAND_WIDTH=640 FLAMEWM_WAYLAND_HEIGHT=360 RUST_BACKTRACE=1 \
      timeout 300 "$BIN_DIR/flamewm-wayland" >"$D/wm.log" 2>&1 &
    WM=$!
    o=""
    for _ in $(seq 1 150); do o="$(origin)"; [[ -n "$o" ]] && break; sleep 0.2; done
    if [[ -z "$o" ]]; then echo "$mode window=missing" >>"$RUN/inner.txt"; kill -TERM -- "-$WM" 2>/dev/null; continue; fi
    echo "$mode origin=$o" >>"$RUN/inner.txt"
    sleep 3
    local_move 320 180
    sleep 1
    import -depth 8 -window root "ppm:$D/ref.ppm" 2>/dev/null
    # Without a window manager on Xvfb, XTest keys reach the nested window only when it holds X input
    # focus, so focus it first (XSetInputFocus, RevertToPointerRoot, CurrentTime).
    python3 -I - "$(xwininfo -root -children 2>/dev/null | awk '/ 640x360\+/{print $1; exit}')" <<'PY'
import ctypes, sys
x11 = ctypes.CDLL("libX11.so.6")
x11.XOpenDisplay.restype = ctypes.c_void_p
x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
x11.XSetInputFocus.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong]
x11.XFlush.argtypes = [ctypes.c_void_p]
dpy = x11.XOpenDisplay(None)
x11.XSetInputFocus(dpy, int(sys.argv[1], 16), 2, 0)
x11.XFlush(dpy)
PY
    # Zoom in with the existing shortcut: 1.0 -> 1.5 -> 2.0.
    python3 -I "$KEY" Super_L+Alt_L+equal; sleep 1
    python3 -I "$KEY" Super_L+Alt_L+equal; sleep 1
    i=0
    for point in "320 180" "2 180" "637 180" "320 180" "320 2" "320 357"; do
      set -- $point
      local_move "$1" "$2"
      sleep 1.2
      import -depth 8 -window root "ppm:$D/s$i.ppm" 2>/dev/null
      printf '%s\t%s\t%s\t%s\n' "$mode" "$i" "$1" "$2" >>"$RUN/steps.tsv"
      i=$((i + 1))
    done
    kill -TERM -- "-$WM" 2>/dev/null
    sleep 1
    kill -KILL -- "-$WM" 2>/dev/null
    wait "$WM" 2>/dev/null
  done
  exit 0
fi

BIN_DIR="${FLAMEWM_BIN_DIR:-${CARGO_TARGET_DIR:-$ROOT/.target}/release}"
[[ "$BIN_DIR" == /* ]] || BIN_DIR="$ROOT/$BIN_DIR"
[[ -x "$BIN_DIR/flamewm-wayland" ]] || { echo "ZOOMFOLLOW: missing $BIN_DIR/flamewm-wayland (cargo build --release -p flamewm-wm-wayland)" >&2; exit 2; }
for tool in Xvfb dbus-run-session python3 xwininfo import; do
  command -v "$tool" >/dev/null || { echo "ZOOMFOLLOW: $tool required" >&2; exit 2; }
done
python3 -I -c 'import numpy' 2>/dev/null || { echo "ZOOMFOLLOW: numpy required" >&2; exit 2; }

STAMP="$(date +%Y%m%d-%H%M%S)"
RUN="$ROOT/.agents/runtime/break/zoomfollow-$STAMP"
# The Wayland socket path must stay under SUN_LEN (108 bytes), so the runtime dir is a short temp dir.
XDG="$(mktemp -d "${TMPDIR:-/tmp}/flamewm-zf.XXXXXX")"
chmod 700 "$XDG"
mkdir -p "$RUN/home"
PTR="$HERE/pointer.py"
KEY="$HERE/key.py"
export RUN BIN_DIR XDG PTR KEY
XDISP=""
for n in $(seq 300 399); do
  [[ -e "/tmp/.X11-unix/X$n" || -e "/tmp/.X$n-lock" ]] || { XDISP=$n; break; }
done
[[ -n "$XDISP" ]] || { echo "ZOOMFOLLOW: no free X display in :300-:399" >&2; exit 2; }
export XDISP

Xvfb ":$XDISP" -screen 0 1600x900x24 -nolisten tcp -noreset >"$RUN/xvfb.log" 2>&1 &
XVFB=$!
trap 'kill "$XVFB" 2>/dev/null; rm -rf "$XDG"' EXIT
for _ in $(seq 1 50); do [[ -S "/tmp/.X11-unix/X$XDISP" ]] && break; sleep 0.2; done
sleep 1

env -i PATH="$PATH" LANG=C.UTF-8 RUN="$RUN" BIN_DIR="$BIN_DIR" XDG="$XDG" XDISP="$XDISP" PTR="$PTR" KEY="$KEY" \
  dbus-run-session -- "$SELF" --inner
rc=$?

verdict=FAIL
if python3 -I - "$RUN" <<'PY'
import sys, os
import numpy as np

run = sys.argv[1]
W, H, ZOOM, TOL, MIN_NCC = 640, 360, 2.0, 8.0, 0.9


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
    i += 1
    if parts[0] != b"P6":
        raise SystemExit("not a P6 PPM")
    w, h = int(parts[1]), int(parts[2])
    return np.frombuffer(data[i:], dtype=np.uint8).reshape(h, w, 3)


def window(root, ox, oy):
    return root[oy:oy + H, ox:ox + W].astype(np.float64).mean(axis=2)


def locate(ref, zoomed):
    # Zoomed pixel k samples source pixel origin + k/2, so every second pixel is a 1x patch.
    patch = zoomed[::2, ::2]
    ph, pw = patch.shape
    t = np.zeros_like(ref)
    t[:ph, :pw] = patch - patch.mean()
    corr = np.fft.ifft2(np.fft.fft2(ref - ref.mean()) * np.conj(np.fft.fft2(t))).real
    sub = corr[:H - ph + 1, :W - pw + 1]
    y, x = np.unravel_index(np.argmax(sub), sub.shape)
    local = ref[y:y + ph, x:x + pw] - ref[y:y + ph, x:x + pw].mean()
    pc = patch - patch.mean()
    ncc = float((local * pc).sum() / (np.linalg.norm(local) * np.linalg.norm(pc) + 1e-9))
    return int(x), int(y), ncc


def push1(origin, at, length):
    if at < origin:
        return at
    if at > origin + length:
        return at - length
    return origin


def model(mode, ptr, prev):
    # The follow-mode contract (see crates/flamewm-wm-wayland/src/zoom.rs), per axis.
    out = []
    for axis, (p, size) in enumerate(((ptr[0], W), (ptr[1], H))):
        length = size / ZOOM
        if mode == "proportional":
            v = p / size * (size - length)
        elif mode == "centred" or prev is None:
            v = p - length / 2
        else:
            v = push1(prev[axis], p, length)
        out.append(min(max(v, 0.0), size - length))
    return tuple(out)


origins = {}
for line in open(os.path.join(run, "inner.txt")):
    if " origin=" in line:
        mode, rest = line.split(" origin=")
        origins[mode] = tuple(int(v) for v in rest.split())
rows = [l.rstrip("\n").split("\t") for l in open(os.path.join(run, "steps.tsv"))]
ok_all = True
print(f"{'mode':<13}{'step':>4}{'pointer':>12}{'measured':>14}{'model':>14}{'ncc':>7}  verdict")
for mode in ("proportional", "centred", "push"):
    if mode not in origins:
        print(f"{mode}: no window"); ok_all = False; continue
    ox, oy = origins[mode]
    ref = window(read_ppm(os.path.join(run, mode, "ref.ppm")), ox, oy)
    prev = None
    seen = set()
    for m, step, px, py in (r for r in rows if r[0] == mode):
        zoomed = window(read_ppm(os.path.join(run, mode, f"s{step}.ppm")), ox, oy)
        mx, my, ncc = locate(ref, zoomed)
        ex, ey = model(mode, (float(px), float(py)), prev)
        prev = (ex, ey)
        ok = abs(mx - ex) <= TOL and abs(my - ey) <= TOL and ncc >= MIN_NCC
        ok_all = ok_all and ok
        seen.add((mx, my))
        print(f"{mode:<13}{step:>4}{px + ',' + py:>12}{f'{mx},{my}':>14}{f'{ex:.0f},{ey:.0f}':>14}{ncc:>7.3f}  {'OK' if ok else 'MISMATCH'}")
    if len(seen) < 3:
        print(f"{mode}: viewport moved to only {len(seen)} distinct origins"); ok_all = False
    print(f"{mode}: window origin={ox},{oy} frames={os.path.join(run, mode)}")
sys.exit(0 if ok_all else 1)
PY
then verdict=PASS; fi
panics=$(grep -c 'panicked' "$RUN"/*/wm.log 2>/dev/null | awk -F: '{s+=$2} END {print s+0}')
[[ "$panics" == 0 ]] || verdict=FAIL
for f in "$RUN"/*/s*.ppm "$RUN"/*/ref.ppm; do
  [[ -f "$f" ]] && command -v convert >/dev/null && convert "$f" "${f%.ppm}.png" 2>/dev/null
done
{
  echo "ZOOMFOLLOW: $verdict panics=$panics inner_rc=$rc $(tr '\n' ' ' <"$RUN/inner.txt" 2>/dev/null)"
  echo "run=$RUN"
} | tee "$RUN/verdict.txt"
[[ "$verdict" == FAIL ]] && exit 1
exit 0

#!/usr/bin/env bash
# startup.sh: cold-start latency probe for FlameWM app binaries.
#
# Measures wall time from exec to (a) first mapped top-level window and (b) first painted
# frame (window pixels stop being one uniform colour), on a private Xvfb with a private
# session D-Bus. Each app runs WARMUPS discarded runs then RUNS timed runs; reports medians.
#
# usage: scripts/perf/startup.sh [--bin-dir DIR] [--runs N] [--warmups N] [--json FILE] [app ...]
#   apps (default: all): explorer terminal imageviewer archiver settings task-manager finder
#   --bin-dir defaults to $CARGO_TARGET_DIR/fast (or target/fast) of this checkout.
# Requires: Xvfb, dbus-daemon, python3 with python-Xlib. Timings are wall-clock on a shared
# machine: compare ratios between runs, not absolute milliseconds.
set -euo pipefail

ROOT="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/../.." && pwd -P)"
BIN_DIR=""
RUNS=5
WARMUPS=1
JSON_OUT=""
APPS=()

while [ $# -gt 0 ]; do
  case "$1" in
    --bin-dir) BIN_DIR="$2"; shift 2 ;;
    --runs) RUNS="$2"; shift 2 ;;
    --warmups) WARMUPS="$2"; shift 2 ;;
    --json) JSON_OUT="$2"; shift 2 ;;
    -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
    *) APPS+=("$1"); shift ;;
  esac
done

if [ -z "$BIN_DIR" ]; then
  TD="${CARGO_TARGET_DIR:-$ROOT/target}"
  BIN_DIR="$TD/fast"
fi
if [ ${#APPS[@]} -eq 0 ]; then
  APPS=(explorer terminal imageviewer archiver settings task-manager finder)
fi

# app alias -> binary name
bin_for() {
  case "$1" in
    explorer) echo flamewm-explorer ;;
    terminal) echo flamewm-terminal ;;
    imageviewer) echo flamewm-imageviewer ;;
    archiver) echo flamewm-archiver ;;
    settings) echo flamewm-settings ;;
    task-manager) echo flamewm-task-manager ;;
    finder) echo flamewm-finder ;;
    *) echo "unknown app: $1" >&2; return 1 ;;
  esac
}

for a in "${APPS[@]}"; do
  b="$(bin_for "$a")"
  [ -x "$BIN_DIR/$b" ] || { echo "missing binary: $BIN_DIR/$b (build with cargo build --profile fast -p ...)" >&2; exit 2; }
done

WORK="$(mktemp -d "${TMPDIR:-/tmp}/flamewm-startup.XXXXXX")"
cleanup() {
  [ -n "${XVFB_PID:-}" ] && kill "$XVFB_PID" 2>/dev/null || true
  [ -n "${DBUS_PID:-}" ] && kill "$DBUS_PID" 2>/dev/null || true
  rm -rf "$WORK"
}
trap cleanup EXIT

# Private display: first free number from 90 upward.
DNUM=90
while [ -e "/tmp/.X${DNUM}-lock" ] || [ -e "/tmp/.X11-unix/X${DNUM}" ]; do DNUM=$((DNUM + 1)); done
export DISPLAY=":${DNUM}"
# 1920x1080: must contain the largest default window (task-manager 1180x805 at y=55). A window that
# extends past the screen edge makes GetImage fail with BadMatch, which reads as "never painted".
Xvfb "$DISPLAY" -screen 0 1920x1080x24 -nolisten tcp >"$WORK/xvfb.log" 2>&1 &
XVFB_PID=$!
for _ in $(seq 1 100); do
  if xdpyinfo -display "$DISPLAY" >/dev/null 2>&1; then break; fi
  sleep 0.05
done
xdpyinfo -display "$DISPLAY" >/dev/null 2>&1 || { echo "Xvfb did not start; see $WORK/xvfb.log" >&2; exit 3; }

# Private session bus, so apps that connect to D-Bus at startup do not hang or fall back.
BUS_ADDR="unix:path=$WORK/bus.sock"
dbus-daemon --session --fork --address="$BUS_ADDR" --print-pid=1 >"$WORK/dbus.pid" 2>"$WORK/dbus.err"
DBUS_PID="$(cat "$WORK/dbus.pid")"

cat >"$WORK/probe.py" <<'PY'
import json, os, signal, subprocess, sys, time
from Xlib import display as xdisplay, X

def probe(argv, env, timeout=20.0):
    disp = xdisplay.Display(os.environ["DISPLAY"])
    root = disp.screen().root
    before = {w.id for w in root.query_tree().children}
    t0 = time.monotonic_ns()
    proc = subprocess.Popen(argv, env=env, stdin=subprocess.DEVNULL,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                            start_new_session=True)
    t_map = t_paint = None
    win = None
    image_error = None
    try:
        deadline = t0 + int(timeout * 1e9)
        while time.monotonic_ns() < deadline:
            if win is None:
                for w in root.query_tree().children:
                    if w.id in before:
                        continue
                    try:
                        attrs = w.get_attributes()
                        geom = w.get_geometry()
                    except Exception:
                        continue
                    if attrs.map_state == X.IsViewable and geom.width >= 100 and geom.height >= 100:
                        win = w
                        t_map = time.monotonic_ns() - t0
                        break
            if win is not None:
                try:
                    img = win.get_image(0, 0, geom_w(win), geom_h(win), X.ZPixmap, 0xFFFFFFFF).data
                except Exception as error:
                    # Recorded in the result so a probe failure is never mistaken for a slow paint.
                    img = b""
                    image_error = f"{type(error).__name__}: {error}"
                # Painted = window pixels are no longer one uniform colour.
                if img and img[:4] * (len(img) // 4) != img:
                    t_paint = time.monotonic_ns() - t0
                    break
            if proc.poll() is not None and win is None:
                break
            time.sleep(0.005)
    finally:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        proc.wait()
        disp.close()
    return {
        "mapped_ms": None if t_map is None else t_map / 1e6,
        "painted_ms": None if t_paint is None else t_paint / 1e6,
        "exit_early": proc.returncode is not None and win is None,
        "image_error": image_error,
    }

def geom_w(w):
    return w.get_geometry().width

def geom_h(w):
    return w.get_geometry().height

if __name__ == "__main__":
    spec = json.loads(sys.argv[1])
    env = dict(os.environ)
    env.update(spec.get("env", {}))
    print(json.dumps(probe(spec["argv"], env)))
PY

ENVDIR="$WORK/home"
mkdir -p "$ENVDIR"
run_env() {
  # Isolated HOME/XDG so user state never changes the measurement; caches persist across runs.
  env -i PATH="$PATH" DISPLAY="$DISPLAY" DBUS_SESSION_BUS_ADDRESS="$BUS_ADDR" \
    PYTHONUSERBASE="$HOME/.local" HOME="$ENVDIR/home"XDG_CONFIG_HOME="$ENVDIR/config" XDG_CACHE_HOME="$ENVDIR/cache" \
    XDG_DATA_HOME="$ENVDIR/data" XDG_RUNTIME_DIR="$ENVDIR/run" LANG=C.UTF-8 "$@"
}
mkdir -p "$ENVDIR/home" "$ENVDIR/config" "$ENVDIR/cache" "$ENVDIR/data" "$ENVDIR/run"
chmod 700 "$ENVDIR/run"

RESULTS="$WORK/results.jsonl"
: >"$RESULTS"
# Fixture for apps that require an operand: a small tar.gz archive in the private work dir.
FIXTURE_DIR="$WORK/fixture"
mkdir -p "$FIXTURE_DIR/sample"
printf 'alpha\n' >"$FIXTURE_DIR/sample/a.txt"
printf 'beta\n' >"$FIXTURE_DIR/sample/b.txt"
tar -czf "$WORK/sample.tar.gz" -C "$FIXTURE_DIR" sample
for a in "${APPS[@]}"; do
  b="$(bin_for "$a")"
  extra=""
  if [ "$a" = archiver ]; then extra=", \"$WORK/sample.tar.gz\""; fi
  total=$((WARMUPS + RUNS))
  for i in $(seq 1 "$total"); do
    # A probe error (for example a dropped X connection) is recorded as a failed run, not a script abort.
    out="$(run_env python3 "$WORK/probe.py" "{\"argv\": [\"$BIN_DIR/$b\"$extra]}" 2>>"$WORK/probe.err" ||
      echo '{"mapped_ms": null, "painted_ms": null, "exit_early": true}')"
    if [ "$i" -gt "$WARMUPS" ]; then
      printf '{"app":"%s","run":%d,"result":%s}\n' "$a" "$((i - WARMUPS))" "$out" >>"$RESULTS"
    fi
  done
done

python3 -I - "$RESULTS" "$JSON_OUT" <<'PY'
import json, statistics, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
apps = []
for r in rows:
    if r["app"] not in apps:
        apps.append(r["app"])
summary = {}
print(f"{'app':<14}{'mapped_med_ms':>15}{'painted_med_ms':>16}{'  mapped_runs_ms':<40}  failed")
for a in apps:
    rs = [r["result"] for r in rows if r["app"] == a]
    m = [x["mapped_ms"] for x in rs if x["mapped_ms"] is not None]
    p = [x["painted_ms"] for x in rs if x["painted_ms"] is not None]
    med_m = round(statistics.median(m), 1) if m else None
    med_p = round(statistics.median(p), 1) if p else None
    summary[a] = {"mapped_median_ms": med_m, "painted_median_ms": med_p,
                  "mapped_runs_ms": [round(x, 1) for x in m],
                  "painted_runs_ms": [round(x, 1) for x in p],
                  "failed_runs": len(rs) - len(p)}
    s = summary[a]
    print(f"{a:<14}{str(med_m):>15}{str(med_p):>16}  {str(s['mapped_runs_ms']):<40}  {s['failed_runs']}")
if len(sys.argv) > 2 and sys.argv[2]:
    json.dump(summary, open(sys.argv[2], "w"), indent=2)
PY

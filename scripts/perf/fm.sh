#!/usr/bin/env bash
# fm.sh: fast FlameWM probe (<=25 s): wakes/s, CPU%, Anon/PSS per process.
# usage: fm.sh <bin-dir|target-dir> [idle|marquee|hover|all] [--json]
#        fm.sh diff a.json b.json
# idle 8 s, marquee 5 s, hover 5 s. Nested winit session on a private Xvfb; XTEST on host display.
# --json: JSON on stdout, table on stderr. Every run also writes RUN/result.json.
set -u
M=/run/media/linsaftw/SDD/meas-flamewm
usage(){ echo "usage: fm.sh <bin-dir|target-dir> [idle|marquee|hover|all] [--json] | fm.sh diff a.json b.json" >&2; exit 2; }
PY=$(cat <<'PY'
import json, math, os, sys, threading, time, datetime
NAMES = ['flamewm-wayland', 'flamewm-desktop', 'flamewm-shell', 'Xwayland']
SECS = {'idle': 8, 'marquee': 5, 'hover': 5}
HZ = os.sysconf('SC_CLK_TCK')
ERRS = []

def snap(pg):
    out = {n: {} for n in NAMES}
    for name in os.listdir('/proc'):
        if not name.isdigit():
            continue
        pid = int(name)
        try:
            raw = open(f'/proc/{pid}/stat').read()
            end = raw.rfind(')')
            comm = raw[raw.find('(') + 1:end]
            f = raw[end + 2:].split()
            if int(f[2]) != pg or comm not in out:
                continue
            key = f'{pid}:{f[19]}'
            wakes = 0
            for t in os.listdir(f'/proc/{pid}/task'):
                for line in open(f'/proc/{pid}/task/{t}/status'):
                    if line.startswith(('voluntary_ctxt_switches:', 'nonvoluntary_ctxt_switches:')):
                        wakes += int(line.split(':')[1])
            roll = {}
            for line in open(f'/proc/{pid}/smaps_rollup'):
                k, _, v = line.partition(':')
                if k in ('Rss', 'Pss', 'Anonymous'):
                    roll[k] = int(v.split()[0])
            out[comm][key] = {'ticks': int(f[11]) + int(f[12]), 'wakes': wakes, **roll}
        except (OSError, ValueError, IndexError):
            continue
    return out

def drive(host, mode, secs):
    # same pointer paths as xtest.py: marquee ellipse drag / right-click then hover sweep
    try:
        from Xlib import X, display
        from Xlib.ext import xtest
        d = display.Display(host)
        root = d.screen().root
        def move(x, y):
            root.warp_pointer(x, y); d.sync()
        def btn(b, down):
            xtest.fake_input(d, X.ButtonPress if down else X.ButtonRelease, b); d.sync()
        if mode == 'marquee':
            move(200, 200); btn(1, True); s = time.monotonic()
            while time.monotonic() - s < secs:
                a = (time.monotonic() - s) * 2.0
                move(int(640 + 500 * math.cos(a)), int(400 + 250 * math.sin(a))); time.sleep(0.01)
            btn(1, False)
        elif mode == 'hover':
            move(640, 400); btn(3, True); btn(3, False); time.sleep(0.5); s = time.monotonic()
            while time.monotonic() - s < secs:
                a = (time.monotonic() - s) * 1.5
                move(int(640 + 120 * math.cos(a)), int(420 + 150 * math.sin(a))); time.sleep(0.01)
            move(5, 5); btn(1, True); btn(1, False)
        d.close()
    except Exception as e:  # keep the sample running, record the failure
        ERRS.append(f'xtest {mode}: {e!r}')

def run_mode(pg, host, mode):
    secs = SECS[mode]
    th = None if mode == 'idle' else threading.Thread(target=drive, args=(host, mode, secs), daemon=True)
    a = snap(pg)
    t0 = time.monotonic()
    if th:
        th.start()
    time.sleep(secs)
    b = snap(pg)
    el = time.monotonic() - t0
    if th:
        th.join(secs + 5)
    procs = {}
    for n in NAMES:
        keys = [k for k in b[n] if k in a[n]]
        if not keys:
            continue
        ticks = sum(b[n][k]['ticks'] - a[n][k]['ticks'] for k in keys)
        wakes = sum(b[n][k]['wakes'] - a[n][k]['wakes'] for k in keys)
        procs[n] = {'wakes': wakes, 'wakes_per_s': round(wakes / el, 2),
                    'cpu_pct': round(100 * ticks / HZ / el, 2), 'pids': len(keys)}
    return {'seconds': secs, 'elapsed_s': round(el, 3), 'procs': procs}, b

def mem_of(b):
    mem = {}
    for n in NAMES:
        vals = list(b[n].values())
        if vals:
            mem[n] = {'anon_mb': round(sum(v['Anonymous'] for v in vals) / 1024, 1),
                      'pss_mb': round(sum(v['Pss'] for v in vals) / 1024, 1),
                      'rss_mb': round(sum(v['Rss'] for v in vals) / 1024, 1)}
    return mem

def render(res, cols_modes):
    L = []
    hdr = f"{'process':<17}"
    for m in cols_modes:
        hdr += f"{m[:3] + ' w/s':>10}{m[:3] + ' cpu%':>10}"
    hdr += f"{'anonMB':>9}{'pssMB':>9}"
    L.append(hdr)
    for n in NAMES + ['total']:
        row = f'{n:<17}'
        for m in cols_modes:
            pm = res['modes'][m]['procs']
            if n == 'total':
                w = sum(p['wakes_per_s'] for p in pm.values()) if pm else None
                c = sum(p['cpu_pct'] for p in pm.values()) if pm else None
            else:
                p = pm.get(n)
                w = p['wakes_per_s'] if p else None
                c = p['cpu_pct'] if p else None
            row += f"{'-' if w is None else f'{w:.2f}':>10}{'-' if c is None else f'{c:.2f}':>10}"
        mm = res['mem'].get(n) if n != 'total' else None
        if n == 'total':
            an = sum(v['anon_mb'] for v in res['mem'].values()) if res['mem'] else None
            ps = sum(v['pss_mb'] for v in res['mem'].values()) if res['mem'] else None
        else:
            an = mm['anon_mb'] if mm else None
            ps = mm['pss_mb'] if mm else None
        row += f"{'-' if an is None else f'{an:.1f}':>9}{'-' if ps is None else f'{ps:.1f}':>9}"
        L.append(row)
    L.append(f"commits: n/a (no commit counter in build)  startup={res['startup_s']}s  bin={res['bin']}")
    if res['errors']:
        L.append('errors: ' + '; '.join(res['errors']))
    return '\n'.join(L)

def diff(pa, pb):
    A, B = json.load(open(pa)), json.load(open(pb))
    L = [f"diff {pa} -> {pb}", f"{'mode':<8}{'process':<17}{'w/s a':>9}{'w/s b':>9}{'delta':>9}{'cpu a':>9}{'cpu b':>9}{'delta':>9}"]
    for m in A['modes']:
        if m not in B['modes']:
            continue
        for n in NAMES:
            x = A['modes'][m]['procs'].get(n); y = B['modes'][m]['procs'].get(n)
            if not x or not y:
                continue
            L.append(f"{m:<8}{n:<17}{x['wakes_per_s']:>9.2f}{y['wakes_per_s']:>9.2f}{y['wakes_per_s']-x['wakes_per_s']:>+9.2f}"
                     f"{x['cpu_pct']:>9.2f}{y['cpu_pct']:>9.2f}{y['cpu_pct']-x['cpu_pct']:>+9.2f}")
    for n in NAMES:
        x = A['mem'].get(n); y = B['mem'].get(n)
        if x and y:
            L.append(f"mem {n:<17} anon {x['anon_mb']:.1f}->{y['anon_mb']:.1f} ({y['anon_mb']-x['anon_mb']:+.1f} MB)  "
                     f"pss {x['pss_mb']:.1f}->{y['pss_mb']:.1f} ({y['pss_mb']-x['pss_mb']:+.1f} MB)")
    print('\n'.join(L))

if sys.argv[1] == 'diff':
    diff(sys.argv[2], sys.argv[3]); sys.exit(0)

# run mode: argv = run PG HOST MODES(csv) JSON(0|1) STARTUP BIN OUT
pg = int(sys.argv[2]); host = sys.argv[3]; modes = sys.argv[4].split(','); want_json = sys.argv[5] == '1'
res = {'time': datetime.datetime.now().isoformat(timespec='seconds'), 'bin': sys.argv[7],
       'startup_s': float(sys.argv[6]), 'modes': {}, 'mem': {}, 'commits': None, 'errors': ERRS}
last = None
for m in modes:
    r, last = run_mode(pg, host, m)
    res['modes'][m] = r
res['mem'] = mem_of(last)
open(sys.argv[8], 'w').write(json.dumps(res, indent=1) + '\n')
table = render(res, modes)
if want_json:
    print(table, file=sys.stderr)
    print(json.dumps(res, indent=1))
else:
    print(table)
PY
)

[ $# -ge 1 ] || usage
if [ "$1" = diff ]; then
  [ $# -eq 3 ] || usage
  exec python3 -c "$PY" diff "$2" "$3"
fi
T=$1; MODE=all; JSON=0
for a in "${@:2}"; do
  case $a in
    --json) JSON=1 ;;
    idle|marquee|hover|all) MODE=$a ;;
    *) usage ;;
  esac
done
case $MODE in idle) MODES=idle ;; marquee) MODES=marquee ;; hover) MODES=hover ;; all) MODES=idle,marquee,hover ;; esac

BIN=""
for c in "$T" "$T/release" "$T/debug"; do
  if [ -x "$c/flamewm-wayland" ]; then BIN=$(readlink -f "$c"); break; fi
done
[ -n "$BIN" ] || { echo "no flamewm-wayland under $T (or $T/release, $T/debug)" >&2; exit 1; }
for b in flamewm-desktop flamewm-shell; do
  [ -x "$BIN/$b" ] || { echo "missing $BIN/$b" >&2; exit 1; }
done

RUN=$M/fast/runs/$(date +%Y%m%d-%H%M%S)-$$
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

T0=$(date +%s.%N)
Xvfb $HOSTD -screen 0 1280x800x24 -nolisten tcp -noreset -ac >"$RUN/xvfb.log" 2>&1 & XV=$!
for _ in $(seq 1 40); do DISPLAY=$HOSTD xdpyinfo >/dev/null 2>&1 && break; sleep .25; done
env -u WAYLAND_DISPLAY -u XAUTHORITY DISPLAY=$HOSTD HOME=$RUN/home XDG_RUNTIME_DIR=$RUN/rt \
  XDG_CONFIG_HOME=$RUN/home/.config XDG_STATE_HOME=$RUN/home/.local/state XDG_CACHE_HOME=$RUN/home/.cache \
  XDG_DATA_HOME=$RUN/home/.local/share FLAMEWM_WAYLAND_BACKEND=winit \
  FLAMEWM_WAYLAND_AUTOSTART=$BIN/flamewm-desktop:$BIN/flamewm-shell PATH=$BIN:/usr/bin:/bin \
  setsid dbus-run-session -- $BIN/flamewm-wayland >"$RUN/compositor.log" 2>&1 & WM=$!
sleep 1
# only ever signal a group we verified we lead (pgid == our child pid)
PG=$(ps -o pgid= -p $WM 2>/dev/null | tr -d ' ')
if [ "$PG" != "$WM" ]; then echo "refusing to signal group: pgid=$PG pid=$WM" >&2; PG=""; exit 1; fi
for _ in $(seq 1 60); do
  pgrep -g $PG -x flamewm-shell >/dev/null && pgrep -g $PG -x flamewm-desktop >/dev/null \
    && pgrep -g $PG -x Xwayland >/dev/null && break
  sleep .25
done
sleep 2
T1=$(date +%s.%N)
STARTUP=$(python3 -I -c 'import sys; print(f"{float(sys.argv[1])-float(sys.argv[2]):.1f}")' "$T1" "$T0")
python3 -c "$PY" run "$PG" "$HOSTD" "$MODES" "$JSON" "$STARTUP" "$BIN" "$RUN/result.json"
RC=$?
echo "run=$RUN" >&2
exit $RC

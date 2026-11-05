#!/usr/bin/env python3
"""Settings slider-drag and tab-switch scenario driver for a running nested FlameWM session.

Sibling of fm.sh and launcher_scenarios.py. Drives host XTEST pointer events against a session started by
settings_session.sh, and reports per-phase CPU ticks, voluntary plus nonvoluntary wakes, and write syscalls
(`syscw` from /proc/<pid>/io, a proxy for WM IPC sends) for the WM processes and flamewm-settings.

usage: settings_scenarios.py <pgid> <host-display> <shot-prefix> \
         --slider x0,x1,y --tabs x,y[;x,y...] [--steps N] [--idle-secs S]

--slider  host-screen pixels of the glass slider track: left end, right end, row.
--tabs    host-screen pixels of the settings tabs to click in turn.
Prints one JSON object on stdout. Requires python3-xlib and xwd+convert for <shot-prefix>.png.
"""
import argparse
import json
import os
import subprocess
import sys
import time

from Xlib import X, display
from Xlib.ext import xtest

NAMES = ('flamewm-wayland', 'flamewm-desktop', 'flamewm-shell', 'Xwayland', 'flamewm-setting')
HZ = os.sysconf('SC_CLK_TCK')


def snap(pgid):
    out = {}
    for name in os.listdir('/proc'):
        if not name.isdigit():
            continue
        try:
            raw = open(f'/proc/{name}/stat').read()
            end = raw.rfind(')')
            comm = raw[raw.find('(') + 1:end]
            fields = raw[end + 2:].split()
            if int(fields[2]) != pgid or comm not in NAMES:
                continue
            wakes = 0
            for task in os.listdir(f'/proc/{name}/task'):
                with open(f'/proc/{name}/task/{task}/status') as status:
                    for line in status:
                        if line.startswith(('voluntary_ctxt_switches:', 'nonvoluntary_ctxt_switches:')):
                            wakes += int(line.split(':')[1])
            syscw = 0
            with open(f'/proc/{name}/io') as io:
                for line in io:
                    if line.startswith('syscw:'):
                        syscw = int(line.split(':')[1])
            out[f'{comm}:{name}'] = {'cpu': (int(fields[11]) + int(fields[12])) / HZ, 'wakes': wakes, 'syscw': syscw}
        except (OSError, IndexError, ValueError):
            continue
    return out


def delta(before, after, seconds, motions=0):
    rows = {}
    for key, b in after.items():
        a = before.get(key, b)
        comm = key.split(':')[0]
        rows[comm] = {
            'cpu_pct': round(100.0 * (b['cpu'] - a['cpu']) / seconds, 2),
            'wakes_per_s': round((b['wakes'] - a['wakes']) / seconds, 1),
            'syscw': b['syscw'] - a['syscw'],
        }
    total = {
        'cpu_pct': round(sum(r['cpu_pct'] for r in rows.values()), 2),
        'wakes_per_s': round(sum(r['wakes_per_s'] for r in rows.values()), 1),
        'syscw': sum(r['syscw'] for r in rows.values()),
    }
    if motions:
        total['syscw_per_motion'] = round(total['syscw'] / motions, 2)
        total['motions'] = motions
    rows['total'] = total
    return rows


class Pointer:
    def __init__(self, host):
        self.d = display.Display(host)
        self.root = self.d.screen().root

    def move(self, x, y):
        self.root.warp_pointer(x, y)
        self.d.sync()

    def button(self, b, down):
        xtest.fake_input(self.d, X.ButtonPress if down else X.ButtonRelease, b)
        self.d.sync()


def drag(p, x0, x1, y, steps, dwell):
    p.move(x0, y)
    p.button(1, True)
    for i in range(1, steps + 1):
        p.move(x0 + (x1 - x0) * i // steps, y)
        time.sleep(dwell)
    p.button(1, False)


def click_tabs(p, tabs, dwell):
    for x, y in tabs:
        p.move(x, y)
        p.button(1, True)
        p.button(1, False)
        time.sleep(dwell)


def shot(host, prefix):
    subprocess.run(['sh', '-c', f'DISPLAY={host} xwd -root -silent | convert xwd:- {prefix}.png'], check=False)


def parse_points(text):
    return [tuple(int(v) for v in pair.split(',')) for pair in text.split(';') if pair]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('pgid', type=int)
    ap.add_argument('host')
    ap.add_argument('prefix')
    ap.add_argument('--slider', required=True)
    ap.add_argument('--tabs', required=True)
    ap.add_argument('--pre', default='', help='x,y[;x,y...] tabs clicked before the idle phase')
    ap.add_argument('--steps', type=int, default=60)
    ap.add_argument('--idle-secs', type=float, default=5.0)
    args = ap.parse_args()
    x0, x1, y = (int(v) for v in args.slider.split(','))
    tabs = parse_points(args.tabs)
    p = Pointer(args.host)
    result = {'phases': {}}

    click_tabs(p, parse_points(args.pre), dwell=1.0)
    shot(args.host, args.prefix + '-pre')
    before = snap(args.pgid)
    time.sleep(args.idle_secs)
    result['phases']['idle'] = delta(before, snap(args.pgid), args.idle_secs)

    before = snap(args.pgid)
    t0 = time.monotonic()
    drag(p, x0, x1, y, args.steps, dwell=0.01)
    elapsed = time.monotonic() - t0
    result['phases']['slider_drag'] = delta(before, snap(args.pgid), elapsed, motions=args.steps)

    shot(args.host, args.prefix + '-after-drag')
    before = snap(args.pgid)
    t0 = time.monotonic()
    click_tabs(p, tabs, dwell=0.5)
    elapsed = time.monotonic() - t0
    result['phases']['tab_switch'] = delta(before, snap(args.pgid), elapsed, motions=len(tabs))
    shot(args.host, args.prefix + '-after-tabs')
    json.dump(result, sys.stdout, indent=1)
    print()


if __name__ == '__main__':
    main()

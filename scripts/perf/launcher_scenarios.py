#!/usr/bin/env python3
"""Launcher/Start scenario driver for a running nested FlameWM session.

Sibling of fm.sh. Drives host XTEST keys and pointer moves against a session
started by the caller, and reports per-scenario CPU ticks and wakes (voluntary
plus nonvoluntary context switches) for the shell, desktop, compositor and
Xwayland processes in the session's process group.

usage: launcher_scenarios.py <pgid> <host-display> [scenario,...]
scenarios: idle, open_close, type, arrows, hover (default: all)
Prints one JSON object per scenario on stdout. Requires python3-xlib.
"""
import json
import math
import os
import sys
import time

NAMES = ('flamewm-wayland', 'flamewm-desktop', 'flamewm-shell', 'Xwayland')
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
            cpu = (int(fields[11]) + int(fields[12])) / HZ
            wakes = 0
            for task in os.listdir(f'/proc/{name}/task'):
                with open(f'/proc/{name}/task/{task}/status') as status:
                    for line in status:
                        if line.startswith(('voluntary_ctxt_switches:', 'nonvoluntary_ctxt_switches:')):
                            wakes += int(line.split(':')[1])
            out[f'{comm}:{name}'] = (cpu, wakes)
        except (OSError, IndexError, ValueError):
            continue
    return out


def delta(before, after, seconds):
    rows = {}
    for key, (cpu1, w1) in after.items():
        cpu0, w0 = before.get(key, (cpu1, w1))
        comm = key.split(':')[0]
        rows[comm] = {
            'cpu_pct': round(100.0 * (cpu1 - cpu0) / seconds, 2),
            'wakes_per_s': round((w1 - w0) / seconds, 1),
        }
    total = {
        'cpu_pct': round(sum(r['cpu_pct'] for r in rows.values()), 2),
        'wakes_per_s': round(sum(r['wakes_per_s'] for r in rows.values()), 1),
    }
    return {'procs': rows, 'total': total}


class Keys:
    def __init__(self, host):
        from Xlib import X, XK, display
        from Xlib.ext import xtest
        self.X, self.XK, self.xtest = X, XK, xtest
        self.d = display.Display(host)
        self.root = self.d.screen().root

    def _code(self, keysym):
        return self.d.keysym_to_keycode(self.XK.string_to_keysym(keysym))

    def tap(self, keysym, hold=0.02):
        code = self._code(keysym)
        self.xtest.fake_input(self.d, self.X.KeyPress, code)
        self.d.sync()
        time.sleep(hold)
        self.xtest.fake_input(self.d, self.X.KeyRelease, code)
        self.d.sync()

    def chord(self, mods, keysym):
        codes = [self._code(m) for m in mods]
        for code in codes:
            self.xtest.fake_input(self.d, self.X.KeyPress, code)
        self.d.sync()
        self.tap(keysym)
        for code in reversed(codes):
            self.xtest.fake_input(self.d, self.X.KeyRelease, code)
        self.d.sync()

    def move(self, x, y):
        self.root.warp_pointer(x, y)
        self.d.sync()


def run(name, keys, pgid, body, seconds=None):
    before = snap(pgid)
    t0 = time.monotonic()
    body()
    elapsed = time.monotonic() - t0
    if seconds is not None and elapsed < seconds:
        time.sleep(seconds - elapsed)
        elapsed = seconds
    after = snap(pgid)
    result = delta(before, after, elapsed)
    result['scenario'] = name
    result['seconds'] = round(elapsed, 2)
    return result


def main():
    pgid = int(sys.argv[1])
    host = sys.argv[2]
    wanted = sys.argv[3].split(',') if len(sys.argv) > 3 else [
        'idle', 'open_close', 'type', 'arrows', 'hover']
    keys = Keys(host)
    results = []

    def open_launcher():
        keys.chord(['Super_L'], 'space')
        time.sleep(0.8)

    def close_launcher():
        keys.chord(['Super_L'], 'space')
        time.sleep(0.5)

    for name in wanted:
        if name == 'idle':
            results.append(run(name, keys, pgid, lambda: time.sleep(5), 5))
        elif name == 'open_close':
            def body():
                for _ in range(5):
                    open_launcher()
                    close_launcher()
            results.append(run(name, keys, pgid, body))
        elif name == 'type':
            def body():
                open_launcher()
                for ch in 'firefox':
                    keys.tap(ch)
                    time.sleep(0.15)
                time.sleep(0.5)
                close_launcher()
            results.append(run(name, keys, pgid, body))
        elif name == 'arrows':
            def body():
                open_launcher()
                for _ in range(30):
                    keys.tap('Down')
                    time.sleep(0.08)
                close_launcher()
            results.append(run(name, keys, pgid, body))
        elif name == 'hover':
            def body():
                open_launcher()
                start = time.monotonic()
                while time.monotonic() - start < 4.0:
                    angle = (time.monotonic() - start) * 2.0
                    keys.move(int(640 + 120 * math.cos(angle)),
                              int(330 + 60 * math.sin(angle)))
                    time.sleep(0.02)
                close_launcher()
            results.append(run(name, keys, pgid, body))
        else:
            raise SystemExit(f'unknown scenario {name}')
    print(json.dumps(results))


if __name__ == '__main__':
    main()

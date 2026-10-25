#!/usr/bin/env python3
"""CPU benchmark for FlameWM processes on one explicit X display.

Samples /proc/<pid>/stat (utime+stime) of an allowlisted set of processes over
a fixed window and reports CPU percent of one core per process role and in
total. Repeats each case N times and reports median/min/max. Output is JSON on
stdout (or --json-out PATH).

Process selection (allowlist only; nothing is signalled or modified):
  * FlameWM roles (wm, desktop, shell, terminal, explorer): executable basename
    matches and /proc/<pid>/environ has DISPLAY=<display>.
  * Xephyr / Xorg: executable basename matches and the command line contains
    the display token (for example ":316").

Cases:
  idle            No input is injected. Always runs.
  rubber-band     XTEST drag on empty desktop (needs python-xlib + XTEST).
  start-toggle    XTEST click at --start-xy (needs python-xlib + XTEST).
  window-move     XTEST drag from --move-from to --move-to (needs python-xlib + XTEST).
  hover-sweep     XTEST motion only, no button press, repeated across
                  --hover-from/--hover-to for the whole sample window. This is
                  the pointer-motion workload: subtract the stationary
                  (idle) numbers to get the motion increment.
XTEST cases run only with --allow-input, because they move the real pointer
and send clicks to whatever X display is named. A case is skipped, with the
reason recorded, when its requirements are missing. --expect-class verifies
that the windows under the hover endpoints carry that WM_CLASS before any
motion is injected, so a hover run cannot silently measure another client.

Usage:
  python3 -I tools/bench-flamewm-cpu.py --display :316 --window 5 --repeat 3
  python3 -I tools/bench-flamewm-cpu.py --display :316 --allow-input \
      --start-xy 20,620 --move-from 300,200 --move-to 500,320
"""

import argparse
import json
import os
import statistics
import sys
import time

ROLE_BASENAMES = {
    "flamewm": "wm",
    "flamewm-desktop": "desktop",
    "flamewm-shell": "shell",
    "flamewm-terminal": "terminal",
    "flamewm-explorer": "explorer",
}
SERVER_BASENAMES = {"Xephyr": "xephyr", "Xorg": "xorg"}
HZ = os.sysconf("SC_CLK_TCK")
ROLE_ORDER = ("wm", "desktop", "shell", "terminal", "explorer", "xephyr", "xorg")


def _basename_of_exe(pid):
    try:
        exe = os.readlink(f"/proc/{pid}/exe")
    except OSError:
        return None
    return os.path.basename(exe.replace(" (deleted)", ""))


def _cmdline(pid):
    try:
        with open(f"/proc/{pid}/cmdline", "rb") as fh:
            return [a.decode(errors="replace") for a in fh.read().split(b"\0") if a]
    except OSError:
        return []


def _environ_display(pid):
    try:
        with open(f"/proc/{pid}/environ", "rb") as fh:
            for item in fh.read().split(b"\0"):
                if item.startswith(b"DISPLAY="):
                    return item[len(b"DISPLAY="):].decode(errors="replace")
    except OSError:
        pass
    return None


def discover(display):
    """Return {pid: role} for allowlisted FlameWM and X server processes on display."""
    found = {}
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        pid = int(entry)
        base = _basename_of_exe(pid)
        if base is None:
            continue
        if base in ROLE_BASENAMES and _environ_display(pid) == display:
            found[pid] = ROLE_BASENAMES[base]
        elif base in SERVER_BASENAMES and display in _cmdline(pid):
            found[pid] = SERVER_BASENAMES[base]
    return found


def read_stat(pid):
    """Return (ticks, starttime) or None if the process is gone."""
    try:
        with open(f"/proc/{pid}/stat") as fh:
            data = fh.read()
    except OSError:
        return None
    rest = data.rsplit(")", 1)[1].split()
    # rest[0] is field 3 (state); utime is field 14 -> rest[11]; stime 15 -> rest[12];
    # starttime is field 22 -> rest[19].
    return int(rest[11]) + int(rest[12]), int(rest[19])


def measure(display, duration, workload=None):
    """Sample allowlisted processes across one case. Returns per-process rows."""
    procs = discover(display)
    base = {pid: read_stat(pid) for pid in procs}
    t0 = time.monotonic()
    if workload is None:
        time.sleep(duration)
    else:
        workload(duration)
    elapsed = time.monotonic() - t0
    rows = []
    for pid, role in sorted(procs.items()):
        before, now = base.get(pid), read_stat(pid)
        # Same PID with a different starttime means the PID was reused: treat as gone.
        if before is None or now is None or before[1] != now[1]:
            rows.append({"pid": pid, "role": role, "gone": True})
            continue
        cpu_s = (now[0] - before[0]) / HZ
        rows.append({"pid": pid, "role": role, "cpu_s": round(cpu_s, 4),
                     "cpu_pct_one_core": round(100.0 * cpu_s / elapsed, 3) if elapsed else 0.0})
    return {"elapsed_s": round(elapsed, 3), "processes": rows,
            "roles_found": sorted(set(procs.values()), key=ROLE_ORDER.index)}


def _xtest_display(display):
    """Open display with XTEST, or return (None, reason)."""
    try:
        from Xlib import X, display as xdisplay  # noqa: F401
        from Xlib.ext import xtest  # noqa: F401
    except ImportError:
        return None, "python-xlib not importable"
    try:
        d = xdisplay.Display(display)
    except Exception as exc:  # noqa: BLE001 - report any open failure as a skip reason
        return None, f"cannot open display {display}: {exc}"
    if not d.has_extension("XTEST"):
        d.close()
        return None, "XTEST extension missing"
    return d, None


def _parse_xy(text):
    x, y = text.split(",")
    return int(x), int(y)


def _xtest_ops(d):
    from Xlib import X
    from Xlib.ext import xtest

    def move(x, y):
        xtest.fake_input(d, X.MotionNotify, x=x, y=y)
        d.sync()

    def press():
        xtest.fake_input(d, X.ButtonPress, 1)
        d.sync()

    def release():
        xtest.fake_input(d, X.ButtonRelease, 1)
        d.sync()

    return move, press, release


def _drag_workload(d, start, end, steps=60, step_s=0.012):
    move, press, release = _xtest_ops(d)

    def run(_duration):
        move(*start)
        press()
        time.sleep(0.05)
        for i in range(1, steps + 1):
            move(start[0] + (end[0] - start[0]) * i // steps,
                 start[1] + (end[1] - start[1]) * i // steps)
            time.sleep(step_s)
        release()
        time.sleep(0.12)
    return run


def _click_workload(d, point):
    move, press, release = _xtest_ops(d)

    def run(_duration):
        move(*point)
        press()
        release()
        time.sleep(0.5)
    return run


def _hover_workload(d, start, end, steps=40, rate_hz=125.0):
    """Motion-only sweep repeated until the sample window is filled.

    No button is ever pressed, so every injected event is an ordinary
    `MotionNotify`: this is the hover workload, not a drag. The sweep reverses
    direction instead of jumping back, so the pointer crosses rows the same way
    a real hand would and `hover` transitions (the repaint trigger) keep firing.
    """
    move, _press, _release = _xtest_ops(d)

    def run(duration):
        deadline = time.monotonic() + duration
        period = 1.0 / rate_hz if rate_hz > 0 else 0.0
        forward = True
        while time.monotonic() < deadline:
            order = range(steps + 1) if forward else range(steps, -1, -1)
            for i in order:
                if time.monotonic() >= deadline:
                    return
                move(start[0] + (end[0] - start[0]) * i // steps,
                     start[1] + (end[1] - start[1]) * i // steps)
                if period:
                    time.sleep(period)
            forward = not forward
    return run


def _window_class_at(d, x, y, max_depth=4):
    """WM_CLASS of the client window under (x, y), or None.

    Walks the window tree rather than only the root's children: a reparenting
    window manager puts the client inside a frame, so WM_CLASS sits on a
    grandchild (FlameWM itself does this). Bounded depth keeps the walk cheap
    and stops it from descending into every client's own widget tree.
    """
    root = d.screen().root
    queue = [(root, 0)]
    while queue:
        window, depth = queue.pop(0)
        try:
            children = window.query_tree().children
        except Exception:  # noqa: BLE001 - a window can vanish mid-walk
            continue
        for child in children:
            try:
                wm_class = child.get_wm_class()
                if wm_class:
                    # `translate_coords` is called on the *destination* window, so
                    # root.translate_coords(child, 0, 0) is the child's root origin.
                    origin = root.translate_coords(child, 0, 0)
                    geometry = child.get_geometry()
                    if (origin.x <= x < origin.x + geometry.width
                            and origin.y <= y < origin.y + geometry.height):
                        return wm_class
            except Exception:  # noqa: BLE001 - the window may vanish mid-walk
                continue
            if depth + 1 < max_depth:
                queue.append((child, depth + 1))
    return None


def run_case(name, args):
    """Run one case once. Returns a result dict; skipped cases carry a reason."""
    if name == "idle":
        return {"case": name, "status": "ran", **measure(args.display, args.window)}
    if not args.allow_input:
        return {"case": name, "status": "skipped", "reason": "XTEST cases need --allow-input"}
    if name == "rubber-band":
        start, end = args.rubber_from, args.rubber_to
        if start is None:
            return {"case": name, "status": "skipped", "reason": "needs --rubber-from/--rubber-to"}
    elif name == "start-toggle":
        if args.start_xy is None:
            return {"case": name, "status": "skipped", "reason": "needs --start-xy"}
    elif name == "window-move":
        if args.move_from is None or args.move_to is None:
            return {"case": name, "status": "skipped", "reason": "needs --move-from and --move-to"}
    elif name == "hover-sweep":
        if args.hover_from is None or args.hover_to is None:
            return {"case": name, "status": "skipped", "reason": "needs --hover-from and --hover-to"}
    d, reason = _xtest_display(args.display)
    if d is None:
        return {"case": name, "status": "skipped", "reason": reason}
    try:
        if name == "hover-sweep" and args.expect_class:
            seen = [_window_class_at(d, *point) for point in (args.hover_from, args.hover_to)]
            if any(wm_class is None or args.expect_class not in wm_class for wm_class in seen):
                return {"case": name, "status": "skipped",
                        "reason": f"hover endpoints are not over WM_CLASS {args.expect_class!r}: {seen!r}"}
        if name == "rubber-band":
            workload = _drag_workload(d, start, end)
        elif name == "start-toggle":
            workload = _click_workload(d, args.start_xy)
        elif name == "hover-sweep":
            workload = _hover_workload(d, args.hover_from, args.hover_to,
                                       steps=args.hover_steps, rate_hz=args.hover_rate)
        else:
            workload = _drag_workload(d, args.move_from, args.move_to)
        return {"case": name, "status": "ran", **measure(args.display, args.window, workload)}
    finally:
        d.close()


def summarize(runs):
    """Median/min/max of per-role and total CPU percent across repeats for one case."""
    roles = {}
    totals = []
    for run in runs:
        total = 0.0
        per_run = {}
        for row in run["processes"]:
            if row.get("gone"):
                continue
            per_run[row["role"]] = per_run.get(row["role"], 0.0) + row["cpu_pct_one_core"]
        for role, pct in per_run.items():
            roles.setdefault(role, []).append(pct)
            total += pct
        totals.append(total)

    def stats(values):
        return {"median": round(statistics.median(values), 3),
                "min": round(min(values), 3), "max": round(max(values), 3)}

    return {"cpu_pct_one_core_by_role": {r: stats(v) for r, v in sorted(roles.items(), key=lambda kv: ROLE_ORDER.index(kv[0]) if kv[0] in ROLE_ORDER else 99)},
            "cpu_pct_one_core_total": stats(totals) if totals else None}


def build_parser():
    p = argparse.ArgumentParser(description="FlameWM per-process CPU benchmark (read-only sampling).")
    p.add_argument("--display", default=os.environ.get("DISPLAY"),
                   help="X display to measure, for example :316 (default: $DISPLAY)")
    p.add_argument("--window", type=float, default=5.0, help="seconds per sample window (default 5)")
    p.add_argument("--repeat", type=int, default=3, help="repeats per case (default 3)")
    p.add_argument("--cases", default="idle,rubber-band,start-toggle,window-move",
                   help="comma-separated subset of: idle, rubber-band, start-toggle, window-move, hover-sweep")
    p.add_argument("--allow-input", action="store_true",
                   help="permit XTEST pointer input on --display (required for non-idle cases)")
    p.add_argument("--rubber-from", type=_parse_xy, metavar="X,Y")
    p.add_argument("--rubber-to", type=_parse_xy, metavar="X,Y")
    p.add_argument("--start-xy", type=_parse_xy, metavar="X,Y", help="click point for the start toggle")
    p.add_argument("--move-from", type=_parse_xy, metavar="X,Y")
    p.add_argument("--move-to", type=_parse_xy, metavar="X,Y")
    p.add_argument("--hover-from", type=_parse_xy, metavar="X,Y",
                   help="first hover-sweep endpoint (needs --hover-to)")
    p.add_argument("--hover-to", type=_parse_xy, metavar="X,Y",
                   help="second hover-sweep endpoint")
    p.add_argument("--hover-steps", type=int, default=40,
                   help="motion events per sweep leg (default 40)")
    p.add_argument("--hover-rate", type=float, default=125.0,
                   help="motion events per second (default 125)")
    p.add_argument("--expect-class", metavar="WM_CLASS",
                   help="refuse hover-sweep unless both endpoints are over this WM_CLASS")
    p.add_argument("--json-out", help="write JSON here instead of stdout")
    return p


def main(argv=None):
    args = build_parser().parse_args(argv)
    if not args.display:
        print("error: --display is required (or set DISPLAY)", file=sys.stderr)
        return 2
    if args.repeat < 1 or args.window <= 0:
        print("error: --repeat must be >= 1 and --window > 0", file=sys.stderr)
        return 2
    known = {"idle", "rubber-band", "start-toggle", "window-move", "hover-sweep"}
    cases = [c.strip() for c in args.cases.split(",") if c.strip()]
    unknown = sorted(set(cases) - known)
    if unknown:
        print(f"error: unknown cases: {unknown}", file=sys.stderr)
        return 2

    report = {"display": args.display, "window_s": args.window, "repeat": args.repeat,
              "clk_tck": HZ, "cases": {}}
    for name in cases:
        results = []
        skip = None
        for _ in range(args.repeat):
            res = run_case(name, args)
            if res["status"] == "skipped":
                skip = res["reason"]
                break
            results.append(res)
        if skip is not None:
            report["cases"][name] = {"status": "skipped", "reason": skip}
            continue
        report["cases"][name] = {"status": "ran", "runs": results, "summary": summarize(results)}

    text = json.dumps(report, indent=2)
    if args.json_out:
        with open(args.json_out, "w") as fh:
            fh.write(text + "\n")
    else:
        print(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())

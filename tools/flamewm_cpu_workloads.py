#!/usr/bin/env python3
"""Per-process CPU of everyday desktop workloads on a headless FlameWM X11 session.

Starts the same Xorg dummy session as flamewm_visual_tests.py, runs each named workload for
WINDOW seconds while sampling utime+stime of every flamewm* process on the display and of Xorg,
and prints one line per workload plus a final `JSON {...}` line. Values are percent of one core.

Usage: flamewm_cpu_workloads.py BIN_DIR WORKLOAD[,WORKLOAD...] [WINDOW]

Workloads: idle, hover-desktop, hover-taskbar, start-toggle, term-idle, term-seq, term-type,
term-move, term-hover, term-flood, explorer-idle, explorer-scroll, explorer-hover, explorer-move.

Env: BENCH_DISPLAY (default 93); OUT keeps session logs there; PERF_OUT wraps the session and
the opened apps in `perf record` (PERF_EXTRA adds perf options, PERF_FREQ sets -F,
PERF_APPS_ONLY skips the session).
"""
import json, os, subprocess, sys, time, contextlib
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parent))
import flamewm_visual_tests as vt
from Xlib.ext import xtest
from Xlib import X

BIN = sys.argv[1]
WORK = sys.argv[2].split(",")
WINDOW = float(sys.argv[3]) if len(sys.argv) > 3 else 6.0
PERF = os.environ.get("PERF_OUT")
W, H = vt.OUTPUT_SIZE


def ticks(display):
    out = {}
    for key, entry in vt.session_pids(display).items():
        with contextlib.suppress(OSError):
            out[key] = vt.cpu_ticks(entry)
    for entry in Path("/proc").iterdir():
        if entry.name.isdigit():
            with contextlib.suppress(OSError):
                if (entry / "comm").read_text().strip() == "Xorg":
                    out[f"Xorg {entry.name}"] = vt.cpu_ticks(entry)
    return out


def measure(session, action):
    before = ticks(session.display_name)
    start = time.monotonic()
    action(start + WINDOW)
    elapsed = time.monotonic() - start
    after = ticks(session.display_name)
    hz = os.sysconf("SC_CLK_TCK")
    res = {}
    for key, value in after.items():
        if key in before:
            pct = 100.0 * (value - before[key]) / hz / elapsed
            if pct > 0:
                res[key.split()[0]] = round(res.get(key.split()[0], 0) + pct, 2)
    return res


def idle(deadline):
    time.sleep(max(0, deadline - time.monotonic()))


def motion(session, x, y):
    xtest.fake_input(session.x, X.MotionNotify, x=x, y=y)
    session.x.sync()


def button(session, num, down):
    xtest.fake_input(session.x, X.ButtonPress if down else X.ButtonRelease, num)
    session.x.sync()


def hover(session, y):
    def run(deadline):
        step = 0
        while time.monotonic() < deadline:
            x = 20 + (step * 9) % (W - 40)
            motion(session, x, y)
            step += 1
            time.sleep(0.008)
    return run


def drag(session, grab):
    def run(deadline):
        while time.monotonic() < deadline:
            motion(session, *grab)
            button(session, 1, True)
            for i in range(60):
                motion(session, grab[0] + i * 6, grab[1] + i * 3)
                time.sleep(0.008)
            for i in range(60, 0, -1):
                motion(session, grab[0] + i * 6, grab[1] + i * 3)
                time.sleep(0.008)
            button(session, 1, False)
    return run


def wheel(session, at):
    def run(deadline):
        motion(session, *at)
        down = True
        count = 0
        while time.monotonic() < deadline:
            b = 5 if down else 4
            button(session, b, True)
            button(session, b, False)
            count += 1
            if count % 40 == 0:
                down = not down
            time.sleep(0.02)
    return run


def typing(session, client):
    def run(deadline):
        while time.monotonic() < deadline:
            session.xdotool("type", "--window", client, "--delay", "30", "echo hello world\n")
    return run


def clicks(session, at):
    def run(deadline):
        while time.monotonic() < deadline:
            motion(session, *at)
            button(session, 1, True)
            button(session, 1, False)
            time.sleep(0.4)
    return run


def close_last(session):
    import signal
    name, process, log = session.processes.pop()
    with contextlib.suppress(ProcessLookupError):
        os.killpg(process.pid, signal.SIGTERM)
    time.sleep(1)
    with contextlib.suppress(ProcessLookupError):
        os.killpg(process.pid, signal.SIGKILL)
    process.wait()
    log.close()
    time.sleep(1)


def frame_grab(session, client):
    frame = session.geometry(session.frame_of(client))
    return frame, (frame[0] + frame[2] // 2, frame[1] + 12)


def main():
    session = vt.Session("single", BIN, Path(os.environ["OUT"]) if os.environ.get("OUT") else None, int(os.environ.get("BENCH_DISPLAY", "93")))
    if PERF:
        orig = session.spawn
        def spawn(name, cmd, env=None):
            if (name == "session" and not os.environ.get("PERF_APPS_ONLY")) or name.startswith(("term", "explorer")):
                cmd = ["perf", "record", "-q", *(os.environ.get("PERF_EXTRA","").split()), "-F", os.environ.get("PERF_FREQ", "1999"), "-e", "cpu-clock:u", "-o", f"{PERF}/{name}.data", "--"] + cmd
            return orig(name, cmd, env)
        session.spawn = spawn
    report = {}
    try:
        session.start()
        time.sleep(2)
        for work in WORK:
            if work == "idle":
                report[work] = measure(session, idle)
            elif work == "hover-taskbar":
                report[work] = measure(session, hover(session, H - 20))
            elif work == "hover-desktop":
                report[work] = measure(session, hover(session, H // 2))
            elif work == "start-toggle":
                report[work] = measure(session, clicks(session, (24, H - 24)))
            elif work.startswith("term"):
                cmd = ["flamewm-terminal"]
                if work == "term-flood":
                    cmd += ["-e", "sh", "-c", "while true; do ls -la /usr/lib; done"]
                elif work == "term-seq":
                    cmd += ["-e", "sh", "-c", "sleep 2.5; seq 1 300000; sleep 100"]
                elif work == "term-top":
                    cmd += ["-e", "top", "-d", "0.5"]
                session.spawn(work, cmd)
                client = vt.wait_until(lambda: session.xdotool_search("flamewm-terminal", by="--class"), 40, 0.2)
                client = client.split()[0] if isinstance(client, str) else client
                session.xdotool("windowmove", client, 300, 200)
                time.sleep(2)
                if work == "term-idle":
                    report[work] = measure(session, idle)
                elif work == "term-type":
                    report[work] = measure(session, typing(session, client))
                elif work == "term-move":
                    _, grab = frame_grab(session, client)
                    report[work] = measure(session, drag(session, grab))
                elif work == "term-hover":
                    frame, _ = frame_grab(session, client)
                    report[work] = measure(session, hover(session, frame[1] + frame[3] // 2))
                else:
                    report[work] = measure(session, idle)
                close_last(session)
            elif work.startswith("explorer"):
                session.spawn(work, ["flamewm-explorer", "/usr/lib"])
                client = vt.wait_until(lambda: session.xdotool_search("flamewm-explorer", by="--class"), 40, 0.2)
                client = client.split()[0] if isinstance(client, str) else client
                session.xdotool("windowmove", client, 200, 100)
                time.sleep(3)
                frame, grab = frame_grab(session, client)
                center = (frame[0] + frame[2] // 2, frame[1] + frame[3] // 2)
                if work == "explorer-idle":
                    report[work] = measure(session, idle)
                elif work == "explorer-scroll":
                    report[work] = measure(session, wheel(session, center))
                elif work == "explorer-hover":
                    report[work] = measure(session, hover(session, center[1]))
                elif work == "explorer-move":
                    report[work] = measure(session, drag(session, grab))
                close_last(session)
            print(work, json.dumps(report[work]), flush=True)
    finally:
        session.stop()
    print("JSON", json.dumps(report))


main()

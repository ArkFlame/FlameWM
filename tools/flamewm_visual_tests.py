#!/usr/bin/env python3
"""Headless visual regression checks for the FlameWM X11 session.

Starts Xorg with the dummy video driver (one or two 1280x800 outputs), a private
session bus and `flamewm-session`, then drives the desktop with xdotool and
checks root screenshots for the bugs that used to be found by hand:

  desktop      every output shows the wallpaper (no black output) and a panel
  start-popup  the Start popup maps quickly, is not black, and the desktop
               returns to its pixels after it closes
  frame        an X client gets a frame and a drop shadow below it
  drag         drag feedback (outline ring or moved frame) shows up quickly
               and the vacated area has no black marks
  minimize     the minimize animation ends with no overlay left mapped and the
               desktop restored where the window was
  second-output (dual layout) a window moved to the second output is framed
               and the first output is restored behind it

Every check prints PASS or FAIL; the exit status is 1 when any check fails.
Screenshots are written under --out for CI artifacts.

Needs: Xorg + xf86-video-dummy, xrandr, xdotool, xprop, ImageMagick `import`,
xterm, dbus-daemon and python-xlib.

Usage:
  python3 -I tools/flamewm_visual_tests.py --layout both --out .target/visual
  python3 -I tools/flamewm_visual_tests.py --bin-dir .target/release --layout dual
  python3 -I tools/flamewm_visual_tests.py --bin-dir .target/release bench
  python3 -I tools/flamewm_visual_tests.py --bin-dir .target/release --layout dual idle-memory
"""

import argparse
import contextlib
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from Xlib import X, display as xdisplay

ROOT = Path(__file__).resolve().parent.parent
OUTPUT_SIZE = (1280, 800)
MODE_NAME = "flamewm-1280x800"
MODELINE = ["83.50", "1280", "1352", "1480", "1680", "800", "803", "809", "831", "-hsync", "+vsync"]
PANEL_BAND = 64
DUMMY_CONFIG = """\
Section "Device"
  Identifier "dummy"
  Driver "dummy"
  VideoRam 256000
EndSection
Section "Monitor"
  Identifier "monitor"
  HorizSync 5.0-1000.0
  VertRefresh 5.0-200.0
EndSection
Section "Screen"
  Identifier "screen"
  Device "dummy"
  Monitor "monitor"
  DefaultDepth 24
  SubSection "Display"
    Depth 24
    Virtual 2560 1024
  EndSubSection
EndSection
"""


class Results:
    def __init__(self):
        self.failed = []

    def check(self, name, ok, detail):
        print(f"{'PASS' if ok else 'FAIL'} {name}: {detail}", flush=True)
        if not ok:
            self.failed.append(name)
        return ok


def run(cmd, env, check=True, timeout=30):
    return subprocess.run(cmd, env=env, check=check, timeout=timeout,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)


def wait_until(predicate, timeout, interval=0.02):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(interval)
    return None


class Frame:
    """One root screenshot as packed RGB bytes."""

    def __init__(self, width, height, pixels):
        self.width, self.height, self.pixels = width, height, pixels

    def pixel(self, x, y):
        offset = (y * self.width + x) * 3
        return self.pixels[offset:offset + 3]

    def luma(self, x, y):
        r, g, b = self.pixel(x, y)
        return (299 * r + 587 * g + 114 * b) // 1000

    def region_stats(self, rect, other=None, step=2):
        """(fraction of near-black pixels, mean luma, fraction differing from `other` by > 24)."""
        x0, y0, width, height = rect
        x1, y1 = min(x0 + width, self.width), min(y0 + height, self.height)
        black = total = luma_sum = changed = 0
        for y in range(max(y0, 0), y1, step):
            for x in range(max(x0, 0), x1, step):
                value = self.luma(x, y)
                total += 1
                luma_sum += value
                black += value < 6
                if other is not None:
                    a, b = self.pixel(x, y), other.pixel(x, y)
                    changed += max(abs(a[0] - b[0]), abs(a[1] - b[1]), abs(a[2] - b[2])) > 24
        if total == 0:
            return 0.0, 0.0, 0.0
        return black / total, luma_sum / total, changed / total


class Session:
    def __init__(self, layout, bin_dir, out_dir, display_number, settings=None):
        self.layout = layout
        self.bin_dir = bin_dir
        self.out_dir = out_dir
        self.display_name = f":{display_number}"
        self.processes = []
        self.temp = Path(tempfile.mkdtemp(prefix=f"flamewm-visual-{layout}-"))
        self.outputs = [(0, 0, *OUTPUT_SIZE)]
        if layout == "dual":
            self.outputs.append((OUTPUT_SIZE[0], 0, *OUTPUT_SIZE))
        self.env = dict(os.environ)
        self.settings = settings
        self.x = None

    def spawn(self, name, cmd, env=None):
        log = open(self.temp / f"{name}.log", "wb")
        process = subprocess.Popen(cmd, env=env or self.env, stdout=log, stderr=subprocess.STDOUT,
                                   start_new_session=True)
        self.processes.append((name, process, log))
        return process

    def start(self):
        config = self.temp / "xorg-dummy.conf"
        config.write_text(DUMMY_CONFIG)
        socket = Path(f"/tmp/.X11-unix/X{self.display_name[1:]}")
        self.spawn("xorg", ["Xorg", self.display_name, "-noreset", "-nolisten", "tcp", "-ac",
                            "-config", str(config), "-logfile", str(self.temp / "Xorg.log")])
        if not wait_until(socket.exists, 15, 0.1):
            raise RuntimeError(f"Xorg did not start; see {self.temp / 'Xorg.log'}")
        home = self.temp / "home"
        runtime = self.temp / "runtime"
        home.mkdir()
        runtime.mkdir(mode=0o700)
        if self.settings:
            config = home / ".config" / "flamewm"
            config.mkdir(parents=True)
            (config / "settings.toml").write_text(
                "".join(f"{key}={value}\n" for key, value in self.settings.items()))
        self.env.update(DISPLAY=self.display_name, HOME=str(home), XDG_RUNTIME_DIR=str(runtime),
                        XDG_CONFIG_HOME=str(home / ".config"), XDG_CACHE_HOME=str(home / ".cache"))
        self.env.pop("WAYLAND_DISPLAY", None)
        if self.bin_dir:
            self.env["PATH"] = f"{self.bin_dir}:{self.env.get('PATH', '')}"
        self.configure_outputs()
        bus = run(["dbus-daemon", "--session", "--fork", "--print-address=1", "--print-pid=1"], self.env)
        address, pid = bus.stdout.split()[:2]
        self.processes.append(("dbus", int(pid), None))
        self.env["DBUS_SESSION_BUS_ADDRESS"] = address
        started = time.monotonic()
        self.spawn("session", ["flamewm-session"])
        self.x = xdisplay.Display(self.display_name)
        if not wait_until(self.wm_ready, 20, 0.1):
            raise RuntimeError(f"window manager did not start; see {self.temp / 'session.log'}")
        if not wait_until(lambda: self.windows_named("FlameWM panel", viewable=True), 20, 0.2):
            raise RuntimeError("no panel mapped")
        self.startup_ms = (time.monotonic() - started) * 1000
        wait_until(lambda: len(self.windows_named("FlameWM panel", viewable=True)) >= len(self.outputs), 5, 0.2)
        time.sleep(3)

    def configure_outputs(self):
        run(["xrandr", "--newmode", MODE_NAME, *MODELINE], self.env)
        command = ["xrandr"]
        for index, (x, y, _, _) in enumerate(self.outputs):
            run(["xrandr", "--addmode", f"DUMMY{index}", MODE_NAME], self.env)
            command += ["--output", f"DUMMY{index}", "--mode", MODE_NAME, "--pos", f"{x}x{y}"]
            if index == 0:
                command.append("--primary")
        run(command, self.env)

    def wm_ready(self):
        root = self.x.screen().root
        atom = self.x.intern_atom("_NET_SUPPORTING_WM_CHECK")
        return root.get_full_property(atom, X.AnyPropertyType) is not None

    def windows_named(self, name, viewable=False):
        found = []
        for window in self.x.screen().root.query_tree().children:
            try:
                if window.get_wm_name() != name:
                    continue
                if viewable and window.get_attributes().map_state != X.IsViewable:
                    continue
                found.append(window)
            except Exception:
                continue
        return found

    def viewable_root_children(self):
        result = set()
        for window in self.x.screen().root.query_tree().children:
            with contextlib.suppress(Exception):
                if window.get_attributes().map_state == X.IsViewable:
                    result.add(window.id)
        return result

    def frame_of(self, client_id):
        window = self.x.create_resource_object("window", client_id)
        root = self.x.screen().root.id
        while True:
            parent = window.query_tree().parent
            if parent.id == root:
                return window
            window = parent

    def geometry(self, window):
        geometry = window.get_geometry()
        origin = window.translate_coords(self.x.screen().root, 0, 0)
        return (-origin.x, -origin.y, geometry.width, geometry.height)

    def capture(self, name):
        width = max(x + w for x, _, w, _ in self.outputs)
        height = max(y + h for _, y, _, h in self.outputs)
        raw = subprocess.run(["import", "-window", "root", "-depth", "8", "rgb:-"], env=self.env,
                             check=True, stdout=subprocess.PIPE, timeout=30).stdout
        if self.out_dir:
            image = self.out_dir / f"{self.layout}-{name}.png"
            subprocess.run(["magick" if shutil.which("magick") else "convert", "-size", f"{width}x{height}",
                            "-depth", "8", "rgb:-", str(image)], input=raw, check=True, timeout=30)
        return Frame(width, height, raw)

    def xdotool(self, *args):
        return run(["xdotool", *[str(a) for a in args]], self.env).stdout.strip()

    def open_client(self, title, geometry="80x24+200+150"):
        process = self.spawn(title, ["xterm", "-T", title, "-geometry", geometry, "-bg", "#d0d4d8",
                                     "-fg", "black", "-e", "sleep", "600"])
        client = wait_until(lambda: self.xdotool_search(title), 10, 0.1)
        if not client:
            raise RuntimeError(f"{title} never mapped")
        time.sleep(1.0)
        return process, client

    def xdotool_search(self, title, by="--name"):
        result = run(["xdotool", "search", "--onlyvisible", by, f"^{title}$"], self.env, check=False)
        ids = result.stdout.split()
        return int(ids[0]) if ids else None

    def apply_boolean_setting(self, key, value):
        bus = ["busctl", f"--address={self.env['DBUS_SESSION_BUS_ADDRESS']}", "--json=short", "call",
               "com.arkflame.FlameWM1", "/com/arkflame/FlameWM1", "com.arkflame.FlameWM1.Settings"]
        snapshot = json.loads(run(bus + ["GetSnapshot"], self.env).stdout)["data"][0]
        revision = snapshot["revision"]["data"]
        run(bus + ["Apply", "ta{sv}", str(revision), "1", key, "b", "true" if value else "false"], self.env)

    def stop(self):
        for name, process, log in reversed(self.processes):
            if isinstance(process, int):
                with contextlib.suppress(ProcessLookupError):
                    os.kill(process, signal.SIGTERM)
                continue
            with contextlib.suppress(ProcessLookupError):
                os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                with contextlib.suppress(ProcessLookupError):
                    os.killpg(process.pid, signal.SIGKILL)
                process.wait()
            log.close()
        if self.out_dir:
            for log in self.temp.glob("*.log"):
                shutil.copy(log, self.out_dir / f"{self.layout}-{log.name}")
        shutil.rmtree(self.temp, ignore_errors=True)


def content_band(output):
    x, y, width, height = output
    return (x, y, width, height - PANEL_BAND)


def check_desktop(session, results, baseline):
    for index, output in enumerate(session.outputs):
        black, luma, _ = baseline.region_stats(content_band(output))
        results.check(f"desktop[{index}].wallpaper", black < 0.05 and luma > 20,
                      f"black={black:.3f} mean_luma={luma:.1f}")
    panels = session.windows_named("FlameWM panel", viewable=True)
    panel_outputs = set()
    for panel in panels:
        px, py, _, _ = session.geometry(panel)
        for index, (ox, oy, ow, oh) in enumerate(session.outputs):
            if ox <= px < ox + ow and oy <= py < oy + oh:
                panel_outputs.add(index)
    results.check("desktop.panels", panel_outputs == set(range(len(session.outputs))),
                  f"panels on outputs {sorted(panel_outputs)} of {len(session.outputs)}")


def check_start_popup(session, results, baseline):
    _, _, _, height = session.outputs[0]
    started = time.monotonic()
    session.xdotool("mousemove", 24, height - 24, "click", 1)
    popup = wait_until(lambda: session.windows_named("FlameWM start", viewable=True), 3)
    latency = (time.monotonic() - started) * 1000
    if not results.check("start-popup.maps", bool(popup), f"{latency:.0f} ms"):
        return
    results.check("start-popup.latency", latency < 1000, f"{latency:.0f} ms (limit 1000)")
    time.sleep(0.6)
    rect = session.geometry(popup[0])
    black, _, _ = session.capture("start-open").region_stats(rect)
    results.check("start-popup.not-black", black < 0.2, f"black={black:.3f} rect={rect}")
    session.xdotool("key", "Escape")
    closed = wait_until(lambda: not session.windows_named("FlameWM start", viewable=True), 3)
    if not closed:
        session.xdotool("mousemove", 640, 300, "click", 1)
        closed = wait_until(lambda: not session.windows_named("FlameWM start", viewable=True), 3)
    results.check("start-popup.closes", bool(closed), "Escape or outside click")
    time.sleep(0.8)
    black, _, changed = session.capture("start-closed").region_stats(rect, baseline)
    results.check("start-popup.restores", changed < 0.02 and black < 0.05,
                  f"changed={changed:.3f} black={black:.3f}")


def check_frame_and_shadow(session, results, baseline, client):
    frame = session.frame_of(client)
    rect = session.geometry(frame)
    client_rect = session.geometry(session.x.create_resource_object("window", client))
    results.check("frame.decorated", rect[3] > client_rect[3] + 10,
                  f"frame={rect} client={client_rect}")
    shot = session.capture("frame")
    x, y, width, height = rect
    band = (x + width // 4, y + height + 2, width // 2, 6)
    _, before, _ = baseline.region_stats(band, step=1)
    _, after, _ = shot.region_stats(band, step=1)
    results.check("frame.shadow", after < before - 3, f"luma below frame {before:.1f} -> {after:.1f}")
    return rect


def check_drag(session, results, baseline, client, rect):
    x, y, width, _ = rect
    grab = (x + width // 2, y + 12)
    before_children = session.viewable_root_children()
    frame = session.frame_of(client)
    session.xdotool("mousemove", *grab, "mousedown", 1)
    time.sleep(0.15)
    started = time.monotonic()
    session.xdotool("mousemove", grab[0] + 40, grab[1] + 30)

    def feedback():
        if session.viewable_root_children() - before_children:
            return "outline"
        return "moved" if session.geometry(frame)[:2] != rect[:2] else None

    kind = wait_until(feedback, 2, 0.005)
    latency = (time.monotonic() - started) * 1000
    results.check("drag.feedback", kind is not None and latency < 150,
                  f"{kind or 'none'} after {latency:.0f} ms (limit 150)")
    for step in range(1, 11):
        session.xdotool("mousemove", grab[0] + 40 + step * 30, grab[1] + 30 + step * 15)
        time.sleep(0.03)
    session.xdotool("mouseup", 1)
    time.sleep(1.0)
    moved = session.geometry(frame)
    results.check("drag.moved", moved[:2] != rect[:2], f"{rect[:2]} -> {moved[:2]}")
    vacated = (x, y, min(width, moved[0] - x), rect[3])
    black, _, changed = session.capture("dragged").region_stats(vacated, baseline)
    results.check("drag.no-black-marks", black < 0.01 and changed < 0.03,
                  f"vacated {vacated} black={black:.3f} changed={changed:.3f}")
    return moved


def check_minimize(session, results, baseline, client, rect):
    before_children = session.viewable_root_children()
    session.xdotool("windowminimize", client)
    time.sleep(2.0)
    leftover = session.viewable_root_children() - before_children
    results.check("minimize.no-overlay-left", not leftover, f"new viewable windows {sorted(leftover)}")
    black, _, changed = session.capture("minimized").region_stats(rect, baseline)
    results.check("minimize.restores-desktop", black < 0.01 and changed < 0.03,
                  f"black={black:.3f} changed={changed:.3f}")


def check_second_output(session, results, baseline):
    second = session.outputs[1]
    process, client = session.open_client("flamewm-visual-second", "60x16+1500+150")
    frame = session.frame_of(client)
    rect = session.geometry(frame)
    on_second = rect[0] >= second[0]
    results.check("second-output.placed", on_second, f"frame={rect}")
    black, _, changed = session.capture("second-output").region_stats(content_band(session.outputs[0]), baseline)
    results.check("second-output.first-untouched", black < 0.05 and changed < 0.03,
                  f"black={black:.3f} changed={changed:.3f}")
    session.xdotool("windowclose", client)
    process.terminate()
    time.sleep(1.0)
    black, _, changed = session.capture("second-output-closed").region_stats(rect, baseline)
    results.check("second-output.restores", black < 0.01 and changed < 0.03,
                  f"black={black:.3f} changed={changed:.3f}")


def run_layout(layout, args, results):
    session = Session(layout, args.bin_dir, args.out, args.display)
    try:
        session.start()
        baseline = session.capture("baseline")
        check_desktop(session, results, baseline)
        check_start_popup(session, results, baseline)
        process, client = session.open_client("flamewm-visual-client")
        rect = check_frame_and_shadow(session, results, baseline, client)
        moved = check_drag(session, results, baseline, client, rect)
        check_minimize(session, results, baseline, client, moved)
        process.terminate()
        if layout == "dual":
            check_second_output(session, results, baseline)
    except Exception as error:
        results.check(f"{layout}.harness", False, str(error))
    finally:
        session.stop()


def session_settings(args):
    """Settings written before the session starts; a black solid wallpaper skips every decode."""
    return {"wallpaperMode": "solid", "wallpaperColor": "#000000"} if args.solid_wallpaper else None


def run_bench(args):
    session = Session("single", args.bin_dir, args.out, args.display, session_settings(args))
    try:
        session.start()
        if args.contents_while_dragging:
            session.apply_boolean_setting("showContentWhileDragging", True)
        terminal = session.spawn("flamewm-terminal", ["flamewm-terminal"])
        client = wait_until(lambda: session.xdotool_search("flamewm-terminal", by="--class"), 10, 0.2)
        if not client:
            raise RuntimeError("flamewm-terminal never mapped")
        session.xdotool("windowmove", client, 300, 200)
        time.sleep(1.5)
        frame = session.geometry(session.frame_of(client))
        grab = (frame[0] + frame[2] // 2, frame[1] + 12)
        bench = [sys.executable, "-I", str(ROOT / "tools" / "bench-flamewm-cpu.py"),
                 "--display", session.display_name, "--window", str(args.window), "--repeat", str(args.repeat),
                 "--cases", "idle,window-move,start-toggle", "--allow-input",
                 "--start-xy", f"24,{OUTPUT_SIZE[1] - 24}",
                 "--move-from", f"{grab[0]},{grab[1]}", "--move-to", f"{grab[0] + 400},{grab[1] + 250}"]
        memory = session_pss(session.display_name)
        report = json.loads(run(bench, session.env, timeout=600).stdout)
        terminal.terminate()
        report["startup_ms"] = round(session.startup_ms)
        report["idle_pss_kib"] = memory
        print(json.dumps(report, indent=2))
        print_markdown(report)
    finally:
        session.stop()


def run_idle_memory(args):
    layout = "single" if args.layout == "single" else "dual"
    session = Session(layout, args.bin_dir, args.out, args.display, session_settings(args))
    try:
        session.start()
        if args.suite:
            for app in SUITE_APPS:
                session.spawn(app, [app])
        time.sleep(args.settle)
        if args.out:
            session.capture("idle-suite" if args.suite else "idle-memory")
        cpu = session_cpu(session.display_name, args.cpu_window)
        processes = session_memory(session.display_name, detail=args.detail)
        for key, data in processes.items():
            data["cpu_pct"] = cpu.get(key, 0.0)
        report = {"layout": layout, "suite": args.suite, "settle_s": args.settle,
                  "cpu_window_s": args.cpu_window, "processes": processes}
        print(json.dumps(report, indent=2))
        print_memory_markdown(processes)
        if args.detail:
            print_memory_detail(processes)
    finally:
        session.stop()


MEMORY_PARTS = ("binary", "libs", "heap", "stacks", "fonts", "shm", "other")
# "Idle Suite" opens these on top of the bare session ("Idle Desktop").
SUITE_APPS = ("flamewm-terminal", "flamewm-explorer", "flamewm-task-manager")
DETAIL_ROWS = 12


def session_pids(display_name):
    """`name pid` -> /proc entry of every flamewm* process on `display_name`."""
    found = {}
    for entry in sorted(Path("/proc").iterdir(), key=lambda item: item.name):
        if not entry.name.isdigit():
            continue
        try:
            name = (entry / "comm").read_text().strip()
            environ = (entry / "environ").read_bytes().split(b"\0")
        except OSError:
            continue
        if name.startswith("flamewm") and f"DISPLAY={display_name}".encode() in environ:
            found[f"{name} {entry.name}"] = entry
    return found


def cpu_ticks(entry):
    fields = (entry / "stat").read_text().rsplit(")", 1)[1].split()
    return int(fields[11]) + int(fields[12])


def session_cpu(display_name, window):
    """Percent of one core each process used over `window` seconds of idle."""
    before = {}
    for key, entry in session_pids(display_name).items():
        with contextlib.suppress(OSError):
            before[key] = (entry, cpu_ticks(entry))
    started = time.monotonic()
    time.sleep(window)
    elapsed = time.monotonic() - started
    hertz = os.sysconf("SC_CLK_TCK")
    usage = {}
    for key, (entry, ticks) in before.items():
        with contextlib.suppress(OSError):
            usage[key] = round(100.0 * (cpu_ticks(entry) - ticks) / hertz / elapsed, 2)
    return usage


def mapping_part(path, exe, guarded):
    if path == exe:
        return "binary"
    if path == "[heap]":
        return "heap"
    if path.startswith("[stack"):
        return "stacks"
    if path.startswith("/dev/shm") or path.startswith("/SYSV") or path.startswith("/memfd:"):
        return "shm"
    if "/fonts/" in path or "fontconfig" in path or path.endswith((".ttf", ".otf", ".ttc", ".pcf.gz")):
        return "fonts"
    if ".so" in path:
        return "libs"
    if not path:
        return "stacks" if guarded else "heap"
    return "other"


def process_memory(entry, detail=False):
    """PSS/RSS of one process, split by the kind of mapping that holds it (KiB)."""
    exe = os.readlink(entry / "exe")
    parts = dict.fromkeys(MEMORY_PARTS, 0)
    rss = 0
    path, part, previous_end, previous_guard = "", "other", None, False
    by_mapping = {}
    for line in (entry / "smaps").read_text().splitlines():
        head = line.split(maxsplit=5)
        if "-" in head[0] and len(head) >= 5 and not head[0].endswith(":"):
            start, end = (int(value, 16) for value in head[0].split("-"))
            path = head[5] if len(head) > 5 else ""
            guarded = previous_guard and previous_end == start and head[1] == "rw-p"
            part = mapping_part(path, exe, guarded)
            previous_end, previous_guard = end, head[1] == "---p" and not path
        elif head[0] == "Pss:":
            parts[part] += int(head[1])
            label = path or f"[anon {part}]"
            by_mapping[label] = by_mapping.get(label, 0) + int(head[1])
        elif head[0] == "Rss:":
            rss += int(head[1])
    status = dict(line.split(":", 1) for line in (entry / "status").read_text().splitlines() if ":" in line)
    result = {"pss": sum(parts.values()), "rss": rss, "threads": int(status["Threads"]), "parts": parts}
    if detail:
        top = sorted(by_mapping.items(), key=lambda item: -item[1])[:DETAIL_ROWS]
        result["mappings"] = [{"mapping": name, "pss": pss} for name, pss in top]
    return result


def session_memory(display_name, detail=False):
    """Per-process memory of every flamewm* process on `display_name`, keyed by `name pid`."""
    processes = {}
    for key, entry in session_pids(display_name).items():
        with contextlib.suppress(OSError):
            processes[key] = process_memory(entry, detail)
    return processes


def print_memory_markdown(processes):
    print("\n| Process | PSS | RSS | Threads | CPU % | " + " | ".join(MEMORY_PARTS) + " |")
    print("| --- " * (5 + len(MEMORY_PARTS)) + "|")
    for name, data in processes.items():
        parts = " | ".join(f"{data['parts'][part]}" for part in MEMORY_PARTS)
        cpu = data.get("cpu_pct", 0.0)
        print(f"| {name} | {data['pss']} | {data['rss']} | {data['threads']} | {cpu:.2f} | {parts} |")
    total_pss = sum(data["pss"] for data in processes.values())
    total_rss = sum(data["rss"] for data in processes.values())
    total_cpu = sum(data.get("cpu_pct", 0.0) for data in processes.values())
    print(f"| total | {total_pss} | {total_rss} | | {total_cpu:.2f} | |\n\n"
          f"KiB; total PSS {total_pss / 1024:.2f} MiB; total CPU {total_cpu:.2f}% of one core")


def print_memory_detail(processes):
    for name, data in processes.items():
        print(f"\n{name}: largest mappings (PSS KiB)")
        for row in data.get("mappings", []):
            print(f"  {row['pss']:>7}  {row['mapping']}")


def session_pss(display_name):
    """PSS in KiB of every flamewm* process on `display_name`, by executable name."""
    pss = {}
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            name = (entry / "comm").read_text().strip()
            environ = (entry / "environ").read_bytes().split(b"\0")
            if not name.startswith("flamewm") or f"DISPLAY={display_name}".encode() not in environ:
                continue
            for line in (entry / "smaps_rollup").read_text().splitlines():
                if line.startswith("Pss:"):
                    pss[name] = pss.get(name, 0) + int(line.split()[1])
        except OSError:
            continue
    return dict(sorted(pss.items()))


def print_markdown(report):
    print(f"\nSession start to panels mapped: {report['startup_ms']} ms\n")
    print("| Process | Idle PSS |")
    print("| --- | --- |")
    for name, kib in report["idle_pss_kib"].items():
        print(f"| {name} | {kib / 1024:.1f} MiB |")
    print(f"| total | {sum(report['idle_pss_kib'].values()) / 1024:.1f} MiB |")
    print("\n| Case | Role | CPU % of one core (median, min-max) |")
    print("| --- | --- | --- |")
    for case, data in report["cases"].items():
        if data.get("status") != "ran":
            print(f"| {case} | - | skipped: {data.get('reason')} |")
            continue
        summary = data["summary"]
        rows = list(summary["cpu_pct_one_core_by_role"].items())
        if summary["cpu_pct_one_core_total"]:
            rows.append(("total", summary["cpu_pct_one_core_total"]))
        for role, stats in rows:
            print(f"| {case} | {role} | {stats['median']:.1f} ({stats['min']:.1f}-{stats['max']:.1f}) |")


# Mirrors scripts/package: these names are hard links to the one multi-call flamewm-apps.
MULTI_CALL_APPS = ("flamewm", "flamewm-desktop", "flamewm-shell", "flamewm-settings",
                   "flamewm-explorer", "flamewm-terminal", "flamewm-task-manager")


def stage_packaged(bin_dir):
    """A bin dir laid out like the installed package, built from hard links into `bin_dir`."""
    source = Path(bin_dir)
    multi_call = source / "flamewm-apps"
    if not multi_call.is_file():
        raise SystemExit(f"error: --packaged needs {multi_call}; build flamewm-apps first")
    staged = source / "packaged"
    shutil.rmtree(staged, ignore_errors=True)
    staged.mkdir()
    for entry in source.iterdir():
        if entry.is_file() and os.access(entry, os.X_OK) and entry.name.startswith("flamewm"):
            target = multi_call if entry.name in MULTI_CALL_APPS else entry
            os.link(target, staged / entry.name)
    return str(staged)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("command", nargs="?", default="test", choices=("test", "bench", "idle-memory"))
    parser.add_argument("--layout", default="both", choices=("single", "dual", "both"))
    parser.add_argument("--bin-dir", help="directory of FlameWM binaries put first on PATH")
    parser.add_argument("--out", type=Path, help="directory for screenshots and logs")
    parser.add_argument("--display", type=int, default=91, help="X display number (default 91)")
    parser.add_argument("--window", type=float, default=5.0, help="bench: seconds per sample")
    parser.add_argument("--repeat", type=int, default=3, help="bench: repeats per case")
    parser.add_argument("--contents-while-dragging", action="store_true",
                        help="bench: move windows live instead of dragging an outline")
    parser.add_argument("--solid-wallpaper", action="store_true",
                        help="bench, idle-memory: start with a black solid wallpaper")
    parser.add_argument("--packaged", action="store_true",
                        help="run the binaries laid out like the package (multi-call hard links)")
    parser.add_argument("--suite", action="store_true",
                        help="idle-memory: also open " + ", ".join(SUITE_APPS) + " (Idle Suite)")
    parser.add_argument("--detail", action="store_true",
                        help="idle-memory: list each process's largest mappings")
    parser.add_argument("--cpu-window", type=float, default=10.0,
                        help="idle-memory: seconds of idle CPU sampling")
    parser.add_argument("--settle", type=float, default=30.0, help="idle-memory: seconds idle before sampling")
    args = parser.parse_args(argv)
    if args.bin_dir:
        args.bin_dir = str(Path(args.bin_dir).resolve())
    if args.packaged:
        args.bin_dir = stage_packaged(args.bin_dir or ".target/release")
    if args.out:
        args.out.mkdir(parents=True, exist_ok=True)
    missing = [tool for tool in ("Xorg", "xrandr", "xdotool", "import", "xterm", "dbus-daemon", "busctl")
               if not shutil.which(tool)]
    if missing:
        print(f"error: missing tools: {', '.join(missing)}", file=sys.stderr)
        return 2
    if args.command == "bench":
        run_bench(args)
        return 0
    if args.command == "idle-memory":
        run_idle_memory(args)
        return 0
    results = Results()
    for layout in (("single", "dual") if args.layout == "both" else (args.layout,)):
        run_layout(layout, args, results)
    if results.failed:
        print(f"{len(results.failed)} check(s) failed: {', '.join(results.failed)}")
        return 1
    print("all visual checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env python3
"""Live pixel acceptance for taskbar and Start-menu translucency on a nested Xephyr display.

Samples one root screenshot (`import -window root ppm:-`) and checks:
- taskbar background against solid (opacity 100), backdrop (opacity 0) and blend (opacity N);
- optional Start menu app-list pixels stay at the backdrop when opened at opacity 0.

Only the display is touched: the Start button is clicked with XTest on the given display.
Pure checks (`parse_ppm`, `expected_bar_pixel`, `check_bar`) are unit tested in
tools/test_flamewm_opacity_acceptance.py without X.
"""

from __future__ import annotations

import argparse
import ctypes
import ctypes.util
import json
import os
import re
import subprocess
import sys
import time

DISPLAY_RE = re.compile(r"^:\d{1,3}(?:\.\d+)?$")
TOLERANCE_SOLID = 6
TOLERANCE_MIDPOINT = 8


def parse_ppm(data: bytes) -> tuple[int, int, bytes]:
    """Parse a binary P6 PPM with maxval 255. Raises ValueError on anything else."""
    if not data.startswith(b"P6"):
        raise ValueError("not a P6 PPM")
    fields: list[bytes] = []
    index = 2
    while len(fields) < 3:
        while index < len(data) and data[index:index + 1].isspace():
            index += 1
        if index < len(data) and data[index:index + 1] == b"#":
            index = data.index(b"\n", index) + 1
            continue
        start = index
        while index < len(data) and not data[index:index + 1].isspace():
            index += 1
        if start == index:
            raise ValueError("truncated PPM header")
        fields.append(data[start:index])
    index += 1
    width, height, maxval = (int(field) for field in fields)
    if maxval != 255 or width <= 0 or height <= 0:
        raise ValueError("unsupported PPM header")
    pixels = data[index:]
    if len(pixels) < width * height * 3:
        raise ValueError("PPM pixel data is short")
    return width, height, pixels


def pixel(image: tuple[int, int, bytes], x: int, y: int) -> tuple[int, int, int]:
    width, height, pixels = image
    if not (0 <= x < width and 0 <= y < height):
        raise ValueError(f"sample ({x},{y}) outside {width}x{height}")
    offset = (y * width + x) * 3
    return pixels[offset], pixels[offset + 1], pixels[offset + 2]


def expected_bar_pixel(
    solid: tuple[int, int, int], backdrop: tuple[int, int, int], opacity_percent: int
) -> tuple[int, int, int]:
    """Composite the taskbar fill (alpha = opacity * 255 / 100, as the shell projects it) over the backdrop."""
    alpha = (opacity_percent * 255) // 100
    return tuple((fill * alpha + under * (255 - alpha) + 127) // 255 for fill, under in zip(solid, backdrop))


def check_bar(
    observed: tuple[int, int, int],
    solid: tuple[int, int, int],
    backdrop: tuple[int, int, int],
    opacity_percent: int,
) -> tuple[bool, tuple[int, int, int]]:
    """Whether the observed bar pixel matches the composite; 100 and 0 use the strict solid tolerance."""
    expected = expected_bar_pixel(solid, backdrop, opacity_percent)
    tolerance = TOLERANCE_SOLID if opacity_percent in (0, 100) else TOLERANCE_MIDPOINT
    ok = all(abs(o - e) <= tolerance for o, e in zip(observed, expected))
    return ok, expected


def start_is_solid(observed: tuple[int, int, int], solid: tuple[int, int, int]) -> bool:
    return all(abs(o - s) <= TOLERANCE_SOLID for o, s in zip(observed, solid))


CENTRE_RE = re.compile(r"centre_rgba=Some\(\((\d+), (\d+), (\d+)\)\)")


def snapshot_centre(log_path: str) -> tuple[int, int, int]:
    """Centre pixel of the last taskbar capture the shell logged (FLAMEWM_DEBUG_BACKDROP=1)."""
    matches = CENTRE_RE.findall(open(log_path, encoding="utf-8", errors="replace").read())
    if not matches:
        raise ValueError(f"no backdrop capture centre in {log_path}")
    return tuple(int(v) for v in matches[-1])


def root_image(display: str) -> tuple[int, int, bytes]:
    data = subprocess.run(
        ["import", "-window", "root", "-depth", "8", "ppm:-"],
        env={**os.environ, "DISPLAY": display},
        check=True,
        capture_output=True,
        timeout=60,
    ).stdout
    return parse_ppm(data)


def click(display: str, x: int, y: int) -> None:
    """XTest left click at root coordinates (x, y) on `display`."""
    x11 = ctypes.cdll.LoadLibrary(ctypes.util.find_library("X11"))
    xtst = ctypes.cdll.LoadLibrary(ctypes.util.find_library("Xtst"))
    x11.XOpenDisplay.restype = ctypes.c_void_p
    x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
    handle = x11.XOpenDisplay(display.encode())
    if not handle:
        raise RuntimeError(f"cannot open display {display}")
    try:
        xtst.XTestFakeMotionEvent(ctypes.c_void_p(handle), -1, x, y, 0)
        x11.XFlush(ctypes.c_void_p(handle))
        time.sleep(0.2)
        xtst.XTestFakeButtonEvent(ctypes.c_void_p(handle), 1, 1, 0)
        x11.XFlush(ctypes.c_void_p(handle))
        time.sleep(0.05)
        xtst.XTestFakeButtonEvent(ctypes.c_void_p(handle), 1, 0, 0)
        x11.XFlush(ctypes.c_void_p(handle))
    finally:
        x11.XCloseDisplay(ctypes.c_void_p(handle))


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--display", required=True, help="nested display, e.g. :128")
    parser.add_argument("--taskbar-opacity", type=int, required=True, choices=range(0, 101), metavar="0-100")
    parser.add_argument("--bar-rect", required=True, help="x,y,width,height of the taskbar in device pixels")
    parser.add_argument("--solid", default="25,27,29", help="taskbarColor r,g,b (default #191b1d)")
    parser.add_argument("--start-open-check", action="store_true",
                        help="click Start and sample app-list pixels (use with startMenuOpacity 0)")
    parser.add_argument("--start-button", default="22,620", help="x,y of the Start button")
    parser.add_argument("--start-sample", default="400,300", help="x,y inside the app list")
    parser.add_argument("--start-solid", default="25,27,29", help="startMenuColor r,g,b (default #191b1d)")
    parser.add_argument("--shell-log", help="shell.log with the FLAMEWM_DEBUG_BACKDROP capture trace; "
                        "its snapshot centre is the exact backdrop reference (blur 0)")
    parser.add_argument("--heights-sweep", action="store_true",
                        help="check heights 34,44,52,64,72 via --launch (needs --shell-log path per run)")
    parser.add_argument("--launch", help="shell template with {height} and {log}: relaunches the stack and "
                        "writes its shell.log (FLAMEWM_DEBUG_BACKDROP=1) to {log}")
    parser.add_argument("--log-dir", default=".", help="directory for per-height logs in --heights-sweep")
    parser.add_argument("--out", help="optional JSON result path")
    return parser.parse_args(argv)


def _triple(text: str) -> tuple[int, ...]:
    return tuple(int(part) for part in text.split(","))


TASKBAR_HEIGHTS = (34, 44, 52, 64, 72)


def apply_runtime(bus: str, changes: list[tuple[str, str, str]]) -> int:
    """Applies live settings over the session D-Bus (`Settings.Apply`, a{sv} with expectedRevision).

    `changes` are (key, busctl type, value), e.g. ("taskbarHeight", "x", "34"). Returns the revision
    used. Reads the current revision first so a stale expectation cannot be sent.
    """
    env = {**os.environ, "DBUS_SESSION_BUS_ADDRESS": bus}
    base = ["busctl", "--user", "--address", bus, "call", "com.arkflame.FlameWM1",
            "/com/arkflame/FlameWM1", "com.arkflame.FlameWM1.Settings"]
    snapshot = subprocess.run(base + ["GetSnapshot"], env=env, check=True, capture_output=True,
                              text=True, timeout=30).stdout
    match = re.search(r'"revision" t (\d+)', snapshot)
    if not match:
        raise RuntimeError("GetSnapshot carried no revision")
    revision = int(match.group(1))
    entries = [part for key, kind, value in changes for part in (key, kind, value)]
    subprocess.run(base + ["Apply", "ta{sv}", str(revision), str(len(changes)), *entries], env=env,
                   check=True, capture_output=True, text=True, timeout=30)
    return revision


def height_sweep(
    display: str, heights: tuple[int, ...], opacity: int, launch: str, solid: tuple[int, int, int], log_dir: str
) -> list[dict]:
    """Relaunches the stack per taskbar height via `launch` (a shell template with {height}, {log}),
    then checks the bar against its logged snapshot. Heights that fail are reported, not skipped."""
    rows = []
    for height in heights:
        log = os.path.join(log_dir, f"shell-h{height}-op{opacity}.log")
        subprocess.run(launch.format(height=height, log=log), shell=True, check=True, timeout=120)
        time.sleep(20)
        image = root_image(display)
        bar_y = 641 - height
        observed = pixel(image, 675, bar_y + height // 2)
        backdrop = snapshot_centre(log)
        ok, expected = check_bar(observed, solid, backdrop, opacity)
        rows.append({"height": height, "opacity": opacity, "observed": observed,
                     "snapshot": backdrop, "expected": expected, "ok": ok})
    return rows


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    if not DISPLAY_RE.match(args.display):
        print(f"refusing display {args.display!r}", file=sys.stderr)
        return 2
    if args.heights_sweep:
        if not args.launch:
            print("--heights-sweep needs --launch", file=sys.stderr)
            return 2
        rows = height_sweep(args.display, TASKBAR_HEIGHTS, args.taskbar_opacity, args.launch,
                            _triple(args.solid), args.log_dir)
        print(json.dumps(rows, indent=2))
        return 0 if all(row["ok"] for row in rows) else 1
    bar_x, bar_y, bar_w, bar_h = _triple(args.bar_rect)
    solid = _triple(args.solid)
    before = root_image(args.display)
    # Sample the wallpaper strip just above the bar as the backdrop reference.
    if args.shell_log:
        backdrop = snapshot_centre(args.shell_log)
    else:
        backdrop = pixel(before, bar_x + bar_w // 2, max(0, bar_y - 12))
    observed = pixel(before, bar_x + bar_w // 2, bar_y + bar_h // 2)
    ok, expected = check_bar(observed, solid, backdrop, args.taskbar_opacity)
    result = {
        "display": args.display,
        "taskbar_opacity": args.taskbar_opacity,
        "bar_observed": observed,
        "backdrop_reference": backdrop,
        "bar_expected": expected,
        "bar_ok": ok,
    }
    if args.start_open_check:
        closed = pixel(before, *_triple(args.start_sample))
        click(args.display, *_triple(args.start_button))
        time.sleep(2)
        opened = pixel(root_image(args.display), *_triple(args.start_sample))
        result["start_closed"] = closed
        result["start_open"] = opened
        # Translucent app list: the sample must not be the opaque menu colour (negative control: solid).
        result["start_ok"] = not start_is_solid(opened, _triple(args.start_solid))
    text = json.dumps(result, indent=2)
    print(text)
    if args.out:
        with open(args.out, "w", encoding="utf-8") as handle:
            handle.write(text + "\n")
    passed = ok and result.get("start_ok", True)
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

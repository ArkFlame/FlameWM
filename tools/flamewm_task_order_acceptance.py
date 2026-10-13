#!/usr/bin/env python3
"""Taskbar task-order acceptance.

Pure order model (mirrors platform TaskStrip: append on open, remove on close,
drag reorder keeps the rest in relative order, focus never reorders) plus a
native driver for an already-running FlameWM on a Xephyr display:

- ground truth order: Panels.GetSnapshot over the stack's private session bus
  (tasks[].order, tasks[].windowId);
- input: XTest via libX11/libXtst ctypes (drag = XTestFakeMotionEvent steps);
- task button pixels: xwd of the panel window.

Never run with an inherited host DISPLAY: --display is mandatory and must be :133.
"""
from __future__ import annotations

import argparse
import ctypes
import ctypes.util
import json
import os
import subprocess
import sys
import time
from pathlib import Path

PANEL_SLOT_PITCH = 42
PANEL_FIRST_SLOT_X = 44  # panel-local left edge of slot 1 (after start button)
BUS_NAME = "com.arkflame.FlameWM1"
OBJECT_PATH = "/com/arkflame/FlameWM1"
PANELS_IFACE = "com.arkflame.FlameWM1.Panels"


# ---------- pure order model (mirrors platform TaskStrip) ----------

def append_new(order: list[str], window: str) -> list[str]:
    return order if window in order else [*order, window]


def remove_closed(order: list[str], closed: str) -> list[str]:
    return [item for item in order if item != closed]


def move(order: list[str], source: int, target: int) -> list[str]:
    """Final insertion index `target`, matching TaskStrip::reorder_on_release."""
    if not (0 <= source < len(order) and 0 <= target < len(order)) or source == target:
        return list(order)
    result = list(order)
    item = result.pop(source)
    result.insert(target, item)
    return result


def reconcile(order: list[str], live: list[str]) -> list[str]:
    """Keep relative order, drop closed windows, append new ones in live order."""
    kept = [item for item in order if item in live]
    for window in live:
        kept = append_new(kept, window)
    return kept


def background_is_clear(button, surround, tolerance: int = 0) -> bool:
    """True when every sampled button pixel matches the surrounding taskbar pixel."""
    if len(button) != len(surround) or not button:
        raise ValueError("sample sets must be non-empty and equal length")
    return all(
        max(abs(a - b) for a, b in zip(p, q)) <= tolerance
        for p, q in zip(button, surround)
    )


def slot_center(panel_x: int, panel_y: int, slot: int) -> tuple[int, int]:
    """Root coordinates of task slot `slot` (1-based) centre."""
    return (panel_x + PANEL_FIRST_SLOT_X + PANEL_SLOT_PITCH // 2 + PANEL_SLOT_PITCH * (slot - 1),
            panel_y + 20)


# ---------- native: display / ground truth ----------

def require_display(display: str) -> str:
    if not display.startswith(":") or not display[1:].isdigit():
        raise SystemExit(f"display must look like :133, got {display!r}")
    return display


def env_for(display: str) -> dict[str, str]:
    env = {k: v for k, v in os.environ.items() if k != "DISPLAY"}
    env["DISPLAY"] = display
    return env


def wm_bus_address(display: str) -> str:
    """Session bus of the WM process running on `display` (from its /proc environ)."""
    for proc in Path("/proc").iterdir():
        if not proc.name.isdigit():
            continue
        try:
            environ = (proc / "environ").read_bytes().split(b"\0")
            cmdline = (proc / "cmdline").read_bytes().split(b"\0")[0].decode(errors="replace")
        except OSError:
            continue
        if not cmdline.endswith("/flamewm") or b"DISPLAY=" + display.encode() not in environ:
            continue
        for item in environ:
            if item.startswith(b"DBUS_SESSION_BUS_ADDRESS="):
                return item.split(b"=", 1)[1].decode()
    raise RuntimeError(f"no flamewm WM found on {display}")


def panels_snapshot(address: str) -> dict:
    out = subprocess.run(
        ["busctl", f"--address={address}", "--json=short", "call", BUS_NAME, OBJECT_PATH,
         PANELS_IFACE, "GetSnapshot"],
        check=True, capture_output=True, text=True,
    ).stdout
    return json.loads(out)


def _plain(variant: dict) -> dict:
    """busctl --json=short a{sv} dict -> plain python values."""
    return {key: value["data"] for key, value in variant.items()}


def panel_and_tasks(address: str) -> tuple[dict, list[dict]]:
    raw = panels_snapshot(address)["data"][0]
    panel = _plain(raw["panels"]["data"][0])
    return panel, [_plain(task) for task in raw["tasks"]["data"]]


def task_order(address: str) -> list[dict]:
    """Ordered task entries: [{'order', 'windowId', 'appId', 'kind'}]."""
    _, tasks = panel_and_tasks(address)
    return sorted(tasks, key=lambda entry: entry["order"])


def window_title(display: str, window_id: int) -> str:
    hex_id = hex(window_id)
    out = subprocess.run(["xprop", "-id", hex_id, "WM_NAME"], env=env_for(display),
                         capture_output=True, text=True).stdout
    return out.split('"', 1)[1].rsplit('"', 1)[0] if '"' in out else ""


def titles_in_order(display: str, address: str) -> list[str]:
    return [window_title(display, int(entry["windowId"])) for entry in task_order(address)]


# ---------- native: XTest ----------

class XTest:
    def __init__(self, display: str) -> None:
        self.x11 = ctypes.cdll.LoadLibrary(ctypes.util.find_library("X11"))
        self.xt = ctypes.cdll.LoadLibrary(ctypes.util.find_library("Xtst"))
        self.x11.XOpenDisplay.restype = ctypes.c_void_p
        self.x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
        self.dpy = ctypes.c_void_p(self.x11.XOpenDisplay(display.encode()))
        if not self.dpy.value:
            raise RuntimeError(f"XOpenDisplay({display}) failed")

    def motion(self, x: int, y: int) -> None:
        self.xt.XTestFakeMotionEvent(self.dpy, -1, x, y, 0)
        self.x11.XFlush(self.dpy)

    def button(self, down: bool, button: int = 1) -> None:
        self.xt.XTestFakeButtonEvent(self.dpy, button, int(down), 0)
        self.x11.XFlush(self.dpy)

    def click(self, x: int, y: int) -> None:
        self.motion(x, y)
        time.sleep(0.2)
        self.button(True)
        time.sleep(0.05)
        self.button(False)

    def drag(self, start: tuple[int, int], end: tuple[int, int], steps: int = 12,
             hold_s: float = 0.02) -> None:
        self.motion(*start)
        time.sleep(0.2)
        self.button(True)
        for step in range(1, steps + 1):
            x = start[0] + (end[0] - start[0]) * step // steps
            y = start[1] + (end[1] - start[1]) * step // steps
            self.motion(x, y)
            time.sleep(hold_s)
        time.sleep(0.1)
        self.button(False)

    def key(self, keysym: str, down: bool) -> None:
        self.x11.XStringToKeysym.restype = ctypes.c_ulong
        sym = self.x11.XStringToKeysym(keysym.encode())
        self.x11.XKeysymToKeycode.restype = ctypes.c_ubyte
        code = self.x11.XKeysymToKeycode(self.dpy, sym)
        self.xt.XTestFakeKeyEvent(self.dpy, code, int(down), 0)
        self.x11.XFlush(self.dpy)

    def close(self) -> None:
        self.x11.XCloseDisplay(self.dpy)


# ---------- native: pixels ----------

def panel_pixels(display: str, panel_window: str) -> "PIL.Image.Image":  # noqa: F821
    from PIL import Image

    xwd = subprocess.run(["xwd", "-id", panel_window, "-silent"], env=env_for(display),
                         check=True, capture_output=True).stdout
    png = subprocess.run(["convert", "xwd:-", "png:-"], input=xwd, check=True,
                         capture_output=True).stdout
    from io import BytesIO

    return Image.open(BytesIO(png)).convert("RGB")


def slot_background_clear(image, slot: int) -> bool:
    """Button area (minus icon box and underline rows) equals taskbar pixels nearby."""
    left = PANEL_FIRST_SLOT_X + PANEL_SLOT_PITCH * (slot - 1)
    columns = range(left + 2, left + PANEL_SLOT_PITCH - 2)
    # Rows above the icon and below the icon but above the underline.
    rows = [3, 5, 8, 36]
    button, surround = [], []
    for x in columns:
        for y in rows:
            button.append(image.getpixel((x, y)))
            # Taskbar reference: same row, past the last slot (no task button there).
            surround.append(image.getpixel((image.width - 300, y)))
    return background_is_clear(button, surround, tolerance=6)


# ---------- acceptance flow ----------

ACCENT = (239, 64, 72)


def panel_window_id(display: str) -> str:
    tree = subprocess.run(["xwininfo", "-root", "-tree"], env=env_for(display),
                          capture_output=True, text=True, check=True).stdout
    for line in tree.splitlines():
        if "FlameWM panel" in line:
            return line.split()[0]
    raise RuntimeError("FlameWM panel window not found")


def accent_columns(image) -> list[int]:
    """Panel-local x columns carrying the 2 px accent insertion bar."""
    cols = []
    for x in range(image.width):
        hits = sum(1 for y in range(4, 40)
                   if max(abs(a - b) for a, b in zip(image.getpixel((x, y)), ACCENT)) <= 24)
        if hits >= 20:
            cols.append(x)
    return cols


def snapshot_titles(display: str, address: str) -> list[str]:
    return [window_title(display, int(entry["windowId"])) for entry in task_order(address)]


def workspace_snapshot(address: str) -> dict:
    raw = subprocess.run(
        ["busctl", f"--address={address}", "--json=short", "call", BUS_NAME, OBJECT_PATH,
         "com.arkflame.FlameWM1.Workspaces", "GetSnapshot"],
        check=True, capture_output=True, text=True,
    ).stdout
    return _plain(json.loads(raw)["data"][0])


def press_key(xt: "XTest", keysym: str) -> None:
    xt.key(keysym, True)
    xt.key(keysym, False)


def extra_checks(display: str, address: str, panel_window: str, px: int, py: int,
                 xt: "XTest", order: list[str], out: dict, image_dir: Path) -> None:
    from PIL import Image  # noqa: F401  (ensures pillow available for pixel checks)

    # (1) marker during a held drag: last slot toward slot 1, sampled mid-way, no release.
    src = slot_center(px, py, len(order))
    dst = slot_center(px, py, 1)
    xt.motion(*src)
    time.sleep(0.3)
    xt.button(True)
    for step in range(1, 9):
        xt.motion(src[0] + (dst[0] - src[0]) * step // 8, src[1])
        time.sleep(0.05)
    time.sleep(0.4)
    held = panel_pixels(display, panel_window)
    held.save(image_dir / "marker-held.png")
    out["marker_held_columns"] = accent_columns(held)
    xt.button(False)
    time.sleep(1.0)
    released = panel_pixels(display, panel_window)
    released.save(image_dir / "marker-released.png")
    out["marker_released_columns"] = accent_columns(released)
    out["after_marker_drag"] = snapshot_titles(display, address)

    # (2) release off the task strip cancels a drag: order must not change.
    before = snapshot_titles(display, address)
    src = slot_center(px, py, 2)
    dst = slot_center(px, py, 4)
    xt.motion(*src)
    time.sleep(0.3)
    xt.button(True)
    for step in range(1, 7):
        xt.motion(src[0] + (dst[0] - src[0]) * step // 6, src[1])
        time.sleep(0.05)
    xt.motion(dst[0], py - 200)  # well above the 44 px panel: off the strip
    time.sleep(0.3)
    xt.button(False)
    time.sleep(1.0)
    out["offstrip_release_before"] = before
    out["offstrip_release_after"] = snapshot_titles(display, address)
    out["offstrip_release_unchanged"] = out["offstrip_release_before"] == out["offstrip_release_after"]

    # (3) minimize then restore a middle slot by clicking it twice.
    middle = 2 if len(order) >= 3 else 1
    mid_before = snapshot_titles(display, address)
    xt.click(*slot_center(px, py, middle))
    time.sleep(1.0)
    xt.click(*slot_center(px, py, middle))
    time.sleep(1.5)
    out["minimize_restore_before"] = mid_before
    out["minimize_restore_after"] = snapshot_titles(display, address)
    out["minimize_restore_unchanged"] = mid_before == out["minimize_restore_after"]


    # (4) workspace 2 and back: order must not change.
    ws_before = snapshot_titles(display, address)
    for target in (1, 0):
        ws = workspace_snapshot(address)
        subprocess.run(
            ["busctl", f"--address={address}", "call", BUS_NAME, OBJECT_PATH,
             "com.arkflame.FlameWM1.Workspaces", "Activate", "ut", str(target),
             str(ws["revision"])],
            check=True, capture_output=True, text=True,
        )
        time.sleep(1.5)
    out["workspace_before"] = ws_before
    out["workspace_after"] = snapshot_titles(display, address)
    out["workspace_unchanged"] = ws_before == out["workspace_after"]


def run_native(display: str, screenshot: Path) -> dict:
    address = wm_bus_address(display)
    panel, _ = panel_and_tasks(address)
    px, py = int(panel["x"]), int(panel["y"])
    panel_window = panel_window_id(display)
    xt = XTest(display)
    result: dict = {"display": display}
    try:
        before = snapshot_titles(display, address)
        result["initial_order"] = before
        for slot in range(1, len(before) + 1):
            xt.click(*slot_center(px, py, slot))
            time.sleep(0.8)
        result["after_focus_order"] = snapshot_titles(display, address)
        result["focus_unchanged"] = result["after_focus_order"] == before
        loops = []
        for _ in range(10):
            for slot in range(1, len(before) + 1):
                xt.click(*slot_center(px, py, slot))
                time.sleep(0.6)
            loops.append(snapshot_titles(display, address))
        result["focus_loop_orders"] = loops
        result["focus_loop_stable"] = all(order == before for order in loops)
        last = len(before)
        xt.drag(slot_center(px, py, last), slot_center(px, py, 1))
        time.sleep(1.0)
        result["after_drag_order"] = snapshot_titles(display, address)
        result["drag_as_expected"] = result["after_drag_order"] == move(before, last - 1, 0)
        extra_checks(display, address, panel_window, px, py, xt,
                     result["after_drag_order"], result, screenshot.parent)
    finally:
        xt.close()
    image = panel_pixels(display, panel_window)
    image.save(screenshot)
    result["button_clear"] = [slot_background_clear(image, slot)
                              for slot in range(1, len(before) + 1)]
    return result


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--display", required=True)
    parser.add_argument("--screenshot", type=Path, required=True)
    parser.add_argument("--native", action="store_true",
                        help="drive the running stack (default: validate arguments only)")
    args = parser.parse_args(argv)
    try:
        require_display(args.display)
    except SystemExit as error:
        print(error, file=sys.stderr)
        return 2
    if not args.native:
        print(f"display={args.display} screenshot={args.screenshot}")
        return 0
    print(json.dumps(run_native(args.display, args.screenshot), indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

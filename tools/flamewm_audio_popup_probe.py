#!/usr/bin/env python3
"""Observe whether an open audio popup holds an X pointer grab that hides taskbar hover.

Runs only on a verified nested display (same range as flamewm_xtest.py). Steps:
1. optional: click the audio tray button with XTest and wait for the popup window;
2. (a) try XGrabPointer from a second client on the root and record the reply status
   (GrabAlreadyGrabbed=1 means another client already holds a pointer grab);
3. (b) sweep the pointer across the taskbar strip with XTest and count EnterNotify /
   MotionNotify delivered to the taskbar window(s) overlapping the strip.

`--selftest` only checks the grab-detection mechanism on a bare display (no FlameWM).
Use `--no-popup` to take the baseline without opening the popup.
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from Xlib import X, display as xdisplay  # noqa: E402
from Xlib.ext import xtest  # noqa: E402

from flamewm_xtest import DISPLAY_RE  # noqa: E402

GRAB_STATUS = {0: "Success", 1: "AlreadyGrabbed", 2: "Frozen", 3: "InvalidTime", 4: "NotViewable"}
POPUP_TITLE = "FlameWM audio"


LIVE_DISPLAY = ":201"  # private nested display for the B7 live gate


def display_allowed(name: str) -> bool:
    return DISPLAY_RE.match(name) is not None or name == LIVE_DISPLAY


def parse_point(text: str) -> tuple[int, int]:
    x, y = text.split(",")
    return int(x), int(y)


def parse_rect(text: str) -> tuple[int, int, int, int]:
    x, y, w, h = (int(part) for part in text.split(","))
    if w <= 0 or h <= 0:
        raise ValueError(f"empty rect {text!r}")
    return x, y, w, h


def find_window_by_title(root, title: str):
    stack = [root]
    while stack:
        win = stack.pop()
        try:
            name = win.get_wm_name()
        except Exception:  # noqa: BLE001 - foreign windows may refuse property reads
            name = None
        if name == title:
            return win
        stack.extend(win.query_tree().children)
    return None


def query_grab_status(second) -> str:
    """Try a non-owner-events pointer grab from another client; release it if we got one."""
    root = second.screen().root
    reply = root.grab_pointer(
        False, 0, X.GrabModeAsync, X.GrabModeAsync, X.NONE, X.NONE, X.CurrentTime
    )
    status = GRAB_STATUS.get(reply, f"unknown({reply})")
    if reply == 0:
        second.ungrab_pointer(X.CurrentTime)
        second.flush()
    return status


def drain(d) -> list:
    """Return every queued event without blocking (pending_events() returns a count)."""
    events = []
    while d.pending_events():
        events.append(d.next_event())
    return events


def fake_move(d, x: int, y: int) -> None:
    xtest.fake_input(d, X.MotionNotify, x=x, y=y)
    d.sync()


def fake_click(d, x: int, y: int) -> None:
    fake_move(d, x, y)
    xtest.fake_input(d, X.ButtonPress, 1)
    xtest.fake_input(d, X.ButtonRelease, 1)
    d.sync()


def selftest(name: str) -> dict:
    holder = xdisplay.Display(name)
    prober = xdisplay.Display(name)
    root = holder.screen().root
    try:
        before = query_grab_status(prober)
        reply = root.grab_pointer(
            False, 0, X.GrabModeAsync, X.GrabModeAsync, X.NONE, X.NONE, X.CurrentTime
        )
        holder.flush()
        during = query_grab_status(prober)
        holder.ungrab_pointer(X.CurrentTime)
        holder.flush()
        holder_status = GRAB_STATUS.get(reply)
        after = query_grab_status(prober)
    finally:
        holder.close()
        prober.close()
    return {
        "mode": "selftest",
        "holder_grab": holder_status,
        "before_holder": before,
        "during_holder": during,
        "after_release": after,
        "ok": before == "Success" and holder_status == "Success" and during == "AlreadyGrabbed" and after == "Success",
    }


def probe(name: str, args: argparse.Namespace) -> dict:
    d = xdisplay.Display(name)
    second = xdisplay.Display(name)
    try:
        root = d.screen().root
        result: dict = {"mode": "popup", "display": name, "popup_opened": False}
        if not args.no_popup:
            fake_click(d, *parse_point(args.audio_button))
            time.sleep(args.settle)
            popup = find_window_by_title(root, POPUP_TITLE)
            result["popup_opened"] = popup is not None
            if popup is None:
                result["ok"] = False
                result["reason"] = "popup window not found"
                return result
        result["grab_status_while_open"] = query_grab_status(second)

        sx, sy, sw, sh = parse_rect(args.taskbar_strip)
        taskbar = [
            win for win in root.query_tree().children
            if _overlaps(root, win, sx, sy, sw, sh)
        ]
        for win in taskbar:
            win.change_attributes(event_mask=X.EnterWindowMask | X.PointerMotionMask)
        d.sync()
        fake_move(d, sx - 1, sy + sh // 2)
        time.sleep(0.2)
        drain(d)
        enters = motions = 0
        steps = max(2, sw // args.step)
        for i in range(steps + 1):
            fake_move(d, sx + i * sw // steps, sy + sh // 2)
            time.sleep(0.05)
            for ev in drain(d):
                if ev.type == X.EnterNotify:
                    enters += 1
                elif ev.type == X.MotionNotify:
                    motions += 1
        result.update({
            "taskbar_windows": len(taskbar),
            "taskbar_enter_events": enters,
            "taskbar_motion_events": motions,
            "ok": True,
        })
        return result
    finally:
        d.close()
        second.close()


def _overlaps(root, win, x: int, y: int, w: int, h: int) -> bool:
    """True when the window's root-relative rectangle intersects x,y,w,h."""
    try:
        geo = win.get_geometry()
        origin = win.translate_coords(root, 0, 0)  # window origin in root coordinates
    except Exception:  # noqa: BLE001 - vanished or foreign windows
        return False
    left, top = origin.x, origin.y
    return not (left + geo.width <= x or x + w <= left or top + geo.height <= y or y + h <= top)


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--display", required=True)
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument("--no-popup", action="store_true")
    parser.add_argument("--audio-button", default="0,0", help="x,y of the audio tray button")
    parser.add_argument("--taskbar-strip", default="0,0,1,1", help="x,y,w,h of the taskbar strip")
    parser.add_argument("--step", type=int, default=20)
    parser.add_argument("--settle", type=float, default=2.0)
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    if not display_allowed(args.display):
        print(json.dumps({"ok": False, "reason": f"display {args.display} outside verified range"}))
        return 2
    result = selftest(args.display) if args.selftest else probe(args.display, args)
    print(json.dumps(result, indent=2))
    return 0 if result.get("ok") else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

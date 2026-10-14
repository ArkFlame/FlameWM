#!/usr/bin/env python3
"""Outside-click dismissal probe for the audio popup (V8b item 3).

Starts a nested FlameWM stack (scripts/xephyr) on --display, opens the audio tray
popup with real XTEST clicks and checks, per rep:

  desktop  click on empty desktop        -> popup dismissed
  task     click the target task item    -> popup dismissed and target window active
  xterm    click the target xterm window -> popup dismissed and the xterm receives the
                                            press (X10 mouse report on its pty)
  escape   Escape key                    -> popup dismissed

Slider drag ending outside is NOT exercised here: without a private PipeWire sink the
Devices slider is absent, and a drag on the host stack would change the host volume.

Usage:
  python3 tools/flamewm_outside_click_probe.py --bin-dir DIR --display :N --out DIR [--reps 5]

Exit: 0 PASS, 1 FAIL, 3 environment blocked.
"""
import argparse
import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

from Xlib import X, XK
from Xlib import display as xdisplay
from Xlib.ext import xtest

ROOT = Path(__file__).resolve().parent.parent
PANEL = "FlameWM panel"
POPUPS = {"audio": "FlameWM audio", "network": "FlameWM network", "calendar": "FlameWM calendar"}
# Tray button x offsets from the panel's right edge (network: explicit_tray.rs; calendar: measured
# by a scan on :105 where clicks at right-3, right-9 and right-15 all opened the calendar).
POPUP_BUTTON_DX = {"audio": 120, "network": 90, "calendar": 9}
TARGET = "outside-target"
DECOY = "outside-decoy"
FILE_MARK = b"\x1b[M"


class Dsp:
    def __init__(self, name):
        self.d = xdisplay.Display(name)
        self.root = self.d.screen().root
        self.net_name = self.d.intern_atom("_NET_WM_NAME")
        self.utf8 = self.d.intern_atom("UTF8_STRING")
        self.active = self.d.intern_atom("_NET_ACTIVE_WINDOW")

    def title(self, w):
        try:
            prop = w.get_full_property(self.net_name, self.utf8)
            if prop:
                return bytes(prop.value).decode("utf-8", "replace")
            return w.get_wm_name() or ""
        except Exception:
            return ""

    def find(self, name):
        found, stack = [], [self.root]
        while stack:
            win = stack.pop()
            try:
                children = win.query_tree().children
            except Exception:
                continue
            for child in children:
                if self.title(child) == name:
                    found.append(child)
                stack.append(child)
        return found

    def viewable(self, name):
        for win in self.find(name):
            try:
                if win.get_attributes().map_state == X.IsViewable:
                    return win
            except Exception:
                continue
        return None

    def rect(self, win):
        geo = win.get_geometry()
        pos = self.root.translate_coords(win, 0, 0)
        return pos.x, pos.y, geo.width, geo.height

    def active_title(self):
        prop = self.root.get_full_property(self.active, X.AnyPropertyType)
        if not prop or not prop.value:
            return ""
        try:
            win = self.d.create_resource_object("window", int(prop.value[0]))
            return self.title(win)
        except Exception:
            return ""

    def click(self, x, y, button=1):
        xtest.fake_input(self.d, X.MotionNotify, x=x, y=y)
        self.d.sync()
        xtest.fake_input(self.d, X.ButtonPress, button)
        self.d.sync()
        xtest.fake_input(self.d, X.ButtonRelease, button)
        self.d.sync()

    def escape(self):
        code = self.d.keysym_to_keycode(XK.string_to_keysym("Escape"))
        xtest.fake_input(self.d, X.KeyPress, code)
        xtest.fake_input(self.d, X.KeyRelease, code)
        self.d.sync()


def wait_for(fn, timeout):
    end = time.time() + timeout
    while True:
        value = fn()
        if value:
            return value
        if time.time() >= end:
            return None
        time.sleep(0.05)


def wait_gone(dsp, name, timeout):
    return wait_for(lambda: dsp.viewable(name) is None, timeout) is not None


def kill_group(proc):
    if proc is None or proc.poll() is not None:
        return
    try:
        os.killpg(proc.pid, signal.SIGTERM)
        proc.wait(timeout=8)
    except Exception:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except Exception:
            pass


def main() -> int:
    p = argparse.ArgumentParser()
    p.add_argument("--bin-dir", required=True)
    p.add_argument("--display", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--reps", type=int, default=5)
    p.add_argument("--settle", type=float, default=8.0)
    p.add_argument("--popup", choices=sorted(POPUPS), default="audio")
    args = p.parse_args()
    POPUP_TITLE = POPUPS[args.popup]
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    bin_dir = Path(args.bin_dir)
    env = {
        **os.environ,
        "PATH": f"{bin_dir}:{os.environ['PATH']}",
        "FLAMEWM_DEBUG": "0",
        "FLAMEWM_XEPHYR_DISPLAY": args.display,
        "FLAMEWM_XEPHYR_BUILD": "0",
        "FLAMEWM_XEPHYR_WM_BIN": str(bin_dir / "flamewm"),
        "FLAMEWM_XEPHYR_DESKTOP_BIN": str(bin_dir / "flamewm-desktop"),
        "FLAMEWM_XEPHYR_SHELL_BIN": str(bin_dir / "flamewm-shell"),
        "FLAMEWM_XEPHYR_OUTPUT_DIR": str(out / "stack-out"),
        "FLAMEWM_XEPHYR_EVIDENCE_ROOT": str(out / "evidence"),
        "FLAMEWM_XEPHYR_DEBUG_ROOT": str(out / "debug"),
    }
    stack = subprocess.Popen([str(ROOT / "scripts" / "xephyr")], cwd=ROOT, env=env,
                             stdout=open(out / "xephyr.out", "w"), stderr=subprocess.STDOUT,
                             start_new_session=True)
    clients = []
    result = {"display": args.display, "reps": []}
    try:
        dsp = None
        for _ in range(120):
            try:
                dsp = Dsp(args.display)
                break
            except Exception:
                time.sleep(0.5)
        if dsp is None:
            print(json.dumps({"verdict": "ENVIRONMENT_BLOCKED", "reason": "display not reachable"}))
            return 3
        panel = wait_for(lambda: dsp.viewable(PANEL), 90)
        if panel is None:
            print(json.dumps({"verdict": "ENVIRONMENT_BLOCKED", "reason": "FlameWM panel not viewable"}))
            return 3
        time.sleep(args.settle)
        env_x = {**env, "DISPLAY": args.display}
        mark = out / "target.bin"
        mark.write_bytes(b"")
        target_cmd = ("stty raw -echo; printf '\\033[?1000h'; "
                      f"while :; do dd bs=1 count=6 2>/dev/null; done >> {mark}")
        clients.append(subprocess.Popen(
            ["xterm", "-title", TARGET, "-geometry", "40x12+420+160", "-e", "sh", "-c", target_cmd],
            env=env_x, start_new_session=True))
        clients.append(subprocess.Popen(
            ["xterm", "-title", DECOY, "-geometry", "40x12+760+330", "-e", "sleep", "600"],
            env=env_x, start_new_session=True))
        tgt = wait_for(lambda: dsp.viewable(TARGET), 20)
        dec = wait_for(lambda: dsp.viewable(DECOY), 20)
        if tgt is None or dec is None:
            print(json.dumps({"verdict": "ENVIRONMENT_BLOCKED", "reason": "xterm clients not viewable"}))
            return 3
        time.sleep(2.0)

        px, py, pw, ph = dsp.rect(panel)
        cy = py + ph // 2
        audio_btn = (px + pw - POPUP_BUTTON_DX[args.popup], cy)
        dx, dy, dw, dh = dsp.rect(dec)
        desk_pt = (40, 40)
        decoy_pt = (dx + dw // 2, dy + dh // 2)
        tx, ty, tw, th = dsp.rect(tgt)
        xterm_pt = (tx + tw // 2, ty + th // 2)

        def focus_decoy():
            # Bounded: up to 3 clicks on the decoy; returns whether the decoy is active.
            # Diagnostics: each attempt records the decoy's mapped state, its rect, and the
            # top-level child under the pointer after the click (who received the press).
            for n in range(3):
                dsp.click(*decoy_pt)
                ok = wait_for(lambda: dsp.active_title() == DECOY, 1.0)
                under = dsp.d.screen().root.query_pointer().child
                result.setdefault("focus_attempts", []).append({
                    "attempt": n + 1, "ok": bool(ok), "active": dsp.active_title(),
                    "decoy_viewable": dsp.viewable(DECOY) is not None,
                    "decoy_rect": list(dsp.rect(dec)), "decoy_pt": list(decoy_pt),
                    "under_pointer": dsp.title(under) if under else ""})
                if ok:
                    return True
            return False

        def open_popup():
            # Bounded: up to 3 clicks on the tray button; an already-open popup counts as open.
            for _ in range(3):
                if dsp.viewable(POPUP_TITLE) is not None:
                    return True
                dsp.click(*audio_btn)
                if wait_for(lambda: dsp.viewable(POPUP_TITLE), 2.0) is not None:
                    return True
                time.sleep(0.5)
            return False

        # Locate the target task item by observation, not by focus alone. A click that leaves
        # the decoy unviewable hit the decoy's own task item (minimized it, focus fell to the
        # target): record it and restore the decoy by clicking the same point. Only a click
        # that activates the target while the decoy stays viewable is the target's item.
        focus_decoy()
        task_x = None
        decoy_task_x = None
        for x in range(px + 60, px + pw - 130, 6):
            focus_decoy()
            dsp.click(x, cy)
            hit = wait_for(lambda: dsp.active_title() == TARGET, 0.4) is not None
            time.sleep(0.3)
            if dsp.viewable(DECOY) is None:
                decoy_task_x = x
                dsp.click(x, cy)
                wait_for(lambda: dsp.viewable(DECOY), 2.0)
                continue
            if hit:
                task_x = x
                break
        result["task_x"] = task_x
        result["decoy_task_x"] = decoy_task_x
        result["decoy_viewable_after_sweep"] = dsp.viewable(DECOY) is not None
        result["active_after_sweep"] = dsp.active_title()
        if task_x is None:
            result["verdict"] = "FAIL"
            result["reason"] = "no taskbar point activated the target"
            print(json.dumps(result, indent=1))
            return 1

        ok_all = True
        for rep in range(1, args.reps + 1):
            r = {"rep": rep, "trace": []}

            def trace(label):
                tgt_ok = dsp.viewable(TARGET) is not None
                r["trace"].append([label, dsp.active_title(), tgt_ok, dsp.viewable(POPUP_TITLE) is not None,
                                   dsp.viewable(DECOY) is not None])
            # desktop
            r["desktop_opened"] = open_popup()
            dsp.click(*desk_pt)
            trace("desk_click")
            r["desktop_dismissed"] = wait_gone(dsp, POPUP_TITLE, 2)
            time.sleep(0.5)
            # xterm receives the click and popup dismisses
            time.sleep(0.5)
            size_before = mark.stat().st_size
            r["xterm_opened"] = open_popup()
            r["xterm_target_viewable"] = dsp.viewable(TARGET) is not None
            dsp.click(*xterm_pt)
            trace("xterm_click")
            r["xterm_dismissed"] = wait_gone(dsp, POPUP_TITLE, 2)
            time.sleep(0.8)
            data = mark.read_bytes()[size_before:]
            r["xterm_report_bytes"] = len(data)
            r["xterm_received"] = FILE_MARK in data
            # task item activates target and dismisses popup
            r["decoy_focused_before_task"] = focus_decoy()
            trace("focus_decoy_before_task")
            r["task_opened"] = open_popup()
            dsp.click(task_x, cy)
            trace("task_click")
            r["task_dismissed"] = wait_gone(dsp, POPUP_TITLE, 2)
            r["task_activated"] = wait_for(lambda: dsp.active_title() == TARGET, 1.5) is not None
            # escape
            r["escape_opened"] = open_popup()
            time.sleep(0.5)
            dsp.escape()
            trace("esc")
            r["escape_dismissed"] = wait_gone(dsp, POPUP_TITLE, 2)
            r["ok"] = all([r["decoy_focused_before_task"], r["desktop_opened"], r["desktop_dismissed"], r["xterm_opened"],
                           r["xterm_dismissed"], r["xterm_received"], r["task_opened"],
                           r["task_dismissed"], r["task_activated"], r["escape_opened"],
                           r["escape_dismissed"]])
            ok_all &= r["ok"]
            result["reps"].append(r)
            focus_decoy()
        result["slider_drag_outside"] = "NOT-RUN (no private sink; host volume must not change)"
        result["verdict"] = "PASS" if ok_all else "FAIL"
        print(json.dumps(result, indent=1))
        return 0 if ok_all else 1
    finally:
        for c in clients:
            kill_group(c)
        kill_group(stack)
        subprocess.run(["pkill", "-f", f"Xephyr {args.display} "], capture_output=True)
        (out / "result.json").write_text(json.dumps(result, indent=1))


if __name__ == "__main__":
    sys.exit(main())

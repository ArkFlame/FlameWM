#!/usr/bin/env python3
"""Native acceptance: a click inside a client focuses, raises and still reaches the client.

Runs against an explicitly named nested Xephyr display (never :97). Two overlapping xterms
are created; XTest clicks drive the pointer. Pure decision helpers are unit tested in
test_flamewm_click_focus_acceptance.py.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import signal
import subprocess
import sys
import time
from pathlib import Path

FORBIDDEN_DISPLAYS = {":97"}
DISPLAY_RE = re.compile(r"^:\d+$")
MOUSE_REPORT = "^[[M"  # cat -v rendering of an xterm X10/1000 mouse report (ESC [ M)


def check_display(name: str) -> str:
    if not DISPLAY_RE.match(name) or name in FORBIDDEN_DISPLAYS:
        raise SystemExit(f"refusing display {name!r}: pass an explicit nested display such as :132")
    return name


def stacking_violations(before: list[int], after: list[int], target: int, control: bool) -> list[str]:
    """Return failure messages for a stacking transition after a click on `target`.

    control=True means the click was on the already-active top window: the stacking must be
    unchanged. control=False means the target must end on top of the stack.
    """
    failures: list[str] = []
    if control:
        if before != after:
            failures.append(f"control click restacked windows: {before} -> {after}")
        return failures
    if not after or after[-1] != target:
        failures.append(f"target {target:#x} not top of stacking: {after}")
    if target not in before:
        failures.append(f"target {target:#x} missing from initial stacking {before}")
    return failures


def active_violations(active: int | None, target: int) -> list[str]:
    if active != target:
        return [f"_NET_ACTIVE_WINDOW {active} != target {target:#x}"]
    return []


def delivery_violations(output_text: str, reports_before: int, reports_after: int) -> list[str]:
    """The click must still reach the client: one more mouse report than before the click."""
    seen = output_text.count(MOUSE_REPORT)
    if seen < reports_before + 1 or reports_after < reports_before + 1:
        return [f"click not delivered to client (reports seen={seen}, expected >{reports_before})"]
    return []


def parse_window_list(text: str) -> list[int]:
    """Parse xprop '_NET_CLIENT_LIST_STACKING(WINDOW): window id # 0x1, 0x2' output."""
    _, _, tail = text.partition("#")
    return [int(token, 16) for token in re.findall(r"0x[0-9a-fA-F]+", tail)]


def _xprop_root(display: str, atom: str) -> str:
    return subprocess.run(
        ["xprop", "-root", "-display", display, atom], capture_output=True, text=True, check=False
    ).stdout


def _root_origin(root, win) -> tuple[int, int]:
    """Root-relative origin of `win` (python-xlib's translate_coords is dst-relative)."""
    t = root.translate_coords(win, 0, 0)
    return t.x, t.y


def _find_client(d, root, title: str):
    stack = [root]
    while stack:
        win = stack.pop()
        try:
            name = win.get_wm_name()
        except Exception:
            name = None
        if name == title:
            return win
        try:
            stack.extend(win.query_tree().children)
        except Exception:
            pass
    return None


def _live(display: str, out_dir: Path) -> dict:
    from Xlib import X, display as xdisplay
    from Xlib.ext import xtest

    d = xdisplay.Display(display)
    root = d.screen().root
    out_dir.mkdir(parents=True, exist_ok=True)
    procs = []
    results: dict = {"display": display, "failures": []}
    try:
        for title, geom, log in (
            ("FCA", "50x12+100+100", out_dir / "a.out"),
            ("FCB", "50x12+300+150", out_dir / "b.out"),
        ):
            script = f'stty -icanon -echo min 1; printf "\\033[?1000h"; cat -v > "{log}"; sleep 600'
            procs.append(subprocess.Popen(
                ["xterm", "-display", display, "-T", title, "-geometry", geom, "-e", "sh", "-c", script]))
            time.sleep(1.5)
        time.sleep(2)
        win_a = _find_client(d, root, "FCA")
        win_b = _find_client(d, root, "FCB")
        if win_a is None or win_b is None:
            raise SystemExit("could not find both xterm client windows")
        a_id, b_id = win_a.id, win_b.id
        ax, ay = _root_origin(root, win_a)
        results.update({"a": hex(a_id), "b": hex(b_id)})

        def click(x: int, y: int) -> None:
            xtest.fake_input(d, X.MotionNotify, x=x, y=y)
            d.sync()
            xtest.fake_input(d, X.ButtonPress, detail=1)
            xtest.fake_input(d, X.ButtonRelease, detail=1)
            d.sync()
            time.sleep(0.8)

        def snapshot() -> tuple[list[int], int | None]:
            stack = parse_window_list(_xprop_root(display, "_NET_CLIENT_LIST_STACKING"))
            active_text = _xprop_root(display, "_NET_ACTIVE_WINDOW")
            m = re.search(r"0x[0-9a-fA-F]+", active_text)
            return stack, (int(m.group(0), 16) if m else None)

        def reports() -> int:
            p = out_dir / "a.out"
            return p.read_text(errors="replace").count(MOUSE_REPORT) if p.exists() else 0

        # Point inside A that is not covered by B (B starts at root x=300).
        pa = (ax + 20, ay + 40)
        before, _ = snapshot()
        r0 = reports()
        click(*pa)
        after, active = snapshot()
        r1 = reports()
        results["stack_before_click"] = [hex(w) for w in before]
        results["stack_after_click"] = [hex(w) for w in after]
        results["active_after_click"] = hex(active) if active else None
        results["failures"] += stacking_violations(before, after, a_id, control=False)
        results["failures"] += active_violations(active, a_id)
        results["failures"] += delivery_violations((out_dir / "a.out").read_text(errors="replace"), r0, r1)

        # Negative control: click the already-active top window; stacking must not change.
        before_ctl, _ = snapshot()
        click(*pa)
        after_ctl, _ = snapshot()
        results["failures"] += stacking_violations(before_ctl, after_ctl, a_id, control=True)

        # Title-bar click on B (behind A after the first click) raises B. x=bx+200 lies right
        # of A's client area (A spans x 101..405), so the press is not hidden under A.
        bx, by = _root_origin(root, win_b)
        click(bx + 200, by - 8)
        after_title, active_title = snapshot()
        results["stack_after_titlebar"] = [hex(w) for w in after_title]
        results["failures"] += stacking_violations(after_ctl, after_title, b_id, control=False)
        results["failures"] += active_violations(active_title, b_id)
    finally:
        for p in procs:
            p.send_signal(signal.SIGTERM)
        d.close()
    results["status"] = "PASS" if not results["failures"] else "FAIL"
    return results


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--display", required=True)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args(argv)
    display = check_display(args.display)
    result = _live(display, args.out)
    print(json.dumps(result, indent=2))
    return 0 if result["status"] == "PASS" else 1


if __name__ == "__main__":
    sys.exit(main())

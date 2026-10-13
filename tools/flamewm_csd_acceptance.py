#!/usr/bin/env python3
"""Native CSD acceptance for FlameWM on an explicitly named nested X display.

Requires a display the operator already started (for example a Xephyr the
operator launched). The harness never starts or kills X servers. It starts
its own flamewm under a private HOME, XDG and D-Bus session, and stops only
the processes it started.

Checks, per client role:
  motif  _MOTIF_WM_HINTS decorations=0  -> no Flame frame, frame == client
  gtk    _GTK_FRAME_EXTENTS=(20,20,20,20) -> no Flame frame, frame == client
  plain  no hints                       -> Flame frame (titlebar above client)
Also: _NET_FRAME_EXTENTS (zeros for CSD, titlebar for plain) and a
_NET_WM_MOVERESIZE move driven by XTEST press/motion/release for motif and
plain windows.

Usage:
  python3 tools/flamewm_csd_acceptance.py --display :124 --wm-bin .target/release/flamewm
  python3 tools/flamewm_csd_acceptance.py --client motif --display :124   (internal)
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FORBIDDEN_DISPLAYS = {":97"}
ROLES = ("motif", "gtk", "plain")
MOVERESIZE_MOVE = 8
MOTIF_HINTS_DECORATIONS = 1 << 1
CLIENT_SIZE = (320, 200)
ROLE_ORIGIN = {"motif": (60, 80), "gtk": (460, 80), "plain": (860, 80)}


# ---------------------------------------------------------------- pure checks
def frame_delta(client: tuple[int, int, int, int], frame: tuple[int, int, int, int]) -> dict:
    """client/frame are (x, y, w, h) in root coordinates."""
    cx, cy, cw, ch = client
    fx, fy, fw, fh = frame
    return {"offset_x": cx - fx, "offset_y": cy - fy, "dw": fw - cw, "dh": fh - ch}


def verdict_for_role(role: str, delta: dict, extents: list[int] | None) -> list[str]:
    """Return failure messages; empty list means the role passes."""
    failures: list[str] = []
    if role in ("motif", "gtk"):
        if delta["dh"] != 0 or delta["dw"] != 0 or delta["offset_y"] != 0:
            failures.append(f"{role}: server frame present ({delta})")
        if extents is None or any(extents):
            failures.append(f"{role}: _NET_FRAME_EXTENTS must be zero, got {extents}")
    elif role == "plain":
        if delta["dh"] <= 0 or delta["offset_y"] <= 0:
            failures.append(f"plain: expected server titlebar frame, got {delta}")
        if extents is None or extents[2] <= 0:
            failures.append(f"plain: _NET_FRAME_EXTENTS top must be >0, got {extents}")
    else:
        failures.append(f"unknown role {role}")
    return failures


MOVE_TOLERANCE_PX = 25  # observed: the first 20px XTEST step can be absorbed by move activation


def moveresize_ok(before: tuple[int, int], after: tuple[int, int], expected: tuple[int, int]) -> bool:
    """A move passes only when both axes moved by the injected delta (within one drag step)."""
    if expected == (0, 0):
        return False
    dx, dy = after[0] - before[0], after[1] - before[1]
    return all(abs(got - want) <= MOVE_TOLERANCE_PX for got, want in ((dx, expected[0]), (dy, expected[1])))


# -------------------------------------------------------------- client helper
def run_client(role: str, display: str) -> int:
    from Xlib import X, display as xdisplay, Xatom
    from Xlib.protocol import event as xevent

    d = xdisplay.Display(display)
    screen = d.screen()
    x, y = ROLE_ORIGIN[role]
    win = screen.root.create_window(
        x, y, CLIENT_SIZE[0], CLIENT_SIZE[1], 0, screen.root_depth,
        X.InputOutput, X.CopyFromParent,
        background_pixel=screen.white_pixel,
        event_mask=X.StructureNotifyMask | X.ExposureMask,
    )
    win.set_wm_name(f"CSD-{role}")
    win.set_wm_class("csd-probe", "CSDProbe")
    if role == "motif":
        atom = d.intern_atom("_MOTIF_WM_HINTS")
        win.change_property(atom, atom, 32, [MOTIF_HINTS_DECORATIONS, 0, 0, 0, 0])
    elif role == "gtk":
        atom = d.intern_atom("_GTK_FRAME_EXTENTS")
        win.change_property(atom, Xatom.CARDINAL, 32, [20, 20, 20, 20])
    win.map()
    d.sync()
    # Stay alive until the harness kills us; answer WM_DELETE_WINDOW politely.
    wm_protocols = d.intern_atom("WM_PROTOCOLS")
    wm_delete = d.intern_atom("WM_DELETE_WINDOW")
    win.change_property(wm_protocols, Xatom.ATOM, 32, [wm_delete])
    while True:
        event = d.next_event()
        if event.type == X.ClientMessage and event.client_type == wm_protocols:
            if event.data[1][0] == wm_delete:
                win.destroy()
                d.flush()
                return 0
    return 0


# ------------------------------------------------------------- native checks
def run_native(display: str, wm_bin: Path, evidence: Path, timeout: float) -> dict:
    from Xlib import X, display as xdisplay, Xatom
    from Xlib.ext import xtest
    from Xlib.protocol import event as xevent

    evidence.mkdir(parents=True, exist_ok=True)
    home = Path(tempfile.mkdtemp(prefix="flamewm-csd-home-", dir=evidence))
    env = dict(os.environ)
    env.pop("WAYLAND_DISPLAY", None)
    env.pop("WAYLAND_SOCKET", None)
    env.update(
        DISPLAY=display,
        HOME=str(home),
        XDG_CONFIG_HOME=str(home / "config"),
        XDG_DATA_HOME=str(home / "data"),
        XDG_CACHE_HOME=str(home / "cache"),
        XDG_RUNTIME_DIR=str(home / "run"),
        FLAMEWM_CONFIG_DIR=str(home / "config" / "flamewm"),
        XDG_SESSION_TYPE="x11",
        # Private HOME hides ~/.local site-packages; keep the caller's import path.
        PYTHONPATH=os.pathsep.join(p for p in sys.path if p),
    )
    (home / "run").mkdir(mode=0o700, exist_ok=True)
    started: list[subprocess.Popen] = []
    result: dict = {"display": display, "checks": [], "failures": [], "evidence": str(evidence)}
    try:
        wm_log = (evidence / "wm.log").open("w")
        wm = subprocess.Popen(
            ["dbus-run-session", "--", str(wm_bin)], env=env, stdout=wm_log, stderr=subprocess.STDOUT
        )
        started.append(wm)
        d = xdisplay.Display(display)
        root = d.screen().root
        supporting = d.intern_atom("_NET_SUPPORTING_WM_CHECK")
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline and root.get_full_property(supporting, X.AnyPropertyType) is None:
            time.sleep(0.2)
        if root.get_full_property(supporting, X.AnyPropertyType) is None:
            result["failures"].append("WM did not publish _NET_SUPPORTING_WM_CHECK")
            return result

        clients: dict[str, subprocess.Popen] = {}
        for role in ROLES:
            clients[role] = subprocess.Popen(
                [sys.executable, str(Path(__file__).resolve()), "--client", role, "--display", display],
                env=env,
            )
            started.append(clients[role])
        time.sleep(3.0)

        windows = {}
        for role in ROLES:
            windows[role] = find_window(d, f"CSD-{role}", deadline)
        for role, win in windows.items():
            if win is None:
                result["failures"].append(f"{role}: client window not found")
                continue
            client_geom = absolute_geom(win, root)
            parent = win.query_tree().parent
            frame_geom = absolute_geom(parent, root) if parent.id != root.id else client_geom
            delta = frame_delta(client_geom, frame_geom)
            extents_raw = win.get_full_property(d.intern_atom("_NET_FRAME_EXTENTS"), Xatom.CARDINAL)
            extents = list(extents_raw.value) if extents_raw is not None else None
            fails = verdict_for_role(role, delta, extents)
            result["checks"].append({"role": role, "client": client_geom, "frame": frame_geom,
                                     "delta": delta, "frame_extents": extents, "pass": not fails})
            result["failures"].extend(fails)

        for role in ("motif", "plain"):
            win = windows.get(role)
            if win is None:
                continue
            before = absolute_geom(win, root)[:2]
            send_moveresize(d, root, win, xevent, X, before)
            press_and_drag(d, xtest, X, before, (120, 60))
            time.sleep(0.5)
            after = absolute_geom(win, root)[:2]
            delta = (after[0] - before[0], after[1] - before[1])
            ok = moveresize_ok(before, after, (120, 60))
            result["checks"].append({"role": role, "moveresize_before": before, "moveresize_after": after,
                                     "moveresize_delta": delta, "pass": ok})
            if not ok:
                result["failures"].append(f"{role}: _NET_WM_MOVERESIZE did not move (delta {delta})")

        shot = evidence / "root.png"
        rc = subprocess.run(["import", "-window", "root", str(shot)], env=env).returncode
        result["screenshot"] = {"path": str(shot), "exit": rc}
        if rc != 0:
            result["failures"].append(f"screenshot exit {rc}")
        return result
    finally:
        for proc in started:
            if proc.poll() is None:
                proc.terminate()
        for proc in started:
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()


def find_window(d, name: str, deadline: float):
    from Xlib import X
    net_name = d.intern_atom("_NET_WM_NAME")
    utf8 = d.intern_atom("UTF8_STRING")
    while time.monotonic() < deadline:
        stack = [d.screen().root]
        while stack:
            node = stack.pop()
            for child in node.query_tree().children:
                prop = child.get_full_property(net_name, utf8)
                title = prop.value.decode() if prop is not None and prop.value is not None else None
                if title is None:
                    title = child.get_wm_name()
                if title == name and child.get_attributes().map_state == X.IsViewable:
                    return child
                stack.append(child)
        time.sleep(0.3)
    return None


def absolute_geom(win, root) -> tuple[int, int, int, int]:
    """Root-relative (x, y, w, h) by summing geometry up to a root child."""
    geom = win.get_geometry()
    x, y = geom.x, geom.y
    parent = win.query_tree().parent
    if parent.id != root.id:
        pgeom = parent.get_geometry()
        x += pgeom.x
        y += pgeom.y
    return (x, y, geom.width, geom.height)


def send_moveresize(d, root, win, xevent, X, origin: tuple[int, int]) -> None:
    from Xlib import Xatom
    # X ClientMessage data is unsigned; a negative root origin is reported by the caller.
    data = [max(origin[0], 0) + 40, max(origin[1], 0) + 10, MOVERESIZE_MOVE, 1, 0]
    atom = d.intern_atom("_NET_WM_MOVERESIZE")
    ev = xevent.ClientMessage(window=win, client_type=atom, data=(32, data))
    root.send_event(ev, event_mask=X.SubstructureRedirectMask | X.SubstructureNotifyMask)
    d.flush()
    time.sleep(0.3)


def press_and_drag(d, xtest, X, origin: tuple[int, int], delta: tuple[int, int]) -> None:
    px, py = origin[0] + 40, origin[1] + 10
    xtest.fake_input(d, X.MotionNotify, x=px, y=py)
    d.sync()
    xtest.fake_input(d, X.ButtonPress, 1)
    d.sync()
    steps = 6
    for step in range(1, steps + 1):
        xtest.fake_input(d, X.MotionNotify, x=px + delta[0] * step // steps, y=py + delta[1] * step // steps)
        d.sync()
        time.sleep(0.05)
    xtest.fake_input(d, X.ButtonRelease, 1)
    d.sync()


# ------------------------------------------------------------------- main
def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--display", help="explicit nested display, for example :124")
    parser.add_argument("--wm-bin", type=Path, default=ROOT / ".target" / "release" / "flamewm")
    parser.add_argument("--evidence-dir", type=Path, help="output root (default: temporary directory)")
    parser.add_argument("--timeout", type=float, default=30.0)
    parser.add_argument("--client", choices=ROLES, help=argparse.SUPPRESS)
    args = parser.parse_args(argv)

    if args.client:
        return run_client(args.client, args.display)
    if not args.display:
        print("FAIL: --display is required (for example :124)", file=sys.stderr)
        return 1
    if args.display in FORBIDDEN_DISPLAYS:
        print(f"FAIL: display {args.display} is reserved for the operator's session", file=sys.stderr)
        return 1
    if not args.wm_bin.is_file():
        print(f"ENVIRONMENT_BLOCKED: wm binary missing: {args.wm_bin}", file=sys.stderr)
        return 3
    evidence = args.evidence_dir or Path(tempfile.mkdtemp(prefix="flamewm-csd-evidence-"))
    result = run_native(args.display, args.wm_bin, evidence, args.timeout)
    (evidence / "result.json").write_text(json.dumps(result, indent=2))
    print(json.dumps(result, indent=2))
    return 0 if not result["failures"] else 2


if __name__ == "__main__":
    sys.exit(main())

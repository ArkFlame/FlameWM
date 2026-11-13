#!/usr/bin/env python3
"""Executed proof for the Settings > Displays arrangement canvas on a private Xvfb and private bus.

Starts a private dbus-daemon and the FlameWM session, then runs flamewm-settings with
FLAMEWM_SETTINGS_FIXTURE_OUTPUTS set to a two-output fixture (DP-1 primary at 0,0; HDMI-1 at
1920,0). The fixture answers GetDisplays and records every topology request to a JSON log; the
log is the oracle for what the Settings window sends to the WM. XTest keys and pointer events
drive the canvas. Checks:
  - the canvas renders 2+ node clusters (pixels differ from the same page with one output, in
    at least two separated clusters);
  - Down nudges the selected output (pixels change) and Enter commits exactly one
    set_display_arrangement request whose placements differ from the start layout;
  - Down then Escape restores the pixels and sends nothing;
  - a pointer drag of the HDMI-1 node commits exactly once on release;
  - a pointer drag cancelled by Escape before release restores the pixels and sends nothing.

Usage: displays_canvas_proof.py BIN_DIR OUT_DIR
  BIN_DIR: directory with flamewm, flamewm-session, flamewm-shell, flamewm-desktop, flamewm-settings.
Prints one PASS/FAIL line per check; exit 0 only when every check passed.
"""

import json
import os
import shutil
import subprocess
import sys
import time

from PIL import Image, ImageChops
from Xlib import X, XK
from Xlib import display as xdisplay
from Xlib.ext import xtest

WINDOW_TITLE = "System Settings"
PAGE_X = 190
# Canvas panel in window-local coordinates of the 960x640 Settings window (fixed layout).
CANVAS = (215, 334, 932, 558)
# HDMI-1 node in canvas-local coordinates before any nudge (from the two-output layout).
HDMI_NODE = (313, 14, 509, 125)
FIXTURE_OUTPUTS = [
    {"connector": "DP-1", "x": 0, "y": 0, "width": 1920, "height": 1080, "primary": True},
    {"connector": "HDMI-1", "x": 1920, "y": 0, "width": 1280, "height": 720},
]


def free_display():
    for number in range(200, 250):
        if not os.path.exists(f"/tmp/.X11-unix/X{number}") and not os.path.exists(f"/tmp/.X{number}-lock"):
            return f":{number}"
    raise SystemExit("FAIL no free display in :200-:249")


def clusters(mask_image, cell=16, min_cells=4):
    """Counts separated blobs of changed pixels on a coarse grid (4-connected)."""
    w, h = mask_image.size
    px = mask_image.load()
    grid = {}
    for gy in range(0, h, cell):
        for gx in range(0, w, cell):
            hit = any(px[x, y] for x in range(gx, min(gx + cell, w)) for y in range(gy, min(gy + cell, h)))
            if hit:
                grid[(gx // cell, gy // cell)] = True
    seen, count = set(), 0
    for start in grid:
        if start in seen:
            continue
        stack, size = [start], 0
        seen.add(start)
        while stack:
            cx, cy = stack.pop()
            size += 1
            for nxt in ((cx + 1, cy), (cx - 1, cy), (cx, cy + 1), (cx, cy - 1)):
                if nxt in grid and nxt not in seen:
                    seen.add(nxt)
                    stack.append(nxt)
        if size >= min_cells:
            count += 1
    return count


def canvas(img):
    return img.crop(CANVAS)


def components(mask, cell=8, min_cells=2):
    """Bounding boxes (x0, y0, x1, y1) of 4-connected blobs of set pixels on a coarse grid."""
    w, h = mask.size
    px = mask.load()
    grid = {}
    for gy in range(0, h, cell):
        for gx in range(0, w, cell):
            if any(px[x, y] for x in range(gx, min(gx + cell, w)) for y in range(gy, min(gy + cell, h))):
                grid[(gx // cell, gy // cell)] = True
    seen, boxes = set(), []
    for start in grid:
        if start in seen:
            continue
        stack, cells = [start], []
        seen.add(start)
        while stack:
            cx, cy = stack.pop()
            cells.append((cx, cy))
            for nxt in ((cx + 1, cy), (cx - 1, cy), (cx, cy + 1), (cx, cy - 1)):
                if nxt in grid and nxt not in seen:
                    seen.add(nxt)
                    stack.append(nxt)
        if len(cells) >= min_cells:
            xs = [c[0] for c in cells]
            ys = [c[1] for c in cells]
            boxes.append((min(xs) * cell, min(ys) * cell, (max(xs) + 1) * cell - 1, (max(ys) + 1) * cell - 1))
    return boxes


def run(bin_dir, out):
    os.makedirs(out, exist_ok=True)
    os.chmod(out, 0o700)
    display = free_display()
    home = os.path.join(out, "home")
    config = os.path.join(home, ".config")
    os.makedirs(config, exist_ok=True)
    bindir = os.path.join(out, "bin")
    os.makedirs(bindir, exist_ok=True)
    for name in ("flamewm", "flamewm-session", "flamewm-shell", "flamewm-desktop", "flamewm-settings"):
        shutil.copy(os.path.join(bin_dir, name), bindir)
    log_path = os.path.join(out, "display-requests.json")
    fixture_path = os.path.join(out, "display-fixture.json")
    with open(fixture_path, "w", encoding="utf-8") as handle:
        json.dump({"log": log_path, "outputs": FIXTURE_OUTPUTS}, handle)
    verdicts = []

    def check(label, ok, detail):
        verdicts.append(bool(ok))
        print("PASS" if ok else "FAIL", label, detail, flush=True)

    def records():
        if not os.path.exists(log_path):
            return []
        with open(log_path, encoding="utf-8") as handle:
            return json.load(handle)

    xvfb = subprocess.Popen(["Xvfb", display, "-screen", "0", "1280x800x24", "-nolisten", "tcp", "-noreset"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    session = settings = None
    bus_pid = None
    try:
        for _ in range(50):
            if os.path.exists(f"/tmp/.X11-unix/X{display[1:]}"):
                break
            time.sleep(0.2)
        base_env = dict(os.environ, DISPLAY=display, HOME=home, XDG_CONFIG_HOME=config,
                        XDG_DATA_HOME=os.path.join(home, ".local/share"), XDG_RUNTIME_DIR=out, TMPDIR=out,
                        XDG_CURRENT_DESKTOP="FlameWM", PATH=f"{bindir}:/usr/bin:/bin", LANG="C.UTF-8")
        base_env.pop("WAYLAND_DISPLAY", None)
        base_env.pop("DBUS_SESSION_BUS_ADDRESS", None)
        fork = subprocess.run(["dbus-daemon", "--session", "--fork", "--print-address=1", "--print-pid=1",
                               "--nopidfile"], env=base_env, capture_output=True, text=True, check=True)
        address, bus_pid = fork.stdout.split()[0], int(fork.stdout.split()[1])
        env = dict(base_env, DBUS_SESSION_BUS_ADDRESS=address)
        session = subprocess.Popen([os.path.join(bindir, "flamewm-session")], env=env, start_new_session=True,
                                   stdout=open(os.path.join(out, "session.out"), "w"), stderr=subprocess.STDOUT)
        owned = False
        for _ in range(120):
            probe = subprocess.run(["dbus-send", "--session", "--print-reply", "--dest=org.freedesktop.DBus",
                                    "/org/freedesktop/DBus", "org.freedesktop.DBus.NameHasOwner",
                                    "string:com.arkflame.FlameWM1"], env=env, capture_output=True, text=True)
            if "boolean true" in probe.stdout:
                owned = True
                break
            time.sleep(0.5)
        check("WM owns com.arkflame.FlameWM1 on the private bus", owned, f"bus={address.split(',')[0]}")
        time.sleep(4)

        d = xdisplay.Display(display)
        root = d.screen().root

        def key(name, wait=1.0):
            code = d.keysym_to_keycode(XK.string_to_keysym(name))
            xtest.fake_input(d, X.KeyPress, code)
            d.sync()
            time.sleep(0.05)
            xtest.fake_input(d, X.KeyRelease, code)
            d.sync()
            time.sleep(wait)

        def find_window(title):
            found = []

            def walk(w):
                try:
                    name = w.get_wm_name()
                except Exception:  # noqa: BLE001 - windows can vanish mid-walk
                    name = None
                if name == title:
                    found.append(w)
                for child in w.query_tree().children:
                    walk(child)
            walk(root)
            return found

        def open_settings(page, fixture):
            proc = subprocess.Popen([os.path.join(bindir, "flamewm-settings"), "--page", page, "--window-size", "960x640"],
                                    env=dict(env, FLAMEWM_SETTINGS_FIXTURE_OUTPUTS=fixture) if fixture else env,
                                    start_new_session=True,
                                    stdout=open(os.path.join(out, f"settings-{page}.out"), "w"),
                                    stderr=subprocess.STDOUT)
            win = None
            for _ in range(60):
                found = find_window(WINDOW_TITLE)
                if found:
                    win = found[0]
                    break
                time.sleep(0.5)
            if win is None:
                return proc, None, None
            time.sleep(4)
            win.set_input_focus(X.RevertToParent, X.CurrentTime)
            d.sync()
            time.sleep(1)
            geometry = win.get_geometry()
            origin = win.translate_coords(root, 0, 0)
            box = (-origin.x, -origin.y, -origin.x + geometry.width, -origin.y + geometry.height)
            return proc, box, (-origin.x, -origin.y)

        def shot(name, box):
            path = os.path.join(out, name + ".png")
            subprocess.run(["import", "-window", "root", path], check=True, env=dict(os.environ, DISPLAY=display))
            return Image.open(path).convert("RGB").crop(box)

        def move(x, y):
            xtest.fake_input(d, X.MotionNotify, x=int(x), y=int(y))
            d.sync()
            time.sleep(0.4)

        def button(press):
            xtest.fake_input(d, X.ButtonPress if press else X.ButtonRelease, 1)
            d.sync()
            time.sleep(0.8)

        # Stage 1: one output versus two outputs; the second output adds a node inside the canvas panel.
        one_out = os.path.join(out, "one-output.json")
        with open(one_out, "w", encoding="utf-8") as handle:
            json.dump({"outputs": FIXTURE_OUTPUTS[:1]}, handle)
        proc, box, _ = open_settings("displays", one_out)
        if box is None:
            check("settings window opens with the one-output fixture", False, "no window")
            return all(verdicts)
        one_canvas = canvas(shot("canvas-one-output", box))
        close(proc)
        time.sleep(2)

        fixture = fixture_path
        settings, box, _ = open_settings("displays", fixture)
        check("settings window opens on the Displays page with the two-output fixture", box is not None,
              f"title={WINDOW_TITLE!r}")
        if box is None:
            return all(verdicts)
        two_canvas = canvas(shot("canvas-two-outputs", box))
        # The second output node is drawn inside the canvas panel (the diff must not be empty).
        node_mask = ImageChops.difference(one_canvas, two_canvas).convert("L").point(lambda v: 255 if v > 20 else 0)
        blobs = components(node_mask)
        check("canvas panel renders the second output node", len(blobs) >= 1, f"blobs={len(blobs)}")
        if not blobs:
            return all(verdicts)

        start_records = len(records())
        # Stage 2: keyboard. Down nudges the selected primary output; Enter keeps it through the WM.
        base = canvas(shot("kb-base", box))
        key("Down", wait=1.5)
        nudged = canvas(shot("kb-down", box))
        check("Down nudges the selected output (canvas pixels change)",
              ImageChops.difference(base, nudged).getbbox() is not None, "kb-base vs kb-down")
        key("Return", wait=2.0)
        after_enter = records()
        commits = [r for r in after_enter[start_records:] if r.get("op") == "set_display_arrangement"]
        check("Enter commits exactly one arrangement request", len(commits) == 1,
              f"new_requests={len(after_enter) - start_records}")
        if commits:
            placed = {p["output"]: (p["x"], p["y"]) for p in commits[0]["placements"]}
            check("committed arrangement moves DP-1 from its start layout",
                  placed.get("DP-1") != (0, 0) and placed.get("HDMI-1") is not None,
                  f"placements={placed}")

        # Stage 3: Down then Escape discards the nudge and sends nothing.
        restored_base = canvas(shot("esc-base", box))
        key("Down", wait=1.5)
        key("Escape", wait=1.5)
        esc_after = canvas(shot("esc-after", box))
        sent = len(records()) - len(after_enter)
        check("Escape discards a keyboard nudge without a request", sent == 0 and
              ImageChops.difference(restored_base, esc_after).getbbox() is None,
              f"new_requests={sent}")

        # Stage 4: pointer. A click selects HDMI-1 at its located node; a press, move and Escape while held
        # sends nothing and restores the node. The node is unmoved afterwards, so Stage 5 presses the same point.
        def root_point(local_box):
            cx = (local_box[0] + local_box[2]) // 2 + CANVAS[0]
            cy = (local_box[1] + local_box[3]) // 2 + CANVAS[1]
            return box[0] + cx, box[1] + cy

        hx, hy = root_point(HDMI_NODE)
        move(hx, hy)
        button(True)
        button(False)
        time.sleep(1.0)
        selected = canvas(shot("select-hdmi", box))
        pre_cancel = len(records())
        move(hx, hy)
        button(True)
        for step in range(1, 6):
            move(hx, hy + step * 10)
        key("Escape", wait=1.0)
        button(False)
        time.sleep(1.5)
        cancelled = canvas(shot("cancel-after", box))
        sent = [r for r in records()[pre_cancel:] if r.get("op") == "set_display_arrangement"]
        check("Escape during a pointer drag sends no request", len(sent) == 0, f"requests={len(sent)}")
        check("Escape during a pointer drag restores the node pixels",
              ImageChops.difference(selected, cancelled).getbbox() is None, "select-hdmi vs cancel-after")

        # Stage 5: pointer drag. Press HDMI-1, move 48 px down (past the snap threshold), release: one commit.
        pre_records = len(records())
        move(hx, hy)
        button(True)
        for step in range(1, 7):
            move(hx, hy + step * 8)
        button(False)
        time.sleep(1.5)
        drag_commits = [r for r in records()[pre_records:] if r.get("op") == "set_display_arrangement"]
        check("pointer drag commits exactly once on release", len(drag_commits) == 1,
              f"commits={len(drag_commits)}")
        return all(verdicts)
    finally:
        close(settings)
        if session is not None:
            session.terminate()
            try:
                session.wait(timeout=10)
            except subprocess.TimeoutExpired:
                session.kill()
        if bus_pid:
            try:
                os.kill(bus_pid, 15)
            except ProcessLookupError:
                pass
        xvfb.terminate()
        try:
            xvfb.wait(timeout=10)
        except subprocess.TimeoutExpired:
            xvfb.kill()


def close(proc):
    if proc is not None and proc.poll() is None:
        proc.terminate()
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()


if __name__ == "__main__":
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    ok = run(sys.argv[1], sys.argv[2])
    print("RESULT", "PASS" if ok else "FAIL", flush=True)
    sys.exit(0 if ok else 1)

#!/usr/bin/env python3
"""Executed proof for the AI usage tray card on a private Xvfb.

Runs flamewm-session (WM, shell, desktop) on a private Xvfb and a private dbus-run-session with a
temp HOME. The card reads its providers from FLAMEWM_USAGE_FIXTURE, so no network or credential
file is touched. Checks, from captured root pixels: the tray usage button opens the card; the two
ready providers draw accent fills whose total width matches their percentages; Escape closes the
card; clicking the tray button again toggles it closed.

Usage: usage_card_proof.py BIN_DIR OUT_DIR
  BIN_DIR: directory with the debug flamewm, flamewm-session, flamewm-shell, flamewm-desktop binaries.
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

SCREEN = (1280, 800)
TRAY_USAGE_CELL = (1023, 778)  # usage tray button (usage.open): gear cell left of the weather tray
CARD_MIN_DIFF_PX = 2000  # changed pixels that count as the card opening or closing
PANEL_TOP = 740  # rows at and below this are the taskbar; the card sits above it
TRACK_WIDTH = 268  # card track width in px (300 px card, 16 px side padding)
FILL_TOLERANCE = 0.2


def shot(out, name, display):
    dest = os.path.join(out, name)
    subprocess.run(["import", "-window", "root", dest], check=True, env=dict(os.environ, DISPLAY=display))
    return Image.open(dest).convert("RGB")


def changed_box(a, b):
    box = ImageChops.difference(a.crop((0, 0, SCREEN[0], PANEL_TOP)), b.crop((0, 0, SCREEN[0], PANEL_TOP))).getbbox()
    return box


def changed_count(a, b):
    diff = ImageChops.difference(a.crop((0, 0, SCREEN[0], PANEL_TOP)), b.crop((0, 0, SCREEN[0], PANEL_TOP)))
    return sum(diff.convert("L").histogram()[17:])


def accent_colour(im, box):
    """Most common saturated colour inside the card box: the accent fill."""
    seen = {}
    x0, y0, x1, y1 = box
    for y in range(y0, y1):
        for x in range(x0, x1):
            r, g, b = im.getpixel((x, y))
            if b - r > 25 and g - r > 10:
                seen[(r, g, b)] = seen.get((r, g, b), 0) + 1
    return max(seen, key=seen.get) if seen else None


def fixture(path):
    now = int(time.time())
    body = [
        {"provider": "claude", "state": "ready", "windows": [
            {"used_percent": 40, "window_seconds": 18000, "resets_at": now + 5400}]},
        {"provider": "command_code", "state": "ready", "windows": [
            {"used_percent": 80, "window_seconds": 604800, "resets_at": now + 172800}]},
    ]
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(body, handle)


def free_display():
    for number in range(150, 200):
        if not os.path.exists(f"/tmp/.X11-unix/X{number}") and not os.path.exists(f"/tmp/.X{number}-lock"):
            return f":{number}"
    raise SystemExit("FAIL no free display in :150-:199")


def run(bin_dir, out):
    display = free_display()
    os.makedirs(out, exist_ok=True)
    home = os.path.join(out, "home")
    os.makedirs(os.path.join(home, ".config"), exist_ok=True)
    bindir = os.path.join(out, "bin")
    os.makedirs(bindir, exist_ok=True)
    for name in ("flamewm", "flamewm-session", "flamewm-shell", "flamewm-desktop"):
        shutil.copy(os.path.join(bin_dir, name), bindir)
    fixture_path = os.path.join(out, "usage-fixture.json")
    fixture(fixture_path)
    verdicts = []

    def check(label, ok, detail):
        verdicts.append(bool(ok))
        print("PASS" if ok else "FAIL", label, detail, flush=True)

    xvfb = subprocess.Popen(["Xvfb", display, "-screen", "0", "1280x800x24", "-nolisten", "tcp", "-noreset"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    session = None
    try:
        for _ in range(50):
            if os.path.exists(f"/tmp/.X11-unix/X{display[1:]}"):
                break
            time.sleep(0.2)
        env = dict(os.environ, DISPLAY=display, HOME=home, XDG_CONFIG_HOME=os.path.join(home, ".config"),
                   XDG_DATA_HOME=os.path.join(home, ".local/share"), XDG_RUNTIME_DIR=out, TMPDIR=out,
                   XDG_CURRENT_DESKTOP="FlameWM", PATH=f"{bindir}:/usr/bin:/bin", LANG="C.UTF-8",
                   FLAMEWM_USAGE_FIXTURE=fixture_path)
        env.pop("WAYLAND_DISPLAY", None)
        session = subprocess.Popen(["dbus-run-session", "--", os.path.join(bindir, "flamewm-session")],
                                   env=env, start_new_session=True,
                                   stdout=open(os.path.join(out, "session.out"), "w"), stderr=subprocess.STDOUT)
        d = xdisplay.Display(display)
        root = d.screen().root
        time.sleep(12)

        def key(name):
            code = d.keysym_to_keycode(XK.string_to_keysym(name))
            xtest.fake_input(d, X.KeyPress, code)
            d.sync()
            time.sleep(0.05)
            xtest.fake_input(d, X.KeyRelease, code)
            d.sync()
            time.sleep(1.0)

        def click(x, y):
            root.warp_pointer(x, y)
            d.sync()
            time.sleep(0.2)
            xtest.fake_input(d, X.ButtonPress, 1)
            xtest.fake_input(d, X.ButtonRelease, 1)
            d.sync()
            time.sleep(1.5)

        closed = shot(out, "card-closed.png", display)
        click(*TRAY_USAGE_CELL)
        opened = shot(out, "card-open.png", display)
        delta = changed_count(closed, opened)
        check("tray usage button opens the card", delta >= CARD_MIN_DIFF_PX, f"changed_px={delta}")
        box = changed_box(closed, opened)
        if box:
            colour = accent_colour(opened, box)
            fill_px = sum(1 for y in range(box[1], box[3]) for x in range(box[0], box[2])
                          if colour and opened.getpixel((x, y)) == colour)
            # Fill rows are 6 px tall: expected accent area is (40% + 80%) of the track.
            expected = (0.40 + 0.80) * TRACK_WIDTH * 6
            ok = colour is not None and abs(fill_px - expected) <= expected * FILL_TOLERANCE
            check("provider fills match fixture percentages", ok,
                  f"accent={colour} px={fill_px} expected~{expected:.0f}")
        else:
            check("provider fills match fixture percentages", False, "no card region")

        key("Escape")
        after_escape = shot(out, "card-escape.png", display)
        check("Escape closes the card", changed_count(opened, after_escape) >= CARD_MIN_DIFF_PX
              and changed_count(closed, after_escape) < CARD_MIN_DIFF_PX,
              f"closed-diff_px={changed_count(closed, after_escape)}")

        click(*TRAY_USAGE_CELL)
        reopened = shot(out, "card-reopen.png", display)
        click(*TRAY_USAGE_CELL)
        toggled = shot(out, "card-toggled.png", display)
        check("second click on the tray button closes the card",
              changed_count(reopened, toggled) >= CARD_MIN_DIFF_PX and changed_count(closed, toggled) < CARD_MIN_DIFF_PX,
              f"toggle_px={changed_count(reopened, toggled)}")
    finally:
        if session is not None:
            try:
                os.killpg(session.pid, 15)
            except OSError:
                pass
        xvfb.terminate()
    return all(verdicts) and len(verdicts) == 4


if __name__ == "__main__":
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    sys.exit(0 if run(sys.argv[1], sys.argv[2]) else 1)

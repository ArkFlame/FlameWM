#!/usr/bin/env python3
"""Executed proof for the notification centre on a private Xvfb and a private dbus-daemon.

Runs flamewm-session on a private Xvfb with a private dbus-daemon and a temp HOME. Notifications
are sent through org.freedesktop.Notifications on that private bus only (gdbus with the private
address). Checks, from captured root pixels: the tray bell opens the centre; a critical row draws
its urgency marker and a value=40 row draws its progress fill; Tab focuses the first row; Delete
dismisses it; Enter invokes the default action of the next row; Escape closes the centre; Do Not
Disturb suppresses a normal toast while a critical one still shows.

Usage: notif_center_proof.py BIN_DIR OUT_DIR
  BIN_DIR: directory with the debug flamewm, flamewm-session, flamewm-shell, flamewm-desktop binaries.
Prints one PASS/FAIL line per check; exit 0 only when every check passed.
"""

import os
import re
import shutil
import subprocess
import sys
import time

from PIL import Image, ImageChops
from Xlib import X, XK
from Xlib import display as xdisplay
from Xlib.ext import xtest

SCREEN = (1280, 800)
PANEL_TOP = 740  # rows at and below this are the taskbar
BELL_CELL = (1143, 778)  # tray-notifications button (notification.center.open)
CENTRE_SEARCH = (800, 220, 1280, PANEL_TOP)  # the centre sits above the taskbar, right side
TOAST_REGION = (880, 0, 1280, 200)  # toasts sit at the top right
CENTRE_MIN_DIFF_PX = 2000
TOAST_MIN_DIFF_PX = 200
ROW_MIN_PX = 20
CRITICAL = (218, 68, 83)  # Breeze negative: critical urgency marker
ACCENT = (223, 112, 62)  # --c-df703e: progress fill and focus ring
DND_TOGGLE_OFFSET = (190, 72)  # Do Not Disturb toggle, from the centre's top-left corner
NOTIFY_ARGS = ["gdbus", "call", "--session", "--dest", "org.freedesktop.Notifications",
               "--object-path", "/org/freedesktop/Notifications",
               "--method", "org.freedesktop.Notifications.Notify"]


def shot(out, name, display):
    dest = os.path.join(out, name)
    subprocess.run(["import", "-window", "root", dest], check=True, env=dict(os.environ, DISPLAY=display))
    return Image.open(dest).convert("RGB")


def changed_count(a, b, box):
    diff = ImageChops.difference(a.crop(box), b.crop(box))
    return sum(diff.convert("L").histogram()[17:])


def changed_box(a, b, search):
    return ImageChops.difference(a.crop(search), b.crop(search)).getbbox()


def colour_count(im, box, target, tol=24):
    region = im.crop(box)
    return sum(1 for px in region.getdata()
               if all(abs(px[i] - target[i]) <= tol for i in range(3)))


def notify(env, app, summary, urgency, expire, value=None, actions=()):
    hints = f"'urgency': <byte {urgency}>"
    if value is not None:
        hints += f", 'value': <int32 {value}>"
    action_list = "[" + ", ".join(f"'{name}'" for name in actions) + "]" if actions else "@as []"
    result = subprocess.run(NOTIFY_ARGS + [app, "uint32 0", "", summary, "body", action_list,
                                           "{" + hints + "}", str(expire)],
                            env=env, capture_output=True, text=True, check=False)
    if result.returncode != 0:
        raise SystemExit(f"FAIL notify {summary!r}: {result.stderr.strip()}")
    return int(re.findall(r"\d+", result.stdout)[-1])


def wait_for_service(env):
    """Waits until the shell owns org.freedesktop.Notifications on the private bus."""
    for _ in range(120):
        owned = subprocess.run(["gdbus", "call", "--session", "--dest", "org.freedesktop.DBus",
                                "--object-path", "/org/freedesktop/DBus", "--method",
                                "org.freedesktop.DBus.NameHasOwner", "org.freedesktop.Notifications"],
                               env=env, capture_output=True, text=True)
        if "true" in owned.stdout:
            return
        time.sleep(0.5)
    raise SystemExit("FAIL org.freedesktop.Notifications never owned on the private bus")


def free_display():
    for number in range(150, 200):
        if not os.path.exists(f"/tmp/.X11-unix/X{number}") and not os.path.exists(f"/tmp/.X{number}-lock"):
            return f":{number}"
    raise SystemExit("FAIL no free display in :150-:199")


def start_private_bus():
    result = subprocess.run(["dbus-daemon", "--session", "--fork", "--print-address=1", "--print-pid=1"],
                            capture_output=True, text=True, check=True)
    address, pid = result.stdout.split()[0], int(result.stdout.split()[1])
    return address, pid


def run(bin_dir, out):
    display = free_display()
    os.makedirs(out, exist_ok=True)
    home = os.path.join(out, "home")
    os.makedirs(os.path.join(home, ".config"), exist_ok=True)
    bindir = os.path.join(out, "bin")
    os.makedirs(bindir, exist_ok=True)
    for name in ("flamewm", "flamewm-session", "flamewm-shell", "flamewm-desktop"):
        shutil.copy(os.path.join(bin_dir, name), bindir)
    verdicts = []

    def check(label, ok, detail):
        verdicts.append(bool(ok))
        print("PASS" if ok else "FAIL", label, detail, flush=True)

    xvfb = subprocess.Popen(["Xvfb", display, "-screen", "0", "1280x800x24", "-nolisten", "tcp", "-noreset"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    bus_pid = None
    session = None
    try:
        for _ in range(50):
            if os.path.exists(f"/tmp/.X11-unix/X{display[1:]}"):
                break
            time.sleep(0.2)
        address, bus_pid = start_private_bus()
        env = dict(os.environ, DISPLAY=display, HOME=home, XDG_CONFIG_HOME=os.path.join(home, ".config"),
                   XDG_DATA_HOME=os.path.join(home, ".local/share"), XDG_STATE_HOME=os.path.join(home, ".local/state"),
                   XDG_RUNTIME_DIR=out, TMPDIR=out, DBUS_SESSION_BUS_ADDRESS=address,
                   XDG_CURRENT_DESKTOP="FlameWM", PATH=f"{bindir}:/usr/bin:/bin", LANG="C.UTF-8")
        env.pop("WAYLAND_DISPLAY", None)
        session = subprocess.Popen([os.path.join(bindir, "flamewm-session")], env=env,
                                   start_new_session=True,
                                   stdout=open(os.path.join(out, "session.out"), "w"), stderr=subprocess.STDOUT)
        d = xdisplay.Display(display)
        root = d.screen().root
        wait_for_service(env)

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

        closed = shot(out, "centre-closed.png", display)
        notify(env, "Mail", "Build finished", 1, 60000, value=40, actions=["default", "Open"])
        time.sleep(1.5)
        notify(env, "Chat", "Disk almost full", 2, 60000)
        time.sleep(2)
        click(*BELL_CELL)
        opened = shot(out, "centre-open.png", display)
        box = changed_box(closed, opened, CENTRE_SEARCH)
        if box is None:
            check("bell opens the centre", False, "no centre region changed")
            return False
        box = (box[0] + CENTRE_SEARCH[0], box[1] + CENTRE_SEARCH[1],
               box[2] + CENTRE_SEARCH[0], box[3] + CENTRE_SEARCH[1])
        check("bell opens the centre", changed_count(closed, opened, box) >= CENTRE_MIN_DIFF_PX,
              f"box={box} changed_px={changed_count(closed, opened, box)}")
        critical_px = colour_count(opened, box, CRITICAL)
        check("critical row draws its urgency marker", critical_px >= ROW_MIN_PX, f"critical_px={critical_px}")
        fill_px = colour_count(opened, box, ACCENT)
        check("value hint draws the row progress fill", fill_px >= ROW_MIN_PX, f"accent_px={fill_px}")

        before_tab = shot(out, "centre-before-tab.png", display)
        key("Tab")
        after_tab = shot(out, "centre-after-tab.png", display)
        focus_px = changed_count(before_tab, after_tab, box)
        check("Tab focuses the first row", focus_px >= 30, f"focus_px={focus_px}")

        key("Delete")
        after_delete = shot(out, "centre-after-delete.png", display)
        check("Delete dismisses the focused row", colour_count(after_delete, box, CRITICAL) == 0
              and colour_count(after_tab, box, CRITICAL) >= ROW_MIN_PX,
              f"critical_px_after={colour_count(after_delete, box, CRITICAL)}")

        key("Tab")
        key("Return")
        after_enter = shot(out, "centre-after-enter.png", display)
        check("Enter invokes the default action and removes its row",
              colour_count(after_enter, box, ACCENT) < ROW_MIN_PX
              and colour_count(after_delete, box, ACCENT) >= ROW_MIN_PX,
              f"accent_px_after={colour_count(after_enter, box, ACCENT)}")

        key("Escape")
        after_escape = shot(out, "centre-after-escape.png", display)
        check("Escape closes the centre", changed_count(after_enter, after_escape, box) >= CENTRE_MIN_DIFF_PX,
              f"changed_px={changed_count(after_enter, after_escape, box)}")

        click(*BELL_CELL)
        reopened = shot(out, "centre-reopen.png", display)
        click(box[0] + DND_TOGGLE_OFFSET[0], box[1] + DND_TOGGLE_OFFSET[1])
        shot(out, "centre-dnd-on.png", display)
        key("Escape")

        before_normal = shot(out, "toast-before-normal.png", display)
        notify(env, "Mail", "Suppressed under DND", 1, 60000)
        time.sleep(2)
        after_normal = shot(out, "toast-after-normal.png", display)
        normal_px = changed_count(before_normal, after_normal, TOAST_REGION)
        check("Do Not Disturb suppresses a normal toast", normal_px < TOAST_MIN_DIFF_PX, f"toast_px={normal_px}")

        before_critical = shot(out, "toast-before-critical.png", display)
        notify(env, "Chat", "Critical under DND", 2, 60000)
        time.sleep(2)
        after_critical = shot(out, "toast-after-critical.png", display)
        critical_toast_px = changed_count(before_critical, after_critical, TOAST_REGION)
        check("Do Not Disturb lets a critical toast through", critical_toast_px >= TOAST_MIN_DIFF_PX,
              f"toast_px={critical_toast_px}")
    finally:
        if session is not None:
            try:
                os.killpg(session.pid, 15)
            except OSError:
                pass
        if bus_pid is not None:
            try:
                os.kill(bus_pid, 15)
            except OSError:
                pass
        xvfb.terminate()
    return all(verdicts) and len(verdicts) == 9


if __name__ == "__main__":
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    sys.exit(0 if run(sys.argv[1], sys.argv[2]) else 1)

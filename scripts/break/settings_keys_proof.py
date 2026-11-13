#!/usr/bin/env python3
"""Executed keyboard proof for the Settings app on a private Xvfb and a private session bus.

Starts a private dbus-daemon, the FlameWM session (WM, shell, desktop) on a private Xvfb, waits
until the WM owns com.arkflame.FlameWM1, then runs flamewm-settings on the same bus with HOME and
XDG_CONFIG_HOME in a temp dir. XTest keys drive the page. Checks:
  - Tab walks the sidebar then the page controls, and each stop paints a focus ring (pixel diff);
  - Space toggles the focused switch exactly once, and Return toggles it back exactly once (the
    value in $XDG_CONFIG_HOME/flamewm/settings.toml, written by the WM, is the oracle);
  - the search field takes one typed character per key and Escape clears it.

Usage: settings_keys_proof.py BIN_DIR OUT_DIR
  BIN_DIR: directory with flamewm, flamewm-session, flamewm-shell, flamewm-desktop, flamewm-settings.
Prints one PASS/FAIL line per check; exit 0 only when every check passed.
"""

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
TOGGLE_X = 600  # page controls sit right of the sidebar (window x >= 186)
PAGE_X = 190  # window-relative x where the page starts (the sidebar is 187px wide)
SETTINGS_KEY = "highContrast"


def free_display():
    """First display number in :200-:249 with no socket, so a stale server cannot be reused."""
    for number in range(200, 250):
        if not os.path.exists(f"/tmp/.X11-unix/X{number}") and not os.path.exists(f"/tmp/.X{number}-lock"):
            return f":{number}"
    raise SystemExit("FAIL no free display in :200-:249")


def run(bin_dir, out):
    os.makedirs(out, exist_ok=True)
    display = free_display()
    home = os.path.join(out, "home")
    config = os.path.join(home, ".config")
    os.makedirs(config, exist_ok=True)
    os.chmod(out, 0o700)
    bindir = os.path.join(out, "bin")
    os.makedirs(bindir, exist_ok=True)
    for name in ("flamewm", "flamewm-session", "flamewm-shell", "flamewm-desktop", "flamewm-settings"):
        shutil.copy(os.path.join(bin_dir, name), bindir)
    toml = os.path.join(config, "flamewm", "settings.toml")
    verdicts = []

    def check(label, ok, detail):
        verdicts.append(bool(ok))
        print("PASS" if ok else "FAIL", label, detail, flush=True)

    xvfb = subprocess.Popen(["Xvfb", display, "-screen", "0", "1280x800x24", "-nolisten", "tcp", "-noreset"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    bus_pid = None
    session = settings = None
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
        # Private bus: every process below gets this address, never the host session bus.
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

        def key(name, wait=0.8):
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

        settings = subprocess.Popen([os.path.join(bindir, "flamewm-settings"), "--page", "accessibility"],
                                    env=env, start_new_session=True,
                                    stdout=open(os.path.join(out, "settings.out"), "w"), stderr=subprocess.STDOUT)
        window = None
        for _ in range(60):
            found = find_window(WINDOW_TITLE)
            if found:
                window = found[0]
                break
            time.sleep(0.5)
        check("settings window opens on the private bus", window is not None, f"title={WINDOW_TITLE!r}")
        if window is None:
            return all(verdicts)
        time.sleep(4)
        # Keys go to the focused window: ask the X server for focus on the settings window before typing.
        window.set_input_focus(X.RevertToParent, X.CurrentTime)
        d.sync()
        time.sleep(1)
        geometry = window.get_geometry()
        origin = window.translate_coords(root, 0, 0)  # window origin on the root is the negated result
        left, top = -origin.x, -origin.y
        box = (left, top, left + geometry.width, top + geometry.height)

        def shot(name):
            path = os.path.join(out, name + ".png")
            subprocess.run(["import", "-window", "root", path], check=True, env=dict(os.environ, DISPLAY=display))
            return Image.open(path).convert("RGB").crop(box)

        def changed(a, b):
            return ImageChops.difference(a, b).getbbox()

        def read_toml():
            # The WM omits a key that holds its default (false), so a missing line reads as "0".
            if os.path.exists(toml):
                for line in open(toml, encoding="utf-8"):
                    if line.startswith(SETTINGS_KEY + "="):
                        return line.split("=", 1)[1].strip()
            return "0"

        # Search: Tab lands on the search field first. A typed character filters the sidebar, and Escape
        # clears the field back to the empty-field image. A single character renders the same image each
        # time it is typed into a cleared field, so a doubled delivery would not reproduce it.
        # Wait for the search field's focus ring before typing: keys sent before the focus lands are lost.
        before_focus = shot("search-pre")
        key("Tab", wait=1.5)
        search_base = shot("search-base")
        check("Tab focuses the sidebar search (ring painted)", changed(before_focus, search_base) is not None,
              "search-pre vs search-base")
        # The search button activates on Return (settings.search.focus); only then do keys reach the field.
        key("Return", wait=1.5)
        search_base = shot("search-active")
        key("z", wait=1.5)
        search_once = shot("search-z")
        check("typed character changes the search field", changed(search_base, search_once) is not None,
              "search-base vs search-z")
        key("Escape")
        search_cleared = shot("search-escaped")
        check("Escape restores the empty search field", changed(search_base, search_cleared) is None,
              "search-base vs search-escaped")
        key("z")
        search_again = shot("search-z-again")
        check("one typed character renders the same image after Escape", changed(search_once, search_again) is None,
              "search-z vs search-z-again")

        # Tab order: every press must paint a focus ring; the first page control sits right of the sidebar.
        prev = shot("tab-base")
        first_control = None
        ring_stops = 0
        silent = []
        for index in range(30):
            key("Tab")
            cur = shot(f"tab-{index:02d}")
            box_changed = changed(prev, cur)
            if box_changed:
                ring_stops += 1
            else:
                silent.append(index)
            # A control stop is one whose change lies right of the sidebar. A stop can also repaint the
            # sidebar (scroll reset), so test the page region alone rather than the change's left edge.
            page_changed = changed(prev.crop((PAGE_X, 0, prev.width, prev.height)),
                                   cur.crop((PAGE_X, 0, cur.width, cur.height)))
            if page_changed:
                first_control = index
                break
            prev = cur
        check("Tab paints a focus ring on every stop before the page", ring_stops == (first_control or 0) + 1,
              f"stops_with_ring={ring_stops} stops={(first_control or 0) + 1} silent={silent}")
        check("Tab reaches the first page control after the sidebar", first_control is not None,
              f"first_control_tab={first_control}")

        # Switch: Space flips the focused switch once. Return then flips it back once.
        before = read_toml()
        key("space", wait=2.5)
        after_space = read_toml()
        check("Space toggles the focused switch exactly once",
              after_space != before and after_space in ("1", "true"),
              f"before={before!r} after={after_space!r}")
        key("Return", wait=2.5)
        after_return = read_toml()
        check("Return toggles the same switch back exactly once",
              after_return != after_space and after_return in ("0", "false"),
              f"after_space={after_space!r} after_return={after_return!r}")

        def ctrl_key(name, wait=1.5):
            ctrl = d.keysym_to_keycode(XK.string_to_keysym("Control_L"))
            code = d.keysym_to_keycode(XK.string_to_keysym(name))
            xtest.fake_input(d, X.KeyPress, ctrl)
            d.sync()
            time.sleep(0.3)
            xtest.fake_input(d, X.KeyPress, code)
            d.sync()
            time.sleep(0.3)
            xtest.fake_input(d, X.KeyRelease, code)
            d.sync()
            time.sleep(0.3)
            xtest.fake_input(d, X.KeyRelease, ctrl)
            d.sync()
            time.sleep(wait)

        ctrl_key("q")
        try:
            settings.wait(timeout=10)
            closed = True
        except subprocess.TimeoutExpired:
            closed = False
        check("Ctrl+Q closes the settings window", closed, f"exit={settings.poll()} windows={len(find_window(WINDOW_TITLE))}")

        # Displays slider: reopen on the Displays page, Tab until a Right press moves the pixels, then Left
        # must restore the exact previous image (one arrow step each way).
        displays = subprocess.Popen([os.path.join(bindir, "flamewm-settings"), "--page", "displays"],
                                    env=env, start_new_session=True,
                                    stdout=open(os.path.join(out, "displays.out"), "w"), stderr=subprocess.STDOUT)
        settings = displays
        dwindow = None
        for _ in range(60):
            found = find_window(WINDOW_TITLE)
            if found:
                dwindow = found[0]
                break
            time.sleep(0.5)
        slider_step = None
        if dwindow is not None:
            time.sleep(4)
            dwindow.set_input_focus(X.RevertToParent, X.CurrentTime)
            d.sync()
            time.sleep(1)
            dgeom = dwindow.get_geometry()
            dorigin = dwindow.translate_coords(root, 0, 0)
            dbox = (-dorigin.x, -dorigin.y, -dorigin.x + dgeom.width, -dorigin.y + dgeom.height)

            def dshot(name):
                path = os.path.join(out, name + ".png")
                subprocess.run(["import", "-window", "root", path], check=True, env=dict(os.environ, DISPLAY=display))
                return Image.open(path).convert("RGB").crop(dbox)

            prev = dshot("slider-base")
            for index in range(40):
                key("Tab", wait=0.6)
                # Keyboard activation (Return) on the focused track arms the slider; arrows then step it.
                key("Return", wait=0.6)
                pre = dshot(f"slider-tab-{index:02d}")
                key("Right", wait=1.0)
                after = dshot(f"slider-right-{index:02d}")
                if changed(pre, after) is not None:
                    key("Left", wait=1.0)
                    restored = dshot(f"slider-left-{index:02d}")
                    slider_step = (index, changed(pre, restored) is None, changed(prev, restored) is None)
                    break
                prev = after
        check("Displays slider: Right moves one step and Left restores it exactly",
              slider_step is not None and slider_step[1],
              f"stop={None if slider_step is None else slider_step[0]} restored={None if slider_step is None else slider_step[1]}")
        if displays.poll() is None:
            os.killpg(displays.pid, 15)
            displays.wait(timeout=10)

        d.flush()
        return all(verdicts)
    finally:
        for proc in (settings, session):
            if proc is not None and proc.poll() is None:
                try:
                    os.killpg(proc.pid, 15)
                except ProcessLookupError:
                    pass
                proc.wait(timeout=10)
        if bus_pid:
            try:
                os.kill(bus_pid, 15)
            except ProcessLookupError:
                pass
        xvfb.terminate()
        xvfb.wait(timeout=10)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    sys.exit(0 if run(sys.argv[1], sys.argv[2]) else 1)

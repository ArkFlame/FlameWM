#!/usr/bin/env python3
"""Executed keyboard proof for the media transport card on a private Xvfb and a private dbus-daemon.

Starts a fake MPRIS player on the private bus (org.mpris.MediaPlayer2.fixture) that logs every
transport call it receives, then runs flamewm-session on the same bus with a temp HOME. Checks,
from captured root pixels and the fake player's log: the tray media button opens the card; Tab
moves focus over the transport stops (pixels change); Tab, Tab, Enter dispatches PlayPause exactly
once; Shift+Tab then Space dispatches Previous exactly once; Escape closes the card.

Usage: media_card_proof.py BIN_DIR OUT_DIR
  BIN_DIR: directory with the debug flamewm, flamewm-session, flamewm-shell, flamewm-desktop binaries.
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

SCREEN = (1280, 800)
TRAY_MEDIA_CELL = (983, 778)  # tray media button (media.toggle): cell left of the usage tray button
CARD_MIN_DIFF_PX = 2000  # changed pixels that count as the card opening or closing
PANEL_TOP = 740  # rows at and below this are the taskbar; the card sits above it
FOCUS_MIN_DIFF_PX = 30  # changed pixels that count as a focus ring moving
BUS_NAME = "org.mpris.MediaPlayer2.fixture"
MPRIS_PLAYER = "org.mpris.MediaPlayer2.Player"


def player_main(address, log_path):
    """Fake MPRIS player: serves the Player and Properties interfaces and logs transport calls."""
    import dbus
    import dbus.mainloop.glib
    import dbus.service
    from gi.repository import GLib

    dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)
    bus = dbus.bus.BusConnection(address)
    bus_name = dbus.service.BusName(BUS_NAME, bus=bus)  # keep alive: dropping it releases the name
    props = {
        "Identity": dbus.String("Fixture"),
        "CanQuit": dbus.Boolean(False),
        "CanRaise": dbus.Boolean(False),
        "HasTrackList": dbus.Boolean(False),
        "CanControl": dbus.Boolean(True),
        "CanPlay": dbus.Boolean(True),
        "CanPause": dbus.Boolean(True),
        "CanGoNext": dbus.Boolean(True),
        "CanGoPrevious": dbus.Boolean(True),
        "CanSeek": dbus.Boolean(False),
        "PlaybackStatus": dbus.String("Playing"),
        "Metadata": dbus.Dictionary({"xesam:title": dbus.String("Fixture Song")}, signature="sv"),
        "Volume": dbus.Double(1.0),
        "Position": dbus.Int64(0),
        "Rate": dbus.Double(1.0),
        "MinimumRate": dbus.Double(1.0),
        "MaximumRate": dbus.Double(1.0),
    }
    table = {"org.mpris.MediaPlayer2.Player": props}

    class Player(dbus.service.Object):
        def __init__(self):
            super().__init__(bus, "/org/mpris/MediaPlayer2")

        def log(self, method):
            with open(log_path, "a", encoding="utf-8") as handle:
                handle.write(method + "\n")

        @dbus.service.method(MPRIS_PLAYER)
        def Next(self):
            self.log("Next")

        @dbus.service.method(MPRIS_PLAYER)
        def Previous(self):
            self.log("Previous")

        @dbus.service.method(MPRIS_PLAYER)
        def PlayPause(self):
            self.log("PlayPause")

        @dbus.service.method(MPRIS_PLAYER)
        def Play(self):
            self.log("Play")

        @dbus.service.method(MPRIS_PLAYER)
        def Pause(self):
            self.log("Pause")

        @dbus.service.method("org.freedesktop.DBus.Properties", in_signature="ss", out_signature="v")
        def Get(self, interface, name):
            return table[interface][name] if interface in table else props.get(name)

        @dbus.service.method("org.freedesktop.DBus.Properties", in_signature="s", out_signature="a{sv}")
        def GetAll(self, interface):
            return dbus.Dictionary(table.get(interface, {}), signature="sv")

    Player()
    GLib.MainLoop().run()


def shot(out, name, display):
    dest = os.path.join(out, name)
    subprocess.run(["import", "-window", "root", dest], check=True, env=dict(os.environ, DISPLAY=display))
    return Image.open(dest).convert("RGB")


def changed_count(a, b):
    diff = ImageChops.difference(a.crop((0, 0, SCREEN[0], PANEL_TOP)), b.crop((0, 0, SCREEN[0], PANEL_TOP)))
    return sum(diff.convert("L").histogram()[17:])


def free_display():
    for number in range(150, 200):
        if not os.path.exists(f"/tmp/.X11-unix/X{number}") and not os.path.exists(f"/tmp/.X{number}-lock"):
            return f":{number}"
    raise SystemExit("FAIL no free display in :150-:199")


def start_private_bus():
    result = subprocess.run(["dbus-daemon", "--session", "--fork", "--print-address=1", "--print-pid=1"],
                            capture_output=True, text=True, check=True)
    lines = result.stdout.split()
    return lines[0], int(lines[1])


def read_log(path):
    if not os.path.exists(path):
        return []
    with open(path, encoding="utf-8") as handle:
        return [line.strip() for line in handle if line.strip()]


def run(bin_dir, out):
    display = free_display()
    os.makedirs(out, exist_ok=True)
    home = os.path.join(out, "home")
    os.makedirs(os.path.join(home, ".config"), exist_ok=True)
    bindir = os.path.join(out, "bin")
    os.makedirs(bindir, exist_ok=True)
    for name in ("flamewm", "flamewm-session", "flamewm-shell", "flamewm-desktop"):
        shutil.copy(os.path.join(bin_dir, name), bindir)
    log_path = os.path.join(out, "mpris-calls.log")
    verdicts = []

    def check(label, ok, detail):
        verdicts.append(bool(ok))
        print("PASS" if ok else "FAIL", label, detail, flush=True)

    xvfb = subprocess.Popen(["Xvfb", display, "-screen", "0", "1280x800x24", "-nolisten", "tcp", "-noreset"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    bus_pid = None
    player = None
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
        # The player imports Xlib from the user site-packages, so it keeps the real HOME.
        player = subprocess.Popen([sys.executable, os.path.abspath(__file__), "--player", address, log_path],
                                  env=dict(env, HOME=os.environ.get("HOME", home)), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(2)
        session = subprocess.Popen([os.path.join(bindir, "flamewm-session")], env=env,
                                   start_new_session=True,
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

        closed = shot(out, "media-closed.png", display)
        click(*TRAY_MEDIA_CELL)
        opened = shot(out, "media-open.png", display)
        delta = changed_count(closed, opened)
        check("tray media button opens the card", delta >= CARD_MIN_DIFF_PX, f"changed_px={delta}")

        before_tab = shot(out, "media-before-tab.png", display)
        key("Tab")
        after_tab = shot(out, "media-after-tab.png", display)
        check("Tab moves focus over the transport stops", changed_count(before_tab, after_tab) >= FOCUS_MIN_DIFF_PX,
              f"focus_px={changed_count(before_tab, after_tab)}")

        key("Tab")
        key("Return")
        calls = read_log(log_path)
        check("Tab, Tab, Enter dispatches PlayPause exactly once", calls == ["PlayPause"], f"calls={calls}")

        xtest.fake_input(d, X.KeyPress, d.keysym_to_keycode(XK.string_to_keysym("Shift_L")))
        d.sync()
        key("Tab")
        xtest.fake_input(d, X.KeyRelease, d.keysym_to_keycode(XK.string_to_keysym("Shift_L")))
        d.sync()
        time.sleep(1.0)
        key("space")
        calls = read_log(log_path)
        check("Shift+Tab then Space dispatches Previous exactly once",
              calls == ["PlayPause", "Previous"], f"calls={calls}")

        key("Escape")
        after_escape = shot(out, "media-escape.png", display)
        check("Escape closes the card", changed_count(opened, after_escape) >= CARD_MIN_DIFF_PX
              and changed_count(closed, after_escape) < CARD_MIN_DIFF_PX,
              f"closed-diff_px={changed_count(closed, after_escape)}")
        check("no transport call without focus after Escape", read_log(log_path) == calls,
              f"calls={read_log(log_path)}")
    finally:
        for proc in (session, player, xvfb):
            if proc is not None and proc.poll() is None:
                proc.terminate()
                try:
                    proc.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    proc.kill()
        if bus_pid is not None:
            try:
                os.kill(bus_pid, 15)
            except OSError:
                pass
    return bool(verdicts) and all(verdicts)


if __name__ == "__main__":
    if len(sys.argv) == 4 and sys.argv[1] == "--player":
        player_main(sys.argv[2], sys.argv[3])
        raise SystemExit(0)
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    raise SystemExit(0 if run(sys.argv[1], sys.argv[2]) else 1)

#!/usr/bin/env python3
"""Executed keyboard sweep for the X11 window switcher on a private Xvfb (double-delivery class).

Runs flamewm-session on a private Xvfb and a private dbus-run-session with four fake X windows that
do not overlap. XTest drives Alt+Tab with Alt held: a Tab (or arrow) tap must step the switcher
exactly once, so its commit lands where one step from the start window lands. The landing window is
read from _NET_ACTIVE_WINDOW after Alt is released. Escape must leave focus on the start window.

Usage: shell_sweep_proof.py BIN_DIR OUT_DIR
  BIN_DIR: directory with the fast flamewm, flamewm-session, flamewm-shell, flamewm-desktop binaries.
Prints one PASS/FAIL line per check; exit 0 only when every check passed.
"""

import os
import shutil
import subprocess
import sys
import time

from Xlib import X, XK
from Xlib import display as xdisplay
from Xlib.ext import xtest

FAKE_WINDOWS = 4
WINDOW_W, WINDOW_H = 300, 300
SLOT_X = [50, 350, 650, 950]  # non-overlapping columns on the 1280x800 sandbox
START = "Fake 0"


def free_display():
    """First display number in :200-:249 with no socket, so a stale server cannot be reused."""
    for number in range(200, 250):
        if not os.path.exists(f"/tmp/.X11-unix/X{number}") and not os.path.exists(f"/tmp/.X{number}-lock"):
            return f":{number}"
    raise SystemExit("FAIL no free display in :200-:249")


def run(bin_dir, out):
    display_name = free_display()
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

    xvfb = subprocess.Popen(["Xvfb", display_name, "-screen", "0", "1280x800x24", "-nolisten", "tcp", "-noreset"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    session = None
    try:
        for _ in range(50):
            if os.path.exists(f"/tmp/.X11-unix/X{display_name[1:]}"):
                break
            time.sleep(0.2)
        env = dict(os.environ, DISPLAY=display_name, HOME=home, XDG_CONFIG_HOME=os.path.join(home, ".config"),
                   XDG_DATA_HOME=os.path.join(home, ".local/share"), XDG_RUNTIME_DIR=out, TMPDIR=out,
                   XDG_CURRENT_DESKTOP="FlameWM", PATH=f"{bindir}:/usr/bin:/bin", LANG="C.UTF-8")
        env.pop("WAYLAND_DISPLAY", None)
        session = subprocess.Popen(["dbus-run-session", "--", os.path.join(bindir, "flamewm-session")],
                                   env=env, start_new_session=True,
                                   stdout=open(os.path.join(out, "session.out"), "w"), stderr=subprocess.STDOUT)
        d = xdisplay.Display(display_name)
        root = d.screen().root
        atom = lambda n: d.intern_atom(n)  # noqa: E731
        time.sleep(8)
        for i in range(FAKE_WINDOWS):
            w = root.create_window(SLOT_X[i], 100, WINDOW_W, WINDOW_H, 0, d.screen().root_depth,
                                   X.InputOutput, background_pixel=0x3050A0 + i * 0x101010)
            w.set_wm_name(f"Fake {i}")
            w.map()
        d.flush()
        time.sleep(5)

        def client_names():
            prop = root.get_full_property(atom("_NET_CLIENT_LIST"), X.AnyPropertyType)
            return [d.create_resource_object("window", wid).get_wm_name() for wid in (prop.value if prop else [])]

        for _ in range(60):
            if sum(1 for n in client_names() if n and n.startswith("Fake ")) >= FAKE_WINDOWS:
                break
            time.sleep(0.5)
        check("four fake windows managed", sum(1 for n in client_names() if n and n.startswith("Fake ")) == FAKE_WINDOWS,
              f"clients={len(client_names())}")

        def tap(name):
            code = d.keysym_to_keycode(XK.string_to_keysym(name))
            xtest.fake_input(d, X.KeyPress, code)
            d.sync()
            time.sleep(0.15)
            xtest.fake_input(d, X.KeyRelease, code)
            d.sync()
            time.sleep(0.4)

        def key_down(name):
            xtest.fake_input(d, X.KeyPress, d.keysym_to_keycode(XK.string_to_keysym(name)))
            d.sync()
            time.sleep(0.2)

        def key_up(name):
            xtest.fake_input(d, X.KeyRelease, d.keysym_to_keycode(XK.string_to_keysym(name)))
            d.sync()
            time.sleep(0.6)

        def active_name():
            prop = root.get_full_property(atom("_NET_ACTIVE_WINDOW"), X.AnyPropertyType)
            wid = prop.value[0] if prop and prop.value else 0
            if not wid:
                return None
            return d.create_resource_object("window", wid).get_wm_name()

        def click(i):
            root.warp_pointer(SLOT_X[i] + 150, 250)
            d.sync()
            time.sleep(0.2)
            xtest.fake_input(d, X.ButtonPress, 1)
            xtest.fake_input(d, X.ButtonRelease, 1)
            d.sync()
            time.sleep(1.0)

        def reset():
            """Focuses Fake 0 with a fixed MRU (3, 2, 1, 0) so every case starts from the same order."""
            for i in (1, 2, 3, 0):
                click(i)
            return active_name() == START

        def alt_session(steps, finish="release"):
            """Holds Alt, taps each step ("Tab", "Shift+Tab", "Right", ...) once, then finishes."""
            key_down("Alt_L")
            for step in steps:
                shift = step.startswith("Shift+")
                if shift:
                    key_down("Shift_L")
                tap(step[len("Shift+"):] if shift else step)
                if shift:
                    key_up("Shift_L")
            if finish != "release":
                tap(finish)
            key_up("Alt_L")
            return active_name()

        check("reset focuses the start window", reset(), f"active={active_name()!r}")
        once = alt_session(["Tab"])
        check("Alt+Tab once lands on another window", once not in (None, START), f"active={once!r}")
        check("Alt+Tab once is repeatable from the same MRU", reset() and alt_session(["Tab"]) == once,
              "second run matched")
        reset()
        twice = alt_session(["Tab", "Tab"])
        check("Alt+Tab Tab lands two steps away, not one", twice not in (None, START, once), f"active={twice!r}")
        reset()
        check("Alt+Tab Right equals Alt+Tab Tab (arrow is one step)", alt_session(["Tab", "Right"]) == twice,
              f"expected={twice!r}")
        reset()
        check("Alt+Tab Left steps back to the start window once", alt_session(["Tab", "Left"]) == START,
              "left after one Tab returns to start")
        reset()
        check("Alt+Tab Tab Shift+Tab steps back one (equals one Tab)", alt_session(["Tab", "Tab", "Shift+Tab"]) == once,
              f"expected={once!r}")
        reset()
        check("Escape cancels: focus stays on the start window", alt_session(["Tab", "Tab"], finish="Escape") == START,
              "Escape after two Tabs")
        reset()
        check("Return commits the one-step selection once", alt_session(["Tab"], finish="Return") == once,
              f"expected={once!r}")

        d.flush()
        return all(verdicts)
    finally:
        if session is not None and session.poll() is None:
            try:
                os.killpg(session.pid, 15)
            except ProcessLookupError:
                pass
            session.wait(timeout=10)
        xvfb.terminate()
        xvfb.wait(timeout=10)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    sys.exit(0 if run(sys.argv[1], sys.argv[2]) else 1)

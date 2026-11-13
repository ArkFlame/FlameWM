#!/usr/bin/env python3
"""Executed keyboard proof for flamewm-terminal on a private Xvfb (no WM, private dbus address).

The shell is /bin/sh in a temp work dir. Oracles are files the shell writes (paste.txt, tab files) and pixel
diffs (typed text, tab bar). Checks:
  - a typed word appears on the prompt line;
  - double-click selects it and Ctrl+Shift+C copies it; Ctrl+Shift+V pastes it into `cat > paste.txt`
    (file contents must equal the word);
  - Ctrl+Shift+T opens a tab (tab bar changes) and its shell runs in a sub directory;
  - Ctrl+Tab returns to the first tab, whose shell still runs in the work dir (pwd file).

Usage: terminal_keys_proof.py BIN_DIR OUT_DIR   (BIN_DIR contains flamewm-terminal)
Prints PASS/FAIL per check; exit 0 only when all pass.
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

TITLE = "FlameWM Terminal"
WORD = "alpha"


def free_display():
    for number in range(300, 340):
        if not os.path.exists(f"/tmp/.X11-unix/X{number}") and not os.path.exists(f"/tmp/.X{number}-lock"):
            return f":{number}"
    raise SystemExit("FAIL no free display")


def run(bin_dir, out):
    os.makedirs(out, exist_ok=True)
    os.chmod(out, 0o700)
    display = free_display()
    home = os.path.join(out, "home")
    work = os.path.join(out, "work")
    os.makedirs(home, exist_ok=True)
    os.makedirs(work, exist_ok=True)
    bindir = os.path.join(out, "bin")
    os.makedirs(bindir, exist_ok=True)
    shutil.copy(os.path.join(bin_dir, "flamewm-terminal"), bindir)
    verdicts = []

    def check(label, ok, detail=""):
        verdicts.append(bool(ok))
        print("PASS" if ok else "FAIL", label, detail, flush=True)

    xvfb = subprocess.Popen(["Xvfb", display, "-screen", "0", "1280x800x24", "-nolisten", "tcp", "-noreset"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    proc = None
    try:
        for _ in range(50):
            if os.path.exists(f"/tmp/.X11-unix/X{display[1:]}"):
                break
            time.sleep(0.2)
        env = dict(os.environ, DISPLAY=display, HOME=home, XDG_CONFIG_HOME=os.path.join(home, ".config"),
                   XDG_DATA_HOME=os.path.join(home, ".local/share"), XDG_RUNTIME_DIR=out, TMPDIR=out,
                   XDG_CURRENT_DESKTOP="FlameWM", LANG="C.UTF-8", SHELL="/bin/sh", PS1="$ ")
        env.pop("WAYLAND_DISPLAY", None)
        env["DBUS_SESSION_BUS_ADDRESS"] = f"unix:path={out}/absent.sock"
        proc = subprocess.Popen([os.path.join(bindir, "flamewm-terminal")], env=env, cwd=work, start_new_session=True,
                                stdout=open(os.path.join(out, "terminal.out"), "w"), stderr=subprocess.STDOUT)
        d = xdisplay.Display(display)
        root = d.screen().root

        def find_window():
            found = []

            def walk(w):
                try:
                    name = w.get_wm_name()
                except Exception:  # noqa: BLE001
                    name = None
                if name == TITLE:
                    found.append(w)
                for child in w.query_tree().children:
                    walk(child)
            walk(root)
            return found

        window = None
        for _ in range(60):
            found = find_window()
            if found:
                window = found[0]
                break
            time.sleep(0.5)
        check("terminal window opens", window is not None, f"title={TITLE!r}")
        if window is None:
            return all(verdicts)
        time.sleep(4)
        window.set_input_focus(X.RevertToParent, X.CurrentTime)
        d.sync()
        time.sleep(1.5)
        geometry = window.get_geometry()
        origin = window.translate_coords(root, 0, 0)
        left, top = -origin.x, -origin.y
        box = (left, top, left + geometry.width, top + geometry.height)

        def shot(name):
            path = os.path.join(out, name + ".png")
            subprocess.run(["import", "-window", "root", path], check=True, env=dict(os.environ, DISPLAY=display))
            return Image.open(path).convert("RGB").crop(box)

        def changed(a, b):
            return ImageChops.difference(a, b).getbbox()

        def key(name, wait=0.6, mods=()):
            held = [d.keysym_to_keycode(XK.string_to_keysym(m)) for m in mods]
            code = d.keysym_to_keycode(XK.string_to_keysym(name))
            for m in held:
                xtest.fake_input(d, X.KeyPress, m)
            d.sync()
            time.sleep(0.15)
            xtest.fake_input(d, X.KeyPress, code)
            d.sync()
            time.sleep(0.05)
            xtest.fake_input(d, X.KeyRelease, code)
            for m in reversed(held):
                xtest.fake_input(d, X.KeyRelease, m)
            d.sync()
            time.sleep(wait)

        shifted = {">": "period", "&": "7", "|": "backslash", "<": "comma"}
        named = {" ": "space", "'": "apostrophe", ".": "period", "/": "slash", "-": "minus", "_": "underscore",
                 "=": "equal", ";": "semicolon"}

        def text(s, wait=0.6):
            for ch in s:
                if ch in shifted:
                    key(shifted[ch], wait=0.08, mods=("Shift_L",))
                else:
                    key(ch if ch.isalnum() else named[ch], wait=0.08)
            time.sleep(wait)

        def pointer(x, y, clicks=1):
            xtest.fake_input(d, X.MotionNotify, x=box[0] + x, y=box[1] + y)
            d.sync()
            time.sleep(0.2)
            for _ in range(clicks):
                xtest.fake_input(d, X.ButtonPress, 1)
                d.sync()
                time.sleep(0.05)
                xtest.fake_input(d, X.ButtonRelease, 1)
                d.sync()
                time.sleep(0.1)
            time.sleep(0.6)

        def read(name):
            path = os.path.join(work, name)
            return open(path, encoding="utf-8").read() if os.path.exists(path) else None

        time.sleep(2)
        base = shot("base")
        text(WORD, wait=1.0)
        typed = shot("typed")
        bbox = changed(base, typed)
        check("typed word appears on the prompt line", bbox is not None)
        if bbox is None:
            return all(verdicts)
        # Double-click the word (its left edge sits just inside the diff box, before the cursor).
        wx = bbox[0] + 6
        wy = (bbox[1] + bbox[3]) // 2
        pointer(wx, wy, clicks=2)
        key("c", wait=0.6, mods=("Control_L", "Shift_L"))
        # Clear the line, start cat, paste with Ctrl+Shift+V, end input with Return and Ctrl+D.
        key("u", wait=0.4, mods=("Control_L",))
        text("cat > paste.txt", wait=0.5)
        key("Return", wait=1.0)
        key("v", wait=0.8, mods=("Control_L", "Shift_L"))
        key("Return", wait=0.8)
        key("d", wait=1.2, mods=("Control_L",))
        time.sleep(0.5)
        pasted = read("paste.txt")
        check("Ctrl+Shift+C copies the selection and Ctrl+Shift+V pastes it", pasted == WORD + "\n",
              f"paste.txt={pasted!r}")
        # Tabs: Ctrl+Shift+T opens a second tab that runs in sub/; Ctrl+Tab returns to the first tab.
        before_tab = shot("before-tab")
        key("t", wait=2.0, mods=("Control_L", "Shift_L"))
        after_tab = shot("after-tab")
        check("Ctrl+Shift+T opens a tab (tab bar changes)", changed(before_tab, after_tab) is not None)
        text("mkdir -p sub && cd sub && pwd > tab2.txt", wait=0.5)
        key("Return", wait=1.5)
        tab2 = read(os.path.join("sub", "tab2.txt")) if os.path.exists(os.path.join(work, "sub", "tab2.txt")) else None
        check("the new tab runs its own shell", tab2 is not None and tab2.strip().endswith("/sub"), f"tab2={tab2!r}")
        key("Tab", wait=1.5, mods=("Control_L",))
        text("pwd > tab1.txt", wait=0.5)
        key("Return", wait=1.5)
        tab1 = read("tab1.txt")
        check("Ctrl+Tab returns to the first tab (its shell is still in work)",
              tab1 is not None and tab1.strip().endswith("/work") and "/sub" not in tab1, f"tab1={tab1!r}")
        d.flush()
        return all(verdicts)
    finally:
        if proc is not None and proc.poll() is None:
            try:
                os.killpg(proc.pid, 15)
            except ProcessLookupError:
                pass
            proc.wait(timeout=10)
        xvfb.terminate()
        xvfb.wait(timeout=10)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    sys.exit(0 if run(sys.argv[1], sys.argv[2]) else 1)

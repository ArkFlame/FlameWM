#!/usr/bin/env python3
"""Executed keyboard proof for flamewm-explorer on a private Xvfb (no WM, private dbus address).

Creates a temp HOME with a few files, opens the explorer on it, and drives XTest keys. Oracles are the
files on disk (rename, trash) and pixel diffs of the window (selection, dialog, focus ring).
Checks: arrows move the selection; type-ahead jumps; Enter opens a folder; Backspace goes up;
F2 renames (on disk); Delete opens confirm, Escape leaves the file; Tab cycles the confirm buttons
with a ring, Enter on Cancel leaves the file; Enter on OK moves the file to trash.

Usage: explorer_keys_proof.py BIN_DIR OUT_DIR   (BIN_DIR contains flamewm-explorer)
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

TITLE = "FlameWM Explorer"


def free_display():
    for number in range(260, 300):
        if not os.path.exists(f"/tmp/.X11-unix/X{number}") and not os.path.exists(f"/tmp/.X{number}-lock"):
            return f":{number}"
    raise SystemExit("FAIL no free display")


def run(bin_dir, out):
    os.makedirs(out, exist_ok=True)
    os.chmod(out, 0o700)
    display = free_display()
    home = os.path.join(out, "home")
    work = os.path.join(home, "work")
    os.makedirs(os.path.join(work, "banana"), exist_ok=True)
    for name in ("apple.txt", "cherry.txt", "delete-me.txt"):
        open(os.path.join(work, name), "w").close()
    open(os.path.join(work, "banana", "child.txt"), "w").close()
    bindir = os.path.join(out, "bin")
    os.makedirs(bindir, exist_ok=True)
    shutil.copy(os.path.join(bin_dir, "flamewm-explorer"), bindir)
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
                   XDG_CURRENT_DESKTOP="FlameWM", LANG="C.UTF-8")
        env.pop("WAYLAND_DISPLAY", None)
        env["DBUS_SESSION_BUS_ADDRESS"] = f"unix:path={out}/absent.sock"
        proc = subprocess.Popen([os.path.join(bindir, "flamewm-explorer"), work], env=env, start_new_session=True,
                                stdout=open(os.path.join(out, "explorer.out"), "w"), stderr=subprocess.STDOUT)
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
        check("explorer window opens", window is not None, f"title={TITLE!r}")
        if window is None:
            return all(verdicts)
        time.sleep(4)
        window.set_input_focus(X.RevertToParent, X.CurrentTime)
        d.sync()
        time.sleep(1)
        geometry = window.get_geometry()
        origin = window.translate_coords(root, 0, 0)
        box = (-origin.x, -origin.y, -origin.x + geometry.width, -origin.y + geometry.height)

        def shot(name):
            path = os.path.join(out, name + ".png")
            subprocess.run(["import", "-window", "root", path], check=True, env=dict(os.environ, DISPLAY=display))
            return Image.open(path).convert("RGB").crop(box)

        def changed(a, b):
            return ImageChops.difference(a, b).getbbox() is not None

        def key(name, wait=0.8, mods=()):
            codes = [d.keysym_to_keycode(XK.string_to_keysym(m)) for m in mods]
            code = d.keysym_to_keycode(XK.string_to_keysym(name))
            for c in codes:
                xtest.fake_input(d, X.KeyPress, c)
            d.sync()
            time.sleep(0.15)
            xtest.fake_input(d, X.KeyPress, code)
            d.sync()
            time.sleep(0.05)
            xtest.fake_input(d, X.KeyRelease, code)
            for c in reversed(codes):
                xtest.fake_input(d, X.KeyRelease, c)
            d.sync()
            time.sleep(wait)

        def typed(text, wait=0.8):
            for ch in text:
                key(ch if ch.isalnum() else {".": "period", "-": "minus", "_": "underscore", " ": "space"}[ch], wait=0.3)
            time.sleep(wait)

        def exists(*parts):
            return os.path.exists(os.path.join(work, *parts))

        base = shot("base")
        # Arrows: Down moves the selection; Up moves it back.
        key("Down", wait=1.0)
        s_down = shot("down")
        check("Down moves the selection", changed(base, s_down))
        key("Up", wait=1.0)
        s_up = shot("up")
        check("Up moves the selection back", changed(s_down, s_up))
        # Type-ahead: 'b' jumps to banana (the only name starting with b).
        # Down first so the selection is not already on banana (the first row) before typing b.
        key("Down", wait=1.0)
        s_pre_b = shot("pre-typeahead-b")
        key("b", wait=1.0)
        s_b = shot("typeahead-b")
        check("type-ahead jumps to the matching row", changed(s_pre_b, s_b))
        # Enter opens the selected folder; Backspace goes back up.
        key("Return", wait=1.5)
        s_in = shot("in-banana")
        check("Enter opens the folder (listing changes)", changed(s_b, s_in))
        key("BackSpace", wait=1.5)
        s_back = shot("back-up")
        check("Backspace goes up (listing changes back)", changed(s_in, s_back))
        # Type-ahead again to cherry, then F2 renames it on disk.
        key("c", wait=1.0)
        s_pre_f2 = shot("pre-f2")
        key("F2", wait=1.0)
        s_f2 = shot("f2")
        check("F2 opens the inline rename field", changed(s_pre_f2, s_f2))
        key("a", wait=0.3, mods=("Control_L",))
        typed("renamed.txt", wait=1.0)
        key("Return", wait=1.5)
        check("F2 rename commits to disk", exists("renamed.txt") and not exists("cherry.txt"),
              f"renamed={exists('renamed.txt')} cherry={exists('cherry.txt')}")
        # Delete: confirm opens; Escape leaves the file.
        key("d", wait=1.0)
        s_pre_del = shot("pre-delete")
        key("Delete", wait=1.5)
        s_confirm = shot("confirm-ok")
        check("Delete opens the confirmation", changed(s_pre_del, s_confirm))
        key("Escape", wait=1.5)
        check("Escape cancels the confirmation and keeps the file",
              exists("delete-me.txt") and not changed(s_pre_del, shot("after-escape")),
              f"exists={exists('delete-me.txt')}")
        # Tab cycles the visible buttons with a ring: OK -> Cancel -> OK.
        key("Delete", wait=1.5)
        s_ok = shot("confirm-focus-ok")
        key("Tab", wait=1.0)
        s_tab = shot("confirm-focus-cancel")
        check("Tab moves the focus ring to Cancel", changed(s_ok, s_tab))
        key("Tab", wait=1.0)
        s_tab2 = shot("confirm-focus-wrap")
        check("Tab wraps the focus ring from Cancel back to OK", changed(s_tab, s_tab2))
        key("Tab", wait=1.0)
        key("Return", wait=1.5)
        check("Enter on the focused Cancel keeps the file", exists("delete-me.txt"))
        # Enter on the default OK moves the file to trash.
        key("d", wait=1.0)
        key("Delete", wait=1.5)
        key("Return", wait=2.5)
        check("Enter on OK moves the file to trash", not exists("delete-me.txt"))
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

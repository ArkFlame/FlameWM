#!/usr/bin/env python3
"""Executed keyboard proof for the taskbar overflow menu and the launcher on a private Xvfb.

Runs flamewm-session (WM, shell, desktop) on a private Xvfb and a private dbus-run-session, with a
private HOME whose applications/ dir holds three fake .desktop entries that append a marker line.
Twelve fake X windows overflow the eight taskbar slots, so the "+4" cell opens the overflow menu.
XTest keys drive the overflow menu (Down/Up/Home/End move the highlight, read from the captured
root pixels of the accent-soft fill; Enter activates a fake window; Tab and Escape close). Then
Super (Start) and Super+space (launcher) with typed queries: the keys move the selection and Return
launches; the marker file names the entry that ran, so each case must add exactly one line.

Usage: shell_keys_proof.py BIN_DIR OUT_DIR
  BIN_DIR: directory with the fast flamewm, flamewm-session, flamewm-shell, flamewm-desktop binaries.
Prints one PASS/FAIL line per check; exit 0 only when every check passed.
"""

import os
import shutil
import subprocess
import sys
import time

from PIL import Image
from Xlib import X, XK
from Xlib import display as xdisplay
from Xlib.ext import xtest

OVERFLOW_CELL = (407, 778)  # "+4" cell of the 1280x800 sandbox taskbar with 12 windows
ROW_PITCH = 31  # overflow row pitch in px, measured from the rendered menu
APPS = [("Proof Alpha", "proof-alpha"), ("Proof Beta", "proof-beta"), ("Proof Gamma", "proof-gamma")]
FAKE_WINDOWS = 12


def shot(out, name):
    dest = os.path.join(out, name)
    subprocess.run(["import", "-window", "root", dest], check=True, env=dict(os.environ, DISPLAY=DISPLAY))
    return Image.open(dest).convert("RGB")


def popup_rows(closed, opened, x=412):
    """Pixel rows of column x that the open menu changed against the closed root: the popup area."""
    return {y for y in range(400, 757) if closed.getpixel((x, y)) != opened.getpixel((x, y))}


def highlight_colour(im, rows, x=412):
    """Accent-soft fill: the most common blue-teal colour (B-R > 25) inside the popup rows."""
    seen = {}
    for y in rows:
        r, g, b = im.getpixel((x, y))
        if b - r > 25:
            seen[(r, g, b)] = seen.get((r, g, b), 0) + 1
    return max(seen, key=seen.get) if seen else None


def hl_runs(im, colour, rows, x=412):
    """Bottom edges of runs in column x, inside the popup rows, that match the highlight colour."""
    edges, inside = [], False
    for y in range(400, 757):
        match = y in rows and all(abs(a - b) <= 6 for a, b in zip(im.getpixel((x, y)), colour))
        if inside and not match:
            edges.append(y - 1)
        inside = match
    return edges


def write_fixtures(home, out):
    apps = os.path.join(home, ".local/share/applications")
    os.makedirs(apps, exist_ok=True)
    for name, slug in APPS:
        with open(os.path.join(apps, slug + ".desktop"), "w", encoding="utf-8") as handle:
            handle.write(f"[Desktop Entry]\nType=Application\nName={name}\n"
                         f"Exec=sh -c 'echo {slug} >> {out}/launched.txt'\n")


def free_display():
    """First display number in :150-:199 with no socket, so a stale server cannot be reused."""
    for number in range(150, 200):
        if not os.path.exists(f"/tmp/.X11-unix/X{number}") and not os.path.exists(f"/tmp/.X{number}-lock"):
            return f":{number}"
    raise SystemExit("FAIL no free display in :150-:199")


def run(bin_dir, out):
    global DISPLAY
    DISPLAY = free_display()
    os.makedirs(out, exist_ok=True)
    home = os.path.join(out, "home")
    os.makedirs(os.path.join(home, ".config"), exist_ok=True)
    write_fixtures(home, out)
    bindir = os.path.join(out, "bin")
    os.makedirs(bindir, exist_ok=True)
    for name in ("flamewm", "flamewm-session", "flamewm-shell", "flamewm-desktop"):
        shutil.copy(os.path.join(bin_dir, name), bindir)
    verdicts = []

    def check(label, ok, detail):
        verdicts.append(bool(ok))
        print("PASS" if ok else "FAIL", label, detail, flush=True)

    xvfb = subprocess.Popen(["Xvfb", DISPLAY, "-screen", "0", "1280x800x24", "-nolisten", "tcp", "-noreset"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    session = None
    try:
        for _ in range(50):
            if os.path.exists(f"/tmp/.X11-unix/X{DISPLAY[1:]}"):
                break
            time.sleep(0.2)
        env = dict(os.environ, DISPLAY=DISPLAY, HOME=home, XDG_CONFIG_HOME=os.path.join(home, ".config"),
                   XDG_DATA_HOME=os.path.join(home, ".local/share"), XDG_RUNTIME_DIR=out, TMPDIR=out,
                   XDG_CURRENT_DESKTOP="FlameWM", PATH=f"{bindir}:/usr/bin:/bin", LANG="C.UTF-8")
        env.pop("WAYLAND_DISPLAY", None)
        session = subprocess.Popen(["dbus-run-session", "--", os.path.join(bindir, "flamewm-session")],
                                   env=env, start_new_session=True,
                                   stdout=open(os.path.join(out, "session.out"), "w"), stderr=subprocess.STDOUT)
        d = xdisplay.Display(DISPLAY)
        root = d.screen().root
        atom = lambda n: d.intern_atom(n)  # noqa: E731
        time.sleep(8)
        for i in range(FAKE_WINDOWS):
            w = root.create_window(100 + i * 10, 100 + i * 10, 300, 200, 0, d.screen().root_depth,
                                   X.InputOutput, background_pixel=0x3050A0 + i * 0x101010)
            w.set_wm_name(f"Fake {i:02d}")
            w.map()
        d.flush()
        time.sleep(5)

        def key(*names):
            codes = [d.keysym_to_keycode(XK.string_to_keysym(n)) for n in names]
            for code in codes:
                xtest.fake_input(d, X.KeyPress, code)
            d.sync()
            time.sleep(0.05)
            for code in reversed(codes):
                xtest.fake_input(d, X.KeyRelease, code)
            d.sync()
            time.sleep(0.6)

        def click(x, y):
            root.warp_pointer(x, y)
            d.sync()
            time.sleep(0.2)
            xtest.fake_input(d, X.ButtonPress, 1)
            xtest.fake_input(d, X.ButtonRelease, 1)
            d.sync()
            time.sleep(1.2)

        def active_window():
            prop = root.get_full_property(atom("_NET_ACTIVE_WINDOW"), X.AnyPropertyType)
            return prop.value[0] if prop and prop.value else 0

        def client_names():
            prop = root.get_full_property(atom("_NET_CLIENT_LIST"), X.AnyPropertyType)
            return [d.create_resource_object("window", wid).get_wm_name() for wid in (prop.value if prop else [])]

        for _ in range(60):
            if sum(1 for n in client_names() if n and n.startswith("Fake ")) >= FAKE_WINDOWS:
                break
            time.sleep(0.5)
        check("twelve fake windows managed", sum(1 for n in client_names() if n and n.startswith("Fake ")) == FAKE_WINDOWS,
              f"clients={len(client_names())}")

        # Overflow menu: open with the "+4" cell, check highlight row 0 and the arrow walk.
        base = shot(out, "menu-closed.png")
        click(*OVERFLOW_CELL)
        opened = shot(out, "menu-open.png")
        # Calibrate from the closed/open diff: row 0 is highlighted on open.
        rows = popup_rows(base, opened)
        colour = highlight_colour(opened, rows)
        top_edge = hl_runs(opened, colour, rows) if colour else []
        check("menu opens with row 0 highlighted", len(top_edge) == 1, f"bottom_edges={top_edge} colour={colour}")
        b0 = top_edge[0] if top_edge else None

        def row_index(im):
            edges = hl_runs(im, colour, rows)
            if len(edges) != 1 or b0 is None:
                return None, edges
            return round((edges[0] - b0) / ROW_PITCH), edges

        steps = [
            ("Down", 1), ("Down", 2), ("Up", 1), ("End", None), ("Home", 0),
        ]
        for index, (name, expect) in enumerate(steps):
            key(name)
            im = shot(out, f"menu-{index}-{name}.png")
            got, edges = row_index(im)
            if name == "End":
                ok = got is not None and got > 2
                check(f"{name} moves highlight to the last row", ok, f"row={got} edges={edges}")
            else:
                check(f"{name} moves highlight to row {expect}", got == expect, f"row={got} edges={edges}")
        key("Down")
        before_active = active_window()
        key("Return")
        time.sleep(1.0)
        after_active = active_window()
        check("Enter closes the overflow menu", row_index(shot(out, "menu-after-enter.png"))[0] is None,
              "no highlight run after Enter")
        activated = d.create_resource_object("window", after_active).get_wm_name() if after_active else None
        check("Enter activates a fake window from the overflow",
              after_active != before_active and bool(activated and activated.startswith("Fake ")),
              f"active={after_active:#x} name={activated!r}")

        for closer in ("Tab", "Escape"):
            click(*OVERFLOW_CELL)
            reopened = row_index(shot(out, f"menu-reopen-{closer}.png"))
            check(f"reopened menu highlights row 0 before {closer}", reopened[0] == 0, f"row={reopened[0]}")
            key(closer)
            closed = row_index(shot(out, f"menu-after-{closer}.png"))[0] is None
            check(f"{closer} closes the overflow menu", closed, "no highlight run after the key")

        # Each case opens a surface, types the query, applies the keys and presses Return. The marker
        # file names the entry that launched, so exactly one new line proves one launch of that row.
        marker = os.path.join(out, "launched.txt")

        def launched():
            return open(marker, encoding="utf-8").read().split() if os.path.exists(marker) else []

        def case(label, opener, moves, expected):
            before = len(launched())
            key(*opener)
            time.sleep(0.8)
            for ch in "proof":
                key(ch)
            for move in moves:
                key(move)
            key("Return")
            time.sleep(2.5)
            new = launched()[before:]
            check(label, new == [expected], f"new={new}")

        # Issue #24: Super opens Start; query, Down, Return launches exactly once. An unselected list
        # has no cursor, so the first Down highlights row 0 (selection model: none -> first row).
        case("issue #24: Super, query, Down, Return launches exactly once", ["Super_L"], ["Down"], "proof-alpha")
        # Launcher (Super+space): Tab, Home, End, PageDown and PageUp move the selection (clamped).
        case("launcher: Super+space, query, Tab Tab, Return launches row 1", ["Super_L", "space"], ["Tab", "Tab"], "proof-beta")
        case("launcher: Tab x3 clamps at the last row", ["Super_L", "space"], ["Tab", "Tab", "Tab"], "proof-gamma")
        case("launcher: End, Return launches the last row", ["Super_L", "space"], ["End"], "proof-gamma")
        case("launcher: Home, Return launches row 0", ["Super_L", "space"], ["End", "Home"], "proof-alpha")
        case("launcher: Page_Down x2 clamps to the last row", ["Super_L", "space"], ["Page_Down", "Page_Down"], "proof-gamma")
        case("launcher: Page_Up clamps to row 0", ["Super_L", "space"], ["End", "Page_Up"], "proof-alpha")
        check("every launch wrote exactly one marker line", len(launched()) == 7, f"launched={launched()}")

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

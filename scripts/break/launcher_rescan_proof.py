#!/usr/bin/env python3
"""Executed proof that a newly installed .desktop file reaches the launcher without logout.

Runs flamewm-session on a private Xvfb with a private dbus-daemon, a temp HOME and temp XDG data
dirs (XDG_DATA_HOME/applications exists and is empty at start; XDG_DATA_DIRS points at an empty
temp dir). Super+Space opens the launcher and the probe query is typed. Checks:
  1. before install: the probe query is typed and Enter does not launch the probe (no marker file);
  2. a .desktop file with a unique Name and Exec=/usr/bin/touch MARKER is dropped into
     XDG_DATA_HOME/applications while the session keeps running;
  3. reopening the launcher and typing the same query draws a result row (captured root pixels
     differ from the pre-install capture inside the screen area above the taskbar);
  4. Enter launches the probe: the Exec runs and creates the marker file;
  5. the shell logged no launcher rescan failure.

Usage: launcher_rescan_proof.py BIN_DIR OUT_DIR
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

PANEL_TOP = 740  # rows at and below this are the taskbar
SEARCH_AREA = (0, 0, 1280, PANEL_TOP)
PROBE_NAME = "Zorblax Probe"
PROBE_QUERY = "zorblax"
PROBE_MIN_PX = 300  # changed pixels the probe's result row must add to the launcher


def shot(out, name, display):
    dest = os.path.join(out, name)
    subprocess.run(["import", "-window", "root", dest], check=True, env=dict(os.environ, DISPLAY=display))
    return Image.open(dest).convert("RGB")


def changed_count(a, b, box):
    diff = ImageChops.difference(a.crop(box), b.crop(box))
    return sum(diff.convert("L").histogram()[17:])


def changed_box(a, b, box):
    return ImageChops.difference(a.crop(box), b.crop(box)).getbbox()


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


def wait_for_wm(display, env):
    for _ in range(120):
        out = subprocess.run(["xprop", "-root", "_NET_SUPPORTING_WM_CHECK"], env=env,
                             capture_output=True, text=True)
        if "window" in out.stdout:
            return True
        time.sleep(0.5)
    return False


def reap_session(out):
    """Kills every process whose environment names this run's private runtime dir."""
    marker = f"XDG_RUNTIME_DIR={out}".encode()
    for entry in os.listdir("/proc"):
        if not entry.isdigit() or int(entry) == os.getpid():
            continue
        try:
            with open(f"/proc/{entry}/environ", "rb") as handle:
                if marker in handle.read().split(b"\0"):
                    os.kill(int(entry), 15)
        except OSError:
            pass


def run(bin_dir, out):
    display = free_display()
    out = os.path.abspath(out)
    os.makedirs(out, exist_ok=True)
    home = os.path.join(out, "home")
    data_home = os.path.join(home, ".local/share")
    applications = os.path.join(data_home, "applications")
    system_share = os.path.join(out, "system-share")
    os.makedirs(os.path.join(home, ".config"), exist_ok=True)
    os.makedirs(applications, exist_ok=True)
    os.makedirs(system_share, exist_ok=True)
    bindir = os.path.join(out, "bin")
    os.makedirs(bindir, exist_ok=True)
    for name in ("flamewm", "flamewm-session", "flamewm-shell", "flamewm-desktop"):
        shutil.copy(os.path.join(bin_dir, name), bindir)
    marker = os.path.join(out, "probe-launched.marker")
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
                   XDG_DATA_HOME=data_home, XDG_DATA_DIRS=system_share,
                   XDG_STATE_HOME=os.path.join(home, ".local/state"),
                   XDG_RUNTIME_DIR=out, TMPDIR=out, DBUS_SESSION_BUS_ADDRESS=address,
                   XDG_CURRENT_DESKTOP="FlameWM", PATH=f"{bindir}:/usr/bin:/bin", LANG="C.UTF-8")
        env.pop("WAYLAND_DISPLAY", None)
        session_log = os.path.join(out, "session.out")
        session = subprocess.Popen([os.path.join(bindir, "flamewm-session")], env=env,
                                   start_new_session=True,
                                   stdout=open(session_log, "w"), stderr=subprocess.STDOUT)
        d = xdisplay.Display(display)
        root = d.screen().root
        check("session window manager owns the display", wait_for_wm(display, env), f"display={display}")
        time.sleep(6)  # shell and desktop settle past their startup grace

        def tap(name, mods=(), settle=0.12):
            codes = [d.keysym_to_keycode(XK.string_to_keysym(m)) for m in mods]
            code = d.keysym_to_keycode(XK.string_to_keysym(name))
            for c in codes:
                xtest.fake_input(d, X.KeyPress, c)
            xtest.fake_input(d, X.KeyPress, code)
            xtest.fake_input(d, X.KeyRelease, code)
            for c in reversed(codes):
                xtest.fake_input(d, X.KeyRelease, c)
            d.sync()
            time.sleep(settle)

        def open_launcher():
            tap("space", ("Super_L",), settle=2.5)

        def type_probe_query():
            for _ in range(len(PROBE_QUERY) + 6):
                tap("BackSpace", settle=0.05)
            for ch in PROBE_QUERY:
                tap(ch, settle=0.12)
            time.sleep(1.5)

        # Phase 1: the probe is not in the catalog yet.
        open_launcher()
        shot(out, "1-launcher-open.png", display)
        type_probe_query()
        absent = shot(out, "2-query-before-install.png", display)
        tap("Return", settle=3.0)
        check("probe absent before install: Enter does not launch it", not os.path.exists(marker),
              f"marker={os.path.exists(marker)}")
        tap("Escape", settle=1.5)

        # Install while the session keeps running; no logout, no shell restart.
        desktop_file = os.path.join(applications, "zorblax-probe.desktop")
        with open(desktop_file, "w") as handle:
            handle.write("[Desktop Entry]\nType=Application\nVersion=1.0\n"
                         f"Name={PROBE_NAME}\nExec=/usr/bin/touch {marker}\nTerminal=false\n")
        time.sleep(1)

        # Phase 2: reopen the launcher; the open-time rescan must list the probe.
        open_launcher()
        type_probe_query()
        present = shot(out, "3-query-after-install.png", display)
        box = changed_box(absent, present, SEARCH_AREA)
        px = changed_count(absent, present, SEARCH_AREA)
        check("probe row appears after install without logout", px >= PROBE_MIN_PX,
              f"changed_px={px} bbox={box}")
        tap("Return", settle=3.0)
        check("Enter launches the newly installed app (Exec ran)", os.path.exists(marker),
              f"marker={os.path.exists(marker)}")

        log_text = open(session_log, errors="replace").read()
        check("no launcher app rescan failure logged", "launcher app rescan failed" not in log_text
              and "panicked at" not in log_text, f"log_bytes={len(log_text)}")
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
        reap_session(out)
        xvfb.terminate()
    return all(verdicts) and len(verdicts) == 5


if __name__ == "__main__":
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    sys.exit(0 if run(sys.argv[1], sys.argv[2]) else 1)

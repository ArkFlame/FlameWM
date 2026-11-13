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

from PIL import Image
from Xlib import X, XK
from Xlib import display as xdisplay
from Xlib.ext import xtest

FAKE_WINDOWS = 4
WINDOW_W, WINDOW_H = 300, 300
SLOT_X = [50, 350, 650, 950]  # non-overlapping columns on the 1280x800 sandbox
START = "Fake 0"
# Task button centres on the 1280x800 sandbox taskbar (calibrated from taskbar-calibration.png).
TASK_BUTTON = {0: (71, 777), 1: (113, 777), 2: (155, 777), 3: (197, 777)}  # 1280x800 sandbox taskbar, 4 tasks

# Stands in for libpam.so.0 (loaded through LD_LIBRARY_PATH). pam_authenticate asks for the password
# through the conversation, as PAM does, logs it once per call, and always refuses, so the lock stays.
FAKE_PAM_C = r"""
#include <stdio.h>
#include <stdlib.h>
struct pam_message { int msg_style; const char *msg; };
struct pam_response { char *resp; int resp_retcode; };
struct pam_conv { int (*conv)(int, const struct pam_message **, struct pam_response **, void *); void *appdata_ptr; };
static struct pam_conv saved;
int pam_start(const char *service, const char *user, const struct pam_conv *conv, void **pamh) {
    (void)service; (void)user; saved = *conv; *pamh = &saved; return 0;
}
int pam_authenticate(void *pamh, int flags) {
    struct pam_message msg = {1, "Password: "};
    const struct pam_message *msgs = &msg;
    struct pam_response *resp = NULL;
    (void)pamh; (void)flags;
    if (saved.conv(1, &msgs, &resp, saved.appdata_ptr) != 0 || !resp) return 7;
    FILE *log = fopen(getenv("FAKE_PAM_LOG"), "a");
    if (log) { fprintf(log, "auth %s\n", resp->resp ? resp->resp : ""); fclose(log); }
    free(resp->resp); free(resp);
    return 7; /* PAM_AUTH_ERR: the lock stays locked */
}
int pam_acct_mgmt(void *pamh, int flags) { (void)pamh; (void)flags; return 7; }
int pam_end(void *pamh, int status) { (void)pamh; (void)status; return 0; }
"""


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
        # One X connection per fake client: a WM "Close" without WM_DELETE_WINDOW kills the client, and
        # that must not take the proof's own connection down with it.
        clients = []
        for i in range(FAKE_WINDOWS):
            conn = xdisplay.Display(display_name)
            clients.append(conn)
            croot = conn.screen().root
            w = croot.create_window(SLOT_X[i], 100, WINDOW_W, WINDOW_H, 0, conn.screen().root_depth,
                                    X.InputOutput, background_pixel=0x3050A0 + i * 0x101010)
            w.set_wm_name(f"Fake {i}")
            w.map()
            conn.flush()
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

        # Taskbar task context menu: right-click a task button, arrows move the row, Return fires one
        # row action, Escape closes and returns focus to the window that was active before the menu.
        def rclick(x, y):
            root.warp_pointer(x, y)
            d.sync()
            time.sleep(0.2)
            xtest.fake_input(d, X.ButtonPress, 3)
            xtest.fake_input(d, X.ButtonRelease, 3)
            d.sync()
            time.sleep(1.2)

        def shot(name):
            dest = os.path.join(out, name)
            subprocess.run(["import", "-window", "root", dest], check=True, env=dict(os.environ, DISPLAY=display_name))
            return Image.open(dest).convert("RGB")

        def names():
            return sorted(n for n in client_names() if n and n.startswith("Fake "))

        park_pointer = (640, 300)

        def park():
            root.warp_pointer(*park_pointer)
            d.sync()
            time.sleep(0.5)

        def diff_count(a, b):
            return sum(1 for x in range(0, 1280, 4) for y in range(0, 800, 4) if a.getpixel((x, y)) != b.getpixel((x, y)))

        reset()
        before = names()
        park()
        shot("taskbar-calibration.png")
        menu_closed = shot("task-menu-closed.png")
        rclick(*TASK_BUTTON[1])
        park()
        menu_open = shot("task-menu-open.png")
        check("right-click on a task button opens its menu", diff_count(menu_closed, menu_open) > 300,
              f"changed={diff_count(menu_closed, menu_open)}")
        for step in ("Down", "Down"):
            tap(step)
        tap("Return")
        time.sleep(1.0)
        after = names()
        check("Down, Down, Return fires exactly one row action (Pin: no window closes)", after == before,
              f"before={before} after={after}")
        reset()
        before = names()
        rclick(*TASK_BUTTON[2])
        tap("Down")
        tap("Return")
        time.sleep(1.5)
        after = names()
        closed = sorted(set(before) - set(after))
        check("Down, Return on a task menu closes that task's window once", len(closed) == 1 and len(after) == len(before) - 1,
              f"closed={closed}")

        reset()
        before = names()
        active_before = active_name()
        rclick(*TASK_BUTTON[3])
        tap("Down")
        tap("Escape")
        time.sleep(0.8)
        check("Escape closes the task menu without firing a row", names() == before,
              f"names={names()}")
        check("Escape returns focus to the window active before the menu", active_name() == active_before,
              f"active={active_name()!r} before={active_before!r}")

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


def run_lock(bin_dir, out):
    """flamewm-lock on a private Xvfb with no WM: password entry through XTest keys. The fake PAM
    logs each password it is asked to check, so one line per Enter proves one submit."""
    verdicts = []

    def check(label, ok, detail):
        verdicts.append(bool(ok))
        print("PASS" if ok else "FAIL", label, detail, flush=True)

    display_name = next(f":{n}" for n in range(250, 300)
                        if not os.path.exists(f"/tmp/.X11-unix/X{n}") and not os.path.exists(f"/tmp/.X{n}-lock"))
    fake_dir = os.path.join(out, "fakepam")
    os.makedirs(fake_dir, exist_ok=True)
    source = os.path.join(fake_dir, "pam_fake.c")
    with open(source, "w", encoding="utf-8") as handle:
        handle.write(FAKE_PAM_C)
    subprocess.run(["cc", "-shared", "-fPIC", "-o", os.path.join(fake_dir, "libpam.so.0"), source], check=True)
    run_dir = os.path.join(out, "lock-run")
    os.makedirs(run_dir, exist_ok=True)
    pam_log = os.path.join(out, "pam-attempts.log")
    xvfb = subprocess.Popen(["Xvfb", display_name, "-screen", "0", "1280x800x24", "-nolisten", "tcp", "-noreset"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    lock = None
    try:
        for _ in range(50):
            if os.path.exists(f"/tmp/.X11-unix/X{display_name[1:]}"):
                break
            time.sleep(0.2)
        env = dict(os.environ, DISPLAY=display_name, XDG_RUNTIME_DIR=run_dir, HOME=run_dir, USER="proofuser",
                   LD_LIBRARY_PATH=fake_dir, FAKE_PAM_LOG=pam_log, LANG="C.UTF-8")
        env.pop("WAYLAND_DISPLAY", None)
        lock = subprocess.Popen([os.path.join(bin_dir, "flamewm-lock")], env=env, start_new_session=True,
                                stdout=open(os.path.join(out, "lock.out"), "w"), stderr=subprocess.STDOUT)
        d = xdisplay.Display(display_name)
        time.sleep(4)

        def tap(name):
            code = d.keysym_to_keycode(XK.string_to_keysym(name))
            xtest.fake_input(d, X.KeyPress, code)
            d.sync()
            time.sleep(0.15)
            xtest.fake_input(d, X.KeyRelease, code)
            d.sync()
            time.sleep(0.5)

        def attempts():
            return open(pam_log, encoding="utf-8").read().splitlines() if os.path.exists(pam_log) else []

        alive = lock.poll() is None
        check("lock process stays up on its private display", alive, f"pid={lock.pid}")
        for name in ("a", "b", "BackSpace", "c"):
            tap(name)
        check("no attempt before Return", attempts() == [], f"attempts={attempts()}")
        tap("Return")
        time.sleep(3)
        check("typed chars once, Backspace once: Return sends the password 'ac' exactly once",
              attempts() == ["auth ac"], f"attempts={attempts()}")
        tap("Escape")
        tap("Return")
        time.sleep(2)
        check("empty password: Return sends nothing", attempts() == ["auth ac"], f"attempts={attempts()}")
        check("lock still running after failed auth", lock.poll() is None, "lock stays locked")
        d.flush()
        return all(verdicts)
    finally:
        if lock is not None and lock.poll() is None:
            lock.kill()  # our own PID: the lock ignores SIGTERM by design
            lock.wait(timeout=10)
        xvfb.terminate()
        xvfb.wait(timeout=10)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    sys.exit(0 if run(sys.argv[1], sys.argv[2]) and run_lock(sys.argv[1], sys.argv[2]) else 1)

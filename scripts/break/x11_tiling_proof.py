#!/usr/bin/env python3
# X11 tiling / snap / double-click proof for scripts/break: private Xvfb, the flamewm X11 WM,
# an Xlib test client and XTest input (no xdotool on the host). Never touches the host display
# or session bus. Usage: python3 scripts/break/x11_tiling_proof.py [BIN_DIR]
# BIN_DIR defaults to $BREAK_BIN_SRC or /tmp/opencode/snapgap-target/fast. Exit 0 only if all checks pass.
import os
import subprocess
import sys
import tempfile
import time

from Xlib import X, XK, display
from Xlib.ext import xtest

BIN = sys.argv[1] if len(sys.argv) > 1 else os.environ.get("BREAK_BIN_SRC", "/tmp/opencode/snapgap-target/fast")
RESULTS = []


def check(name, ok, detail=""):
    RESULTS.append(bool(ok))
    print(("PASS " if ok else "FAIL ") + name + (" " + str(detail) if detail else ""), flush=True)


def approx(a, b, tol):
    return all(abs(x - y) <= tol for x, y in zip(a, b))


def settle(fn, pred, timeout=3.0):
    end = time.time() + timeout
    value = fn()
    while not pred(value) and time.time() < end:
        time.sleep(0.05)
        value = fn()
    return value


def main():
    tmp = tempfile.mkdtemp(prefix="x11tile-")
    home = os.path.join(tmp, "home")
    os.makedirs(home)
    num = next(n for n in range(180, 200) if not os.path.exists(f"/tmp/.X11-unix/X{n}"))
    disp = f":{num}"
    xvfb = subprocess.Popen(["Xvfb", disp, "-screen", "0", "1280x800x24", "-nolisten", "tcp", "-noreset"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    wm = None
    try:
        d = None
        end = time.time() + 10
        while d is None and time.time() < end:
            try:
                d = display.Display(disp)
            except Exception:
                time.sleep(0.1)
        if d is None:
            raise SystemExit("proof: Xvfb did not accept clients")
        env = {"PATH": "/usr/bin:/bin", "HOME": home, "XDG_CONFIG_HOME": home, "XDG_RUNTIME_DIR": tmp,
               "TMPDIR": tmp, "DISPLAY": disp, "LANG": "C.UTF-8", "XDG_CURRENT_DESKTOP": "FlameWM",
               "DBUS_SESSION_BUS_ADDRESS": f"unix:path={tmp}/absent.sock"}
        wm = subprocess.Popen([os.path.join(BIN, "flamewm")], env=env, stdout=open(os.path.join(tmp, "wm.log"), "w"),
                              stderr=subprocess.STDOUT)
        root = d.screen().root

        def atom(name):
            return d.intern_atom(name)

        def root_prop(name):
            p = root.get_full_property(atom(name), X.AnyPropertyType)
            return list(p.value) if p else []

        settle(lambda: root_prop("_NET_SUPPORTING_WM_CHECK"), bool, 15)
        check("wm claimed _NET_SUPPORTING_WM_CHECK", root_prop("_NET_SUPPORTING_WM_CHECK"))
        win = root.create_window(200, 150, 400, 300, 0, d.screen().root_depth, X.InputOutput, X.CopyFromParent,
                                 background_pixel=0x204060, event_mask=X.ButtonPressMask | X.StructureNotifyMask)
        win.set_wm_name("flamewm-tile-probe")
        win.map()
        d.sync()
        settle(lambda: win.query_tree().parent.id != root.id, bool, 10)

        def rect(w):
            g = w.get_geometry()
            t = root.translate_coords(w, 0, 0)
            return (t.x, t.y, g.width, g.height)

        def frame():
            return rect(win.query_tree().parent)

        def states():
            return set(root_prop_of(win, "_NET_WM_STATE"))

        def root_prop_of(w, name):
            p = w.get_full_property(atom(name), X.AnyPropertyType)
            return list(p.value) if p else []

        mv, mh = atom("_NET_WM_STATE_MAXIMIZED_VERT"), atom("_NET_WM_STATE_MAXIMIZED_HORZ")

        def maximized():
            return {mv, mh} <= states()

        def tap(*names):
            codes = [d.keysym_to_keycode(XK.string_to_keysym(n)) for n in names]
            for c in codes:
                xtest.fake_input(d, X.KeyPress, c)
            for c in reversed(codes):
                xtest.fake_input(d, X.KeyRelease, c)
            d.sync()

        def move(x, y):
            xtest.fake_input(d, X.MotionNotify, x=x, y=y)
            d.sync()

        def button(down):
            xtest.fake_input(d, X.ButtonPress if down else X.ButtonRelease, 1)
            d.sync()

        def drag_to(x, y, steps=12):
            cur = d.screen().root.query_pointer()
            x0, y0 = cur.root_x, cur.root_y
            for i in range(1, steps + 1):
                move(x0 + (x - x0) * i // steps, y0 + (y - y0) * i // steps)
                time.sleep(0.02)

        def overlays():
            out = []
            for c in root.query_tree().children:
                a = c.get_attributes()
                if a.override_redirect and a.map_state == X.IsViewable:
                    out.append(rect(c))
            return out

        def title_point(fr):
            yoff = win.query_tree().parent.translate_coords(win, 0, 0).y
            return fr[0] + fr[2] // 2, fr[1] + max(yoff, 1) // 2, yoff

        time.sleep(0.5)
        g0 = frame()
        ty_probe = title_point(g0)
        check("titlebar present above client", ty_probe[2] > 0, f"yoff={ty_probe[2]}")

        # 1. keyboard tiling Super+arrow, undo restores the exact previous geometry
        # The snap/maximise base is the WM's own maximised rect (window margin and dock reserve applied),
        # which is not the raw _NET_WORKAREA; measure it once instead of assuming it.
        tap("Super_L", "Up")
        area = settle(frame, lambda r: maximized())
        check("Super+Up from floating maximises", maximized(), sorted(states()))
        area = frame()
        tap("Super_L", "Down")
        r, mx = settle(lambda: (frame(), maximized()), lambda v: v[0] == g0 and not v[1])
        check("Super+Down restores from maximised exactly", r == g0 and not mx, f"got={r} want={g0} maximised={mx}")
        tap("Super_L", "Left")
        r = settle(frame, lambda r: approx(r, (area[0], area[1], area[2] // 2, area[3]), 4))
        check("Super+Left tiles left half", approx(r, (area[0], area[1], area[2] // 2, area[3]), 4), r)
        tap("Super_L", "Up")
        r = settle(frame, lambda r: approx(r, (area[0], area[1], area[2] // 2, area[3] // 2), 4))
        check("Super+Up from left half gives top-left quarter", approx(r, (area[0], area[1], area[2] // 2, area[3] // 2), 4), r)
        tap("Super_L", "Down")
        tap("Super_L", "Down")
        r = settle(frame, lambda r: r == g0)
        check("Super+Down x2 restores exact pre-tile geometry", r == g0, f"got={r} want={g0}")
        tap("Super_L", "Up")
        r = settle(frame, lambda r: approx(r, area, 4) and maximized())
        check("Super+Up from floating gives maximised area", approx(r, area, 4) and maximized(), r)
        tap("Super_L", "Down")
        r, mx = settle(lambda: (frame(), maximized()), lambda v: v[0] == g0 and not v[1])
        check("Super+Down restores from maximised exactly", r == g0 and not mx, f"got={r} want={g0} maximised={mx}")

        # 2. titlebar double-click maximises then restores
        tx, ty, _ = title_point(frame())
        move(tx, ty)
        for _ in range(2):
            button(True)
            button(False)
            time.sleep(0.05)
        r = settle(frame, lambda r: approx(r, area, 4))
        check("titlebar double-click maximises", maximized() and approx(r, area, 4), (r, sorted(states())))
        tx, ty, _ = title_point(frame())
        move(tx, ty)
        for _ in range(2):
            button(True)
            button(False)
            time.sleep(0.05)
        r = settle(frame, lambda r: r == g0)
        check("titlebar double-click restores exact geometry", r == g0 and not maximized(), f"got={r} want={g0}")

        # 3. drag to top-left corner: preview overlay while held, then release into the quarter
        fr = frame()
        tx, ty, _ = title_point(fr)
        move(tx, ty)
        button(True)
        time.sleep(0.05)
        drag_to(area[0] + 3, area[1] + 3)
        want_q = (area[0], area[1], area[2] // 2, area[3] // 2)
        settle(overlays, lambda o: any(approx(x, want_q, 6) for x in o))
        check("drag to corner shows quarter preview overlay", any(approx(x, want_q, 6) for x in overlays()), overlays())
        button(False)
        r = settle(frame, lambda r: approx(r, want_q, 4))
        check("release at corner snaps to top-left quarter", approx(r, want_q, 4), r)

        # 4. drag away from snapped window: restores size, keeps grab proportion
        fr = frame()
        tx, ty, _ = title_point(fr)
        frac = (tx - fr[0]) / fr[2]
        move(tx, ty)
        button(True)
        time.sleep(0.05)
        drag_to(700, 450)
        button(False)
        r = settle(frame, lambda r: r[2] == g0[2] and r[3] == g0[3])
        check("drag away restores pre-snap size", r[2] == g0[2] and r[3] == g0[3], f"got={r} want size={g0[2:]}")
        ptr = d.screen().root.query_pointer()
        got_frac = (ptr.root_x - r[0]) / r[2]
        check("drag away keeps proportional grab", abs(got_frac - frac) <= 0.05, f"frac {got_frac:.3f} want {frac:.3f}")
    finally:
        if wm is not None:
            wm.terminate()
            try:
                wm.wait(5)
            except subprocess.TimeoutExpired:
                wm.kill()
        xvfb.terminate()
        xvfb.wait(5)
    passed = sum(RESULTS)
    print(f"RESULT {passed}/{len(RESULTS)} passed", flush=True)
    return 0 if RESULTS and passed == len(RESULTS) else 1


if __name__ == "__main__":
    sys.exit(main())

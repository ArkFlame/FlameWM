#!/usr/bin/env python3
# Driver for scripts/break/wl_tiling_proof.sh: XTest input on the private Xvfb, nested compositor
# geometry from the client configure log (size, maximized) and the client's pixel box (position).
# Env from the launcher: DISPLAY, SOCK, RUN, W, H. Exit 0 only if every check passes.
import os
import subprocess
import sys
import time

from Xlib import X, XK, display
from Xlib.ext import xtest

RUN = os.environ["RUN"]
W, H = int(os.environ["W"]), int(os.environ["H"])
CLIENT_RGB = bytes.fromhex("E0A020")
LOG = os.path.join(RUN, "configure.log")
RESULTS = []


def check(name, ok, detail=""):
    RESULTS.append(bool(ok))
    print(("PASS " if ok else "FAIL ") + name + (" " + str(detail) if detail else ""), flush=True)


def settle(fn, pred, timeout=4.0):
    end = time.time() + timeout
    value = fn()
    while not pred(value) and time.time() < end:
        time.sleep(0.05)
        value = fn()
    return value


def configs():
    try:
        with open(LOG) as f:
            rows = [tuple(int(v) for v in line.split()) for line in f if line.strip()]
    except FileNotFoundError:
        return []
    return rows


def last():
    rows = configs()
    return rows[-1] if rows else None  # (w, h, maximized, activated)


def size_is(want, tol=4):
    def pred(c):
        return c is not None and abs(c[0] - want[0]) <= tol and abs(c[1] - want[1]) <= tol
    return pred


def pixel_box(img):
    """Bounding box (x0,y0,x1,y1) in image coords of CLIENT_RGB pixels, or None."""
    data, w, h = img
    stride = w * 3
    top = bottom = None
    left, right = w, -1
    for y in range(h):
        row = data[y * stride:(y + 1) * stride]
        i = row.find(CLIENT_RGB)
        while i != -1 and i % 3:
            i = row.find(CLIENT_RGB, i + 1)
        if i == -1:
            continue
        j = row.rfind(CLIENT_RGB)
        while j % 3:
            j = row.rfind(CLIENT_RGB, 0, j + 2)
        top = y if top is None else top
        bottom = y
        left = min(left, i // 3)
        right = max(right, j // 3)
    return None if top is None else (left, top, right, bottom)


def screenshot(path):
    subprocess.run(["import", "-depth", "8", "-window", "root", f"ppm:{path}"],
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False)
    with open(path, "rb") as f:
        raw = f.read()
    parts = raw.split(b"\n", 3)
    w, h = (int(v) for v in parts[1].split())
    return parts[3], w, h


def client_box(path):
    return pixel_box(screenshot(path))


def main():
    d = display.Display(os.environ["DISPLAY"])
    root = d.screen().root
    # Nested winit window: the root child with the requested size.
    nested = None
    end = time.time() + 20
    while nested is None and time.time() < end:
        for c in root.query_tree().children:
            g = c.get_geometry()
            if g.width == W and g.height == H and c.get_attributes().map_state == X.IsViewable:
                nested = c
                break
        time.sleep(0.2)
    if nested is None:
        print("FAIL nested compositor window not found", flush=True)
        return 1
    o = root.translate_coords(nested, 0, 0)
    ox, oy = o.x, o.y
    print(f"nested window origin=({ox},{oy}) size={W}x{H}", flush=True)

    def move(x, y):
        xtest.fake_input(d, X.MotionNotify, x=x, y=y)
        d.sync()

    def button(down):
        xtest.fake_input(d, X.ButtonPress if down else X.ButtonRelease, 1)
        d.sync()

    def tap(*names):
        # Without a WM the X focus is PointerRoot; pin it to the nested window so keys land there.
        nested.set_input_focus(X.RevertToParent, X.CurrentTime)
        d.sync()
        codes = [d.keysym_to_keycode(XK.string_to_keysym(n)) for n in names]
        for c in codes:
            xtest.fake_input(d, X.KeyPress, c)
        for c in reversed(codes):
            xtest.fake_input(d, X.KeyRelease, c)
        d.sync()

    def pointer():
        p = root.query_pointer()
        return p.root_x, p.root_y

    def drag_to(x, y, steps=12):
        x0, y0 = pointer()
        for i in range(1, steps + 1):
            move(x0 + (x - x0) * i // steps, y0 + (y - y0) * i // steps)
            time.sleep(0.03)

    def box():
        return client_box(os.path.join(RUN, "shot.ppm"))  # root-window pixels, root coords

    # Client mapped at the first configure (floating, not maximized).
    settle(last, lambda c: c is not None)
    move(ox + W // 2, oy + H // 2)
    time.sleep(0.5)
    c0 = settle(last, lambda c: c is not None)
    FLOAT = (c0[0], c0[1])  # the compositor's floating size: the restore target for every layout
    check("client starts floating (unmaximized)", c0[2] == 0, c0)
    b0 = settle(box, lambda b: b is not None)
    if b0 is None:
        print("FAIL client pixels not visible in nested output", flush=True)
        return 1
    print(f"initial client box={b0}", flush=True)

    def maxed(c):
        return c is not None and c[2] == 1

    # 1. keyboard tiling
    tap("Super_L", "Up")
    cm = settle(last, lambda c: maxed(c))
    # Frame model (model/geometry.rs): work = output, window margin 2 on outer edges and 1 inside,
    # client = frame minus the titlebar. Derive the titlebar height from the maximised configure.
    T = H - 4 - cm[1]
    LEFT = (W // 2 - 3, cm[1])
    QUAD = (W // 2 - 3, H // 2 - 3 - T)
    check("maximised client fills output minus margin and titlebar", size_is((W - 4, H - 4 - T), 0)(cm), cm)
    check("Super+Up maximises", maxed(cm), cm)
    tap("Super_L", "Down")
    cr = settle(last, lambda c: size_is(FLOAT)(c) and not maxed(c))
    check("Super+Down restores floating size exactly", size_is(FLOAT, 0)(cr) and not maxed(cr), cr)
    tap("Super_L", "Left")
    cl = settle(last, size_is(LEFT))
    check("Super+Left tiles left half", size_is(LEFT)(cl), cl)
    tap("Super_L", "Up")
    cq = settle(last, size_is(QUAD))
    check("Super+Up from left half gives top-left quarter", size_is(QUAD)(cq), cq)
    tap("Super_L", "Down")
    tap("Super_L", "Down")
    cr2 = settle(last, size_is(FLOAT, 0))
    check("Super+Down x2 restores floating size", size_is(FLOAT, 0)(cr2), cr2)

    # 2. titlebar double-click maximises then restores
    def dbl_click(x, y):
        move(x, y)
        for _ in range(2):
            button(True)
            button(False)
            time.sleep(0.05)

    b = box()
    dbl_click((b[0] + b[2]) // 2, b[1] - 2)
    cdm = settle(last, maxed)
    check("titlebar double-click maximises", maxed(cdm), cdm)
    b = box()
    dbl_click((b[0] + b[2]) // 2, b[1] - 2)
    cdr = settle(last, lambda c: size_is(FLOAT, 0)(c) and not maxed(c))
    check("titlebar double-click restores floating size", size_is(FLOAT, 0)(cdr) and not maxed(cdr), cdr)

    # 3. drag to top-left corner: release snaps to the quarter
    time.sleep(0.5)  # let the restore repaint before measuring the client box
    b = box()
    print(f"corner drag start box={b}", flush=True)
    move((b[0] + b[2]) // 2, b[1] - 2)
    button(True)
    time.sleep(0.05)
    drag_to(ox + 4, oy + 4)
    button(False)
    cc = settle(last, size_is(QUAD))
    check("corner drag release snaps to top-left quarter", size_is(QUAD)(cc), cc)
    snapped_left = settle(box, lambda bb: bb is not None and bb[0] <= 2)
    snapped_w = cc[0] if cc else 0

    # 4. drag away: size restores to floating and the grab keeps its proportion
    tx = (snapped_left[0] + snapped_left[2]) // 2 if snapped_left else 0
    frac = (tx - snapped_left[0]) / snapped_w if snapped_left and snapped_w else -1
    move(tx, snapped_left[1] - 2 if snapped_left else 0)
    button(True)
    time.sleep(0.05)
    drag_to(ox + 700, oy + 450)
    button(False)
    cu = settle(last, size_is(FLOAT, 0))
    check("drag away restores floating size", size_is(FLOAT, 0)(cu), cu)
    bu = settle(box, lambda bb: bb is not None and snapped_left is not None and abs(bb[0] - snapped_left[0]) > 4)
    got = (pointer()[0] - bu[0]) / cu[0] if bu else -1
    check("drag away keeps proportional grab", bu is not None and abs(got - frac) <= 0.05,
          f"frac {got:.3f} want {frac:.3f}")
    print(f"RESULT {sum(RESULTS)}/{len(RESULTS)} passed", flush=True)
    return 0 if RESULTS and all(RESULTS) else 1


if __name__ == "__main__":
    sys.exit(main())

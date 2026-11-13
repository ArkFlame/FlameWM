#!/usr/bin/env python3
# Driver for scripts/break/multi_output.sh: maximise on a debug virtual output, then remove that
# output and check the window migrated inside the primary output. Input is XTest on the private Xvfb;
# geometry comes from the client's configure log and its pixel box in the nested window.
# Env from the launcher: DISPLAY, RUN, W, H, VW, VH, FLAG (removal flag path). Exit 0 only if all PASS.
import os
import subprocess
import sys
import time

from Xlib import X, XK, display
from Xlib.ext import xtest

RUN = os.environ["RUN"]
W, H = int(os.environ["W"]), int(os.environ["H"])
VW, VH = int(os.environ["VW"]), int(os.environ["VH"])
FLAG = os.environ["FLAG"]
CLIENT_RGB = bytes.fromhex("E0A020")
LOG = os.path.join(RUN, "configure.log")
COMP_LOG = os.path.join(RUN, "comp.log")
RESULTS = []


def check(name, ok, detail=""):
    RESULTS.append(bool(ok))
    print(("PASS " if ok else "FAIL ") + name + (" " + str(detail) if detail else ""), flush=True)


def settle(fn, pred, timeout=5.0):
    end = time.time() + timeout
    value = fn()
    while not pred(value) and time.time() < end:
        time.sleep(0.05)
        value = fn()
    return value


def configs():
    try:
        with open(LOG) as f:
            return [tuple(int(v) for v in line.split()) for line in f if line.strip()]
    except FileNotFoundError:
        return []


def last():
    rows = configs()
    return rows[-1] if rows else None  # (w, h, maximized, activated)


def maxed(c):
    return c is not None and c[2] == 1


def comp_log():
    try:
        with open(COMP_LOG, errors="replace") as f:
            return f.read()
    except FileNotFoundError:
        return ""


def pixel_box(img):
    """Bounding box (x0,y0,x1,y1) of CLIENT_RGB pixels in image coords, or None."""
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


def root_pixels(path):
    subprocess.run(["import", "-depth", "8", "-window", "root", f"ppm:{path}"],
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False)
    with open(path, "rb") as f:
        raw = f.read()
    parts = raw.split(b"\n", 3)
    w, h = (int(v) for v in parts[1].split())
    return parts[3], w, h


def main():
    d = display.Display(os.environ["DISPLAY"])
    root = d.screen().root
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

    def tap(*names):
        nested.set_input_focus(X.RevertToParent, X.CurrentTime)
        d.sync()
        codes = [d.keysym_to_keycode(XK.string_to_keysym(n)) for n in names]
        for c in codes:
            xtest.fake_input(d, X.KeyPress, c)
        for c in reversed(codes):
            xtest.fake_input(d, X.KeyRelease, c)
        d.sync()

    def box():
        return pixel_box(root_pixels(os.path.join(RUN, "shot.ppm")))

    move(ox + W // 2, oy + H // 2)
    first = settle(last, lambda c: c is not None)
    check("client mapped on virtual output (configure received)", first is not None, first)
    check("virtual output registered", "virtual-1" in comp_log(), "comp.log")

    # Window is homed on virtual-1 (x>=1024), so the primary nested window must not show it.
    time.sleep(0.5)
    hidden = box() is None
    check("unmaximised client not drawn on primary output", hidden, box())

    # 1. Maximise on the second output: fits its work area (VW x VH, margin 2 on each outer edge).
    tap("Super_L", "Up")
    cm = settle(last, maxed)
    check("Super+Up maximises the client on the virtual output", maxed(cm), cm)
    check("maximised width fits virtual output work area",
          cm is not None and cm[0] == VW - 4, cm)
    check("maximised height fits virtual output work area",
          cm is not None and 0 < cm[1] <= VH - 4 - 20, cm)

    # 2. Remove the virtual output: the window must migrate inside the primary output.
    open(FLAG, "w").close()
    removed = settle(comp_log, lambda s: "virtual-1 removed" in s, timeout=10)
    check("compositor removed virtual-1 on flag file", "virtual-1 removed" in removed, "comp.log")
    time.sleep(0.5)
    cr = last()
    check("migrated client configure fits the primary output",
          cr is not None and cr[0] <= W and cr[1] <= H, cr)
    b = settle(box, lambda v: v is not None)
    inside = b is not None and 0 <= b[0] and b[2] < W and 0 <= b[1] and b[3] < H
    check("migrated client is drawn inside the primary output bounds", inside, b)

    ok = bool(RESULTS) and all(RESULTS)
    print("MULTI_OUTPUT_CHECKS", "PASS" if ok else "FAIL", f"{sum(RESULTS)}/{len(RESULTS)}", flush=True)
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())

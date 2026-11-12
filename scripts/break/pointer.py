#!/usr/bin/env python3
# XTest pointer driver for scripts/break (no xdotool on the host).
# Usage (DISPLAY must be the private sandbox display):
#   pointer.py sweep Y X0 X1 N SECONDS   triangular sweep, N motion events spread over SECONDS
#   pointer.py at X Y                    one absolute root-coordinate motion
import ctypes
import sys
import time

x11 = ctypes.CDLL("libX11.so.6")
xtst = ctypes.CDLL("libXtst.so.6")
x11.XOpenDisplay.restype = ctypes.c_void_p
x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
x11.XFlush.argtypes = [ctypes.c_void_p]
xtst.XTestFakeMotionEvent.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_ulong]


def main():
    if len(sys.argv) == 4 and sys.argv[1] == "at":
        x, y = int(sys.argv[2]), int(sys.argv[3])
        dpy = x11.XOpenDisplay(None)
        if not dpy:
            print("pointer: cannot open display", file=sys.stderr)
            return 2
        xtst.XTestFakeMotionEvent(dpy, -1, x, y, 0)
        x11.XFlush(dpy)
        return 0
    if len(sys.argv) != 7 or sys.argv[1] != "sweep":
        print(__doc__, file=sys.stderr)
        return 2
    y, x0, x1, n, secs = (int(v) for v in sys.argv[2:7])
    dpy = x11.XOpenDisplay(None)
    if not dpy:
        print("pointer: cannot open display", file=sys.stderr)
        return 2
    step = secs / max(n, 1)
    span = max(n - 1, 1)
    for i in range(n):
        t = 2 * i / span
        t = t if t <= 1 else 2 - t  # triangular: left to right and back
        xtst.XTestFakeMotionEvent(dpy, -1, int(x0 + (x1 - x0) * t), y, 0)
        x11.XFlush(dpy)
        time.sleep(step)
    print(f"pointer: sweep moves={n} y={y} x={x0}..{x1} secs={secs}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

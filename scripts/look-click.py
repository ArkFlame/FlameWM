#!/usr/bin/env python3
# Click at root-window coordinates on a private X display through the XTEST extension (ctypes, no Xlib).
# usage: python3 -I scripts/look-click.py <display> <x> <y>   (display must be a private Xvfb, >= :440)
import ctypes
import ctypes.util
import sys

display, x, y = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
if not display.startswith(":") or int(display[1:]) < 440:
    raise SystemExit("look-click: refuses displays below :440")
x11 = ctypes.CDLL(ctypes.util.find_library("X11"))
xtst = ctypes.CDLL(ctypes.util.find_library("Xtst"))
x11.XOpenDisplay.restype = ctypes.c_void_p
x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
dpy = x11.XOpenDisplay(display.encode())
if not dpy:
    raise SystemExit(f"look-click: cannot open {display}")
xtst.XTestFakeMotionEvent(ctypes.c_void_p(dpy), -1, x, y, 0)
xtst.XTestFakeButtonEvent(ctypes.c_void_p(dpy), 1, 1, 0)
xtst.XTestFakeButtonEvent(ctypes.c_void_p(dpy), 1, 0, 0)
x11.XFlush(ctypes.c_void_p(dpy))
x11.XCloseDisplay(ctypes.c_void_p(dpy))

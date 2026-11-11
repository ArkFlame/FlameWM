#!/usr/bin/env python3
# XTest driver that holds Alt and taps Tab, so the Alt+Tab switcher stays open for SECONDS.
# Usage: alt_hold.py X Y SECONDS   (moves the pointer to X,Y first: Xvfb without a WM focuses the
# window under the pointer). DISPLAY must be the private sandbox display. Releases Alt on exit.
import ctypes
import sys
import time

if len(sys.argv) != 4:
    print(__doc__, file=sys.stderr)
    sys.exit(2)
x, y, hold = int(sys.argv[1]), int(sys.argv[2]), float(sys.argv[3])

x11 = ctypes.CDLL("libX11.so.6")
xtst = ctypes.CDLL("libXtst.so.6")
x11.XOpenDisplay.restype = ctypes.c_void_p
x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
x11.XDefaultScreen.argtypes = [ctypes.c_void_p]
x11.XRootWindow.restype = ctypes.c_ulong
x11.XRootWindow.argtypes = [ctypes.c_void_p, ctypes.c_int]
x11.XQueryTree.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.POINTER(ctypes.c_ulong),
                           ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.POINTER(ctypes.c_ulong)),
                           ctypes.POINTER(ctypes.c_uint)]
x11.XGetGeometry.argtypes = [ctypes.c_void_p, ctypes.c_ulong] + [ctypes.c_void_p] * 7
x11.XSetInputFocus.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong]
x11.XFree.argtypes = [ctypes.c_void_p]
x11.XStringToKeysym.restype = ctypes.c_ulong
x11.XStringToKeysym.argtypes = [ctypes.c_char_p]
x11.XKeysymToKeycode.restype = ctypes.c_ubyte
x11.XKeysymToKeycode.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
x11.XFlush.argtypes = [ctypes.c_void_p]
xtst.XTestFakeKeyEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
xtst.XTestFakeMotionEvent.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_ulong]


def focus_largest_child(dpy):
    # Xvfb has no window manager, so nothing takes input focus. Give it to the largest top-level
    # window (the nested compositor's host window) so XTest keys reach it.
    screen = x11.XDefaultScreen(dpy)
    root = x11.XRootWindow(dpy, screen)
    kids = ctypes.POINTER(ctypes.c_ulong)()
    nkids = ctypes.c_uint()
    r, parent = ctypes.c_ulong(), ctypes.c_ulong()
    x11.XQueryTree(dpy, root, ctypes.byref(r), ctypes.byref(parent), ctypes.byref(kids), ctypes.byref(nkids))
    best, best_area = 0, -1
    for i in range(nkids.value):
        win = kids[i]
        rr = ctypes.c_ulong()
        gx, gy = ctypes.c_int(), ctypes.c_int()
        gw, gh, gb, gd = ctypes.c_uint(), ctypes.c_uint(), ctypes.c_uint(), ctypes.c_uint()
        if x11.XGetGeometry(dpy, win, ctypes.byref(rr), ctypes.byref(gx), ctypes.byref(gy),
                            ctypes.byref(gw), ctypes.byref(gh), ctypes.byref(gb), ctypes.byref(gd)):
            if gw.value * gh.value > best_area:
                best, best_area = win, gw.value * gh.value
    if kids:
        x11.XFree(kids)
    if best:
        x11.XSetInputFocus(dpy, best, 1, 0)  # RevertToPointerRoot, CurrentTime
        x11.XFlush(dpy)
    return best


def main():
    dpy = x11.XOpenDisplay(None)
    if not dpy:
        print("alt_hold: cannot open display", file=sys.stderr)
        return 2
    alt = x11.XKeysymToKeycode(dpy, x11.XStringToKeysym(b"Alt_L"))
    tab = x11.XKeysymToKeycode(dpy, x11.XStringToKeysym(b"Tab"))
    if not alt or not tab:
        print("alt_hold: no keycode for Alt_L or Tab", file=sys.stderr)
        return 2
    focus = focus_largest_child(dpy)
    print(f"alt_hold: input focus on window 0x{focus:x}")
    xtst.XTestFakeMotionEvent(dpy, x11.XDefaultScreen(dpy), x, y, 0)
    x11.XFlush(dpy)
    time.sleep(0.5)
    xtst.XTestFakeKeyEvent(dpy, alt, 1, 0)
    x11.XFlush(dpy)
    time.sleep(0.2)
    xtst.XTestFakeKeyEvent(dpy, tab, 1, 0)
    x11.XFlush(dpy)
    time.sleep(0.05)
    xtst.XTestFakeKeyEvent(dpy, tab, 0, 0)
    x11.XFlush(dpy)
    time.sleep(hold)
    xtst.XTestFakeKeyEvent(dpy, alt, 0, 0)
    x11.XFlush(dpy)
    print(f"alt_hold: Alt+Tab opened the switcher for {hold}s at {x},{y}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

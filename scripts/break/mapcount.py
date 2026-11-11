#!/usr/bin/env python3
# Map-event probe for scripts/break (break-b5). Counts MapNotify/UnmapNotify for direct children of
# the root window (SubstructureNotify on root) on the sandbox display, so label or popup flicker
# during a hover sweep shows up as map churn. Run with DISPLAY set to the private sandbox display.
# Usage: mapcount.py SECONDS    prints "ready" once selected, then "maps=N unmaps=M windows=K secs=S"
import ctypes
import sys
import time

MAP_NOTIFY = 19
UNMAP_NOTIFY = 18
SUBSTRUCTURE_NOTIFY_MASK = 1 << 19

x11 = ctypes.CDLL("libX11.so.6")
x11.XOpenDisplay.restype = ctypes.c_void_p
x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
x11.XDefaultRootWindow.restype = ctypes.c_ulong
x11.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
x11.XSelectInput.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_long]
x11.XPending.argtypes = [ctypes.c_void_p]
x11.XPending.restype = ctypes.c_int
x11.XNextEvent.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
x11.XFlush.argtypes = [ctypes.c_void_p]


def main():
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    secs = float(sys.argv[1])
    dpy = x11.XOpenDisplay(None)
    if not dpy:
        print("mapcount: cannot open display", file=sys.stderr)
        return 2
    root = x11.XDefaultRootWindow(dpy)
    x11.XSelectInput(dpy, root, SUBSTRUCTURE_NOTIFY_MASK)
    x11.XFlush(dpy)
    print("ready", flush=True)
    maps = unmaps = 0
    windows = set()
    buf = ctypes.create_string_buffer(256)
    # XMapEvent / XUnmapEvent on LP64: type@0, serial@8, send_event@16, display@24, event@32, window@40.
    start = time.monotonic()
    while time.monotonic() - start < secs:
        if not x11.XPending(dpy):
            time.sleep(0.01)
            continue
        x11.XNextEvent(dpy, buf)
        etype = ctypes.c_int.from_buffer(buf).value
        if etype not in (MAP_NOTIFY, UNMAP_NOTIFY):
            continue
        win = ctypes.c_ulong.from_buffer(buf, 40).value
        windows.add(win)
        if etype == MAP_NOTIFY:
            maps += 1
        else:
            unmaps += 1
    print(f"maps={maps} unmaps={unmaps} windows={len(windows)} secs={secs:g}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

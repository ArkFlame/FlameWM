#!/usr/bin/env python3
# Minimal X client for scripts/break focus/urgency checks (no xdotool). DISPLAY must be the sandbox display.
# Usage: xclient.py SECONDS [none]   maps window "flamewm-probe", sends _NET_ACTIVE_WINDOW (source 1, no
#   timestamp, i.e. an unsolicited app request with no user input) to the root, then holds the window for
#   SECONDS. With "none" the request is skipped (control: map-only focus).
#   Prints "window=0x... requested" once the request is flushed; the caller inspects the WM with xprop.
import ctypes
import sys
import time

x11 = ctypes.CDLL("libX11.so.6")
c_ulong, c_int, c_void_p = ctypes.c_ulong, ctypes.c_int, ctypes.c_void_p
x11.XOpenDisplay.restype = c_void_p
x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
x11.XDefaultRootWindow.restype = c_ulong
x11.XDefaultRootWindow.argtypes = [c_void_p]
x11.XCreateSimpleWindow.restype = c_ulong
x11.XCreateSimpleWindow.argtypes = [c_void_p, c_ulong, c_int, c_int, ctypes.c_uint, ctypes.c_uint,
                                    ctypes.c_uint, c_ulong, c_ulong]
x11.XStoreName.argtypes = [c_void_p, c_ulong, ctypes.c_char_p]
x11.XMapWindow.argtypes = [c_void_p, c_ulong]
x11.XInternAtom.restype = c_ulong
x11.XInternAtom.argtypes = [c_void_p, ctypes.c_char_p, c_int]
x11.XSendEvent.argtypes = [c_void_p, c_ulong, c_int, ctypes.c_long, ctypes.c_void_p]
x11.XFlush.argtypes = [c_void_p]
x11.XSync.argtypes = [c_void_p, c_int]


def main():
    if len(sys.argv) not in (2, 3):
        print(__doc__, file=sys.stderr)
        return 2
    secs = float(sys.argv[1])
    send = len(sys.argv) == 2
    dpy = x11.XOpenDisplay(None)
    if not dpy:
        print("xclient: cannot open display", file=sys.stderr)
        return 2
    root = x11.XDefaultRootWindow(dpy)
    win = x11.XCreateSimpleWindow(dpy, root, 100, 100, 300, 200, 0, 0, 0x202020)
    x11.XStoreName(dpy, win, b"flamewm-probe")
    x11.XMapWindow(dpy, win)
    x11.XSync(dpy, 0)
    time.sleep(0.6)  # let the WM manage the window before the request; inside the 2 s recent-input window
    net_active = x11.XInternAtom(dpy, b"_NET_ACTIVE_WINDOW", 0)
    # XClientMessageEvent on LP64: type@0 serial@8 send_event@16 display@24 window@32
    # message_type@40 format@48 data.l[0..4]@56.. ; source=1 (application), timestamp=0, active=0.
    ev = ctypes.create_string_buffer(192)
    ctypes.c_int.from_buffer(ev, 0).value = 33  # ClientMessage
    ctypes.c_int.from_buffer(ev, 16).value = 1  # send_event
    ctypes.c_void_p.from_buffer(ev, 24).value = dpy
    ctypes.c_ulong.from_buffer(ev, 32).value = win
    ctypes.c_ulong.from_buffer(ev, 40).value = net_active
    ctypes.c_int.from_buffer(ev, 48).value = 32
    ctypes.c_long.from_buffer(ev, 56).value = 1
    # SubstructureRedirectMask | SubstructureNotifyMask on the root, as EWMH clients send.
    if send:
        x11.XSendEvent(dpy, root, 0, (1 << 19) | (1 << 20), ev)
    x11.XFlush(dpy)
    print(f"window={win:#x} {'requested' if send else 'mapped-only'}", flush=True)
    time.sleep(secs)
    return 0


if __name__ == "__main__":
    sys.exit(main())

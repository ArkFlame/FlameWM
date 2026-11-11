#!/usr/bin/env python3
# XTest key driver for scripts/break (no xdotool on the host). DISPLAY must be the sandbox display.
# Usage: key.py CHORD [CHORD...]   each CHORD is keysyms joined by '+': key.py Super_L+space
#   Modifiers are pressed first, the last key is tapped, then the modifiers are released.
import ctypes
import sys
import time

x11 = ctypes.CDLL("libX11.so.6")
xtst = ctypes.CDLL("libXtst.so.6")
x11.XOpenDisplay.restype = ctypes.c_void_p
x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
x11.XStringToKeysym.restype = ctypes.c_ulong
x11.XStringToKeysym.argtypes = [ctypes.c_char_p]
x11.XKeysymToKeycode.restype = ctypes.c_ubyte
x11.XKeysymToKeycode.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
x11.XFlush.argtypes = [ctypes.c_void_p]
xtst.XTestFakeKeyEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]


def press(dpy, code, down):
    xtst.XTestFakeKeyEvent(dpy, code, 1 if down else 0, 0)
    x11.XFlush(dpy)


def main():
    if len(sys.argv) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    dpy = x11.XOpenDisplay(None)
    if not dpy:
        print("key: cannot open display", file=sys.stderr)
        return 2
    for chord in sys.argv[1:]:
        codes = []
        for name in chord.split("+"):
            sym = x11.XStringToKeysym(name.encode())
            code = x11.XKeysymToKeycode(dpy, sym) if sym else 0
            if not code:
                print(f"key: no keycode for {name}", file=sys.stderr)
                return 2
            codes.append(code)
        for code in codes[:-1]:
            press(dpy, code, True)
        time.sleep(0.05)
        press(dpy, codes[-1], True)
        time.sleep(0.05)
        press(dpy, codes[-1], False)
        for code in reversed(codes[:-1]):
            press(dpy, code, False)
        time.sleep(0.15)
    print(f"key: pressed {' '.join(sys.argv[1:])}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

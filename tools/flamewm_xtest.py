#!/usr/bin/env python3
"""Send explicitly requested input to a verified nested Xephyr display."""

from __future__ import annotations

import argparse
import ctypes
import ctypes.util
import re
import sys
import time
from pathlib import Path


DISPLAY_RE = re.compile(r"^:(?:9[7-9]|1[01][0-9])(?:\.\d+)?$")

MODIFIER_ALIASES = {
    "ctrl": "ctrl",
    "control": "ctrl",
    "shift": "shift",
    "alt": "alt",
    "super": "super",
    "meta": "super",
    "win": "super",
}
MODIFIER_KEYSYMS = {"ctrl": "Control_L", "shift": "Shift_L", "alt": "Alt_L", "super": "Super_L"}


def parse_chord(text: str) -> tuple[list[str], str]:
    """Split 'mod+mod+KEY' into canonical modifier names and the target keysym name.

    Raises ValueError for empty tokens, unknown or duplicate modifiers, and a
    missing target key. Performs no X11 access.
    """
    parts = [part.strip() for part in text.split("+")]
    if any(not part for part in parts):
        raise ValueError(f"empty token in chord {text!r}")
    *names, key = parts
    modifiers: list[str] = []
    for name in names:
        canonical = MODIFIER_ALIASES.get(name.lower())
        if canonical is None:
            raise ValueError(f"unknown modifier {name!r}")
        if canonical in modifiers:
            raise ValueError(f"duplicate modifier {name!r}")
        modifiers.append(canonical)
    if MODIFIER_ALIASES.get(key.lower()) is not None:
        raise ValueError(f"chord {text!r} is missing a target key after its modifiers")
    return modifiers, key


MAX_HOLD_MS = 60000


class UnmappedKeysym(RuntimeError):
    """Raised when a keysym name has no keycode on the nested display."""


def parse_hold(text: str) -> tuple[str, int]:
    """Split 'KEYSYM:MS' into the keysym name and a hold duration in milliseconds.

    Raises ValueError for a missing separator, an empty keysym, or a duration that is not
    an integer in 1..MAX_HOLD_MS. Performs no X11 access.
    """
    name, separator, duration = text.partition(":")
    if not separator or not name.strip():
        raise ValueError(f"expected KEYSYM:MS, got {text!r}")
    if not re.fullmatch(r"\d+", duration):
        raise ValueError(f"hold duration must be a non-negative integer of milliseconds, got {duration!r}")
    milliseconds = int(duration)
    if not 1 <= milliseconds <= MAX_HOLD_MS:
        raise ValueError(f"hold duration must be 1..{MAX_HOLD_MS} ms, got {milliseconds}")
    return name.strip(), milliseconds


class X11Backend:
    """Keycode and XTEST key adapter over an already opened, ownership-checked display."""

    def __init__(self, x11, xtst, display) -> None:
        self._x11, self._xtst, self._display = x11, xtst, display

    def keycode(self, keysym: str) -> int:
        symbol = self._x11.XStringToKeysym(keysym.encode())
        return int(self._x11.XKeysymToKeycode(self._display, symbol)) if symbol else 0

    def fake_key(self, code: int, down: bool) -> bool:
        return bool(self._xtst.XTestFakeKeyEvent(self._display, code, 1 if down else 0, 0))

    def fake_button(self, button: int, down: bool) -> bool:
        return bool(self._xtst.XTestFakeButtonEvent(self._display, button, 1 if down else 0, 0))

    def fake_motion(self, x: int, y: int) -> bool:
        return bool(self._xtst.XTestFakeMotionEvent(self._display, -1, x, y, 0))

    def query_keymap(self) -> bytes:
        """Read-only XQueryKeymap snapshot: 32 bytes, one bit per keycode (no events sent)."""
        buffer = (ctypes.c_char * 32)()
        self._x11.XQueryKeymap(self._display, buffer)
        return bytes(buffer)


HELD_MODIFIER_KEYSYMS = ("Shift_L", "Shift_R", "Control_L", "Control_R", "Alt_L", "Alt_R", "Super_L", "Super_R", "Meta")


def modifiers_from_keymap(backend, keymap: bytes) -> list[str]:
    """Return the HELD_MODIFIER_KEYSYMS whose keycode bit is set in KEYMAP. Performs no X11 writes."""
    held = []
    for name in HELD_MODIFIER_KEYSYMS:
        code = backend.keycode(name)
        if code and keymap[code // 8] >> (code % 8) & 1:
            held.append(name)
    return held


def release_stuck_modifiers(backend) -> list[str]:
    """Send one XTEST key release for each modifier the keymap reports down; return the names released."""
    released = modifiers_from_keymap(backend, backend.query_keymap())
    for name in released:
        if not backend.fake_key(backend.keycode(name), False):
            raise RuntimeError(f"XTEST release rejected for stuck modifier {name!r}")
    return released


def press_key(backend, keysym: str, down: bool) -> None:
    """Send exactly one key press (down=True) or release (down=False) for KEYSYM."""
    code = backend.keycode(keysym)
    if not code:
        raise UnmappedKeysym(f"unknown/unmapped keysym {keysym!r}")
    if not backend.fake_key(code, down):
        raise RuntimeError(f"XTEST key {'press' if down else 'release'} rejected for {keysym!r}")


def held_key_events(backend, keysym: str, hold_ms: int, sleep=time.sleep) -> None:
    """Press KEYSYM, keep it down for hold_ms milliseconds, then release it.

    A rejected press sends no release and no sleep. Once the press succeeds, the release is
    always attempted, even when the hold is interrupted.
    """
    press_key(backend, keysym, down=True)
    try:
        sleep(hold_ms / 1000.0)
    finally:
        press_key(backend, keysym, down=False)


WHEEL_BUTTONS = {"up": 4, "down": 5}
MAX_WHEEL_STEPS = 100


def parse_wheel(text: str) -> tuple[int, int]:
    """Split 'up|down[:N]' into the X wheel button (4 up, 5 down) and a repeat count.

    N defaults to 1 and must be an integer in 1..MAX_WHEEL_STEPS. Raises ValueError otherwise.
    Performs no X11 access.
    """
    direction, separator, count = text.partition(":")
    button = WHEEL_BUTTONS.get(direction)
    if button is None:
        raise ValueError(f"expected up|down[:N], got {text!r}")
    if not separator:
        return button, 1
    if not re.fullmatch(r"\d+", count):
        raise ValueError(f"wheel steps must be a non-negative integer, got {count!r}")
    steps = int(count)
    if not 1 <= steps <= MAX_WHEEL_STEPS:
        raise ValueError(f"wheel steps must be 1..{MAX_WHEEL_STEPS}, got {steps}")
    return button, steps


def wheel_clicks(backend, button: int, steps: int) -> None:
    """Send STEPS press+release pairs for wheel BUTTON, stopping at the first rejected event."""
    for _ in range(steps):
        if not backend.fake_button(button, True):
            raise RuntimeError(f"XTEST wheel press rejected for button {button}")
        if not backend.fake_button(button, False):
            raise RuntimeError(f"XTEST wheel release rejected for button {button}")


MAX_DRAG_STEPS = 200
DEFAULT_DRAG_STEPS = 10
DEFAULT_DRAG_HOLD_MS = 300


def parse_drag(text: str) -> tuple[tuple[int, int], tuple[int, int], int, int]:
    """Split 'X1,Y1,X2,Y2[:STEPS[:HOLD_MS]]' into start, end, move steps and final hold in ms.

    STEPS is 1..MAX_DRAG_STEPS (default 10); HOLD_MS is 0..MAX_HOLD_MS (default 300). Raises ValueError
    otherwise. Performs no X11 access.
    """
    head, *rest = text.split(":")
    match = re.fullmatch(r"(\d+),(\d+),(\d+),(\d+)", head)
    if not match or len(rest) > 2 or any(not re.fullmatch(r"\d+", part) for part in rest):
        raise ValueError(f"expected X1,Y1,X2,Y2[:STEPS[:HOLD_MS]], got {text!r}")
    x1, y1, x2, y2 = (int(part) for part in match.groups())
    steps = int(rest[0]) if rest else DEFAULT_DRAG_STEPS
    hold_ms = int(rest[1]) if len(rest) == 2 else DEFAULT_DRAG_HOLD_MS
    if not 1 <= steps <= MAX_DRAG_STEPS:
        raise ValueError(f"drag steps must be 1..{MAX_DRAG_STEPS}, got {steps}")
    if hold_ms > MAX_HOLD_MS:
        raise ValueError(f"drag hold must be 0..{MAX_HOLD_MS} ms, got {hold_ms}")
    return (x1, y1), (x2, y2), steps, hold_ms


def drag_events(backend, start, end, steps: int, hold_ms: int, release: bool = True, sleep=time.sleep) -> None:
    """Move to START, press button 1, move to END in STEPS equal increments, hold, then optionally release.

    A rejected event aborts the sequence. Once the press succeeded and release is requested, the release
    is attempted even when a later motion is rejected or the hold is interrupted.
    """
    if not backend.fake_motion(*start):
        raise RuntimeError("XTEST drag start motion rejected")
    if not backend.fake_button(1, True):
        raise RuntimeError("XTEST drag press rejected")
    try:
        for index in range(1, steps + 1):
            x = start[0] + (end[0] - start[0]) * index // steps
            y = start[1] + (end[1] - start[1]) * index // steps
            if not backend.fake_motion(x, y):
                raise RuntimeError(f"XTEST drag motion {index}/{steps} rejected")
            sleep(0.02)
        sleep(hold_ms / 1000.0)
    finally:
        if release and not backend.fake_button(1, False):
            raise RuntimeError("XTEST drag release rejected")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--display", required=True, help="nested display in :97..:119 only")
    parser.add_argument("--owner-token", required=True, help="invocation-specific Xephyr ownership token")
    parser.add_argument("--owner-marker", required=True, help="private marker written by owning Xephyr harness")
    parser.add_argument("--click", metavar="X,Y", help="move pointer and click button 1 at coordinates")
    parser.add_argument("--move", metavar="X,Y", help="move pointer without a button event")
    parser.add_argument("--key", metavar="KEYSYM", help="press and release an X keysym")
    parser.add_argument("--key-down", metavar="KEYSYM", help="press KEYSYM and leave it down until --key-up")
    parser.add_argument("--key-up", metavar="KEYSYM", help="release a KEYSYM previously left down by --key-down")
    parser.add_argument("--hold", metavar="KEYSYM:MS", help="press KEYSYM, hold it for MS milliseconds, then release")
    parser.add_argument(
        "--chord",
        metavar="MOD+...+KEY",
        help="hold modifiers (ctrl, shift, alt, super/meta/win), press and release KEY, then release modifiers",
    )
    parser.add_argument("--wheel", metavar="up|down[:N]", help="scroll wheel up (button 4) or down (button 5), N steps (default 1)")
    parser.add_argument("--drag", metavar="X1,Y1,X2,Y2[:STEPS[:HOLD_MS]]",
                        help="press button 1 at X1,Y1, move to X2,Y2 in STEPS increments, hold HOLD_MS, then release")
    parser.add_argument("--drag-keep", action="store_true", help="with --drag: leave button 1 down (end with --release)")
    parser.add_argument("--release", action="store_true", help="release button 1 left down by --drag --drag-keep")
    parser.add_argument("--query-modifiers", action="store_true",
                        help="read-only: print the XQueryKeymap snapshot and held modifiers (exclusive)")
    parser.add_argument("--release-modifiers", action="store_true",
                        help="release each modifier XQueryKeymap reports down, then print the snapshot (exclusive)")
    args = parser.parse_args()
    if not DISPLAY_RE.fullmatch(args.display):
        parser.error("--display must be a nested display in :97..:119")
    if not re.fullmatch(r"[0-9a-f]{64}", args.owner_token):
        parser.error("--owner-token must be a 256-bit hexadecimal token")
    input_actions = (args.click or args.move or args.key or args.chord or args.key_down or args.key_up or args.hold
                     or args.wheel or args.drag or args.release)
    modifier_actions = args.query_modifiers or args.release_modifiers
    if not (input_actions or modifier_actions):
        parser.error("request at least one of --click, --move, --key, --chord, --key-down, --key-up, --hold, "
                     "--wheel, --drag, --release, --query-modifiers or --release-modifiers")
    if modifier_actions and input_actions:
        parser.error("--query-modifiers and --release-modifiers cannot be combined with input actions")
    if args.drag_keep and not args.drag:
        parser.error("--drag-keep needs --drag")
    if args.drag:
        try:
            args.drag_spec = parse_drag(args.drag)
        except ValueError as error:
            parser.error(f"--drag: {error}")
    if args.hold:
        try:
            args.hold_spec = parse_hold(args.hold)
        except ValueError as error:
            parser.error(f"--hold: {error}")
    if args.wheel:
        try:
            args.wheel_spec = parse_wheel(args.wheel)
        except ValueError as error:
            parser.error(f"--wheel: {error}")
    if args.click:
        match = re.fullmatch(r"(\d+),(\d+)", args.click)
        if not match:
            parser.error("--click must be X,Y non-negative integers")
        args.click_xy = tuple(map(int, match.groups()))
    if args.move:
        match = re.fullmatch(r"(\d+),(\d+)", args.move)
        if not match:
            parser.error("--move must be X,Y non-negative integers")
        args.move_xy = tuple(map(int, match.groups()))
    if args.chord:
        try:
            args.chord_parts = parse_chord(args.chord)
        except ValueError as error:
            parser.error(f"--chord: {error}")
    return args


def main() -> int:
    args = parse_args()
    try:
        marker = Path(args.owner_marker).read_text(encoding="ascii").splitlines()
    except OSError as error:
        print(f"INPUT_BLOCKED: Xephyr ownership marker unavailable: {error}", file=sys.stderr)
        return 3
    if marker != [args.display, args.owner_token]:
        print("INPUT_BLOCKED: Xephyr ownership marker does not match requested display/token", file=sys.stderr)
        return 3
    x11_path, xtst_path = ctypes.util.find_library("X11"), ctypes.util.find_library("Xtst")
    if not x11_path or not xtst_path:
        print("INPUT_BLOCKED: libX11/libXtst unavailable", file=sys.stderr)
        return 3
    x11, xtst = ctypes.CDLL(x11_path), ctypes.CDLL(xtst_path)
    x11.XOpenDisplay.argtypes, x11.XOpenDisplay.restype = [ctypes.c_char_p], ctypes.c_void_p
    x11.XServerVendor.argtypes, x11.XServerVendor.restype = [ctypes.c_void_p], ctypes.c_char_p
    x11.XCloseDisplay.argtypes = [ctypes.c_void_p]
    display = x11.XOpenDisplay(args.display.encode())
    if not display:
        print(f"INPUT_BLOCKED: cannot open nested display {args.display}", file=sys.stderr)
        return 3
    try:
        vendor = x11.XServerVendor(display)
        if not vendor:
            print(f"INPUT_BLOCKED: server vendor is not Xephyr: {vendor!r}", file=sys.stderr)
            return 3
        xtst.XTestFakeMotionEvent.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_ulong]
        xtst.XTestFakeButtonEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
        xtst.XTestFakeKeyEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
        x11.XStringToKeysym.argtypes, x11.XStringToKeysym.restype = [ctypes.c_char_p], ctypes.c_ulong
        x11.XKeysymToKeycode.argtypes, x11.XKeysymToKeycode.restype = [ctypes.c_void_p, ctypes.c_ulong], ctypes.c_ubyte
        x11.XQueryKeymap.argtypes = [ctypes.c_void_p, ctypes.c_char * 32]
        x11.XSync.argtypes = [ctypes.c_void_p, ctypes.c_int]
        if args.click:
            x, y = args.click_xy
            if not xtst.XTestFakeMotionEvent(display, -1, x, y, 0) or not xtst.XTestFakeButtonEvent(display, 1, 1, 0) or not xtst.XTestFakeButtonEvent(display, 1, 0, 0):
                raise RuntimeError("XTEST pointer event was rejected")
        if args.move:
            x, y = args.move_xy
            if not xtst.XTestFakeMotionEvent(display, -1, x, y, 0):
                raise RuntimeError("XTEST pointer motion was rejected")
        if args.key:
            symbol = x11.XStringToKeysym(args.key.encode())
            code = x11.XKeysymToKeycode(display, symbol) if symbol else 0
            if not code:
                print(f"INPUT_BLOCKED: unknown/unmapped keysym {args.key!r}", file=sys.stderr)
                return 2
            if not xtst.XTestFakeKeyEvent(display, code, 1, 0) or not xtst.XTestFakeKeyEvent(display, code, 0, 0):
                raise RuntimeError("XTEST key event was rejected")
        if args.chord:
            modifiers, target = args.chord_parts
            pressed: list[int] = []
            try:
                for name in [MODIFIER_KEYSYMS[mod] for mod in modifiers] + [target]:
                    symbol = x11.XStringToKeysym(name.encode())
                    code = x11.XKeysymToKeycode(display, symbol) if symbol else 0
                    if not code:
                        print(f"INPUT_BLOCKED: unknown/unmapped keysym {name!r} in chord {args.chord!r}", file=sys.stderr)
                        return 2
                    if not xtst.XTestFakeKeyEvent(display, code, 1, 0):
                        raise RuntimeError(f"XTEST key press rejected for {name!r}")
                    pressed.append(code)
                if not xtst.XTestFakeKeyEvent(display, pressed[-1], 0, 0):
                    raise RuntimeError("XTEST key release rejected for chord target")
                pressed.pop()
            finally:
                for code in reversed(pressed):
                    xtst.XTestFakeKeyEvent(display, code, 0, 0)
        backend = X11Backend(x11, xtst, display)
        if args.query_modifiers or args.release_modifiers:
            if args.release_modifiers:
                released = release_stuck_modifiers(backend)
                x11.XSync(display, 0)
                print(f"MODIFIERS_RELEASED: {','.join(released) or 'none'}")
            keymap = backend.query_keymap()
            held = modifiers_from_keymap(backend, keymap)
            print(f"KEYMAP_SNAPSHOT: {keymap.hex()} HELD_MODIFIERS: {','.join(held) or 'none'}")
            return 0
        if args.key_down:
            press_key(backend, args.key_down, down=True)
        if args.key_up:
            press_key(backend, args.key_up, down=False)
        if args.hold:
            held_key_events(backend, *args.hold_spec)
        if args.wheel:
            wheel_clicks(backend, *args.wheel_spec)
        if args.drag:
            drag_events(backend, *args.drag_spec, release=not args.drag_keep)
        if args.release and not backend.fake_button(1, False):
            raise RuntimeError("XTEST button release rejected")
        x11.XSync.argtypes = [ctypes.c_void_p, ctypes.c_int]
        x11.XSync(display, 0)
        print(
            f"INPUT_SENT_UNVERIFIED: display={args.display} click={args.click or 'none'} "
            f"key={args.key or 'none'} chord={args.chord or 'none'} "
            f"key_down={args.key_down or 'none'} key_up={args.key_up or 'none'} hold={args.hold or 'none'} "
            f"wheel={args.wheel or 'none'} drag={args.drag or 'none'}"
        )
        return 0
    except UnmappedKeysym as error:
        print(f"INPUT_BLOCKED: {error}", file=sys.stderr)
        return 2
    except Exception as error:
        print(f"INPUT_FAIL: {error}", file=sys.stderr)
        return 1
    finally:
        x11.XCloseDisplay(display)


if __name__ == "__main__":
    raise SystemExit(main())

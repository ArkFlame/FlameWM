#!/usr/bin/env python3
"""Flicker sampler for window transitions (minimize, restore, maximize, unmaximize, open).

Each sample labels the window region of the root screenshot as one of:
  client      the real client occupies the static window rect in full
  overlay     the transition overlay is visible (pixels present, not the static client rect)
  background  neither the client nor the overlay paints the region

A flicker is a sample that is not what its neighbours say it is:
  * a client or background sample strictly between two overlay samples, or
  * a background sample strictly between two non-background samples.
Expected start and end states (client before the first overlay, client after the last) are not flicker.

Live use (fresh isolated stack only, never a user's display):
  python3 -I tools/flamewm_flicker_acceptance.py --display :123 --rect X,Y,W,H --ink R,G,B \
      --bg R,G,B --seconds 2 --interval-ms 2
"""

from __future__ import annotations

import argparse
import ctypes
import ctypes.util
import sys
import time
from dataclasses import dataclass

CLIENT = "client"
OVERLAY = "overlay"
BACKGROUND = "background"
LABELS = (CLIENT, OVERLAY, BACKGROUND)

# Labels for a moved window (settle after a move): see `classify_move`.
HIDDEN = "hidden"
STALE = "stale"
MOVING = "moving"


def count_hidden_frames(labels: list[str]) -> int:
    """Samples where the window was expected on screen but nothing of it was found (A01 hide)."""
    return sum(1 for label in labels if label == HIDDEN)


def count_afterimage_frames(labels: list[str]) -> int:
    """Samples that show the window only where it used to be (a leftover finish overlay)."""
    return sum(1 for label in labels if label == STALE)


def _box_at(box, rect, size, tolerance: int) -> bool:
    x0, y0, x1, y1 = box
    rx, ry, rw, rh = rect
    cw, ch = size
    return (
        abs(x0 - rx) <= tolerance
        and abs(y0 - ry) <= tolerance
        and abs((x1 - x0 + 1) - (rw or cw)) <= tolerance
        and abs((y1 - y0 + 1) - (rh or ch)) <= tolerance
    )


def classify_move(ink_box, client_size, old_rect, target_rect, tolerance: int = 2) -> str:
    """Label one root frame sampled around a moved window.

    `ink_box` is the ink bounding box `(x0, y0, x1, y1)` inside the sampled region, or ``None``
    when the region holds no ink at all. `old_rect`/`target_rect` are the client rects at the move
    start and at the release, both relative to the same region, as `(x, y, w, h)`.

    hidden: the window is nowhere in the region, i.e. it was taken off the root (the A01 defect
            after a move used to hide the frame to read the root behind it).
    stale:  the window is only at the old rect, i.e. a finished transition overlay still covers the
            frame at its previous position.
    client: the ink box is the client at its target rect.
    moving: anything else (the window is still travelling between the two rects).
    """
    if ink_box is None:
        return HIDDEN
    if _box_at(ink_box, old_rect, client_size, tolerance) and not _box_at(
        ink_box, target_rect, client_size, tolerance
    ):
        return STALE
    if _box_at(ink_box, target_rect, client_size, tolerance):
        return CLIENT
    return MOVING


def count_flicker(labels: list[str]) -> int:
    """Count samples that contradict their neighbours (see module docstring)."""
    for label in labels:
        if label not in LABELS:
            raise ValueError(f"unknown label: {label!r}")
    first_overlay = _first_index(labels, OVERLAY)
    last_overlay = _last_index(labels, OVERLAY)
    flagged: set[int] = set()
    if first_overlay is not None and last_overlay is not None:
        flagged.update(i for i in range(first_overlay + 1, last_overlay) if labels[i] != OVERLAY)
    first_solid = _first_index(labels, lambda value: value != BACKGROUND)
    last_solid = _last_index(labels, lambda value: value != BACKGROUND)
    if first_solid is not None and last_solid is not None:
        flagged.update(
            i for i in range(first_solid + 1, last_solid) if labels[i] == BACKGROUND
        )
    return len(flagged)


def _first_index(labels, match):
    test = match if callable(match) else (lambda value: value == match)
    for index, value in enumerate(labels):
        if test(value):
            return index
    return None


def _last_index(labels, match):
    test = match if callable(match) else (lambda value: value == match)
    for index in range(len(labels) - 1, -1, -1):
        if test(labels[index]):
            return index
    return None


def classify_argb(argb: bytes, width: int, height: int, client_rect_pixels: tuple[int, int, int, int],
                  ink: tuple[int, int, int], background: tuple[int, int, int]) -> str:
    """Label one BGRX region capture. `ink` is the client's solid fill; `background` the root colour.

    `client_rect_pixels` is the static client rect inside the captured region (x, y, w, h). The
    client label needs ink covering that whole rect. Any other ink is overlay.
    """
    ink_b, ink_g, ink_r = ink[2], ink[1], ink[0]
    bg_b, bg_g, bg_r = background[2], background[1], background[0]
    cx, cy, cw, ch = client_rect_pixels
    stride = width * 4
    inked_in_rect = 0
    inked_outside = 0
    for y in range(height):
        row = y * stride
        for x in range(width):
            offset = row + x * 4
            pixel = (argb[offset], argb[offset + 1], argb[offset + 2])
            if pixel == (ink_b, ink_g, ink_r):
                if cx <= x < cx + cw and cy <= y < cy + ch:
                    inked_in_rect += 1
                else:
                    inked_outside += 1
    if inked_in_rect == cw * ch and inked_outside == 0:
        return CLIENT
    if inked_outside > 0 or inked_in_rect > 0:
        return OVERLAY
    return BACKGROUND


class XImageSampler:
    """Reads the root window region with XGetImage via ctypes. Linux/Xlib only."""

    ZPIXMAP = 2
    ALL_PLANES = ~0

    def __init__(self, display_name: str) -> None:
        path = ctypes.util.find_library("X11")
        if not path:
            raise RuntimeError("libX11 not found")
        self.xlib = ctypes.cdll.LoadLibrary(path)
        self.xlib.XOpenDisplay.restype = ctypes.c_void_p
        self.xlib.XOpenDisplay.argtypes = [ctypes.c_char_p]
        self.xlib.XDefaultRootWindow.restype = ctypes.c_ulong
        self.xlib.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
        self.xlib.XGetImage.restype = ctypes.c_void_p
        self.xlib.XGetImage.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_int,
                                        ctypes.c_uint, ctypes.c_uint, ctypes.c_ulong, ctypes.c_int]
        self.xlib.XDestroyImage.argtypes = [ctypes.c_void_p]
        self.display = self.xlib.XOpenDisplay(display_name.encode())
        if not self.display:
            raise RuntimeError(f"cannot open display {display_name}")
        self.root = self.xlib.XDefaultRootWindow(self.display)

    def capture(self, x: int, y: int, width: int, height: int) -> bytes:
        image = self.xlib.XGetImage(self.display, self.root, x, y, width, height,
                                    self.ALL_PLANES, self.ZPIXMAP)
        if not image:
            raise RuntimeError("XGetImage failed")
        try:
            # struct _XImage: int width, height, xoffset, format; char *data; int byte_order,
            # bitmap_unit, bitmap_bit_order, bitmap_pad; int depth; int bytes_per_line; int bits_per_pixel
            data_ptr = ctypes.c_void_p.from_address(image + 16).value
            bytes_per_line = ctypes.c_int.from_address(image + 44).value
            return ctypes.string_at(data_ptr, bytes_per_line * height)
        finally:
            self.xlib.XDestroyImage(image)


@dataclass(frozen=True)
class SampleSpec:
    x: int
    y: int
    width: int
    height: int
    client_rect: tuple[int, int, int, int]
    ink: tuple[int, int, int]
    background: tuple[int, int, int]


def run_live(display: str, spec: SampleSpec, seconds: float, interval_ms: float) -> list[str]:
    sampler = XImageSampler(display)
    labels: list[str] = []
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        argb = sampler.capture(spec.x, spec.y, spec.width, spec.height)
        labels.append(classify_argb(argb, spec.width, spec.height, spec.client_rect, spec.ink,
                                    spec.background))
        time.sleep(interval_ms / 1000.0)
    return labels


def _parse_rgb(text: str) -> tuple[int, int, int]:
    parts = tuple(int(part) for part in text.split(","))
    if len(parts) != 3 or any(not 0 <= part <= 255 for part in parts):
        raise argparse.ArgumentTypeError("expected R,G,B in 0..255")
    return parts  # type: ignore[return-value]


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--display", required=True)
    parser.add_argument("--rect", required=True, help="X,Y,W,H of the window region on the root")
    parser.add_argument("--client-rect", required=True, help="X,Y,W,H of the static client inside the region")
    parser.add_argument("--ink", required=True, type=_parse_rgb)
    parser.add_argument("--bg", required=True, type=_parse_rgb)
    parser.add_argument("--seconds", type=float, default=2.0)
    parser.add_argument("--interval-ms", type=float, default=2.0)
    args = parser.parse_args(argv)
    x, y, w, h = (int(v) for v in args.rect.split(","))
    cx, cy, cw, ch = (int(v) for v in args.client_rect.split(","))
    spec = SampleSpec(x, y, w, h, (cx, cy, cw, ch), args.ink, args.bg)
    labels = run_live(args.display, spec, args.seconds, args.interval_ms)
    print(f"samples={len(labels)} flicker={count_flicker(labels)}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

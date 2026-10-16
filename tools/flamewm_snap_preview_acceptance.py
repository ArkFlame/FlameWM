#!/usr/bin/env python3
"""Native acceptance for the border-only snap preview (see-through interior).

Drives a window drag on an isolated nested Xephyr display with XTest, holds the
pointer inside a snap zone, and checks that:
  * the preview overlay's XShape bounding region is a ring, not the full rect;
  * the preview interior pixels equal the pre-drag pixels behind it;
  * the ring pixels carry the accent colour.

Pure sampler functions are unit tested in test_flamewm_snap_preview_acceptance.py.
Never targets :97 or :123..:130 (other users' and builders' displays).
"""

from __future__ import annotations

import argparse
import ctypes
import ctypes.util
import re
import struct
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path

# Neutral WM default accent (live_settings.rs); the ring follows the configured
# `accent` setting, so pass --accent to match it.
DEFAULT_ACCENT = (0x7F, 0x87, 0x8E)
FORBIDDEN_DISPLAYS = {":97"} | {f":{n}" for n in range(123, 131)}
DISPLAY_RE = re.compile(r"^:(131)$")


@dataclass(frozen=True)
class Image:
    width: int
    height: int
    pixels: bytes  # 4 bytes per pixel, BGRX little-endian, row stride = width*4

    def rgb(self, x: int, y: int) -> tuple[int, int, int]:
        i = (y * self.width + x) * 4
        return (self.pixels[i + 2], self.pixels[i + 1], self.pixels[i])


def parse_xwd(data: bytes) -> Image:
    """Parse an XWD (X Window Dump) produced by `xwd -root -silent` at 32 bpp."""
    words = struct.unpack(">25I", data[:100])
    header_size, width, height = words[0], words[4], words[5]
    bits_per_pixel, bytes_per_line = words[11], words[12]
    byte_order, ncolors = words[7], words[19]
    if bits_per_pixel != 32:
        raise ValueError(f"unsupported bits_per_pixel={bits_per_pixel}")
    if byte_order != 0:
        raise ValueError("expected LSBFirst XWD byte order")
    start = header_size + ncolors * 12
    rows = []
    for y in range(height):
        base = start + y * bytes_per_line
        rows.append(data[base : base + width * 4])
    return Image(width, height, b"".join(rows))


def region_pixels(img: Image, rect: tuple[int, int, int, int]) -> list[tuple[int, int, int]]:
    x0, y0, w, h = rect
    return [img.rgb(x, y) for y in range(y0, y0 + h) for x in range(x0, x0 + w)]


def close(a: tuple[int, int, int], b: tuple[int, int, int], tol: int) -> bool:
    return all(abs(p - q) <= tol for p, q in zip(a, b))


def classify_interior(
    now: list[tuple[int, int, int]],
    behind: list[tuple[int, int, int]],
    tol: int = 6,
) -> dict:
    """Fraction of interior pixels equal to the pre-drag pixels behind them."""
    if len(now) != len(behind) or not now:
        raise ValueError("interior sample sizes must match and be non-empty")
    matches = sum(1 for a, b in zip(now, behind) if close(a, b, tol))
    black = sum(1 for a in now if close(a, (0, 0, 0), tol))
    return {
        "samples": len(now),
        "match_fraction": matches / len(now),
        "black_fraction": black / len(now),
    }


def parse_accent(text: str) -> tuple[int, int, int]:
    """Parse the configured `accent` setting, `#RRGGBB` or bare `RRGGBB`."""
    digits = text.removeprefix("#")
    if len(digits) != 6 or any(c not in "0123456789abcdefABCDEF" for c in digits):
        raise ValueError(f"accent must be #RRGGBB, got {text!r}")
    return (int(digits[0:2], 16), int(digits[2:4], 16), int(digits[4:6], 16))


def classify_ring(
    ring: list[tuple[int, int, int]],
    accent: tuple[int, int, int] = DEFAULT_ACCENT,
    tol: int = 24,
) -> dict:
    if not ring:
        raise ValueError("ring sample must be non-empty")
    matches = sum(1 for p in ring if close(p, accent, tol))
    return {"samples": len(ring), "accent_fraction": matches / len(ring)}


def verdict(interior: dict, ring: dict, rects_ok: bool) -> str:
    if not rects_ok:
        return "FAIL_SHAPE_NOT_RING"
    if interior["match_fraction"] >= 0.98 and ring["accent_fraction"] >= 0.98:
        return "PASS_SEE_THROUGH"
    if interior["black_fraction"] >= 0.98:
        return "FAIL_BLACK_INTERIOR"
    return "FAIL_INTERIOR_MISMATCH"


def shape_is_ring(rects: list[tuple[int, int, int, int]], w: int, h: int, border: int) -> bool:
    """True when the bounding region covers the border ring and leaves the centre empty."""
    covered = [[False] * w for _ in range(h)]
    for x, y, rw, rh in rects:
        for yy in range(max(0, y), min(h, y + rh)):
            for xx in range(max(0, x), min(w, x + rw)):
                covered[yy][xx] = True
    total = sum(sum(row) for row in covered)
    if total >= w * h or total == 0:
        return False
    cy, cx = h // 2, w // 2
    if covered[cy][cx] or not covered[cy][border - 1] or covered[cy][border]:
        return False
    return all(covered[0][x] and covered[h - 1][x] for x in range(w)) and all(
        covered[y][0] and covered[y][w - 1] for y in range(h)
    )


# ---- live X access (ctypes; no Python X client dependency) -----------------

class XRectangle(ctypes.Structure):
    _fields_ = [("x", ctypes.c_short), ("y", ctypes.c_short),
                ("width", ctypes.c_ushort), ("height", ctypes.c_ushort)]


def _libs():
    x11 = ctypes.CDLL(ctypes.util.find_library("X11") or "libX11.so.6")
    xtst = ctypes.CDLL(ctypes.util.find_library("Xtst") or "libXtst.so.6")
    xext = ctypes.CDLL(ctypes.util.find_library("Xext") or "libXext.so.6")
    x11.XOpenDisplay.restype = ctypes.c_void_p
    x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x11.XFlush.argtypes = [ctypes.c_void_p]
    x11.XCloseDisplay.argtypes = [ctypes.c_void_p]
    xext.XShapeGetRectangles.restype = ctypes.POINTER(XRectangle)
    xext.XShapeGetRectangles.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int,
                                         ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_int)]
    xext.XShapeCombineRectangles.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int,
                                             ctypes.c_int, ctypes.c_int, ctypes.c_void_p,
                                             ctypes.c_int, ctypes.c_int, ctypes.c_int]
    return x11, xtst, xext


class Pointer:
    def __init__(self, display: str) -> None:
        self.x11, self.xtst, self.xext = _libs()
        self.dpy = self.x11.XOpenDisplay(display.encode())
        if not self.dpy:
            raise RuntimeError(f"cannot open display {display}")

    def move(self, x: int, y: int) -> None:
        self.xtst.XTestFakeMotionEvent(self.dpy, -1, x, y, 0)
        self.x11.XFlush(self.dpy)

    def button(self, press: bool) -> None:
        self.xtst.XTestFakeButtonEvent(self.dpy, 1, int(press), 0)
        self.x11.XFlush(self.dpy)

    def shape_rects(self, window: int) -> list[tuple[int, int, int, int]]:
        count = ctypes.c_int(0)
        ordering = ctypes.c_int(0)
        ptr = self.xext.XShapeGetRectangles(self.dpy, window, 0, ctypes.byref(count),
                                            ctypes.byref(ordering))
        return [(ptr[i].x, ptr[i].y, ptr[i].width, ptr[i].height) for i in range(count.value)]

    def set_full_bounding(self, window: int, w: int, h: int) -> None:
        """Negative control: restore a rectangular bounding region (the pre-fix behaviour)."""
        rect = XRectangle(0, 0, w, h)
        self.xext.XShapeCombineRectangles(self.dpy, window, 0, 0, 0, ctypes.byref(rect), 1, 0, 1)
        self.x11.XFlush(self.dpy)


def screenshot(display: str) -> Image:
    out = subprocess.run(["xwd", "-root", "-silent", "-display", display], check=True,
                         capture_output=True).stdout
    return parse_xwd(out)


def find_preview(display: str) -> tuple[int, tuple[int, int, int, int]] | None:
    tree = subprocess.run(["xwininfo", "-display", display, "-root", "-tree"], check=True,
                          capture_output=True, text=True).stdout
    for line in tree.splitlines():
        if '"FlameWM snap preview"' not in line:
            continue
        win = int(line.strip().split()[0], 16)
        geo = re.search(r"(\d+)x(\d+)\+(-?\d+)\+(-?\d+)\s+\+(-?\d+)\+(-?\d+)", line)
        if not geo:
            continue
        w, h, x, y = (int(geo.group(i)) for i in (1, 2, 5, 6))
        return win, (x, y, w, h)
    return None


def run(
    display: str,
    grab: tuple[int, int],
    target_x: int,
    out_dir: Path,
    hold: float,
    accent: tuple[int, int, int],
) -> int:
    if display in FORBIDDEN_DISPLAYS or not DISPLAY_RE.match(display):
        print(f"refusing display {display}: only :131 is allowed for this harness", file=sys.stderr)
        return 2
    out_dir.mkdir(parents=True, exist_ok=True)
    pointer = Pointer(display)
    before = screenshot(display)
    (out_dir / "before.xwd").write_bytes(subprocess.run(
        ["xwd", "-root", "-silent", "-display", display], capture_output=True, check=True).stdout)

    gx, gy = grab
    pointer.move(gx, gy)
    time.sleep(0.2)
    pointer.button(True)
    steps = 40
    for i in range(1, steps + 1):
        pointer.move(gx + (target_x - gx) * i // steps, gy)
        time.sleep(0.02)
    time.sleep(hold)

    found = find_preview(display)
    if found is None:
        pointer.button(False)
        print("FAIL: no snap preview window while held in zone", file=sys.stderr)
        return 1
    win, (px, py, pw, ph) = found
    rects = pointer.shape_rects(win)
    held = screenshot(display)
    (out_dir / "held.xwd").write_bytes(subprocess.run(
        ["xwd", "-root", "-silent", "-display", display], capture_output=True, check=True).stdout)

    report = evaluate(before, held, (px, py, pw, ph), rects, accent)
    report["shape_rects"] = rects
    report["negative_control"] = None

    pointer.set_full_bounding(win, pw, ph)
    time.sleep(0.3)
    neg = screenshot(display)
    report["negative_control"] = evaluate(before, neg, (px, py, pw, ph), [(0, 0, pw, ph)], accent)
    pointer.button(False)

    for key, value in report.items():
        print(f"{key}: {value}")
    return 0 if report["verdict"] == "PASS_SEE_THROUGH" else 1


def evaluate(before: Image, held: Image, geo, rects, accent: tuple[int, int, int]) -> dict:
    px, py, pw, ph = geo
    border = 2
    # Interior sample: band below the dragged window (which sits at the top-left of the zone) and
    # above the panel. Ring sample: the top border row, clear of the dragged window.
    sx, sy = px + 20, py + 480
    sw, sh = max(1, pw - 40), max(1, min(100, ph - 480 - 4))
    interior_now = region_pixels(held, (sx, sy, sw, sh))
    interior_before = region_pixels(before, (sx, sy, sw, sh))
    left_ring = region_pixels(held, (px + 20, py, max(1, pw - 40), border))
    interior = classify_interior(interior_now, interior_before)
    ring = classify_ring(left_ring, accent)
    rects_ok = shape_is_ring(rects, pw, ph, border) if pw <= 2000 and ph <= 2000 else False
    return {
        "preview_geometry": geo,
        "interior_sample": (sx, sy, sw, sh),
        "interior": interior,
        "ring": ring,
        "verdict": verdict(interior, ring, rects_ok),
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--display", default=":131")
    parser.add_argument("--grab", default="400,185", help="titlebar press point x,y")
    parser.add_argument("--target-x", type=int, default=1, help="x to drag toward the left edge")
    parser.add_argument("--hold", type=float, default=1.5)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument(
        "--accent",
        type=parse_accent,
        default=DEFAULT_ACCENT,
        help="configured accent #RRGGBB the ring must match (default: WM neutral)",
    )
    args = parser.parse_args(argv)
    gx, gy = (int(v) for v in args.grab.split(","))
    return run(args.display, (gx, gy), args.target_x, args.out, args.hold, args.accent)


if __name__ == "__main__":
    raise SystemExit(main())

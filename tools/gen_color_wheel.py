#!/usr/bin/env python3
"""Generates assets/web/color-wheel.png: a 64x64 conic hue wheel with a circular alpha mask.

Stdlib only and deterministic, so re-running it reproduces the committed PNG byte for byte.
Usage: python3 -I tools/gen_color_wheel.py [output-path]
"""

import colorsys
import math
import pathlib
import struct
import sys
import zlib

SIZE = 64
SUPERSAMPLE = 4
SATURATION = 0.85
VALUE = 1.0


def hue_rgb(angle_turns):
    red, green, blue = colorsys.hsv_to_rgb(angle_turns % 1.0, SATURATION, VALUE)
    return round(red * 255), round(green * 255), round(blue * 255)


def coverage(x, y):
    """Fraction of the sub-samples of pixel (x, y) that fall inside the disc."""
    radius = SIZE / 2.0
    inside = 0
    for sy in range(SUPERSAMPLE):
        for sx in range(SUPERSAMPLE):
            px = x + (sx + 0.5) / SUPERSAMPLE - radius
            py = y + (sy + 0.5) / SUPERSAMPLE - radius
            if px * px + py * py <= radius * radius:
                inside += 1
    return inside / (SUPERSAMPLE * SUPERSAMPLE)


def pixel(x, y):
    dx = x + 0.5 - SIZE / 2.0
    dy = y + 0.5 - SIZE / 2.0
    # Clockwise from the top, so the hue sweep starts at red at 12 o'clock.
    angle = (math.atan2(dx, -dy) / (2 * math.pi)) % 1.0
    red, green, blue = hue_rgb(angle)
    return red, green, blue, round(coverage(x, y) * 255)


def png_chunk(kind, data):
    body = kind + data
    return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)


def encode(rows):
    raw = b"".join(b"\x00" + bytes(channel for pixel_value in row for channel in pixel_value) for row in rows)
    header = struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + png_chunk(b"IHDR", header)
        + png_chunk(b"IDAT", zlib.compress(raw, 9))
        + png_chunk(b"IEND", b"")
    )


def main():
    root = pathlib.Path(__file__).resolve().parent.parent
    output = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else root / "assets/web/color-wheel.png"
    rows = [[pixel(x, y) for x in range(SIZE)] for y in range(SIZE)]
    output.write_bytes(encode(rows))
    print(f"wrote {output}")


if __name__ == "__main__":
    main()

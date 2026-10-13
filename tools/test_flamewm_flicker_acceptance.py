#!/usr/bin/env python3
"""Unit tests for the flicker classifier. Run: python3 -I tools/test_flamewm_flicker_acceptance.py"""

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import flamewm_flicker_acceptance as flicker  # noqa: E402


class CountFlickerTests(unittest.TestCase):
    def test_clean_minimize_has_no_flicker(self):
        labels = ["client", "client", "overlay", "overlay", "background", "background"]
        self.assertEqual(flicker.count_flicker(labels), 0)

    def test_client_blink_between_overlay_samples_is_flicker(self):
        labels = ["overlay", "overlay", "client", "overlay", "overlay"]
        self.assertEqual(flicker.count_flicker(labels), 1)

    def test_background_gap_between_overlay_samples_is_flicker(self):
        labels = ["overlay", "background", "background", "overlay"]
        self.assertEqual(flicker.count_flicker(labels), 2)

    def test_client_then_background_then_overlay_at_start_is_flicker(self):
        # The real window vanishes before the first overlay frame is painted.
        labels = ["client", "background", "overlay", "overlay"]
        self.assertEqual(flicker.count_flicker(labels), 1)

    def test_background_between_client_samples_is_flicker(self):
        labels = ["client", "background", "client"]
        self.assertEqual(flicker.count_flicker(labels), 1)

    def test_rejects_unknown_label(self):
        with self.assertRaises(ValueError):
            flicker.count_flicker(["client", "bogus"])


def _argb(pixels):
    """Build BGRX bytes from (r, g, b) tuples."""
    out = bytearray()
    for r, g, b in pixels:
        out += bytes((b, g, r, 0))
    return bytes(out)


class ClassifyTests(unittest.TestCase):
    INK = (200, 30, 30)
    BG = (10, 10, 10)

    def test_client_when_ink_fills_static_rect_exactly(self):
        # 4x2 region, static client rect is the left 2x2; right two columns are background.
        argb = _argb([self.INK if x < 2 else self.BG for y in range(2) for x in range(4)])
        self.assertEqual(flicker.classify_argb(argb, 4, 2, (0, 0, 2, 2), self.INK, self.BG), flicker.CLIENT)

    def test_overlay_when_ink_is_outside_the_static_rect(self):
        argb = _argb([self.INK if x >= 2 else self.BG for y in range(2) for x in range(4)])
        self.assertEqual(flicker.classify_argb(argb, 4, 2, (0, 0, 2, 2), self.INK, self.BG), flicker.OVERLAY)

    def test_background_when_no_ink(self):
        argb = _argb([self.BG] * 8)
        self.assertEqual(flicker.classify_argb(argb, 4, 2, (0, 0, 2, 2), self.INK, self.BG), flicker.BACKGROUND)

    def test_partial_client_rect_is_overlay_not_client(self):
        # Ink covers only one column of the static rect: the client is not fully shown.
        argb = _argb([self.INK if x == 0 else self.BG for y in range(2) for x in range(4)])
        self.assertEqual(flicker.classify_argb(argb, 4, 2, (0, 0, 2, 2), self.INK, self.BG), flicker.OVERLAY)


if __name__ == "__main__":
    unittest.main()

"""Unit tests for the pure snap-preview sampler (no X server required)."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import flamewm_snap_preview_acceptance as acc  # noqa: E402


class SamplerTests(unittest.TestCase):
    def test_interior_equal_to_behind_is_see_through(self):
        behind = [(10, 200, 30)] * 50
        result = acc.classify_interior(list(behind), behind)
        self.assertEqual(result["match_fraction"], 1.0)
        self.assertEqual(result["black_fraction"], 0.0)

    def test_black_interior_is_flagged(self):
        behind = [(10, 200, 30)] * 50
        black = [(0, 0, 0)] * 50
        result = acc.classify_interior(black, behind)
        self.assertEqual(result["black_fraction"], 1.0)
        self.assertEqual(
            acc.verdict(result, acc.classify_ring([acc.DEFAULT_ACCENT] * 10), True),
            "FAIL_BLACK_INTERIOR",
        )

    def test_dark_red_ring_is_not_accent(self):
        # Accent blended over black at alpha 0xdd: the pre-fix translucent border.
        dark = (207, 55, 62)
        self.assertEqual(acc.classify_ring([dark] * 8)["accent_fraction"], 0.0)
        self.assertEqual(acc.classify_ring([acc.DEFAULT_ACCENT] * 8)["accent_fraction"], 1.0)

    def test_ring_matches_configured_accent_not_hardcoded_red(self):
        blue = acc.parse_accent("#123456")
        self.assertEqual(acc.classify_ring([blue] * 8, blue)["accent_fraction"], 1.0)
        self.assertEqual(acc.classify_ring([(0xEF, 0x40, 0x48)] * 8, blue)["accent_fraction"], 0.0)

    def test_parse_accent_accepts_hash_and_bare_hex_and_rejects_garbage(self):
        self.assertEqual(acc.parse_accent("#EF4048"), (0xEF, 0x40, 0x48))
        self.assertEqual(acc.parse_accent("7f878e"), (0x7F, 0x87, 0x8E))
        with self.assertRaises(ValueError):
            acc.parse_accent("#GG0000")

    def test_verdict_requires_ring_rects_and_both_samples(self):
        good_interior = acc.classify_interior([(1, 2, 3)] * 20, [(1, 2, 3)] * 20)
        good_ring = acc.classify_ring([acc.DEFAULT_ACCENT] * 20)
        self.assertEqual(acc.verdict(good_interior, good_ring, True), "PASS_SEE_THROUGH")
        self.assertEqual(acc.verdict(good_interior, good_ring, False), "FAIL_SHAPE_NOT_RING")
        bad_interior = acc.classify_interior([(0, 0, 0)] * 20, [(100, 150, 200)] * 20)
        self.assertEqual(acc.verdict(bad_interior, good_ring, True), "FAIL_BLACK_INTERIOR")

    def test_mismatch_sample_sizes_rejected(self):
        with self.assertRaises(ValueError):
            acc.classify_interior([(0, 0, 0)], [(0, 0, 0), (1, 1, 1)])

    def test_shape_ring_accepts_four_border_rects(self):
        rects = [(0, 0, 10, 2), (0, 2, 2, 6), (8, 2, 2, 6), (0, 8, 10, 2)]
        self.assertTrue(acc.shape_is_ring(rects, 10, 10, 2))

    def test_shape_full_rect_is_rejected(self):
        self.assertFalse(acc.shape_is_ring([(0, 0, 10, 10)], 10, 10, 2))

    def test_shape_with_interior_hole_missing_border_is_rejected(self):
        rects = [(0, 0, 10, 2), (0, 2, 2, 6), (8, 2, 2, 6)]
        self.assertFalse(acc.shape_is_ring(rects, 10, 10, 2))

    def test_xwd_parser_roundtrip(self):
        import struct

        width, height, bpl = 2, 2, 8
        header = [0] * 25
        header[0] = 100
        header[4], header[5] = width, height
        header[7] = 0  # LSBFirst
        header[11] = 32
        header[12] = bpl
        header[19] = 0
        data = struct.pack(">25I", *header)
        pix = bytes([1, 2, 3, 0, 4, 5, 6, 0]) + bytes([7, 8, 9, 0, 10, 11, 12, 0])
        img = acc.parse_xwd(data + pix)
        self.assertEqual(img.rgb(0, 0), (3, 2, 1))
        self.assertEqual(img.rgb(1, 1), (12, 11, 10))

    def test_refuses_forbidden_displays(self):
        self.assertEqual(acc.run(":97", (0, 0), 0, Path("/tmp/claude-1000/x"), 0, acc.DEFAULT_ACCENT), 2)
        self.assertEqual(acc.run(":125", (0, 0), 0, Path("/tmp/claude-1000/x"), 0, acc.DEFAULT_ACCENT), 2)


if __name__ == "__main__":
    unittest.main()

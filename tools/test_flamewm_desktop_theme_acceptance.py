#!/usr/bin/env python3
"""Unit tests for the pure helpers of flamewm_desktop_theme_acceptance.py (no X11, no processes)."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import flamewm_desktop_theme_acceptance as harness  # noqa: E402


class NewEntries(unittest.TestCase):
    def test_reports_only_added_names(self):
        self.assertEqual(harness.new_entries(["a"], ["a", "New Folder"]), ["New Folder"])

    def test_removed_or_unchanged_yields_nothing(self):
        self.assertEqual(harness.new_entries(["a", "b"], ["a"]), [])


class BlueIconCenter(unittest.TestCase):
    def test_centre_of_new_blue_glyph(self):
        before = [(10, 10, 10)] * 400  # 20 px wide
        after = [(10, 10, 10)] * 400
        for y in range(5, 15):
            for x in range(4, 14):
                after[y * 20 + x] = (61, 174, 233)
        self.assertEqual(harness.blue_icon_center(before, after, 20, min_pixels=100), (8, 10))  # mean of x 4..13 and y 5..14

    def test_white_label_or_no_glyph_is_not_located(self):
        before = [(10, 10, 10)] * 400
        after = [(255, 255, 255)] * 400
        self.assertIsNone(harness.blue_icon_center(before, after, 20, min_pixels=100))


class PickOther(unittest.TestCase):
    def test_skips_the_active_value(self):
        self.assertEqual(harness.pick_other("#e53935", harness.ACCENTS), "#1e88e5")
        self.assertEqual(harness.pick_other("breeze", harness.ICON_THEMES), "Adwaita")

    def test_unknown_active_value_takes_first_candidate(self):
        self.assertEqual(harness.pick_other(None, harness.ICON_THEMES), "Adwaita")

    def test_desktop_gets_session_cursor_search_path(self):
        self.assertEqual(harness.DESKTOP_XCURSOR_PATH, "/usr/share/icons")


class ChangedPixels(unittest.TestCase):
    def test_identical_regions_have_no_change(self):
        pixels = [(10, 10, 10)] * 400
        self.assertEqual(harness.changed_pixels(pixels, list(pixels)), 0)

    def test_drawn_icon_counts_each_changed_pixel(self):
        base = [(10, 10, 10)] * 400
        drawn = [(200, 200, 200)] * 120 + [(10, 10, 10)] * 280
        self.assertEqual(harness.changed_pixels(base, drawn), 120)

    def test_noise_under_tolerance_is_ignored(self):
        self.assertEqual(harness.changed_pixels([(10, 10, 10)] * 4, [(14, 10, 10)] * 4), 0)

    def test_size_mismatch_is_an_error(self):
        with self.assertRaises(ValueError):
            harness.changed_pixels([(0, 0, 0)], [(0, 0, 0), (1, 1, 1)])


class ItemVerdict(unittest.TestCase):
    def test_removed_then_restored_passes(self):
        self.assertEqual(harness.item_verdict(5000, 10, 200), harness.PASS)

    def test_nothing_rendered_is_not_present(self):
        self.assertEqual(harness.item_verdict(3, 0, 200), harness.NOT_PRESENT)

    def test_not_restored_fails(self):
        self.assertEqual(harness.item_verdict(5000, 5000, 200), harness.FAIL)


class CursorSignature(unittest.TestCase):
    def test_same_shape_same_digest(self):
        self.assertEqual(harness.cursor_signature([1, 2, 3]), harness.cursor_signature([1, 2, 3]))

    def test_order_and_alpha_change_digest(self):
        self.assertNotEqual(harness.cursor_signature([1, 2, 3]), harness.cursor_signature([3, 2, 1]))
        self.assertNotEqual(harness.cursor_signature([0xFF000000]), harness.cursor_signature([0x00000000]))

    def test_empty_is_offset_basis(self):
        self.assertEqual(harness.cursor_signature([]), 0xcbf29ce484222325)


class OverrideParsing(unittest.TestCase):
    def test_parses_named_points(self):
        self.assertEqual(harness.parse_override(["rename-ok=640,400", "desktop=900,300"]),
                         {"rename-ok": (640, 400), "desktop": (900, 300)})

    def test_empty_list(self):
        self.assertEqual(harness.parse_override([]), {})


class DisplayRule(unittest.TestCase):
    def test_default_display_is_in_xtest_range_and_not_user_display(self):
        self.assertEqual(harness.DEFAULT_DISPLAY, ":105")
        self.assertTrue(harness.tray.display_allowed(harness.DEFAULT_DISPLAY))
        self.assertFalse(harness.tray.display_allowed(":97"))


if __name__ == "__main__":
    unittest.main()

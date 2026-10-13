"""Pure unit tests for tools/flamewm_tray_popups_acceptance.py (no X access)."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import flamewm_tray_popups_acceptance as acc  # noqa: E402

MENU = (0x10, 0x20, 0x30)
WALLPAPER = [(200, 120, 90), (60, 40, 180), (30, 220, 10), (150, 150, 150)]

XWININFO = """xwininfo: Window id: 0xc0002a "FlameWM network"

  Absolute upper-left X:  120
  Absolute upper-left Y:  300
  Relative upper-left X:  0
  Relative upper-left Y:  0
  Width: 310
  Height: 260
  Map State: IsViewable
"""

TREE = """  0x800001 "FlameWM Desktop": ()  1350x641+0+0  +0+0
     0xc00052 "FlameWM calendar": ()  1x1+0+0  +0+0
     0xc0002a "FlameWM network": ()  310x260+120+300  +120+300
"""


class DisplayGuard(unittest.TestCase):
    def test_allows_nested_test_display(self):
        self.assertTrue(acc.display_allowed(":134"))

    def test_refuses_user_display_and_reserved_range(self):
        # Negative controls: :97 is the user's session, :123-:133 are other workers' displays.
        for display in (":97", ":123", ":131", ":133"):
            self.assertFalse(acc.display_allowed(display), display)

    def test_refuses_malformed(self):
        self.assertFalse(acc.display_allowed("134"))
        self.assertFalse(acc.display_allowed(":134; rm"))


class ParsePopupWindow(unittest.TestCase):
    def test_geometry_and_map_state(self):
        self.assertEqual(acc.parse_geometry(XWININFO), (120, 300, 310, 260))
        self.assertTrue(acc.parse_map_state(XWININFO))

    def test_unmapped_window_is_not_mapped(self):
        # Negative control: an unmapped popup must not count as open.
        self.assertFalse(acc.parse_map_state(XWININFO.replace("IsViewable", "IsUnmapped")))

    def test_finds_exact_title_only(self):
        self.assertEqual(acc.find_window_id(TREE, "FlameWM network"), "0xc0002a")
        # Negative control: a prefix of the title must not match another popup.
        self.assertIsNone(acc.find_window_id(TREE, "FlameWM net"))
        self.assertIsNone(acc.find_window_id(TREE, "FlameWM audio"))

    def test_missing_geometry_raises(self):
        with self.assertRaises(ValueError):
            acc.parse_geometry("no geometry here")


class StripSampling(unittest.TestCase):
    def test_points_stay_inside_padding_strip(self):
        rect = (120, 300, 310, 260)
        points = acc.strip_points(rect)
        self.assertTrue(points)
        for px, py in points:
            self.assertGreaterEqual(px, rect[0] + 4)
            self.assertLess(px, rect[0] + 13)  # padding is 13px: no text sampled
            self.assertTrue(rect[1] < py < rect[1] + rect[3])


class CheckPopup(unittest.TestCase):
    def test_opacity_100_accepts_solid_menu_colour(self):
        samples = [MENU] * 20
        self.assertTrue(acc.check_popup(samples, 100, MENU)["ok"])

    def test_opacity_100_rejects_wallpaper_showing_through(self):
        # Negative control: a translucent-looking capture at full opacity fails.
        self.assertFalse(acc.check_popup(WALLPAPER * 5, 100, MENU)["ok"])

    def test_opacity_0_accepts_frosted_backdrop(self):
        self.assertTrue(acc.check_popup(WALLPAPER * 5, 0, MENU)["ok"])

    def test_opacity_0_rejects_solid_fill(self):
        # Negative control: a solid colour fill (no backdrop) must fail at opacity 0.
        self.assertFalse(acc.check_popup([MENU] * 20, 0, MENU)["ok"])

    def test_empty_samples_fail(self):
        self.assertFalse(acc.check_popup([], 0, MENU)["ok"])


if __name__ == "__main__":
    unittest.main()

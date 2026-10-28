"""Regression: X11 backdrop provider and its consumers must evolve together.

Pass 22's cumulative overlay accidentally restored an older backdrop.rs while
leaving the newer multi-monitor and moving-window callers in the checkout.
"""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]
CORE = ROOT / "crates/flamewm-render-x11/src/backdrop.rs"
WINDOW = ROOT / "crates/flamewm-render-x11/src/app/window_backdrop.rs"
POPUP = ROOT / "crates/flamewm-render-x11/src/surface_controller/backdrop_shape.rs"


class BackdropApiConsistency(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.core = CORE.read_text(encoding="utf-8")
        cls.window = WINDOW.read_text(encoding="utf-8")
        cls.popup = POPUP.read_text(encoding="utf-8")

    def test_wallpaper_outputs_property_read_by_both_consumers(self):
        self.assertIn('"_FLAMEWM_WALLPAPER_OUTPUTS"', self.core)
        self.assertRegex(self.core, r"unsafe fn published_wallpaper_outputs\s*\(")
        self.assertIn('published_wallpaper_outputs(self.display)', self.window)
        self.assertIn('published_wallpaper_outputs(self.display)', self.popup)

    def test_canvas_creator_receives_output_layout(self):
        self.assertRegex(self.core, r"fn wallpaper_canvas\s*\(\s*spec: &WindowBackdrop,\s*root: \(u32, u32\),\s*outputs: &\[\(i32, i32, u32, u32\)\]")
        self.assertIn('wallpaper_canvas(&state.spec, root, &outputs)', self.window)
        self.assertIn('desktop_canvas(', self.core)

    def test_window_recrop_tracks_canvas_epoch(self):
        self.assertRegex(self.core, r"pub fn recrop\s*\(")
        self.assertIn('canvas.recrop(&old.words, old.root, (rx, ry), (width, height))', self.window)
        self.assertIn('old.canvas_epoch == Some(epoch)', self.window)
        self.assertIn('canvas_epoch,', self.window)
        self.assertIn('fn recrop_matches_a_fresh_crop_for_every_shift()', self.core)

    def test_popup_uses_layout_aware_crop(self):
        self.assertIn('desktop.crop(root, &outputs, rect, opacity, blur)', self.popup)
        self.assertRegex(self.core, r"pub fn crop\s*\(\s*&mut self,\s*root: \(u32, u32\),\s*outputs: &\[\(i32, i32, u32, u32\)\]")

    def test_shared_owner_not_duplicated(self):
        self.assertEqual(len(re.findall(r'^type CanvasLayout\s*=', self.core, re.M)), 1)
        self.assertEqual(self.core.count('pub(crate) unsafe fn published_wallpaper_outputs'), 1)
        self.assertEqual(self.core.count('pub fn recrop('), 1)


if __name__ == '__main__':
    unittest.main()

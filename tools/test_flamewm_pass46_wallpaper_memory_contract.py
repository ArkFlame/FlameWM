"""Static companion guard for the Pass 46 wallpaper allocation changes.

Actual pixel equivalence is exercised by Rust tests in wallpaper.rs; this test
only protects the source-level allocation structure when Rust is unavailable.
"""
from pathlib import Path
import os
import unittest

ROOT = Path(os.getenv("FLAMEWM_SOURCE_ROOT", Path(__file__).resolve().parents[1]))


class WallpaperMemoryPass46(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.source = (ROOT / "crates/flamewm-image-core/src/wallpaper.rs").read_text()
        cls.composer = cls.source.split("pub fn compose(", 1)[1].split("pub fn compose_region(", 1)[0]
        cls.solid = cls.source.split("fn solid(", 1)[1].split("fn blit(", 1)[0]

    def test_cropped_fill_uses_visible_region_instead_of_oversized_scaled_buffer(self):
        self.assertIn("if fit == WallpaperFit::Fill", self.composer)
        self.assertIn("(sw, sh) != (width, height)", self.composer)
        self.assertIn("(sw, sh) != (natural.width, natural.height)", self.composer)
        self.assertIn("return compose_region(natural, width, height, fit, (0, 0, width, height));", self.composer)
        self.assertIn("cropped_fill_uses_only_the_visible_canvas_and_matches_reference", self.source)

    def test_canvas_fill_reserves_once_and_preserves_rgba(self):
        self.assertIn("let mut pixels = vec![0u8; count];", self.solid)
        self.assertIn("for pixel in pixels.chunks_exact_mut(4)", self.solid)
        self.assertIn("pixel.copy_from_slice(&colour);", self.solid)
        self.assertNotIn("colour.iter().copied().cycle().take(count).collect()", self.solid)
        self.assertIn("solid_canvas_reserves_once_and_preserves_all_channels", self.source)

    def test_direct_scaling_fast_path_and_borrowed_blit_remain(self):
        self.assertIn("Cow::Borrowed(natural)", self.composer)
        self.assertIn("Cow::Owned(scale_rgba(natural, sw, sh, ScaleFilter::Area)?)", self.composer)
        self.assertIn("if (x, y) == (0, 0) && (sw, sh) == (width, height)", self.composer)
        self.assertIn("blit(&mut canvas, &scaled, x, y);", self.composer)

    def test_existing_region_oracle_still_cross_checks_all_fit_modes(self):
        self.assertIn("compose_region_matches_cropped_compose_for_every_fit_root_rect_and_natural", self.source)
        self.assertIn("reference_compose(&source, root.0, root.1, fit)", self.source)
        self.assertIn("WallpaperFit::Center", self.source)
        self.assertIn("WallpaperFit::Tile", self.source)


if __name__ == "__main__":
    unittest.main()

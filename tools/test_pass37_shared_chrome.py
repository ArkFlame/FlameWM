#!/usr/bin/env python3
"""Source and asset contracts for the Pass 37 shared native titlebar fallback.

Native Rust tests and graphical evidence remain separate acceptance gates.
"""
from pathlib import Path
import unittest
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]


class SharedWindowControls(unittest.TestCase):
    def test_shared_raster_has_all_roles_and_bounded_allocation(self):
        source = (ROOT / "crates/flamewm-skin/src/recipes/window_chrome.rs").read_text()
        self.assertIn("pub fn control_glyph_rgba(", source)
        self.assertIn("if !(4..=128).contains(&edge)", source)
        for control in ("Minimize", "Maximize", "Restore", "Close"):
            self.assertIn(f"WindowControlRole::{control} =>", source)
        self.assertIn("control_glyphs_render_all_four_semantics_without_heavy_assets", source)

    def test_both_native_backends_use_the_same_fallback(self):
        x11 = (ROOT / "crates/flamewm-wm-x11/src/chrome.rs").read_text()
        wayland = (ROOT / "crates/flamewm-wm-wayland/src/render.rs").read_text()
        self.assertIn("recipe::control_glyph_rgba(role.skin(), edge, rgb)?", x11)
        self.assertIn("chrome::control_glyph_rgba(role, glyph_edge, GLYPH)", wayland)
        self.assertNotIn("fn line(pixels", wayland)
        self.assertIn("WindowControlRole::Maximize if maximized => WindowControlRole::Restore", wayland)
        self.assertIn("close_hover_is_local_to_control_and_keeps_alpha_corners", wayland)

    def test_first_party_svg_controls_exist_and_have_no_external_resources(self):
        icons = ROOT / "assets/web/breeze"
        for name in ("minimize", "maximize", "restore", "close"):
            asset = icons / f"window-{name}.svg"
            self.assertTrue(asset.is_file(), asset)
            tree = ET.parse(asset)
            self.assertEqual(tree.getroot().attrib["viewBox"], "0 0 16 16")
            text = asset.read_text()
            text = text.replace('xmlns="http://www.w3.org/2000/svg"', "")
            for forbidden in ("<script", "http://", "https://", "xlink:href", "foreignObject"):
                self.assertNotIn(forbidden, text)
            self.assertIn('class="ColorScheme-Text"', text)

    def test_packaging_keeps_icon_assets_but_does_not_mark_svg_executable(self):
        manifest = (ROOT / "packaging/source-manifest.txt").read_text().splitlines()
        self.assertIn("assets/", manifest)
        self.assertNotIn("@exec assets/web/github.svg", manifest)
        self.assertNotIn("@exec assets/web/paypal.svg", manifest)
        self.assertFalse(any(line.startswith("@exec assets/") for line in manifest))


if __name__ == "__main__":
    unittest.main()

import re
#!/usr/bin/env python3
"""Source and asset contracts for the Pass 37 shared native titlebar fallback.

Native Rust tests and graphical evidence remain separate acceptance gates.
"""
from pathlib import Path
import unittest
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]


class SharedWindowControls(unittest.TestCase):
    def test_both_native_backends_render_the_breeze_svgs(self):
        wayland = (ROOT / "crates/flamewm-wm-wayland/src/render.rs").read_text()
        self.assertIn("render_with_color_scheme", wayland)
        self.assertIn("WindowControlRole::Maximize if maximized => WindowControlRole::Restore", wayland)

    def test_first_party_svg_controls_exist_and_have_no_external_resources(self):
        icons = ROOT / "assets/web/breeze"
        for name in ("minimize", "maximize", "restore", "close"):
            asset = icons / f"window-{name}.svg"
            self.assertTrue(asset.is_file(), asset)
            tree = ET.parse(asset)
            text = asset.read_text()
            text = text.replace('xmlns="http://www.w3.org/2000/svg"', "")
            for forbidden in ("<script", "http://", "https://", "xlink:href", "foreignObject"):
                self.assertNotIn(forbidden, text)
            self.assertIn(re.sub(r'\s+', '', 'class="ColorScheme-Text"'), re.sub(r'\s+', '', text))

    def test_packaging_keeps_icon_assets_but_does_not_mark_svg_executable(self):
        manifest = (ROOT / "packaging/source-manifest.txt").read_text().splitlines()
        self.assertIn("assets/", manifest)
        self.assertNotIn("@exec assets/web/github.svg", manifest)
        self.assertNotIn("@exec assets/web/paypal.svg", manifest)
        self.assertFalse(any(line.startswith("@exec assets/") for line in manifest))


if __name__ == "__main__":
    unittest.main()

"""Pass 51: titlebar test fixture must use its imported, scale-aware resize owner."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]
CHROME = ROOT / 'crates/flamewm-wm-x11/src/chrome.rs'
LAYOUT = ROOT / 'crates/flamewm-wm-x11/src/frame/layout.rs'


class ChromeScaleFixture(unittest.TestCase):
    def test_titlebar_scale_fixture_uses_in_scope_resize_function(self):
        source = CHROME.read_text()
        begin = source.index('fn windows11_square_hover_coverage_at_100_135_200_percent()')
        end = source.index('\n    #[test]', begin)
        test = source[begin:end]
        self.assertRegex(test, r'use crate::frame::layout::\{[^}]*resize_thickness_for_titlebar[^}]*\}')
        self.assertIn('controls_width_for_titlebar(full), resize_thickness_for_titlebar(full)', test)
        self.assertNotRegex(test, r'\bRESIZE_T\b')

    def test_production_backplates_use_same_dynamic_resize_contract(self):
        source = CHROME.read_text()
        layout = LAYOUT.read_text()
        begin = source.index('pub fn control_backplates_for(')
        end = source.index('\n/// Composite AA cell backgrounds', begin)
        segment = source[begin:end]
        self.assertIn('resize_thickness_for_titlebar(h)', segment)
        self.assertIn('pub fn resize_thickness_for_titlebar(titlebar_h: i32)', layout)
        self.assertIn('controls_width_for_titlebar(h)', segment)


if __name__ == '__main__':
    unittest.main()

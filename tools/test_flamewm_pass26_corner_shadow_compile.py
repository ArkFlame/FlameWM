"""Pass 26 source-contract regression for Cargo test imports, AA dock and labels."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parent.parent

def read(path):
    return (ROOT / path).read_text()


class Pass26Tests(unittest.TestCase):
    def test_desktop_focus_test_imports_are_explicit(self):
        module = read('crates/flamewm-render-x11/src/app/events/tests/reactor_presentation_regressions.rs')
        imported = module.split('};', 1)[0]
        self.assertIn('desktop_takes_focus_on_press', imported)
        self.assertIn('X11WindowRole', imported)
        self.assertIn('desktop_primary_press_claims_focus_for_selected_file_shortcuts', module)

    def test_dock_rounding_follows_device_scale_and_classic_resets(self):
        code = read('crates/flamewm-shell/src/runtime.rs')
        self.assertIn('let logical_radius = radius as f32 / panel_ui_scale(', code)
        self.assertIn('document.corner_radius("taskbar", logical_radius)', code)
        self.assertIn('set_dock_frame(record.panel, edge, radius)', code)
        self.assertIn('if entry.border_radius != Some(radius)', read('crates/flamewm-render-core/src/model/style.rs'))
        renderer = read('crates/flamewm-render-x11/src/app/render.rs')
        self.assertIn('crate::xrender_aa::fill_rounded_rect(', renderer)
        self.assertIn('crate::xrender_aa::stroke_rounded_rect(', renderer)
        self.assertIn('crate::xshape::rounded_mask_spans(self.width, self.height, self.dock_corner_radius)', renderer)

    def test_shadow_has_shared_compiler_codec_and_paint_owners(self):
        css = read('ui/desktop/desktop.css')
        self.assertIn('text-shadow:0px 1px 2px #000000b8', css)
        self.assertIn('"text-shadow" =>', read('crates/flamewm-render-compiler/src/compile.rs'))
        core = read('crates/flamewm-render-core/src/paint.rs')
        self.assertIn('if let Some(shadow) = style.text_shadow', core)
        self.assertLess(core.index('x: rect.x + shadow.offset_x'), core.index('text: Arc::clone(&text)'))
        codec = read('crates/flamewm-render-core/src/codec.rs')
        self.assertIn('text_shadow: if version >= FORMAT_VERSION_SHADOW_LEGACY', codec)
        self.assertIn('v5_style_decodes_without_text_shadow', codec)
        self.assertIn('v6_shadow_roundtrip_preserves_offset_and_color', codec)
        self.assertIn('FORMAT_VERSION: u16 = 8', read('crates/flamewm-render-core/src/model/mod.rs'))

    def test_shadows_only_on_desktop_labels(self):
        css = read('ui/desktop/desktop.css')
        self.assertEqual(css.count('text-shadow:'), 1)
        self.assertIn('desktop_label_text_shadow_is_inherited_but_menus_remain_clean', read('crates/flamewm-render-compiler/src/compile.rs'))


if __name__ == '__main__':
    unittest.main()

"""Pass 26 source-contract regression for Cargo test imports, AA dock and labels."""
from pathlib import Path
import re
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
        # Dock radius publish moved from runtime.rs into runtime/panels.rs.
        code = read('crates/flamewm-shell/src/runtime.rs') + read('crates/flamewm-shell/src/runtime/panels.rs')
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
        # Desktop-label shadow became the shared --text-shadow token (commit ba51c1af).
        compiler = read('crates/flamewm-render-compiler/src/compile.rs')
        self.assertIn('DEFAULT_TEXT_SHADOW: &str = "0px 1px 2px #00000059"', compiler)
        self.assertIn('"text-shadow" =>', compiler)
        core = read('crates/flamewm-render-core/src/paint.rs')
        self.assertIn('let shadow = style.text_shadow.filter(|_| document.ui_text_shadow());', core)
        self.assertLess(core.index('x: rect.x + shadow.offset_x'), core.index('text: Arc::clone(&text)'))
        codec = read('crates/flamewm-render-core/src/codec.rs')
        self.assertIn('text_shadow: if version >= FORMAT_VERSION_SHADOW_LEGACY', codec)
        self.assertIn('v5_style_decodes_without_text_shadow', codec)
        self.assertIn('v6_shadow_roundtrip_preserves_offset_and_color', codec)
        self.assertIn('FORMAT_VERSION: u16 = 10',read('crates/flamewm-render-core/src/model/mod.rs'))

    def test_shadows_only_on_desktop_labels(self):
        # Superseded (ba51c1af): text shadow is a shared inherited token, so the
        # old per-label rule is gone. Check no desktop-label rule overrides it and
        # that the token opt-out test exists.
        css = read('ui/desktop/desktop.css')
        self.assertNotRegex(css, r'\.desktop-label[^{]*\{[^}]*text-shadow')
        self.assertIn('root_text_shadow_token_overrides_default_and_none_opts_out', read('crates/flamewm-render-compiler/src/compile.rs'))


if __name__ == '__main__':
    unittest.main()

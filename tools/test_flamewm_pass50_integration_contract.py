"""Pass 50 integration guards for independent agent patches and their shared contracts."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parent.parent


def source(path):
    return (ROOT / path).read_text()


class Pass50Integration(unittest.TestCase):
    def test_all_scale_steps_supported_without_five_presets_fallback(self):
        core = source('crates/flamewm-platform/src/scale.rs')
        settings = source('crates/flamewm-settings/src/lib.rs')
        self.assertIn('percent % 5 == 0', core)
        self.assertIn('display_scale_from_slider(text)?', settings)
        self.assertIn('self.client.set_shell_scale(percent)', settings)
        self.assertIn('scale_drag: Option<(OutputId, u64, u16)>', settings)
        self.assertIn('current_generation != generation', settings)

    def test_slider_markup_and_ui_projector_exist(self):
        markup = source('ui/settings.html')
        projection = source('crates/flamewm-settings/src/lib.rs')
        for name in ('display-scale-track', 'display-scale-fill', 'display-scale-thumb', 'display-scale-value'):
            self.assertIn(f'id="{name}"', markup)
            if name != "display-scale-track":
                self.assertIn(f'"{name}"', projection)
        self.assertIn('aria-valuemin="100" aria-valuemax="200"', markup)

    def test_desktop_grid_scales_all_steps(self):
        grid = source('crates/flamewm-desktop-core/src/layout.rs')
        self.assertIn('scale_percent % 5 == 0', grid)
        self.assertIn('for scale in (100..=200).step_by(5)', grid)

    def test_global_cursor_shadow_has_live_independent_process_observers(self):
        exports = ('crates/flamewm-ui-x11/src/lib.rs', 'crates/flamewm-wm-x11/src/lib.rs', 'crates/flamewm-wm/src/lib.rs')
        for path in exports:
            self.assertIn('apply_cursor_shadow', source(path), path)
        for path in ('crates/flamewm-shell/src/cursor_theme.rs', 'crates/flamewm-settings/src/lib.rs',
                     'crates/flamewm-wm/src/main.rs', 'crates/flamewm-desktop/src/main.rs'):
            self.assertIn('apply_cursor_shadow(', source(path), path)
        wm = source('crates/flamewm-wm/src/main.rs')
        self.assertGreaterEqual(wm.count('sync_cursor_shadow(snapshot);'), 2)
        self.assertIn('sync_cursor_shadow(snapshot);', wm.split('let observer: SettingsObserver', 1)[1].split('let input = xsettings_input_from_snapshot', 1)[0])

    def test_native_shadow_cursor_checks_allocations_before_write(self):
        cursor = source('crates/flamewm-render-x11/src/native/cursor.rs')
        self.assertIn('(*new_set).images.is_null()', cursor)
        self.assertIn('(*created).pixels.is_null()', cursor)
        self.assertLess(cursor.index('(*created).pixels.is_null()'), cursor.index('ptr::copy_nonoverlapping(shadow.as_ptr()'))

    def test_default_icon_preview_uses_catalog_resolution(self):
        picker = source('crates/flamewm-settings/src/theme_picker.rs')
        settings = source('crates/flamewm-settings/src/lib.rs')
        self.assertIn('pub fn selected_preview(', picker)
        self.assertIn('self.themes.selected_preview(kind, &value)', settings)
        self.assertIn('id="icon-theme-selected-preview"', source('ui/settings.html'))

    def test_cursor_policy_preserves_fallback_and_curated_chooser(self):
        picker = source('crates/flamewm-settings/src/theme_picker.rs')
        session = source('crates/flamewm-session-core/src/lib.rs')
        self.assertIn('with_curated_cursor_themes(', picker)
        self.assertIn('MCMOJAVE_CURSOR_THEME_ID', session)
        self.assertIn('CursorThemeFamily::Oxygen', picker)
        self.assertIn('CURSOR_THEME_ID: &str = "FlameWM-Breeze-Dark"', session)

    def test_windows11_control_hover_has_style_dependent_corner_policy(self):
        code = source('crates/flamewm-wm-x11/src/wm/chrome_runtime.rs')
        frame = source('crates/flamewm-wm-x11/src/frame/chrome.rs')
        self.assertIn('titlebar_height', code)
        self.assertIn('titlebar_height', frame)
        self.assertIn('scaled_corner_radius(', frame)

    def test_shadow_blur_is_native_renderer_feature(self):
        compiler = source('crates/flamewm-render-compiler/src/compile.rs')
        painter = source('crates/flamewm-render-core/src/paint.rs')
        desktop_css = source('ui/desktop/desktop.css')
        self.assertIn('blur_radius', compiler)
        self.assertIn('blur_radius', painter)
        self.assertIn('text-shadow:0px 1px 2px #000000b8', desktop_css)


if __name__ == '__main__':
    unittest.main()

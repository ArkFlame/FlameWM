"""Focused offline checks for Pass18 UI source owner contracts (not native X11 proof)."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]
def source(p): return (ROOT / p).read_text(encoding='utf-8')

class DesktopPass18(unittest.TestCase):
    def test_delete_uses_typed_key_and_preserves_editing(self):
        s=source('crates/flamewm-desktop/src/main.rs')
        self.assertIn('UiControllerEvent::Key(key) => handle_desktop_key(key, document, &mut state)', s)
        self.assertIn('key.phase != UiKeyPhase::Press || key.key != "Delete"', s)
        self.assertIn('state.rename.is_some() || sticky::is_editing(state)', s)
        self.assertIn('state.pending_delete = Some(paths)', s)
        self.assertIn('show_confirm(document, state)', s)
    def test_drag_hides_source_then_restores_all_selections(self):
        s=source('crates/flamewm-desktop/src/main.rs')
        self.assertIn('source_ids: Vec<String>', s)
        self.assertIn('drag.originals.iter().map(|item| item.id.clone()).collect()',s)
        self.assertIn('document.position("desktop-drag-ghost", gx, gy)',s)
        self.assertIn('if ghost.source_ids.contains(&item.id)',s)
        self.assertIn('document.visible(&format!("desktop-item-{index}"), false)',s)
        self.assertIn('document.visible(&format!("desktop-item-{index}"), true)',s)
        self.assertIn('UiActionPhase::Motion | UiActionPhase::Release',s)
    def test_native_radial_is_painted_only_on_hover(self):
        h=source('ui/desktop/index.html'); c=source('ui/desktop/desktop.css')
        s=source('crates/flamewm-desktop/src/main.rs'); core=source('crates/flamewm-desktop/src/convert_wheel.rs')
        self.assertIn('id="convert-wheel-highlight"',h)
        self.assertIn('data-action="desktop.convert.surface"',h)
        self.assertNotIn('desktop-convert-wheel-art',h)
        self.assertNotIn('eight-petal',c.lower())
        self.assertNotIn('f3b891',c)
        self.assertIn('projection::project_menu_surface(document, "desktop-convert-wheel", &state.settings)',s)
        self.assertIn('convert_wheel::sector_at',s)
        self.assertIn('convert_wheel::highlight_pixels',s)
        self.assertIn('UiActionPhase::Hover',s)
        self.assertIn('pub fn sector_at(',core)
        self.assertIn('pub fn highlight_pixels(',core)
        for i in range(8): self.assertIn(f'data-action="convert.slot.{i}"',h)
    def test_strict_compiler_sources_no_legacy_asset(self):
        b=source('crates/flamewm-desktop/build.rs')
        ui=source('crates/flamewm-desktop/ui_contract.rs')
        self.assertIn('"convert-wheel-highlight"',ui)
        self.assertNotIn('wheel_art_path',b)
        self.assertFalse((ROOT/'ui/desktop/convert-wheel.svg').exists())
        self.assertNotIn('text-align:',source('ui/desktop/desktop.css'))
    def test_desktop_click_claims_keyboard_focus_without_touching_other_roles(self):
        events=source('crates/flamewm-render-x11/src/app/events.rs')
        app=source('crates/flamewm-render-x11/src/app/mod.rs')
        self.assertIn('desktop_takes_focus_on_press(self.role, button.button)',events)
        self.assertIn('XSetInputFocus(',events)
        self.assertIn('pub(crate) role: X11WindowRole',app)
        self.assertIn('role: config.role',app)
    def test_ten_prototypes_remain_separate_from_application(self):
        # The HTML preview gallery is delivered separately, NOT packaged into FlameWM.
        self.assertFalse((ROOT/'ui/desktop/convert-wheel-10-concepts.html').exists())

if __name__ == '__main__': unittest.main()

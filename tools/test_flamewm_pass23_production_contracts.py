"""Source guards for installed-asset paths, primary RandR ownership and hotkeys.
These do not substitute for Cargo or native X11 acceptance.
"""
from pathlib import Path
import unittest
ROOT=Path(__file__).resolve().parents[1]
def src(path): return (ROOT/path).read_text()

class ProductionContracts(unittest.TestCase):
    def test_titlebar_icons_use_installed_root(self):
        s=src('crates/flamewm-wm-x11/src/chrome.rs')
        block=s.split('pub fn control_glyph_raster_shared(',1)[1].split('/// Backing mask behind a control glyph.',1)[0]
        self.assertIn('titlebar_backdrop::packaged_root()',block)
        self.assertNotIn('env!("CARGO_MANIFEST_DIR")',block)
        self.assertIn('fallback_control_glyph(role, edge, scheme.text)',block)
    def test_wm_titlebar_and_task_icons_reuse_one_asset_root(self):
        s=src('crates/flamewm-wm-x11/src/wm/mod.rs')
        self.assertIn('crate::frame::titlebar_backdrop::packaged_root()',s)
    def test_weather_broadcast_all_monitor_panels(self):
        s=src('crates/flamewm-shell/src/app.rs'); block=s.split('fn tick_weather(',1)[1].split('fn push_weather_feed(',1)[0]
        self.assertIn('self.surfaces.panel_handles()',block)
        self.assertNotIn('runtime.with_document(self.surfaces.panel(),',block)
        self.assertIn('runtime.redraw(panel)',block)
    def test_randr_two_providers_bind_physical_primary(self):
        for rel in ['crates/flamewm-wm-x11/src/lib.rs','crates/flamewm-x11-desktop/src/lib.rs']:
            s=src(rel); self.assertIn('randr_get_output_primary(self.root)',s)
            self.assertIn('primary: output == primary_output',s)
            self.assertIn('outputs.sort_by(|a, b| a.connector.cmp(&b.connector))',s)
    def test_desktop_name_from_metadata_even_without_exec(self):
        s=src('crates/flamewm-applications/src/lib.rs')
        self.assertIn('pub fn desktop_file_display_name(',s)
        d=src('crates/flamewm-desktop/src/main.rs')
        self.assertIn('desktop_item_label(state, item)',d)
        self.assertIn('desktop_file_display_name(&item.path)',d)
    def test_all_four_native_shortcut_bindings(self):
        s=src('crates/flamewm-platform/src/shortcuts.rs')
        for binding in ['Super+E','Ctrl+Alt+E','Super+T','Ctrl+Alt+T']:
            self.assertIn(f'KeyBinding::assigned("{binding}")',s)
        events=src('crates/flamewm-wm-x11/src/wm/events.rs')
        for action in ['ACTION_OPEN_FILES_SUPER','ACTION_OPEN_FILES_CTRL_ALT','ACTION_OPEN_TERMINAL_SUPER','ACTION_OPEN_TERMINAL_CTRL_ALT']:
            self.assertIn(action,events)
    def test_no_fabricated_monitor_drag_action(self):
        s=src('crates/flamewm-settings/src/lib.rs')
        self.assertNotIn('settings.display.drag-simulation',s)
    def test_doctor_never_introspects_wallet_contents(self):
        s=src('tools/flamewm_production_doctor.py')
        self.assertIn('busctl',s)
        for secret in ['kwallet-query','kwalletcli','secret-tool lookup','getPassword']:
            self.assertNotIn(secret,s)

if __name__=='__main__': unittest.main()

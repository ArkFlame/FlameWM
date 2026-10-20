#!/usr/bin/env python3
"""Pass27 contracts: output identity, DPI monitor positioning, and real WM input."""
import unittest
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]
def source(path): return (ROOT/path).read_text()

class Pass27Contracts(unittest.TestCase):
    def test_session_is_flamewm_without_activating_kde_display_managers(self):
        session = source('crates/flamewm-session-core/src/lib.rs')
        self.assertIn('"XDG_CURRENT_DESKTOP".to_owned(), "FlameWM"', session)
        self.assertNotIn('"FlameWM:KDE"', session)
        self.assertIn('"XDG_SESSION_DESKTOP"', session)
        self.assertIn('"KDE_SESSION_VERSION"', session)
        self.assertIn('"/usr/bin/kwalletd6"', session)
        runner = source('crates/flamewm-wm/src/bin/flamewm-session.rs')
        self.assertIn('dbus-update-activation-environment', runner)
        self.assertIn('"XDG_CURRENT_DESKTOP"', runner)
        compat = source('crates/flamewm-wm/src/session_app_compat.rs')
        self.assertIn('flamewm-mimeapps.list', compat)
        self.assertIn('kde-mimeapps.list', compat)
        self.assertIn('Exec=env XDG_CURRENT_DESKTOP=KDE', compat)
        self.assertIn('finalize_pam_kwallet_login', runner)
    def test_monitor_layout_refresh_covers_secondary_panels(self):
        app = source('crates/flamewm-shell/src/app.rs')
        runtime = source('crates/flamewm-shell/src/runtime.rs')
        self.assertIn('self.surfaces.panel_topology_drifted(snapshot)', app)
        self.assertIn('panel_context_is_current(&record.placement, snapshot)', runtime)
        self.assertIn('secondary_monitor_swap_with_unchanged_generation_is_detected', runtime)
        self.assertIn('mixed_panel_edges_resolve_independently', runtime)
        wm_runtime = source('crates/flamewm-wm-x11/src/runtime.rs')
        head = wm_runtime.index('let display_before = host.cached_display_snapshot();')
        refresh = wm_runtime.index('if wm.take_display_topology_dirty()')
        self.assertLess(head, refresh, 'PRE-refresh snapshot required for RandR signal')
        self.assertIn('let panels_before = host.panels_snapshot();', wm_runtime[head:refresh])
        self.assertIn('panels_after != panels_before', wm_runtime[refresh:])
    def test_portal_config_is_packaged(self):
        portal = source('packaging/xdg-desktop-portal/flamewm-portals.conf')
        self.assertIn('OpenURI=kde;gtk', portal)
        self.assertIn('FileChooser=kde;gtk', portal)
        self.assertIn('flamewm-portals.conf', source('scripts/package'))
    def test_meta_left_button_reuses_titlebar_reducer_and_capture(self):
        pointer = source('crates/flamewm-wm-x11/src/wm/pointer.rs')
        self.assertIn('state & 0x0040 != 0', pointer)
        self.assertIn('source: resources.title_drag', pointer)
        self.assertIn('FrameEvent::Press', pointer)
        self.assertIn('Allow::ASYNC_POINTER', pointer)
        self.assertIn('Allow::REPLAY_POINTER', pointer)
    def test_settings_layout_rejects_invalid_outputs_and_handles_multiple_monitors(self):
        adapter = source('crates/flamewm-settings/src/display_layout.rs')
        self.assertIn('active.len() < 2', adapter)
        self.assertIn('active.swap(position, next)', adapter)
        self.assertIn('active.len() != plan.outputs.len()', adapter)
        self.assertIn('rollback_if_safe', adapter)
        self.assertIn('snapshot.pending.is_some()', adapter)
        self.assertIn('selected monitor is not an active RandR output', adapter)
        self.assertIn('Command::new("xrandr").args(&args)', adapter)
    def test_session_boot_restores_validated_order_and_no_stale_kde_session_name(self):
        runner = source('crates/flamewm-wm/src/bin/flamewm-session.rs')
        order = source('crates/flamewm-session-core/src/display_order.rs')
        wm = source('crates/flamewm-wm/src/main.rs')
        self.assertIn('restore_saved_display_order();', runner)
        self.assertIn('display_order::load_order()', runner)
        self.assertIn('"--right-of"', runner)
        self.assertIn('save_order(left: &str, right: &str)', order)
        self.assertIn('key == "XDG_SESSION_DESKTOP"', wm)
    def test_packager_preflights_required_portal_config(self):
        package = source('scripts/package')
        self.assertIn('"$ROOT/packaging/xdg-desktop-portal/flamewm-portals.conf"; do', package)
    def test_display_cards_control_native_layout(self):
        ui = source('ui/settings.html')
        settings = source('crates/flamewm-settings/src/lib.rs')
        self.assertIn('settings.display.arrange.left', ui)
        self.assertIn('settings.display.arrange.right', ui)
        self.assertIn('monitor_drag: Option<(usize, f32, u64, OutputId)>', settings)
        self.assertIn('event.phase == UiActionPhase::Press', settings)
        self.assertIn('distance.abs() >= 40.0', settings)
        self.assertIn('display_layout::plan(snapshot, &output, side)', settings)
        self.assertIn('display_layout::apply(&layout)', settings)
        self.assertIn('display_layout::verify(&layout, observed)', settings)
        self.assertIn('ordered_output_indices()', settings)

if __name__ == '__main__': unittest.main()

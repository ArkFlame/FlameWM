#!/usr/bin/env python3
"""Pass 35 source-level gates; native Cargo/Xephyr proofs are separate."""
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(path):
    return (ROOT / path).read_text(encoding="utf-8")


class XephyrPanelStartup(unittest.TestCase):
    def test_zero_mode_with_valid_root_geometry_remains_drawable(self):
        src = read("crates/flamewm-platform/src/services/panels.rs")
        self.assertIn("output.connected && output.geometry.is_valid()", src)
        self.assertNotIn("output.geometry.is_valid() && output.current_mode.0 != 0", src)
        self.assertIn("fn legacy_xephyr_root_without_randr_mode_still_has_a_panel()", src)
        self.assertIn("Rect::new(0, 597, 1350, 44)", src)

    def test_disabled_and_disconnected_outputs_still_cannot_acquire_panels(self):
        src = read("crates/flamewm-platform/src/services/panels.rs")
        self.assertIn("fn attached_but_disabled_crtc_cannot_create_phantom_panel()", src)
        self.assertIn("fn disconnected_or_geometryless_modes_never_create_panels()", src)
        native = read("crates/flamewm-wm-x11/src/lib.rs")
        self.assertIn("single_output_root_geometry(single_randr_output, root_size, &modes)", native)
        self.assertIn("if !single_output || !root_size.is_valid()", native)
        self.assertIn("fn lone_xephyr_output_without_mode_keeps_its_root_geometry()", native)

    def test_bootstrap_reconciles_displays_before_panels(self):
        src = read("crates/flamewm-control-core/src/lib.rs")
        body = src.split("ControlRequest::GetShellBootstrap => {", 1)[1].split("ControlRequest::GetWindows => {", 1)[0]
        refresh = body.index("let displays = host.display_snapshot()?;")
        reconcile = body.index("host.resync_panels_for_displays(&displays)?;")
        collect = body.index("panels: host.panels_snapshot(),")
        self.assertTrue(refresh < reconcile < collect)
        self.assertIn("displays,", body)
        tests = read("crates/flamewm-control-core/tests/platform_control.rs")
        self.assertIn("fn shell_bootstrap_reconciles_display_and_panels_before_reply()", tests)

    def test_hotplug_restores_persisted_panel_presentation(self):
        src = read("crates/flamewm-platform/src/host.rs")
        body = src.split("pub fn resync_panels_for_displays(", 1)[1].split("pub fn begin_display_mode(", 1)[0]
        self.assertIn("self.seed_persisted_panel_edges();", body)
        self.assertIn("self.panels.revision() != previous_revision", body)
        self.assertIn("reconcile_taskbar_height", body)
        self.assertIn("reconcile_taskbar_frame", body)
        self.assertIn("fn panel_edge_persists_across_output_disconnect_reconnect()", src)

    def test_shell_failure_reports_output_state(self):
        src = read("crates/flamewm-shell/src/runtime.rs")
        self.assertIn("GetPanels returned no panel (displays: [{outputs}])", src)


if __name__ == "__main__":
    unittest.main()

#!/usr/bin/env python3
"""Pass 33: ensure all four agent changes and Pass 32 packaging coexist."""
from pathlib import Path
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[1]


def read(relative: str) -> str:
    return (ROOT / relative).read_text(encoding="utf-8")


class Pass33MergeContract(unittest.TestCase):
    def test_agent01_cursor_placement_reaches_wm_map(self):
        manager = read("crates/flamewm-wm-x11/src/wm/manage.rs")
        layout = read("crates/flamewm-wm-x11/src/frame/geometry.rs")
        self.assertIn("can_place_near_pointer", manager)
        self.assertIn("query_pointer(screen.root)", manager)
        self.assertIn("plan_initial_map_near_pointer", manager)
        self.assertIn("plan_initial_map_near_pointer", layout)

    def test_agent02_launch_feedback_has_real_shell_consumer(self):
        shell = read("crates/flamewm-shell/src/main.rs")
        self.assertIn("launch_feedback::LaunchFeedback::connect", shell)
        self.assertIn("launch_feedback::LaunchFeedback", shell)
        self.assertIn("StartupX11Observer", read("crates/flamewm-shell/src/launch_feedback.rs"))
        self.assertIn("fn broadcast(", read("crates/flamewm-applications/src/launch_feedback.rs"))
        self.assertTrue((ROOT / "ui/shell/launch-feedback.html").is_file())

    def test_agent03_wallet_integration_preserved(self):
        session = read("crates/flamewm-wm/src/bin/flamewm-session.rs")
        compatible = read("crates/flamewm-wm/src/session_app_compat.rs")
        self.assertIn("session_app_compat::finalize_pam_kwallet_login", session)
        self.assertIn("pub fn finalize_pam_kwallet_login", compatible)
        self.assertNotIn("kwallet_delete", compatible)
        self.assertNotIn("password_store_migrate", compatible)

    def test_agent04_topology_and_spatial_ui_preserved(self):
        runtime = read("crates/flamewm-wm-x11/src/runtime.rs")
        service = read("crates/flamewm-platform/src/services/panels.rs")
        layout = read("crates/flamewm-settings/src/display_layout.rs")
        self.assertIn("host.cached_display_snapshot()", runtime)
        self.assertIn("host.resync_panels_for_displays", runtime)
        # Pass 35 allows Xephyr's drawable root with ModeId(0), while still
        # rejecting disconnected outputs and disabled, zero-size CRTCs.
        self.assertIn("output.connected && output.geometry.is_valid()", service)
        self.assertIn("fn attached_but_disabled_crtc_cannot_create_phantom_panel()", service)
        self.assertIn("fn legacy_xephyr_root_without_randr_mode_still_has_a_panel()", service)
        self.assertIn("rollback_if_safe", layout)
        self.assertIn("settings.display.arrange.left", read("ui/settings.html"))
        self.assertIn("monitor_drag: Option<", read("crates/flamewm-settings/src/lib.rs"))

    def test_session_addon_is_actual_packaged_entrypoint(self):
        self.assertIn("Exec=/usr/libexec/flamewm/flamewm-session-wrapper", read("packaging/flamewm-session.desktop"))
        package = read("scripts/package")
        self.assertIn('"$ROOT/scripts/flamewm-session-wrapper"', package)
        self.assertIn('"$ROOT/tools/flamewm_desktop_services.py"', package)
        for file in ("scripts/flamewm-session-wrapper", "tools/flamewm_desktop_services.py"):
            self.assertTrue((ROOT / file).stat().st_mode & 0o111)
            self.assertIn("@exec " + file, read("packaging/source-manifest.txt"))

    def test_cargo_autobins_guard_and_removed_helper(self):
        manifest = tomllib.loads(read("crates/flamewm-wm/Cargo.toml"))
        self.assertIs(manifest["package"]["autobins"], False)
        self.assertEqual({x["name"] for x in manifest["bin"]}, {"flamewm", "flamewm-session"})
        self.assertFalse((ROOT / "crates/flamewm-wm/src/bin/desktop_compat.rs").exists())
        self.assertIn('#[path = "../session_app_compat.rs"]', read("crates/flamewm-wm/src/bin/flamewm-session.rs"))

    def test_addon_does_not_redeclare_kde_desktop(self):
        self.assertIn("XDG_CURRENT_DESKTOP", read("tools/flamewm_desktop_services.py"))
        self.assertIn('DesktopNames=FlameWM', read("packaging/flamewm-session.desktop"))
        self.assertNotIn('DesktopNames=KDE', read("packaging/flamewm-session.desktop"))


if __name__ == "__main__":
    unittest.main()

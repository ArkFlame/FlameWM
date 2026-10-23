"""Regression checks for the Pass 42 screenshot merge and release wiring.

These are static contract checks, not substitutes for Cargo or native X11 tests.
"""
import re
import tomllib
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(relative):
    return (ROOT / relative).read_text(encoding="utf-8")


class ScreenshotMergeChecks(unittest.TestCase):
    def test_workspace_auto_discovers_screenshot_crate_and_lock_entry(self):
        workspace = tomllib.loads(read("Cargo.toml"))
        self.assertEqual(workspace["workspace"]["members"], ["crates/*"])
        manifest = tomllib.loads(read("crates/flamewm-screenshot/Cargo.toml"))
        self.assertEqual(manifest["package"]["name"], "flamewm-screenshot")
        self.assertEqual(manifest["bin"][0]["name"], "flamewm-screenshot")
        self.assertEqual(manifest["dependencies"]["png"], "=0.18.1")
        entries = tomllib.loads(read("Cargo.lock"))["package"]
        screenshot = [p for p in entries if p["name"] == "flamewm-screenshot"]
        self.assertEqual(len(screenshot), 1)
        self.assertIn("x11rb", screenshot[0]["dependencies"])
        self.assertIn("png", screenshot[0]["dependencies"])

    def test_concrete_x11_clipboard_request_limit_trait_is_imported(self):
        # RustConnection's maximum_request_bytes() is supplied by this trait.
        # The generic Connection bound in ui.rs does not establish scope here.
        clipboard = read("crates/flamewm-screenshot/src/clipboard.rs")
        # The call may be split across lines (`let chunk = conn\n    .maximum_request_bytes()`).
        self.assertRegex(clipboard, r"\bconn\s*\.\s*maximum_request_bytes\s*\(\s*\)")
        self.assertRegex(
            clipboard,
            r"use\s+x11rb::connection::\{[^}]*\bRequestConnection\b[^}]*\};",
        )

    def test_all_screenshot_sources_present(self):
        for path in ("main.rs", "capture.rs", "ui.rs", "clipboard.rs"):
            with self.subTest(path=path):
                self.assertTrue((ROOT / "crates/flamewm-screenshot/src" / path).is_file())

    def test_actions_flow_from_api_to_platform_and_wm(self):
        api = read("crates/flamewm-api/src/shortcuts.rs")
        platform = read("crates/flamewm-platform/src/shortcuts.rs")
        wm = read("crates/flamewm-wm-x11/src/wm/events.rs")
        for action in ("ACTION_SCREENSHOT_FULL", "ACTION_SCREENSHOT_REGION", "ACTION_SCREENSHOT_WINDOW"):
            with self.subTest(action=action):
                self.assertIn(f"pub const {action}", api)
                self.assertIn(action, platform)
                self.assertIn(action, wm)
        for keysym in ('"Print"', '"Shift+Print"', '"Ctrl+Print"'):
            self.assertIn(keysym, platform)
        self.assertIn('Command::new("flamewm-screenshot")', wm)

    def test_settings_rows_and_registry_are_bound(self):
        rust = read("crates/flamewm-settings/src/lib.rs")
        html = read("ui/settings.html")
        for slot in ("shot-full", "shot-region", "shot-window"):
            with self.subTest(slot=slot):
                self.assertIn(f'slot: "{slot}"', rust)
                self.assertIn(f'id="hotkey-key-{slot}"', html)
                self.assertIn(f'data-action="settings.hotkey.{slot}"', html)

    def test_release_binary_and_launcher_registered_in_all_package_paths(self):
        package = read("scripts/package")
        installed = read("packaging/install-manifest.txt")
        # Both release paths in scripts/package walk an explicit binary list: the
        # existence check and the install loop. Every loop must carry the binaries.
        loops = re.findall(r"^\s*for binary in ([^;]*);\s*do\b", package, re.MULTILINE)
        self.assertGreaterEqual(len(loops), 2, "expected the release existence and install binary loops")
        for index, binaries in enumerate(loops):
            available = binaries.split()
            for binary in ("flamewm-settings", "flamewm-screenshot", "flamewm-render-x11"):
                with self.subTest(loop=index, binary=binary):
                    self.assertIn(binary, available)
        for expected in (
            '"$ROOT/packaging/flamewm-screenshot.desktop"',
            '"$STAGE/usr/share/applications/flamewm-screenshot.desktop"',
        ):
            self.assertIn(expected, package)
        self.assertIn(".target/release/flamewm-screenshot", installed)
        self.assertIn("packaging/flamewm-screenshot.desktop", installed)

    def test_screenshot_desktop_file_has_valid_commands(self):
        desktop = read("packaging/flamewm-screenshot.desktop")
        self.assertIn("Type=Application", desktop)
        self.assertIn("TryExec=flamewm-screenshot", desktop)
        self.assertIn("Exec=flamewm-screenshot --region --annotate", desktop)
        for section in ("[Desktop Action Screen]", "[Desktop Action Window]", "[Desktop Action Region]"):
            self.assertIn(section, desktop)

    def test_pass41_compile_hotfix_remains_present(self):
        sticky = read("crates/flamewm-desktop/src/sticky.rs")
        main = read("crates/flamewm-desktop/src/main.rs")
        self.assertIn("pointer_on_output", sticky)
        self.assertIn("grid_for_output(state, &output)", main)
        self.assertIn("let origin = grid_for_output(state, &output).cell_to_pixel(cell);", main)

    def test_source_allowlist_includes_new_crate(self):
        manifest = read("packaging/source-manifest.txt")
        self.assertIn("crates/\n", manifest)
        self.assertIn("packaging/\n", manifest)
        self.assertIn("docs/\n", manifest)


if __name__ == "__main__":
    unittest.main()

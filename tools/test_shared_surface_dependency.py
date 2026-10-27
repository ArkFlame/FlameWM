"""Guard the direct-dependency contract used by the shared X11 surface adapter.

An indirect workspace crate dependency cannot be imported with `use flamewm_api`;
the importing crate must name that dependency in its own Cargo.toml.
"""
from __future__ import annotations

import tomllib
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "crates/flamewm-ui-x11/Cargo.toml"
LOCK = ROOT / "Cargo.lock"
SOURCE = ROOT / "crates/flamewm-ui-x11/src/surface_style.rs"


class X11SurfaceDependencyContract(unittest.TestCase):
    def test_direct_api_dependency_exists_with_local_workspace_path(self):
        manifest = tomllib.loads(MANIFEST.read_text())
        api = manifest["dependencies"].get("flamewm-api")
        self.assertIsInstance(api, dict)
        self.assertEqual((MANIFEST.parent / api["path"]).resolve(), ROOT / "crates/flamewm-api")
        self.assertIn("use flamewm_api::settings::SettingsSnapshot;", SOURCE.read_text())

    def test_cargo_lock_records_the_direct_dependency(self):
        lock = tomllib.loads(LOCK.read_text())
        entries = [item for item in lock["package"] if item["name"] == "flamewm-ui-x11"]
        self.assertEqual(len(entries), 1)
        self.assertIn("flamewm-api", entries[0]["dependencies"])
        self.assertEqual(sum(item["name"] == "flamewm-api" for item in lock["package"]), 1)

    def test_build_preflight_precedes_release_build(self):
        script = (ROOT / "scripts/xephyr").read_text()
        self.assertLess(script.index("tools/test_shared_surface_dependency.py"),
                        script.index("cargo build --quiet --release"))

    def test_no_unnecessary_parentheses_warnings_reintroduced(self):
        sources = (
            "crates/flamewm-render-x11/src/app/mod.rs",
            "crates/flamewm-render-x11/src/external_drawable.rs",
            "crates/flamewm-render-x11/src/native/image.rs",
        )
        for source in sources:
            self.assertNotRegex((ROOT / source).read_text(), r"let prem = \|c: u32\| \(\(c \*")
        canary = (ROOT / "crates/flamewm-x11-canary/src/main.rs").read_text()
        self.assertNotIn("(w.map_state != u8::from(MapState::VIEWABLE))", canary)


if __name__ == "__main__":
    unittest.main()

#!/usr/bin/env python3
"""Regression tests for the shared rim contract in ui_contract_guard.check_rim_tokens.

Run: python3 -I tools/test_rim_token_guard.py
"""

from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import ui_contract_guard as guard  # noqa: E402

REPO = Path(__file__).resolve().parents[1]


def build_tree(root, sheet_text=None, percent=25):
    for rel in guard.RIM_SHEETS:
        path = root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(":root { --rim:#ffffff3f; }\n.x { border:1px solid var(--rim); }\n")
    if sheet_text is not None:
        (root / guard.RIM_SHEETS[0]).write_text(sheet_text)
    platform = root / "crates/flamewm-platform/src/settings.rs"
    platform.parent.mkdir(parents=True, exist_ok=True)
    platform.write_text(f"surface_border_opacity: {percent},\n")
    glass = root / "crates/flamewm-ui-core/src/glass.rs"
    glass.parent.mkdir(parents=True, exist_ok=True)
    glass.write_text(f"pub const RIM_PERCENT: u8 = {percent};\n")
    return root


class RimTokenGuardTests(unittest.TestCase):
    def run_guard(self, **kwargs):
        with tempfile.TemporaryDirectory() as tmp:
            build_tree(Path(tmp), **kwargs)
            errors = []
            guard.check_rim_tokens(Path(tmp), errors)
            return errors

    def test_clean_tree_passes(self):
        self.assertEqual(self.run_guard(), [])

    def test_literal_border_colour_fails(self):
        errors = self.run_guard(sheet_text=":root { --rim:#ffffff3f; }\n.x { border:1px solid #46484b; }\n")
        self.assertTrue(any("hardcoded border colour '#46484b'" in e for e in errors), errors)

    def test_rgba_border_colour_fails(self):
        errors = self.run_guard(sheet_text=":root { --rim:#ffffff3f; }\n.x { border-color: rgba(255,255,255,.2); }\n")
        self.assertTrue(any("hardcoded border colour" in e for e in errors), errors)

    def test_transparent_border_is_allowed(self):
        errors = self.run_guard(sheet_text=":root { --rim:#ffffff3f; }\n.x { border:1px solid #00000000; }\n")
        self.assertEqual(errors, [])

    def test_rim_colour_must_match_percent_owner(self):
        with tempfile.TemporaryDirectory() as tmp:
            build_tree(Path(tmp))
            (Path(tmp) / guard.RIM_SHEETS[0]).write_text(":root { --rim:#ffffff3d; }\n")
            errors = []
            guard.check_rim_tokens(Path(tmp), errors)
            self.assertTrue(any("--rim #ffffff3d != RIM_PERCENT" in e for e in errors), errors)

    def test_platform_opacity_must_match_owner(self):
        with tempfile.TemporaryDirectory() as tmp:
            build_tree(Path(tmp))
            (Path(tmp) / "crates/flamewm-platform/src/settings.rs").write_text(
                "surface_border_opacity: 8,\n")
            errors = []
            guard.check_rim_tokens(Path(tmp), errors)
            self.assertTrue(any("surface_border_opacity default" in e for e in errors), errors)

    def test_real_tree_passes(self):
        errors = []
        guard.check_rim_tokens(REPO, errors)
        self.assertEqual(errors, [])


if __name__ == "__main__":
    unittest.main()

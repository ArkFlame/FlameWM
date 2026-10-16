#!/usr/bin/env python3
"""Negative end-to-end test: the architecture guard must fail on forbidden bypasses.

Copies the live source tree into a temp root, injects a geometry bypass and a
render bypass, and asserts the guard CLI exits non-zero. Run:
    python3 -I tools/test_architecture_guard.py
"""
from __future__ import annotations

import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
COPY_DIRS = ("crates", "docs", "scripts", "ui", "tools")
COPY_FILES = (".gitignore",)
IGNORE = shutil.ignore_patterns("target", "__pycache__", ".vendor")

# Forbidden: a second window-geometry writer outside the frame engine
# (J18-G01) and a direct legacy paint call outside the title owner (J18-G04).
GEOMETRY_BYPASS = "crates/flamewm-wm-x11/src/bypass_geometry.rs"
GEOMETRY_BYPASS_SRC = "pub fn nudge(s: &mut State, r: Rect) { s.outer = r; }\n"
RENDER_BYPASS = "crates/flamewm-wm-x11/src/bypass_render.rs"
RENDER_BYPASS_SRC = "pub fn draw(x: Ctx) { paint::paint_frame(x); }\n"
SURFACE_OWNER = "crates/flamewm-render-x11/src/native/target.rs"


def run_guard(root: Path) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, "-I", str(root / "tools" / "architecture_guard.py")],
        capture_output=True,
        text=True,
        cwd=root,
    )


class ArchitectureGuardNegativeTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp(prefix="arch-guard-"))
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)
        for name in COPY_DIRS:
            shutil.copytree(ROOT / name, self.tmp / name, ignore=IGNORE, symlinks=True)
        for name in COPY_FILES:
            if (ROOT / name).is_file():
                shutil.copy2(ROOT / name, self.tmp / name)

    def test_clean_copy_passes(self) -> None:
        result = run_guard(self.tmp)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_missing_required_owner_fails_loudly(self) -> None:
        # Each path is a live owner the checks read; deleting it must not be a silent skip.
        for owner in (
            "crates/flamewm-shell/src/app.rs",
            "crates/flamewm-skin/src/icons.rs",
            "crates/flamewm-wm-x11/src/frame/controller.rs",
            "scripts/xephyr",
            "tools/performance_retention.py",
        ):
            with self.subTest(owner=owner):
                copy = Path(tempfile.mkdtemp(prefix="arch-guard-owner-"))
                self.addCleanup(shutil.rmtree, copy, ignore_errors=True)
                for name in COPY_DIRS:
                    shutil.copytree(ROOT / name, copy / name, ignore=IGNORE, symlinks=True)
                for name in COPY_FILES:
                    if (ROOT / name).is_file():
                        shutil.copy2(ROOT / name, copy / name)
                (copy / owner).unlink()
                result = run_guard(copy)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn(owner, result.stdout + result.stderr)
                self.assertNotIn("OK architecture boundary guard", result.stdout)

    def test_injected_geometry_and_render_bypass_fails(self) -> None:
        (self.tmp / GEOMETRY_BYPASS).write_text(GEOMETRY_BYPASS_SRC, encoding="utf-8")
        (self.tmp / RENDER_BYPASS).write_text(RENDER_BYPASS_SRC, encoding="utf-8")
        result = run_guard(self.tmp)
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("j18-g01-frame-geometry-owner", result.stderr)
        self.assertIn("j18-g04-no-old-decoration-paint", result.stderr)

    def test_j7_surface_owner_checks_its_own_text(self) -> None:
        # J7-G8 reads the surface owner's own text, not paint.rs: a surface file
        # lacking the semantic-foreground token fails; one containing it does not.
        surface = self.tmp / SURFACE_OWNER
        original = surface.read_text(encoding="utf-8")
        surface.write_text(original + "\n// no foreground token here\n", encoding="utf-8")
        result = run_guard(self.tmp)
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("j7-g8-semantic-foreground", result.stdout + result.stderr)

        surface.write_text(
            original + "\nfn fg(style: &Style) { resolve_color(style.color); }\n",
            encoding="utf-8",
        )
        result = run_guard(self.tmp)
        self.assertNotIn("j7-g8-semantic-foreground", result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()

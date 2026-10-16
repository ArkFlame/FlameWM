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

    def test_injected_geometry_and_render_bypass_fails(self) -> None:
        (self.tmp / GEOMETRY_BYPASS).write_text(GEOMETRY_BYPASS_SRC, encoding="utf-8")
        (self.tmp / RENDER_BYPASS).write_text(RENDER_BYPASS_SRC, encoding="utf-8")
        result = run_guard(self.tmp)
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("j18-g01-frame-geometry-owner", result.stderr)
        self.assertIn("j18-g04-no-old-decoration-paint", result.stderr)


if __name__ == "__main__":
    unittest.main()

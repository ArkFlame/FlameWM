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
import tomllib
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
COPY_DIRS = ("crates", "docs", "scripts", "ui", "tools")
# Cargo.toml supplies the workspace version the guard validates `remove_by` against.
COPY_FILES = (".gitignore", "Cargo.toml")
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


def copy_tree(target: Path) -> None:
    """Copy the tree the guard reads into `target`."""
    for name in COPY_DIRS:
        shutil.copytree(ROOT / name, target / name, ignore=IGNORE, symlinks=True)
    for name in COPY_FILES:
        if (ROOT / name).is_file():
            shutil.copy2(ROOT / name, target / name)


class ArchitectureGuardNegativeTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp(prefix="arch-guard-"))
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)
        copy_tree(self.tmp)

    def test_clean_copy_passes(self) -> None:
        result = run_guard(self.tmp)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("OK architecture boundary guard", result.stdout)

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
                copy_tree(copy)
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


class RemoveByValidationTest(unittest.TestCase):
    """`remove_by` must parse as MAJOR.MINOR.PATCH and outlive the workspace version.

    Every case edits the copied TOML under the temp root, so the live
    docs/ARCHITECTURE_EXCEPTIONS.toml is never touched.
    """

    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp(prefix="arch-guard-removeby-"))
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)
        copy_tree(self.tmp)
        self.workspace_version = self._workspace_version()
        self.toml = self.tmp / "docs" / "ARCHITECTURE_EXCEPTIONS.toml"
        self.original = self.toml.read_text(encoding="utf-8")
        self.assertIn('remove_by = "', self.original)

    def _workspace_version(self) -> tuple[int, int, int]:
        with (self.tmp / "Cargo.toml").open("rb") as stream:
            raw = tomllib.load(stream)["workspace"]["package"]["version"]
        return tuple(int(part) for part in raw.split("."))

    def _rewrite(self, value: str) -> None:
        """Set the first exception's `remove_by` to `value`, keeping the rest intact."""
        marker = 'remove_by = "'
        start = self.original.index(marker) + len(marker)
        end = self.original.index('"', start)
        self.toml.write_text(
            self.original[:start] + value + self.original[end:], encoding="utf-8"
        )

    def test_malformed_remove_by_fails(self) -> None:
        # Every case runs the real CLI, so the list stays small on purpose: a
        # malformed value must fail, but shape variants are one code path.
        for bad in ("0.2", "1.0.0.0", "v0.2.0", ""):
            with self.subTest(remove_by=bad):
                self._rewrite(bad)
                result = run_guard(self.tmp)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("remove_by", result.stderr)
                if bad:
                    self.assertIn("MAJOR.MINOR.PATCH", result.stderr)
                self.assertNotIn("OK architecture boundary guard", result.stdout)

    def test_expired_remove_by_fails(self) -> None:
        major, minor, patch = self.workspace_version
        expired = (
            f"{major}.{minor}.{patch}",  # exactly the workspace version: not strictly future
            "0.0.9",  # the historical stale deadline the maintainer reported
        )
        for value in expired:
            with self.subTest(remove_by=value):
                self._rewrite(value)
                result = run_guard(self.tmp)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("has expired", result.stderr)
                self.assertIn(value, result.stderr)
                self.assertNotIn("OK architecture boundary guard", result.stdout)

    def test_future_remove_by_passes(self) -> None:
        major, minor, patch = self.workspace_version
        for value in (f"{major}.{minor}.{patch + 1}", f"{major}.{minor + 1}.0"):
            with self.subTest(remove_by=value):
                self._rewrite(value)
                result = run_guard(self.tmp)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn("OK architecture boundary guard", result.stdout)
                self.assertNotIn("ERROR architecture exception", result.stderr)


if __name__ == "__main__":
    unittest.main()

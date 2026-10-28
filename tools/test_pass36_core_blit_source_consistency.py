#!/usr/bin/env python3
"""Regression gates for the reported mixed-source build and invalid X11 core blits.

These tests validate source contracts only; they do not claim Cargo or live X11 proof.
"""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class CargoSourceConsistency(unittest.TestCase):
    def run_guard(self, root):
        return subprocess.run(
            [sys.executable, "-I", str(ROOT / "tools/verify_cargo_manifests.py"), str(root)],
            capture_output=True, text=True, timeout=15,
        )

    def test_real_archive_source_exports_graphics_contract(self):
        api = (ROOT / "crates/flamewm-api/src/lib.rs").read_text()
        graphics = (ROOT / "crates/flamewm-api/src/graphics.rs").read_text()
        self.assertIn("pub mod graphics;", api)
        for symbol in ("pub enum GraphicsProvider", "pub enum DisplayProtocol", "pub fn choose_provider("):
            self.assertIn(symbol, graphics)
        self.assertEqual(self.run_guard(ROOT).returncode, 0)

    def test_mixed_source_from_old_overlay_fails_before_cargo_build(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "crates/flamewm-api/src").mkdir(parents=True)
            (root / "crates/flamewm-render-x11/src/app").mkdir(parents=True)
            (root / "Cargo.toml").write_text('[workspace]\nmembers = ["crates/*"]\n')
            (root / "crates/flamewm-api/Cargo.toml").write_text('[package]\nname="flamewm-api"\nversion="0.1.3"\n')
            (root / "crates/flamewm-render-x11/Cargo.toml").write_text('[package]\nname="flamewm-render-x11"\nversion="0.1.3"\n')
            (root / "crates/flamewm-render-x11/src/app/mod.rs").write_text('use flamewm_api::graphics::GraphicsProvider;\n')
            lib = root / "crates/flamewm-api/src/lib.rs"
            module = root / "crates/flamewm-api/src/graphics.rs"
            lib.write_text('pub mod geometry;\n')
            failed = self.run_guard(root)
            self.assertNotEqual(failed.returncode, 0)
            self.assertIn("does not export", failed.stderr)
            lib.write_text('pub mod graphics;\n')
            failed = self.run_guard(root)
            self.assertNotEqual(failed.returncode, 0)
            self.assertIn("graphics.rs is missing", failed.stderr)
            module.write_text('pub enum GraphicsProvider {}\npub enum DisplayProtocol {}\npub fn choose_provider() {}\n')
            self.assertEqual(self.run_guard(root).returncode, 0)


class X11FallbackDepthSafety(unittest.TestCase):
    def test_cached_external_drawable_uses_actual_depth_and_cache_reset(self):
        source = (ROOT / "crates/flamewm-render-x11/src/external_drawable.rs").read_text()
        self.assertIn('XGetGeometry(display, drawable', source)
        self.assertIn('unsafe { external_drawable_depth(view_display, drawable)? }', source)
        self.assertIn('upload_opaque_argb32_image(view_display, view_screen, fresh, depth, &words, w, h)', source)
        self.assertIn('create_pixmap(view_display, drawable, w, h, depth, root_w, root_h)', source)
        self.assertIn('self.invalidate_image_cache();', source[source.index('pub unsafe fn retarget('):source.index('fn session_view(')])
        self.assertNotIn('built = Some(words);\n                self.image_cache.insert(', source)

    def test_secondary_painter_rejects_alpha_and_matches_depth(self):
        source = (ROOT / "crates/flamewm-render-x11/src/external_drawable.rs").read_text()
        path = source[source.index('    pub fn rgba_blit('):source.index('    /// Apply the rounded-corner shape mask', source.index('    pub fn rgba_blit('))]
        self.assertLess(path.index('has_non_opaque_word(&argb)'), path.index('create_pixmap('))
        self.assertIn('let depth = if core { self.app.depth as u32 } else { 32 };', path)
        self.assertIn('create_pixmap(self.app.display, root, w, h, depth', path)
        self.assertIn('upload_opaque_argb32_image(', path)
        self.assertIn('rgba_blit needs XRender for non-opaque pixels', path)

    def test_core_upload_matches_visual_masks(self):
        source = (ROOT / "crates/flamewm-render-x11/src/app/images.rs").read_text()
        path = source[source.index('pub(crate) unsafe fn upload_opaque_argb32_image('):source.index('pub(crate) fn image_cache_key(')]
        self.assertIn('XDefaultDepth(display, screen)', path)
        self.assertIn('XMatchVisualInfo(', path)
        self.assertIn('rgb_to_pixel(', path)
        self.assertIn('XPutPixel(', path)
        self.assertIn('XPutImage(', path)
        self.assertIn('XDestroyImage(', path)


if __name__ == '__main__':
    unittest.main()

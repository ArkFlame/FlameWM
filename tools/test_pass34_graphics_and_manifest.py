"""Pass 34 source gates. They are not a substitute for native Rust + GPU tests."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class GraphicsBackendIntegration(unittest.TestCase):
    def test_both_x11_render_owners_use_same_negotiation(self):
        scene = (ROOT / "crates/flamewm-render-x11/src/app/mod.rs").read_text()
        chrome = (ROOT / "crates/flamewm-render-x11/src/external_drawable.rs").read_text()
        for source in (scene, chrome):
            self.assertIn('choose_provider(', source)
            self.assertIn('DisplayProtocol::X11', source)
            self.assertIn('FLAMEWM_X11_RENDER_PROVIDER', source)
            self.assertIn('if provider == GraphicsProvider::XCore', source)
        self.assertIn('xrender = None;', chrome)
        self.assertIn('xrender = None;', scene)

    def test_core_x11_cannot_keep_composited_argb_without_xrender(self):
        source = (ROOT / 'crates/flamewm-render-x11/src/app/mod.rs').read_text()
        self.assertIn('&& !core_requested', source)
        self.assertIn('alpha_mode = SurfaceAlphaMode::OpaqueFallback;', source)
        self.assertLess(source.index('let core_requested ='), source.index('let (visual, depth, mut alpha_mode)'))

    def test_wayland_real_startup_uses_negotiator_and_gl_backend(self):
        main = (ROOT / "crates/flamewm-wm-wayland/src/main.rs").read_text()
        render = (ROOT / "crates/flamewm-wm-wayland/src/render.rs").read_text()
        self.assertIn('choose_provider(DisplayProtocol::Wayland', main)
        self.assertIn('FLAMEWM_WAYLAND_RENDER_PROVIDER', main)
        self.assertIn('WinitGraphicsBackend<GlowRenderer>', render)
        self.assertIn('backend.submit(', render)
        self.assertIn('backend.bind()', render)

    def test_elision_shared_and_used_by_live_x11_and_wayland_paths(self):
        skin = (ROOT / 'crates/flamewm-skin/src/recipes/window_chrome.rs').read_text()
        x11 = (ROOT / 'crates/flamewm-wm-x11/src/frame/chrome.rs').read_text()
        wayland = (ROOT / 'crates/flamewm-wm-wayland/src/render.rs').read_text()
        self.assertIn('pub fn elide_title(', skin)
        self.assertIn('pub fn title_available_width(', skin)
        self.assertLess(x11.index('::elide_title('), x11.index('let title_width ='))
        self.assertIn('chrome::elide_title(', wayland)
        self.assertIn('let visible_title', wayland)

    def test_invalid_feature_dependency_is_rejected_before_a_release(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / 'crates' / 'bad').mkdir(parents=True)
            (root / 'Cargo.toml').write_text('[workspace]\nmembers=["crates/*"]\n')
            (root / 'crates/bad/Cargo.toml').write_text('[package]\nname="bad"\nversion="0.1.0"\n[features]\nrustix={workspace=true,features=["net"]}\n')
            test = subprocess.run([sys.executable, '-I', str(ROOT / 'tools/verify_cargo_manifests.py'), str(root)], capture_output=True, text=True)
            self.assertNotEqual(test.returncode, 0)
            self.assertIn('feature \'rustix\' must be an array', test.stderr)


if __name__ == '__main__':
    unittest.main()

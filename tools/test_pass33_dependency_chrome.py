import re
#!/usr/bin/env python3
"""Source-contract checks; live compositor and Cargo checks remain separate gates."""
import pathlib
import tomllib
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]


def manifest(name):
    return tomllib.loads((ROOT / 'crates' / name / 'Cargo.toml').read_text())


class AudioDependencyContract(unittest.TestCase):
    def test_pulse_optional_but_explicit_for_x11_wm(self):
        lib = manifest('flamewm-integrations-linux')
        pulse = lib['dependencies']['pulse']
        self.assertTrue(pulse['optional'])
        self.assertEqual(lib['features']['default'], [])
        self.assertIn('dep:pulse', lib['features']['pulse-audio'])
        self.assertIn('pulse-audio', manifest('flamewm-wm-x11')['dependencies']['flamewm-integrations-linux']['features'])
        for consumer in ('flamewm-desktop', 'flamewm-settings', 'flamewm-explorer', 'flamewm-shell'):
            self.assertNotIn('pulse-audio', manifest(consumer)['dependencies']['flamewm-integrations-linux'].get('features', []))

    def test_disabled_pulse_module_and_integration_test_are_gated(self):
        lib = (ROOT / 'crates/flamewm-integrations-linux/src/lib.rs').read_text()
        self.assertIn('#[cfg(feature = "pulse-audio")]\npub mod pulse;', lib)
        test = (ROOT / 'crates/flamewm-integrations-linux/tests/pulse_stress.rs').read_text()
        self.assertTrue(test.startswith('#![cfg(feature = "pulse-audio")]'))

    def test_shared_versions_and_drm_feature(self):
        root = tomllib.loads((ROOT / 'Cargo.toml').read_text())
        for dep in ('dbus', 'chrono'):
            self.assertIn(dep, root['workspace']['dependencies'])
        wayland = manifest('flamewm-wm-wayland')
        self.assertNotIn('backend_drm', wayland['dependencies']['smithay']['features'])
        self.assertIn('smithay/backend_drm', wayland['features']['drm'])
        self.assertIn('renderer_glow', wayland['dependencies']['smithay']['features'])


class RenderSafetyContract(unittest.TestCase):
    def test_smithay_api_result_is_converted_to_option(self):
        src = (ROOT / 'crates/flamewm-wm-wayland/src/render.rs').read_text()
        self.assertIn(').ok().map(Elem::Memory)', re.sub(r'\s+', '', src))
        self.assertIn(re.sub(r'\s+', '', 'chrome::control_button_geometries'), re.sub(r'\s+', '', src))
        self.assertIn(re.sub(r'\s+', '', 'chrome::center_title_x'), re.sub(r'\s+', '', src))
        self.assertIn(re.sub(r'\s+', '', 'managed.mode == Mode::Maximized'), re.sub(r'\s+', '', src))
        self.assertIn(re.sub(r'\s+', '', 'part.maximized != maximized'), re.sub(r'\s+', '', src))
        self.assertIn(re.sub(r'\s+', '', 'if rect.w <= 0 || rect.w > 32_767'), re.sub(r'\s+', '', src))

    def test_xwayland_selection_owner_timestamp_is_correct_type(self):
        src = (ROOT / 'crates/flamewm-wm-wayland/src/xwayland.rs').read_text()
        self.assertIn(re.sub(r'\s+', '', 'set_selection_owner(owner, selection, 0_u32)'), re.sub(r'\s+', '', src))

    def test_opaque_fallback_cannot_damage_before_reporting_alpha_error(self):
        src = (ROOT / 'crates/flamewm-render-x11/src/external_drawable.rs').read_text()
        start = src.index('fn blit_cached_argb32')
        end = src.index('pub fn apply_rounded_shape', start)
        path = src[start:end]
        self.assertLess(path.rindex('has_non_opaque_word'), path.index('self.image_cache.get(&key)'))
        self.assertLess(path.rindex('has_non_opaque_word'), path.rindex('XCopyArea('))


if __name__ == '__main__':
    unittest.main()

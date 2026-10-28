#!/usr/bin/env python3
"""Non-native regression guards for nested Wayland/XWayland integration (no Cargo)."""
import pathlib
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]

def source(path):
    return (ROOT / path).read_text()

class WaylandVisualContract(unittest.TestCase):
    def test_nested_matches_xephyr_viewport_and_has_override(self):
        w = source('crates/flamewm-wm-wayland/src/main.rs')
        self.assertIn('NESTED_DEFAULT_WIDTH: u32 = 1350', w)
        self.assertIn('NESTED_DEFAULT_HEIGHT: u32 = 641', w)
        self.assertIn('explicit_window_size()', w)
        self.assertNotIn('.with_maximized(true)', w)
        shell = source('scripts/wayland')
        self.assertIn('FLAMEWM_WAYLAND_WIDTH:-1350', shell)
        self.assertIn('FLAMEWM_WAYLAND_HEIGHT:-641', shell)
        self.assertIn('dbus-run-session', shell)

    def test_titlebar_uses_skin_and_not_solid_rgb_squares(self):
        model = source('crates/flamewm-wm-wayland/src/model.rs')
        render = source('crates/flamewm-wm-wayland/src/render.rs')
        self.assertIn('flamewm_skin::recipes::window_chrome::WINDOW_CHROME.metrics.titlebar', model)
        self.assertIn('MemoryRenderBuffer::from_slice', render)
        self.assertIn('Fourcc::Argb8888', render)
        self.assertNotIn('BUTTON_MINIMIZE:', render)
        self.assertIn('fn chrome_pixels(', render)
        self.assertIn('rounded_alpha', render)

    def test_xwayland_compositor_selects_private_screen_and_keeps_owner(self):
        x = source('crates/flamewm-wm-wayland/src/xwayland.rs')
        self.assertIn('b"_NET_WM_CM_S0"', x)
        self.assertIn('get_selection_owner(selection)', x)
        self.assertIn('set_selection_owner(owner, selection, 0_u32)', x)
        self.assertIn('_compositor_owner: Some(owner)', x.replace('self._compositor_owner = Some(owner)', '_compositor_owner: Some(owner)'))
        self.assertIn('props.claim_compositor()', x)

    def test_buffer_age_egl_query_removed_in_nested_path(self):
        render = source('crates/flamewm-wm-wayland/src/render.rs')
        self.assertNotIn('backend.buffer_age()', '\n'.join(line for line in render.splitlines() if not line.lstrip().startswith('//')))
        self.assertIn('let age = 0;', render)

    def test_single_output_alias_never_guesses_on_multiple_outputs(self):
        host = source('crates/flamewm-platform/src/host.rs')
        self.assertIn('if name == "default"', host)
        self.assertIn('connected.next().is_none()', host)
        self.assertIn('self.persisted_output_id(output)', host)

    def test_xcreatepixmap_failure_has_actionable_x11_error(self):
        resource = source('crates/flamewm-render-x11/src/native/xresource.rs')
        self.assertIn('event.error_code', resource)
        self.assertIn('TRAP_ERROR.load', resource)
        self.assertIn('request={}', resource)

    def test_shutdown_cleans_nested_children(self):
        state = source('crates/flamewm-wm-wayland/src/state.rs')
        self.assertIn('impl Drop for State', state)
        self.assertIn('child.kill()', state)

if __name__ == '__main__':
    unittest.main()

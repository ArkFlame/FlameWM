import re
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
        self.assertIn(re.sub(r'\s+', '', 'NESTED_DEFAULT_WIDTH: u32 = 1350'), re.sub(r'\s+', '', w))
        self.assertIn(re.sub(r'\s+', '', 'NESTED_DEFAULT_HEIGHT: u32 = 641'), re.sub(r'\s+', '', w))
        self.assertIn(re.sub(r'\s+', '', 'explicit_window_size()'), re.sub(r'\s+', '', w))
        self.assertNotIn('.with_maximized(true)', w)
        shell = source('scripts/wayland')
        self.assertIn(re.sub(r'\s+', '', 'FLAMEWM_WAYLAND_WIDTH:-1350'), re.sub(r'\s+', '', shell))
        self.assertIn(re.sub(r'\s+', '', 'FLAMEWM_WAYLAND_HEIGHT:-641'), re.sub(r'\s+', '', shell))
        self.assertIn(re.sub(r'\s+', '', 'dbus-run-session'), re.sub(r'\s+', '', shell))

    def test_titlebar_uses_skin_and_not_solid_rgb_squares(self):
        model = source('crates/flamewm-wm-wayland/src/model.rs')
        render = source('crates/flamewm-wm-wayland/src/render.rs')
        self.assertIn(re.sub(r'\s+', '', 'flamewm_skin::recipes::window_chrome::WINDOW_CHROME.metrics.titlebar'), re.sub(r'\s+', '', model))
        self.assertIn(re.sub(r'\s+', '', 'MemoryRenderBuffer::from_slice'), re.sub(r'\s+', '', render))
        self.assertIn(re.sub(r'\s+', '', 'Fourcc::Argb8888'), re.sub(r'\s+', '', render))
        self.assertNotIn('BUTTON_MINIMIZE:', render)
        self.assertIn(re.sub(r'\s+', '', 'fn chrome_pixels('), re.sub(r'\s+', '', render))
        self.assertIn(re.sub(r'\s+', '', 'rounded_alpha'), re.sub(r'\s+', '', render))

    def test_xwayland_compositor_selects_private_screen_and_keeps_owner(self):
        x = source('crates/flamewm-wm-wayland/src/xwayland.rs')
        self.assertIn(re.sub(r'\s+', '', 'b"_NET_WM_CM_S0"'), re.sub(r'\s+', '', x))
        self.assertIn(re.sub(r'\s+', '', 'get_selection_owner(selection)'), re.sub(r'\s+', '', x))
        self.assertIn(re.sub(r'\s+', '', 'set_selection_owner(owner, selection, 0_u32)'), re.sub(r'\s+', '', x))
        self.assertIn('_compositor_owner: Some(owner)', x.replace('self._compositor_owner = Some(owner)', '_compositor_owner: Some(owner)'))
        self.assertIn(re.sub(r'\s+', '', 'props.claim_compositor()'), re.sub(r'\s+', '', x))

    def test_buffer_age_egl_query_removed_in_nested_path(self):
        render = source('crates/flamewm-wm-wayland/src/render.rs')
        self.assertNotIn('backend.buffer_age()', '\n'.join(line for line in render.splitlines() if not line.lstrip().startswith('//')))
        self.assertIn(re.sub(r'\s+', '', 'let age = 0;'), re.sub(r'\s+', '', render))

    def test_single_output_alias_never_guesses_on_multiple_outputs(self):
        host = source('crates/flamewm-platform/src/host.rs')
        self.assertIn(re.sub(r'\s+', '', 'if name == "default"'), re.sub(r'\s+', '', host))
        self.assertIn(re.sub(r'\s+', '', 'connected.next().is_none()'), re.sub(r'\s+', '', host))
        self.assertIn(re.sub(r'\s+', '', 'self.persisted_output_id(output)'), re.sub(r'\s+', '', host))

    def test_xcreatepixmap_failure_has_actionable_x11_error(self):
        resource = source('crates/flamewm-render-x11/src/native/xresource.rs')
        self.assertIn(re.sub(r'\s+', '', 'event.error_code'), re.sub(r'\s+', '', resource))
        self.assertIn(re.sub(r'\s+', '', 'TRAP_ERROR.load'), re.sub(r'\s+', '', resource))
        self.assertIn(re.sub(r'\s+', '', 'request={}'), re.sub(r'\s+', '', resource))

    def test_shutdown_cleans_nested_children(self):
        state = source('crates/flamewm-wm-wayland/src/state.rs')
        self.assertIn(re.sub(r'\s+', '', 'impl Drop for State'), re.sub(r'\s+', '', state))
        self.assertIn(re.sub(r'\s+', '', 'child.kill()'), re.sub(r'\s+', '', state))

if __name__ == '__main__':
    unittest.main()

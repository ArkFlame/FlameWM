"""Offline contract gate for retained desktop drag geometry (native Rust tests live in model/tests.rs)."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parent.parent
GEOMETRY = ROOT / 'crates/flamewm-render-core/src/model/geometry.rs'
EVENTS = ROOT / 'crates/flamewm-render-x11/src/app/events.rs'
DESKTOP = ROOT / 'crates/flamewm-desktop/src/main.rs'
# Drag ghost painting moved out of main.rs into its own module (commit-pending split).
DESKTOP_DRAG = ROOT / 'crates/flamewm-desktop/src/drag.rs'
TESTS = ROOT / 'crates/flamewm-render-core/src/model/tests.rs'
PACKAGE = ROOT / 'scripts/package'
SESSION = ROOT / 'packaging/flamewm-session.desktop'

class DragGeometryContract(unittest.TestCase):
    def test_position_mutation_invalidates_layout_and_full_redraw(self):
        source = GEOMETRY.read_text()
        part = source.split('pub fn set_position_px', 1)[1].split('pub fn set_size_px', 1)[0]
        self.assertIn('entry.left == Some(left) && entry.top == Some(top)', part)
        self.assertIn('entry.left = Some(left);', part)
        self.assertIn('entry.top = Some(top);', part)
        # Isolated (absolute) nodes go through touch_geometry: paint + in-place layout patch.
        self.assertRegex(part, r'self\.touch_geometry\(index\);\s*Ok\(\(\)\)')

    def test_dimension_mutation_also_invalidates_without_repeated_motion_cost(self):
        source = GEOMETRY.read_text()
        part = source.split('pub fn set_size_px', 1)[1].split('pub fn geometry_override', 1)[0]
        self.assertIn('entry.width == Some(width) && entry.height == Some(height)', part)
        self.assertRegex(part, r'self\.touch_geometry\(index\);\s*Ok\(\(\)\)')

    def test_drag_remains_captured_and_projected_at_pointer_motion(self):
        self.assertIn('self.interaction.active', EVENTS.read_text())
        source = DESKTOP.read_text()
        self.assertIn('UiActionPhase::Motion => {', source)
        self.assertIn('show_drag_ghost(document, state, action.x, action.y)?;', source)
        drag = DESKTOP_DRAG.read_text()
        self.assertIn('document.position("desktop-drag-ghost", gx, gy)?;', drag)
        self.assertIn('document.visible("desktop-drag-ghost", true)?;', drag)

    def test_native_geometry_regressions_are_permanent(self):
        text = TESTS.read_text()
        self.assertIn('pointer_motion_repositions_drag_ghost_and_invalidates_retained_frame', text)
        self.assertIn('size_mutation_invalidates_layout_and_repeated_size_is_noop', text)
        self.assertIn('SurfaceDamage::Full', text)

    def test_deb_installs_sddm_discoverable_x11_session(self):
        package = PACKAGE.read_text()
        session = SESSION.read_text()
        self.assertIn('"$STAGE/usr/share/xsessions/flamewm.desktop"', package)
        self.assertIn('Exec=/usr/bin/flamewm-session', session)
        self.assertIn('TryExec=/usr/bin/flamewm-session', session)
        self.assertNotIn('wayland-sessions', package)

if __name__ == '__main__':
    unittest.main()

"""Offline tests for the isolated native workspace acceptance harness."""
from __future__ import annotations

import sys
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

sys.path.insert(0, str(Path(__file__).parent))
import flamewm_workspace_edge_acceptance as edge


class FakeProperty:
    value = [3]


class NativeHarnessTests(unittest.TestCase):
    def test_property_first_handles_missing_and_present_properties(self):
        window = mock.Mock()
        window.get_full_property.return_value = None
        self.assertIsNone(edge.property_first(window, 123, mock.Mock()))
        window.get_full_property.return_value = FakeProperty()
        self.assertEqual(edge.property_first(window, 123, mock.Mock()), 3)

    def test_display_selection_never_touches_login_display(self):
        with mock.patch.object(Path, 'exists', return_value=False):
            self.assertEqual(edge.first_owned_display(), 97)
        with mock.patch.object(Path, 'exists', return_value=True):
            self.assertIsNone(edge.first_owned_display())

    def test_absent_binary_is_environment_blocked(self):
        with self.assertRaises(EnvironmentError):
            edge.run(Path('/this-does-not-exist-flamewm'), Path('/tmp'))

    def test_wait_until_reports_timeout_instead_of_false_success(self):
        self.assertIsNone(edge.wait_until(lambda: False, seconds=0.0))

    def test_grip_must_remain_inside_titlebar_after_warp(self):
        frame = SimpleNamespace(x=8, y=100, width=700, height=400)
        self.assertTrue(edge.held_titlebar_grip(48, 112, frame))
        self.assertFalse(edge.held_titlebar_grip(48, 200, frame))
        self.assertFalse(edge.held_titlebar_grip(900, 112, frame))
        self.assertFalse(edge.held_titlebar_grip(0, 112, frame))

    def test_native_scenario_uses_six_desktop_grid_and_vertical_axes(self):
        from inspect import getsource
        source = getsource(edge.run)
        self.assertIn("'FLAMEWM_WORKSPACES': '6'", source)
        self.assertIn("for expected in (1, 2, 0)", source)
        self.assertIn("'top edge 2 → 5", source)
        self.assertIn("'bottom edge 5 → 2", source)
        self.assertNotIn("'FLAMEWM_WORKSPACES': '3'", source)

    def test_large_window_must_stay_at_least_half_visible(self):
        on_screen = SimpleNamespace(x=-100, y=100, width=900, height=400)
        stranded = SimpleNamespace(x=-895, y=100, width=900, height=400)
        oversized = SimpleNamespace(x=-500, y=100, width=1700, height=400)
        self.assertTrue(edge.enough_frame_visible(on_screen, 1280))
        self.assertFalse(edge.enough_frame_visible(stranded, 1280))
        self.assertTrue(edge.enough_frame_visible(oversized, 1280))


if __name__ == '__main__':
    unittest.main()

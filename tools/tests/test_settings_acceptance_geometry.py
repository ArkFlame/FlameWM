"""Settings window geometry must be read after the WM has placed the window, not from the first mapped read."""
import importlib.util
import pathlib
import sys
import types
import unittest

_TOOLS = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(_TOOLS))
_spec = importlib.util.spec_from_file_location("flamewm_settings_acceptance", _TOOLS / "flamewm_settings_acceptance.py")
harness = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = harness
_spec.loader.exec_module(harness)


def geo(x, y, w=820, h=540):
    return {"x": x, "y": y, "width": w, "height": h}


class WaitForSettingsWindow(unittest.TestCase):
    def _run(self, reads):
        queue = list(reads)
        original = harness.window_geometry
        harness.window_geometry = lambda ctx, path: queue.pop(0) if queue else geo(460, 216)
        try:
            return harness.wait_for_settings_window(types.SimpleNamespace(), pathlib.Path("/nonexistent"), timeout=5.0)
        finally:
            harness.window_geometry = original

    def test_stale_pre_placement_read_is_not_returned(self):
        result = self._run([geo(0, 0), geo(460, 216), geo(460, 216)])
        self.assertEqual((result["x"], result["y"]), (460, 216))

    def test_settled_position_is_returned(self):
        result = self._run([geo(460, 216), geo(460, 216)])
        self.assertEqual((result["x"], result["y"]), (460, 216))


if __name__ == "__main__":
    unittest.main()


class TaskbarUnrelatedControl(unittest.TestCase):
    def _measure(self, inside, edge="bottom", extent=44):
        return {"edge": edge, "extent": extent, "inside": inside, "band_digest": str(inside)}

    def test_compositor_drift_within_tolerance_is_unchanged(self):
        self.assertTrue(harness.taskbar_unrelated_unchanged(self._measure((9, 10, 10)), self._measure((10, 10, 12))))

    def test_real_repaint_is_changed(self):
        self.assertFalse(harness.taskbar_unrelated_unchanged(self._measure((9, 10, 10)), self._measure((40, 69, 101))))

    def test_edge_or_extent_change_is_changed(self):
        self.assertFalse(harness.taskbar_unrelated_unchanged(self._measure((9, 10, 10)), self._measure((9, 10, 10), edge="top")))
        self.assertFalse(harness.taskbar_unrelated_unchanged(self._measure((9, 10, 10)), self._measure((9, 10, 10), extent=40)))

    def test_missing_measure_is_changed(self):
        self.assertFalse(harness.taskbar_unrelated_unchanged(None, self._measure((9, 10, 10))))

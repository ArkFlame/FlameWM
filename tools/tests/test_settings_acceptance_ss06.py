"""SS06 snap preview: a window-sized preview is the move outline, not a snap zone."""
import importlib.util
import pathlib
import sys
import unittest

_TOOLS = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(_TOOLS))
_spec = importlib.util.spec_from_file_location("flamewm_settings_acceptance", _TOOLS / "flamewm_settings_acceptance.py")
harness = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = harness
_spec.loader.exec_module(harness)

WINDOW = {"x": 459, "y": 215, "width": 820, "height": 540}


class Ss06SnapRect(unittest.TestCase):
    def test_window_sized_outline_is_not_a_snap_preview(self):
        self.assertFalse(harness.ss06_is_snap_rect({"x": 640, "y": 400, "width": 820, "height": 540}, WINDOW))

    def test_framed_outline_measured_in_ss06_is_not_a_snap_preview(self):
        # Measured control rect: 822x572 for a 820x540 client.
        self.assertFalse(harness.ss06_is_snap_rect({"x": 229, "y": 385, "width": 822, "height": 572}, WINDOW))

    def test_half_screen_zone_is_a_snap_preview(self):
        self.assertTrue(harness.ss06_is_snap_rect({"x": 2, "y": 2, "width": 637, "height": 752}, WINDOW))

    def test_no_preview_is_not_a_snap_preview(self):
        self.assertFalse(harness.ss06_is_snap_rect(None, WINDOW))


if __name__ == "__main__":
    unittest.main()

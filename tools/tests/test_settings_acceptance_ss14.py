"""SS14 nav-row sample: the default point is inside the selected row, which is a solid accent fill."""
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


class Ss14AccentSample(unittest.TestCase):
    def test_default_point_is_inside_selected_theme_row(self):
        x, y = (int(part) for part in harness.SS14_ACCENT_PIXEL_DEFAULT.split(","))
        # Selected Theme row spans client y 109..139 on 820x540; the sidebar Search field ends above it.
        self.assertTrue(109 <= y <= 139, y)
        self.assertTrue(0 <= x < 180, x)

    def test_selected_row_expects_solid_accent_not_alpha_tint(self):
        self.assertEqual(harness.expected_selected_row_pixel("#2878D7"), (0x28, 0x78, 0xD7))

    def test_invalid_accent_has_no_expected_pixel(self):
        self.assertIsNone(harness.expected_selected_row_pixel("accent"))


if __name__ == "__main__":
    unittest.main()

"""SS06 selection fill: expected red is the active accent at the fill opacity (rubber.rs), not a fixed red."""
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


class Ss06FillRed(unittest.TestCase):
    def test_default_accent_at_twenty_percent(self):
        # #3DAEE9 red channel 61 at 20% over black.
        self.assertEqual(harness.ss06_fill_red("#3DAEE9", 20), 12)

    def test_fifty_percent_on_default_accent(self):
        self.assertEqual(harness.ss06_fill_red("#3DAEE9", 50), 30)

    def test_missing_accent_or_opacity_has_no_expectation(self):
        self.assertIsNone(harness.ss06_fill_red(None, 20))
        self.assertIsNone(harness.ss06_fill_red("#3DAEE9", None))


if __name__ == "__main__":
    unittest.main()

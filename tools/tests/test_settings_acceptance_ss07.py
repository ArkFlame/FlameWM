"""SS07 taskbar expectation: the Apply transaction must put the bar over a black wallpaper."""
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


class Ss07TaskbarChanges(unittest.TestCase):
    def test_transaction_sets_black_solid_wallpaper_behind_the_bar(self):
        changes = harness.ss07_taskbar_surface_changes(53, 67, "#284F77")
        self.assertEqual(changes["wallpaperMode"], "'solid'")
        self.assertEqual(changes["wallpaperColor"], "'#000000'")

    def test_transaction_keeps_taskbar_and_surface_values(self):
        changes = harness.ss07_taskbar_surface_changes(53, 67, "#284F77")
        self.assertEqual(changes["taskbarHeight"], "uint32 53")
        self.assertEqual(changes["surfaceOpacity"], "uint32 67")
        self.assertEqual(changes["surfaceColor"], "'#284F77'")


if __name__ == "__main__":
    unittest.main()

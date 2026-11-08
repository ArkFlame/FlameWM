"""SS15 mode-count parse: GVariant prints an empty typed array as <@aa{sv} []>."""
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

EMPTY_MODES = ("({'generation': <uint64 575332448>, 'hasPending': <false>, 'outputCount': <uint32 1>, "
               "'outputs': <[{'connected': <true>, 'connector': <'default'>, 'height': <800>, "
               "'id': <'default'>, 'mode': <uint64 0>, 'modes': <@aa{sv} []>, 'primary': <true>, "
               "'width': <1280>, 'x': <0>, 'y': <0>}]>},)")


class Ss15ModeCount(unittest.TestCase):
    def test_empty_typed_mode_array_counts_zero(self):
        self.assertEqual(harness.display_mode_count(EMPTY_MODES), 0)

    def test_plain_mode_array_still_counts(self):
        text = EMPTY_MODES.replace("<@aa{sv} []>", "<[{'w': <1>}, {'w': <2>}]>")
        self.assertEqual(harness.display_mode_count(text), 2)


if __name__ == "__main__":
    unittest.main()

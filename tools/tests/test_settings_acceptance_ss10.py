"""SS10 mode-count parse: an empty typed modes array prints as <@aa{sv} []> and must count 0, not unparsable."""
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

SNAPSHOT = ("({'generation': <uint64 575332448>, 'hasPending': <false>, 'outputCount': <uint32 1>, "
            "'outputs': <[{'connected': <true>, 'connector': <'default'>, 'height': <800>, "
            "'id': <'default'>, 'mode': <uint64 0>, 'modes': <@aa{sv} []>, 'primary': <true>, "
            "'width': <1280>, 'x': <0>, 'y': <0>}]>},)")


class Ss10ModeCount(unittest.TestCase):
    def test_empty_typed_mode_array_counts_zero(self):
        self.assertEqual(harness.display_output_mode_count(SNAPSHOT, "default"), 0)

    def test_plain_mode_array_still_counts(self):
        text = SNAPSHOT.replace("<@aa{sv} []>", "<[{'w': <1>}, {'w': <2>}]>")
        self.assertEqual(harness.display_output_mode_count(text, "default"), 2)

    def test_unknown_output_is_none(self):
        self.assertIsNone(harness.display_output_mode_count(SNAPSHOT, "missing"))


if __name__ == "__main__":
    unittest.main()

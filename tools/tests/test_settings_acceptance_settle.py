"""Settings revision settle: a revision still being written by a starting client is not used until it holds."""
import importlib.util
import pathlib
import sys
import unittest
from unittest import mock

_TOOLS = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(_TOOLS))
_spec = importlib.util.spec_from_file_location("flamewm_settings_acceptance", _TOOLS / "flamewm_settings_acceptance.py")
harness = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = harness
_spec.loader.exec_module(harness)


def snapshot(revision: int) -> str:
    return f"({{'revision': <uint64 {revision}>, 'accent': <'#3DAEE9'>}},)"


class SettleRevision(unittest.TestCase):
    def test_returns_revision_only_after_it_holds(self):
        # 1 (stale, before the shell writes), then 2 holds: the stale 1 must not be returned.
        sequence = iter([snapshot(1), snapshot(2), snapshot(2), snapshot(2), snapshot(2), snapshot(2)])
        clock = iter(i * 0.3 for i in range(10000))
        with mock.patch.object(harness, "dbus_settings_snapshot", side_effect=lambda ctx: (next(sequence), "cmd")), \
                mock.patch.object(harness.time, "monotonic", lambda: next(clock)), \
                mock.patch.object(harness.time, "sleep", lambda s: None):
            self.assertEqual(harness.settings_revision_settled(object(), timeout=60.0, quiet=0.5), 2)

    def test_gives_up_with_last_seen_revision_at_timeout(self):
        counter = iter(range(1, 10000))
        clock = iter(i * 0.3 for i in range(10000))
        with mock.patch.object(harness, "dbus_settings_snapshot", side_effect=lambda ctx: (snapshot(next(counter)), "cmd")), \
                mock.patch.object(harness.time, "monotonic", lambda: next(clock)), \
                mock.patch.object(harness.time, "sleep", lambda s: None):
            self.assertIsNotNone(harness.settings_revision_settled(object(), timeout=1.0, quiet=1.0))


if __name__ == "__main__":
    unittest.main()

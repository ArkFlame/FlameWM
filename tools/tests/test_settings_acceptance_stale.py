"""Settings.Apply StaleRevision: the harness re-reads the settled revision and applies once more."""
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

STALE = "Error: GDBus.Error:com.arkflame.FlameWM1.Error.StaleRevision: StaleRevision: settings revision is stale"


def snapshot(revision: int) -> str:
    return f"({{'revision': <uint64 {revision}>, 'accent': <'#3DAEE9'>}},)"


class StaleRetry(unittest.TestCase):
    def test_stale_apply_retries_once_with_settled_revision(self):
        applied_with = []
        replies = iter([(STALE, "cmd", 1), ("()", "cmd", 0)])

        def fake_apply(ctx, revision, changes):
            applied_with.append(revision)
            return next(replies)

        clock = iter(i * 0.3 for i in range(10000))
        with mock.patch.object(harness, "dbus_apply_settings", side_effect=fake_apply), \
                mock.patch.object(harness, "dbus_settings_snapshot", side_effect=lambda ctx: (snapshot(5), "cmd")), \
                mock.patch.object(harness.time, "monotonic", lambda: next(clock)), \
                mock.patch.object(harness.time, "sleep", lambda s: None):
            result = harness.apply_and_read_settings(object(), 3, {"x": "uint32 1"}, {"commands": []})
        self.assertEqual(applied_with, [3, 5])
        self.assertEqual(result["revision"], 5)

    def test_second_stale_reply_is_not_retried_again(self):
        applied_with = []

        def fake_apply(ctx, revision, changes):
            applied_with.append(revision)
            return (STALE, "cmd", 1)

        clock = iter(i * 0.3 for i in range(10000))
        with mock.patch.object(harness, "dbus_apply_settings", side_effect=fake_apply), \
                mock.patch.object(harness, "dbus_settings_snapshot", side_effect=lambda ctx: (snapshot(5), "cmd")), \
                mock.patch.object(harness.time, "monotonic", lambda: next(clock)), \
                mock.patch.object(harness.time, "sleep", lambda s: None):
            result = harness.apply_and_read_settings(object(), 3, {"x": "uint32 1"}, {"commands": []})
        self.assertIsNone(result)
        self.assertEqual(applied_with, [3, 5])


if __name__ == "__main__":
    unittest.main()

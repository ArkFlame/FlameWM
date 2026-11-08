"""Per-scenario reset: stop all owned children except Xephyr, wipe the private config tree, restart the WM."""
import importlib.util
import pathlib
import sys
import tempfile
import types
import unittest

_TOOLS = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(_TOOLS))
_spec = importlib.util.spec_from_file_location("flamewm_settings_acceptance", _TOOLS / "flamewm_settings_acceptance.py")
harness = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = harness
_spec.loader.exec_module(harness)


class FakeProc:
    def __init__(self, alive=True):
        self.alive = alive

    def poll(self):
        return None if self.alive else 1


class Owned:
    def __init__(self, name, alive=True):
        self.name = name
        self.proc = FakeProc(alive)


class ResetScenarioState(unittest.TestCase):
    def _ctx(self, root):
        ctx = harness.Ctx(root)
        ctx.owned = [Owned("xephyr"), Owned("flamewm"), Owned("settings-ss10-displays"), Owned("desktop-ss17")]
        ctx.wm = ctx.owned[1]
        self.stopped = []
        ctx.stop = lambda owned: self.stopped.append(owned.name)
        ctx.spawn = lambda name, argv, env: Owned(name, alive=self.wm_alive)
        return ctx

    def _args(self):
        return types.SimpleNamespace(wm_bin="/bin/true", settle_seconds=0)

    def test_stops_everything_but_xephyr_and_wipes_config(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            stale = root / "home" / ".config" / "flamewm" / "settings.toml"
            stale.parent.mkdir(parents=True)
            stale.write_text("scale.default=125\n")
            self.wm_alive = True
            ctx = self._ctx(root)
            self.assertTrue(harness.reset_scenario_state(ctx, self._args(), "SS05"))
            self.assertEqual(self.stopped, ["desktop-ss17", "settings-ss10-displays", "flamewm"])
            self.assertEqual([o.name for o in ctx.owned], ["xephyr"])
            self.assertFalse(stale.exists())
            for name in harness.PRIVATE_TREE_DIRS:
                self.assertTrue((root / name).is_dir(), name)
            self.assertEqual(ctx.wm.name, "flamewm-ss05")

    def test_reports_failed_wm_start(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.wm_alive = False
            ctx = self._ctx(pathlib.Path(tmp))
            self.assertFalse(harness.reset_scenario_state(ctx, self._args(), "SS12"))

    def test_isolated_turns_failed_reset_into_environment_block(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.wm_alive = False
            ctx = self._ctx(pathlib.Path(tmp))
            entry = harness.isolated(ctx, self._args(), "SS15", lambda c, a: {"status": "PASS"})
            self.assertEqual(entry["status"], harness.STATUS_ENV_BLOCKED)


if __name__ == "__main__":
    unittest.main()

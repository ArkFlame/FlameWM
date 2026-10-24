"""Fixture tests for scripts/dev-lane with a fake cargo: no Rust build runs."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
LANE = ROOT / "scripts/dev-lane"
FAKE_CARGO = """#!/bin/sh
printf 'CARGO_TARGET_DIR=%s ARGS=%s\\n' "$CARGO_TARGET_DIR" "$*" >> "$FLAMEWM_FAKE_EVENTS"
exit 0
"""


class DevLaneTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="flamewm-dev-lane-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "checkout"
        (self.root / "scripts").mkdir(parents=True)
        shutil.copy2(LANE, self.root / "scripts/dev-lane")
        fakebin = Path(self.temp.name) / "fakebin"
        fakebin.mkdir()
        cargo = fakebin / "cargo"
        cargo.write_text(FAKE_CARGO, encoding="utf-8")
        cargo.chmod(0o755)
        self.events = Path(self.temp.name) / "events.txt"
        self.base = Path(self.temp.name) / "targets"
        self.env = os.environ.copy()
        self.env.update({
            "PATH": f"{fakebin}{os.pathsep}{self.env.get('PATH', '')}",
            "FLAMEWM_TARGET_BASE": str(self.base),
            "FLAMEWM_FAKE_EVENTS": str(self.events),
        })
        self.env.pop("CARGO_TARGET_DIR", None)

    def lane(self, *args):
        return subprocess.run([str(self.root / "scripts/dev-lane"), *args], capture_output=True,
                              text=True, timeout=30, env=self.env)

    def test_check_uses_dev_target_and_locked_per_crate_selection(self):
        result = self.lane("check", "flamewm-terminal", "flamewm-explorer")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.events.read_text().splitlines(),
                         [f"CARGO_TARGET_DIR={self.base / 'dev'} ARGS=check --all-targets -p flamewm-terminal "
                          "-p flamewm-explorer --locked"])

    def test_test_mode_uses_dev_target(self):
        self.assertEqual(self.lane("test", "flamewm-terminal").returncode, 0)
        self.assertEqual(self.events.read_text().splitlines(),
                         [f"CARGO_TARGET_DIR={self.base / 'dev'} ARGS=test -p flamewm-terminal --locked"])

    def test_fast_mode_uses_fast_profile_and_fast_target_not_release(self):
        self.assertEqual(self.lane("fast", "flamewm-desktop").returncode, 0)
        line = self.events.read_text().strip()
        self.assertEqual(line, f"CARGO_TARGET_DIR={self.base / 'fast'} ARGS=build --profile fast -p flamewm-desktop --locked")
        self.assertNotIn("release", line)

    def test_usage_errors_exit_two_without_running_cargo(self):
        for args in ((), ("check",), ("release", "flamewm-desktop")):
            result = self.lane(*args)
            self.assertEqual(result.returncode, 2, args)
            self.assertIn("usage:", result.stderr)
        self.assertFalse(self.events.exists())


if __name__ == "__main__":
    unittest.main()

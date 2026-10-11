"""Unit tests for the installed-prefix Settings acceptance wrapper composition.

The native runner is replaced by a fake that writes a fixture result.json, so no
X server, Xephyr, or display is touched. Run with:
    python3 -I -m unittest discover -s tools -p 'test_flamewm_settings_installed_acceptance.py' -q
"""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))

import flamewm_settings_installed_acceptance as inst  # noqa: E402


CANONICAL_IDS = [f"SS{n:02d}" for n in range(20)]


class InstalledVerdictComposeTests(unittest.TestCase):
    def setUp(self) -> None:
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.prefix = self.root / "prefix"
        (self.prefix / "usr/bin").mkdir(parents=True)
        (self.prefix / "usr/bin/flamewm").write_bytes(b"wm-binary")
        (self.prefix / "usr/bin/flamewm-settings").write_bytes(b"settings-binary")
        self.hashes = {
            "usr/bin/flamewm": inst.sha256(self.prefix / "usr/bin/flamewm"),
            "usr/bin/flamewm-settings": inst.sha256(self.prefix / "usr/bin/flamewm-settings"),
        }
        self.run_dir = self.root / "run"

    def native_document(self, verdict: str, passing: set[str]) -> dict:
        scenarios = [
            {"id": sid, "status": "PASS" if sid in passing else "NOT_RUN"}
            for sid in CANONICAL_IDS
        ]
        return {
            "verdict": verdict,
            "wm_bin": str(self.prefix / "usr/bin/flamewm"),
            "settings_bin": str(self.prefix / "usr/bin/flamewm-settings"),
            "scenarios": scenarios,
        }

    def run_with_native(self, native: dict, returncode: int = 0) -> tuple[dict, int]:
        def fake_native_runner(command, **_kwargs):
            acceptance_dir = Path(command[command.index("--run-dir") + 1])
            acceptance_dir.mkdir(parents=True, exist_ok=True)
            (acceptance_dir / "result.json").write_text(json.dumps(native), encoding="utf-8")
            return subprocess.CompletedProcess(command, returncode, "", "")

        args = inst.parse_args(["--prefix", str(self.prefix), "--run-dir", str(self.run_dir)])
        with patch.object(inst, "validate_prefix", return_value=(self.prefix, self.hashes, "0.0.0-test")), \
                patch.object(inst.subprocess, "run", side_effect=fake_native_runner):
            return inst.run(args)

    def test_overall_verdict_is_native_incomplete_with_ss01_gate_surfaced(self) -> None:
        native = self.native_document("INCOMPLETE", passing={"SS00", "SS01", "SS02", "SS03", "SS04", "SS05", "SS07"})
        document, code = self.run_with_native(native)
        self.assertEqual(document["native_verdict"], "INCOMPLETE")
        self.assertEqual(document["verdict"], "INCOMPLETE")
        self.assertNotEqual(document["verdict"], "PASS")
        self.assertEqual(document["launch_gate"], "PASS")
        self.assertEqual(code, 0)

    def test_native_fail_is_overall_fail_with_nonzero_exit(self) -> None:
        native = self.native_document("FAIL", passing={"SS01"})
        document, code = self.run_with_native(native, returncode=0)
        self.assertEqual(document["verdict"], "FAIL")
        self.assertEqual(document["launch_gate"], "FAIL")
        self.assertEqual(code, 1)

    def test_ss01_not_pass_keeps_gate_verdict_over_native_incomplete(self) -> None:
        native = self.native_document("INCOMPLETE", passing={"SS00", "SS02"})
        document, code = self.run_with_native(native)
        self.assertEqual(document["launch_gate"], "NOT_RUN")
        self.assertEqual(document["verdict"], "NOT_RUN")
        self.assertEqual(code, 3)


class InstalledBehavioralMappingTests(unittest.TestCase):
    def _native(self, **over) -> dict:
        statuses = {sid: "PASS" for sid in inst.BEHAVIORAL_SCENARIOS}
        statuses.update(over)
        return {"scenarios": [{"id": sid, "status": status} for sid, status in statuses.items()]}

    def test_launch_only_pass_is_not_ss18_pass(self) -> None:
        status, reason, _ = inst.installed_ss18(True, {"scenarios": [{"id": "SS01", "status": "PASS"}]})
        self.assertEqual(status, "NOT_RUN")
        self.assertIn("SS12=None", reason)

    def test_all_behavioral_pass_is_pass(self) -> None:
        self.assertEqual(inst.installed_ss18(True, self._native())[0], "PASS")

    def test_one_behavioral_fail_is_fail(self) -> None:
        status, reason, _ = inst.installed_ss18(True, self._native(SS12="FAIL"))
        self.assertEqual(status, "FAIL")
        self.assertIn("SS12", reason)

    def test_changed_binaries_fail_even_when_scenarios_pass(self) -> None:
        self.assertEqual(inst.installed_ss18(False, self._native())[0], "FAIL")

    def test_missing_native_result_is_not_run(self) -> None:
        self.assertEqual(inst.installed_ss18(False, None)[0], "NOT_RUN")


if __name__ == "__main__":
    unittest.main()

"""Regression tests for the fail-closed glibc baseline gate.

Run: python3 -I -m unittest discover -s tools -p 'test_flamewm_glibc_baseline.py' -v
"""
import pathlib
import subprocess
import sys
import tempfile
import tomllib
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import flamewm_glibc_baseline as gate  # noqa: E402

ROOT = pathlib.Path(gate.__file__).resolve().parents[1]
GATE = ROOT / "tools" / "flamewm_glibc_baseline.py"
BASELINE_FILE = ROOT / "packaging" / "target-baseline.toml"


def run_gate(*args):
    return subprocess.run(
        [sys.executable, "-I", str(GATE), *map(str, args)],
        capture_output=True,
        text=True,
        check=False,
    )


def dynamic_elf():
    """A host ELF that really requires GLIBC symbols, or skip."""
    for candidate in (sys.executable, "/bin/sh"):
        path = pathlib.Path(candidate)
        if not path.is_file():
            continue
        with path.open("rb") as handle:
            is_elf = handle.read(len(gate.ELF_MAGIC)) == gate.ELF_MAGIC
        if is_elf and gate.required_versions(path):
            return path
    return None


class BaselineParsingTest(unittest.TestCase):
    def test_toml_is_the_single_baseline_source(self):
        data = tomllib.loads(BASELINE_FILE.read_text(encoding="utf-8"))["debian"]
        self.assertEqual(sorted(data), ["codename", "glibc", "image", "release"])
        self.assertEqual(gate.read_baseline(BASELINE_FILE), data["glibc"])

    def test_missing_baseline_file_is_fail_closed(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(gate.GateError):
                gate.read_baseline(pathlib.Path(tmp) / "absent.toml")


class VersionOrderingTest(unittest.TestCase):
    def test_three_component_versions_rank_below_two_component_baseline(self):
        self.assertLess(gate.version_tuple("2.2.5"), gate.version_tuple("2.36"))
        self.assertLess(gate.version_tuple("2.9"), gate.version_tuple("2.17"))
        self.assertEqual(gate.version_tuple("2.36"), gate.version_tuple("2.36"))
        self.assertGreater(gate.version_tuple("2.43"), gate.version_tuple("2.36"))

    def test_non_numeric_version_is_rejected(self):
        with self.assertRaises(gate.GateError):
            gate.version_tuple("GLIBC_PRIVATE")


class GateExitTest(unittest.TestCase):
    def setUp(self):
        self.elf = dynamic_elf()
        if self.elf is None:
            self.skipTest("host provides no dynamically linked ELF for the gate to inspect")

    def test_exceeding_baseline_exits_one(self):
        result = run_gate("--baseline", "1.0", self.elf)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("FLAMEWM-GLIBC-ERROR:", result.stderr)
        self.assertIn("baseline 1.0", result.stderr)

    def test_high_baseline_exits_zero(self):
        result = run_gate("--baseline", "99.0", self.elf)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("OK glibc-baseline: baseline=99.0", result.stdout)

    def test_empty_directory_exits_one(self):
        with tempfile.TemporaryDirectory() as tmp:
            result = run_gate("--dir", tmp)
        self.assertEqual(result.returncode, 1)
        self.assertIn("no ELF files found", result.stderr)

    def test_non_elf_file_exits_one(self):
        with tempfile.TemporaryDirectory() as tmp:
            plain = pathlib.Path(tmp) / "script.sh"
            plain.write_text("#!/bin/sh\n", encoding="utf-8")
            result = run_gate(plain)
        self.assertEqual(result.returncode, 1)
        self.assertIn("not an ELF file", result.stderr)


if __name__ == "__main__":
    unittest.main()

"""Fixture tests for tools/fast-bench.sh with a fake cargo: no Rust build runs."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
BENCH = ROOT / "tools/fast-bench.sh"
FAKE_CARGO = """#!/bin/sh
printf 'CARGO_TARGET_DIR=%s ARGS=%s\\n' "$CARGO_TARGET_DIR" "$*" >> "$FLAMEWM_FAKE_EVENTS"
case " $* " in
  *" $FLAMEWM_FAKE_FAIL_CRATE "*) [ -n "$FLAMEWM_FAKE_FAIL_CRATE" ] && exit 1 ;;
esac
echo "BENCH rows=3 cycles=1"
exit 0
"""


class FastBenchLaneTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="flamewm-fast-bench-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "checkout"
        (self.root / "tools").mkdir(parents=True)
        shutil.copy2(BENCH, self.root / "tools/fast-bench.sh")
        self.base = Path(self.temp.name) / "targets"
        fakebin = Path(self.temp.name) / "fakebin"
        fakebin.mkdir()
        cargo = fakebin / "cargo"
        cargo.write_text(FAKE_CARGO, encoding="utf-8")
        cargo.chmod(0o755)
        self.events = Path(self.temp.name) / "events.txt"
        self.env = os.environ.copy()
        self.env.update({
            "PATH": f"{fakebin}{os.pathsep}{self.env.get('PATH', '')}",
            "FLAMEWM_TARGET_BASE": str(self.base),
            "FLAMEWM_FAKE_EVENTS": str(self.events),
            "FLAMEWM_FAKE_FAIL_CRATE": "",
        })
        self.env.pop("CARGO_TARGET_DIR", None)

    def run_bench(self):
        return subprocess.run(["bash", str(self.root / "tools/fast-bench.sh")], capture_output=True,
                              text=True, timeout=60, env=self.env)

    def test_all_suites_share_one_target_dir_and_keep_bench_rows(self):
        result = self.run_bench()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        lines = self.events.read_text().splitlines()
        self.assertEqual(len(lines), 4)
        targets = {line.split(" ARGS=")[0] for line in lines}
        self.assertEqual(targets, {f"CARGO_TARGET_DIR={self.base / 'fast-bench'}"})
        self.assertIn("-p flamewm-terminal bench_row_rebuilds", lines[0])
        self.assertIn("-p flamewm-render-x11 bench_mask_cache", lines[3])
        self.assertRegex(result.stdout, r"(?m)^terminal\s+rows\s+3$")
        self.assertRegex(result.stdout, r"(?m)^wall_ms=\d+ exit=0$")

    def test_failing_suite_keeps_shared_target_and_nonzero_exit(self):
        self.env["FLAMEWM_FAKE_FAIL_CRATE"] = "flamewm-desktop"
        result = self.run_bench()
        self.assertEqual(result.returncode, 1)
        self.assertIn("flamewm-desktop", result.stderr)
        self.assertIn("wall_ms=", result.stdout)
        self.assertIn("exit=1", result.stdout)
        targets = {line.split(" ARGS=")[0] for line in self.events.read_text().splitlines()}
        self.assertEqual(len(targets), 1)


if __name__ == "__main__":
    unittest.main()

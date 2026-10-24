"""Fixture tests for tools/bench-cargo-build.sh with a fake cargo: no Rust build runs."""
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
BENCH = ROOT / "tools/bench-cargo-build.sh"
FAKE_CARGO = """#!/bin/sh
case "$1" in
  --version) echo "cargo 0.0.0-fake"; exit 0 ;;
esac
echo "fake cargo args=$* target=$CARGO_TARGET_DIR jobs=$CARGO_BUILD_JOBS"
exit "${FLAMEWM_FAKE_EXIT:-0}"
"""


@unittest.skipUnless(Path("/usr/bin/time").exists(), "GNU time is required by bench-cargo-build.sh")
class BenchCargoBuildTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="flamewm-bench-cargo-")
        self.addCleanup(self.temp.cleanup)
        self.base = Path(os.path.realpath(self.temp.name))
        self.bin = self.base / "bin"
        self.bin.mkdir()
        self.target = self.base / "target"
        self.logs = self.base / "logs"
        cargo = self.bin / "cargo"
        cargo.write_text(FAKE_CARGO, encoding="utf-8")
        cargo.chmod(0o755)
        self.env = dict(os.environ)
        self.env["PATH"] = f"{self.bin}{os.pathsep}{os.environ['PATH']}"
        self.env["FLAMEWM_BENCH_LOG_DIR"] = str(self.logs)
        self.env.pop("CARGO_TARGET_DIR", None)

    def run_bench(self, *args, env_extra=None):
        env = dict(self.env)
        env.update(env_extra or {})
        return subprocess.run(
            ["bash", str(BENCH), "--target-dir", str(self.target), "--jobs", "3", "--label", "fixture", *args],
            env=env, capture_output=True, text=True, timeout=60,
        )

    def rows(self):
        lines = (self.logs / "runs.tsv").read_text(encoding="utf-8").splitlines()
        return [dict(zip(lines[0].split("\t"), line.split("\t"))) for line in lines[1:]]

    def test_success_records_evidence_row(self):
        result = self.run_bench("--profile", "fast")
        self.assertEqual(result.returncode, 0, result.stderr)
        row = self.rows()[-1]
        self.assertEqual(row["exit"], "0")
        self.assertEqual(row["profile"], "fast")
        self.assertEqual(row["jobs"], "3")
        self.assertEqual(row["target_dir"], str(self.target))
        self.assertEqual(row["label"], "fixture")
        self.assertEqual(row["cargo"], "cargo 0.0.0-fake")
        self.assertIn("--profile fast --workspace --locked", row["command"])
        self.assertNotEqual(row["wall_s"], "NA")
        self.assertNotEqual(row["max_rss_mb"], "NA")
        self.assertTrue(Path(self.logs / f"{row['run_id']}.log").read_text().find("jobs=3") >= 0)

    def test_child_exit_code_is_propagated_and_recorded(self):
        result = self.run_bench("--profile", "fast", env_extra={"FLAMEWM_FAKE_EXIT": "7"})
        self.assertEqual(result.returncode, 7, result.stderr)
        self.assertEqual(self.rows()[-1]["exit"], "7")

    def test_refuses_when_same_target_dir_is_busy(self):
        busy_bin = self.bin / "rustc"
        busy_bin.write_text("#!/bin/sh\nsleep 30\n", encoding="utf-8")
        busy_bin.chmod(0o755)
        holder = subprocess.Popen(
            [str(busy_bin), "30"],
            env=dict(self.env, CARGO_TARGET_DIR=str(self.target)),
        )
        self.addCleanup(holder.kill)
        time.sleep(0.2)
        result = self.run_bench("--profile", "fast")
        self.assertEqual(result.returncode, 3, result.stderr)
        self.assertIn("refusing to start", result.stderr)
        self.assertFalse((self.logs / "runs.tsv").exists())

    def test_rejects_non_numeric_jobs(self):
        result = subprocess.run(
            ["bash", str(BENCH), "--target-dir", str(self.target), "--jobs", "x"],
            env=self.env, capture_output=True, text=True, timeout=60,
        )
        self.assertEqual(result.returncode, 2)


if __name__ == "__main__":
    unittest.main()

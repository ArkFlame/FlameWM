"""Unit tests for flamewm_frame_interval parsing and summaries. No display required."""

from __future__ import annotations

import contextlib
import io
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import flamewm_frame_interval as fi  # noqa: E402

LOG = """\
FLAMEWM_PROFILE_SUMMARY
PROCESS name=flamewm cpu_total=12.00ms cpu_pct=1.2 window=10000.00ms
 smaps pss=100kB rss=200kB swap=0kB
PRESENT_INTERVAL:
 present-interval count=120 p50=16.67ms p95=33.33ms max=48.10ms
MEMORY_TOP:
 heap current=1B peak=2B
FLAMEWM_PROFILE_SUMMARY
PROCESS name=flamewm cpu_total=5.00ms cpu_pct=0.5 window=10000.00ms
MEMORY_TOP:
FLAMEWM_PROFILE_SUMMARY
PROCESS name=flamewm cpu_total=9.00ms cpu_pct=0.9 window=10000.00ms
PRESENT_INTERVAL:
 present-interval count=60 p50=8.00ms p95=20.00ms max=70.25ms
"""


class ParseLogTests(unittest.TestCase):
    def test_parses_only_windows_with_present_interval(self) -> None:
        records = fi.parse_log(LOG, "fallback")
        self.assertEqual(len(records), 2)
        self.assertEqual(records[0]["window"], 1)
        self.assertEqual(records[0]["count"], 120)
        self.assertAlmostEqual(records[0]["p50_ms"], 16.67)
        self.assertAlmostEqual(records[0]["p95_ms"], 33.33)
        self.assertAlmostEqual(records[0]["max_ms"], 48.10)
        self.assertEqual(records[1]["window"], 3)
        self.assertEqual(records[1]["count"], 60)

    def test_process_name_comes_from_process_line(self) -> None:
        text = LOG.replace("PROCESS name=flamewm", "PROCESS name=flamewm-shell", 1)
        records = fi.parse_log(text, "fallback")
        self.assertEqual(records[0]["process"], "flamewm-shell")
        self.assertEqual(records[1]["process"], "flamewm")

    def test_no_present_lines_yields_empty(self) -> None:
        self.assertEqual(fi.parse_log("FLAMEWM_PROFILE_SUMMARY\nPROCESS name=x cpu_total=1.00ms cpu_pct=0.1 window=1.00ms\n", "x"), [])

    def test_present_line_before_any_window_is_ignored(self) -> None:
        text = " present-interval count=1 p50=1.00ms p95=1.00ms max=1.00ms\n"
        self.assertEqual(fi.parse_log(text, "x"), [])


class SummaryTests(unittest.TestCase):
    def test_nearest_rank(self) -> None:
        self.assertEqual(fi.nearest_rank([1.0, 2.0, 3.0, 4.0], 50), 2.0)
        self.assertEqual(fi.nearest_rank([1.0, 2.0, 3.0, 4.0], 95), 4.0)
        self.assertEqual(fi.nearest_rank([5.0], 95), 5.0)

    def test_process_summary(self) -> None:
        records = fi.parse_log(LOG, "fallback")
        s = fi.summarize_process(records)
        self.assertEqual(s["windows"], 2)
        self.assertEqual(s["count"], 180)
        self.assertAlmostEqual(s["p50_ms"], 8.00)
        self.assertAlmostEqual(s["p95_ms"], 33.33)
        self.assertAlmostEqual(s["max_ms"], 70.25)

    def test_format_report_lists_process_and_windows(self) -> None:
        out = fi.format_report(fi.parse_log(LOG, "fallback"))
        self.assertIn("flamewm windows=2 count=180", out)
        self.assertIn("flamewm window=1 count=120 p50=16.67ms p95=33.33ms max=48.10ms", out)
        self.assertIn("flamewm window=3 count=60", out)

    def test_format_report_empty(self) -> None:
        self.assertEqual(fi.format_report([]), "no present-interval lines found\n")


class LoadLogsTests(unittest.TestCase):
    def test_reads_profile_logs_from_directory(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            Path(tmp, "flamewm.profile.log").write_text(LOG, encoding="utf-8")
            Path(tmp, "ignored.txt").write_text(LOG, encoding="utf-8")
            records = fi.load_logs(Path(tmp))
        self.assertEqual(len(records), 2)
        self.assertEqual({r["process"] for r in records}, {"flamewm"})


class MainTests(unittest.TestCase):
    def test_orchestrate_prints_steps_without_running(self) -> None:
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            rc = fi.main(["--orchestrate"])
        self.assertEqual(rc, 0)
        self.assertIn("FLAMEWM_PROFILE=1", buf.getvalue())
        self.assertIn("tools/flamewm_xtest.py", buf.getvalue())

    def test_missing_directory_is_error(self) -> None:
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            rc = fi.main(["/nonexistent-frame-interval-dir"])
        self.assertEqual(rc, 2)

    def test_no_arguments_is_error(self) -> None:
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            rc = fi.main([])
        self.assertEqual(rc, 2)


if __name__ == "__main__":
    unittest.main()

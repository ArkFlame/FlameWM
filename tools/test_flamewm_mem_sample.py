import contextlib
import io
import json
import os
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import flamewm_mem_sample as mod  # noqa: E402

SMAPS_FIXTURE = """\
Rss:                5120 kB
Pss:                2048 kB
Shared_Clean:       1024 kB
Private_Clean:      1000 kB
Private_Dirty:      2096 kB
Anonymous:          3000 kB
"""

STATUS_FIXTURE = """\
Name:\tflamewm
VmPeak:\t  200000 kB
VmHWM:\t    6144 kB
Threads:\t12
"""


class ParserTests(unittest.TestCase):
    def test_smaps_rollup_fields_in_bytes(self):
        got = mod.parse_smaps_rollup(SMAPS_FIXTURE)
        self.assertEqual(got, {
            "rss": 5120 * 1024,
            "pss": 2048 * 1024,
            "private_dirty": 2096 * 1024,
            "private_clean": 1000 * 1024,
        })

    def test_smaps_rollup_missing_field_raises(self):
        with self.assertRaises(ValueError):
            mod.parse_smaps_rollup("Rss: 1 kB\nPss: 1 kB\n")

    def test_smaps_rollup_bad_unit_raises(self):
        bad = SMAPS_FIXTURE.replace("5120 kB", "5120 MB")
        with self.assertRaises(ValueError):
            mod.parse_smaps_rollup(bad)

    def test_status_fields(self):
        got = mod.parse_status(STATUS_FIXTURE)
        self.assertEqual(got, {"vmhwm": 6144 * 1024, "threads": 12})

    def test_status_missing_threads_raises(self):
        with self.assertRaises(ValueError):
            mod.parse_status("VmHWM:\t10 kB\n")

    def test_parse_pid_arg(self):
        self.assertEqual(mod.parse_pid_arg("wm=123"), ("wm", 123))

    def test_parse_pid_arg_rejects_bad_input(self):
        for bad in ("wm", "=12", "wm=", "wm=abc", "wm=0"):
            with self.subTest(bad=bad):
                with self.assertRaises(Exception):
                    mod.parse_pid_arg(bad)


class StatsTests(unittest.TestCase):
    def test_median_and_p95_nearest_rank(self):
        values = list(range(1, 21))  # 1..20
        self.assertEqual(mod.percentile95(values), 19)
        self.assertEqual(mod.summarize(values), {"median": 10.5, "p95": 19})

    def test_single_value(self):
        self.assertEqual(mod.summarize([7]), {"median": 7, "p95": 7})

    def test_target_constant(self):
        self.assertEqual(mod.TARGET_BYTES, 35651584)
        self.assertEqual(mod.TARGET_BYTES, 34 * 1024 * 1024)

    def test_build_report_combined_and_reached(self):
        mib_bytes = 1024 * 1024
        pids = [("wm", 1), ("shell", 2)]

        def rec(comm, rss_mib, pss_mib):
            return {
                "rss": rss_mib * mib_bytes, "pss": pss_mib * mib_bytes,
                "private_dirty": 0, "private_clean": 0, "vmhwm": 0,
                "threads": 1, "comm": comm, "exe": None,
            }

        rounds = [
            {"wm": rec("wm", 25, 10), "shell": rec("sh", 10, 5)},
            {"wm": rec("wm", 27, 11), "shell": rec("sh", 14, 6)},
        ]
        report = mod.build_report(pids, rounds, 2, 0.0)
        rss = report["combined"]["rss"]
        self.assertEqual(rss["median_mib"], 38.0)  # (35 + 41) / 2
        self.assertFalse(rss["median_reached"])  # 38 MiB is over the 34 MiB target
        pss = report["combined"]["pss"]
        self.assertEqual(pss["median_mib"], 16.0)
        self.assertTrue(pss["median_reached"])  # 16 MiB is under the target
        self.assertEqual(report["processes"]["wm"]["metrics"]["rss"]["median"], 26 * mib_bytes)

    def test_median_reached_means_at_or_under_target(self):
        mib_bytes = 1024 * 1024
        pids = [("wm", 1)]

        def rounds_at(mib):
            return [
                {"wm": {"rss": mib * mib_bytes, "pss": mib * mib_bytes, "private_dirty": 0,
                        "private_clean": 0, "vmhwm": 0, "threads": 1, "comm": "wm", "exe": None}}
                for _ in range(2)
            ]

        under = mod.build_report(pids, rounds_at(20), 2, 0.0)["combined"]["rss"]
        at = mod.build_report(pids, rounds_at(mod.TARGET_MIB), 2, 0.0)["combined"]["rss"]
        over = mod.build_report(pids, rounds_at(50), 2, 0.0)["combined"]["rss"]
        self.assertTrue(under["median_reached"])
        self.assertTrue(at["median_reached"])
        self.assertFalse(over["median_reached"])


class LiveSmokeTests(unittest.TestCase):
    def test_self_pid_sample(self):
        if not os.path.isdir(f"/proc/{os.getpid()}"):
            self.skipTest("no /proc")
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            rc = mod.main([
                "--pid", f"self={os.getpid()}",
                "--samples", "2", "--interval", "0",
            ])
        self.assertEqual(rc, 0)
        report = json.loads(buf.getvalue())
        self.assertEqual(report["samples"], 2)
        proc = report["processes"]["self"]
        self.assertEqual(proc["pid"], os.getpid())
        self.assertGreater(proc["metrics"]["rss"]["median"], 0)
        self.assertGreaterEqual(proc["metrics"]["threads"]["median"], 1)
        self.assertIn("pss", report["combined"])
        self.assertEqual(report["combined"]["rss"]["target_bytes"], 35651584)

    def test_missing_pid_fails(self):
        err = io.StringIO()
        with contextlib.redirect_stderr(err), contextlib.redirect_stdout(io.StringIO()):
            rc = mod.main(["--pid", "ghost=2147483646"])
        self.assertEqual(rc, 2)


if __name__ == "__main__":
    unittest.main()

"""Pure unit tests for the idle wakeup comparison runner. No X server is touched.

Run with:
    python3 -I tools/test_flamewm_idle_wakeups_compare.py
"""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))

import flamewm_idle_wakeups_compare as wk  # noqa: E402


STATUS = """Name:\tflamewm
voluntary_ctxt_switches:\t{vol}
nonvoluntary_ctxt_switches:\t{nonvol}
"""


class ParseTests(unittest.TestCase):
    def test_status_separates_voluntary_from_nonvoluntary(self) -> None:
        got = wk.parse_status(STATUS.format(vol=11, nonvol=3))
        self.assertEqual(got, {"vol": 11, "nonvol": 3})

    def test_status_without_fields_is_rejected(self) -> None:
        with self.assertRaises(ValueError):
            wk.parse_status("Name:\tflamewm\n")

    def test_stat_ticks_survive_parentheses_in_comm(self) -> None:
        # utime=7 (field 14), stime=5 (field 15); comm contains ") (" on purpose.
        stat = "42 (flame) (wm) S 1 42 42 0 -1 4194560 0 0 0 0 7 5 0 0 20 0 3 0 1 0 0\n"
        self.assertEqual(wk.parse_stat_ticks(stat), 12)


class WindowDeltaTests(unittest.TestCase):
    def test_delta_is_per_window_and_grouped_by_thread_name(self) -> None:
        start = {
            "1": {"comm": "flamewm", "ticks": 10, "vol": 100, "nonvol": 1},
            "2": {"comm": "flamewm-wm-timer", "ticks": 0, "vol": 50, "nonvol": 0},
        }
        end = {
            "1": {"comm": "flamewm", "ticks": 12, "vol": 103, "nonvol": 1},
            "2": {"comm": "flamewm-wm-timer", "ticks": 1, "vol": 58, "nonvol": 2},
        }
        got = wk.window_delta(start, end)
        self.assertEqual(got["total"], {"vol": 11, "nonvol": 2, "cpu_ticks": 3})
        self.assertEqual(got["threads"]["flamewm-wm-timer"]["vol"], 8)

    def test_thread_created_in_window_counts_from_zero(self) -> None:
        end = {"9": {"comm": "worker", "ticks": 4, "vol": 6, "nonvol": 0}}
        self.assertEqual(wk.window_delta({}, end)["total"]["vol"], 6)

    def test_exited_thread_is_not_counted_as_negative(self) -> None:
        start = {"3": {"comm": "gone", "ticks": 4, "vol": 9, "nonvol": 0}}
        self.assertEqual(wk.window_delta(start, {})["total"]["vol"], 0)


class SummaryTests(unittest.TestCase):
    def _run(self, label: str, vol: int) -> dict:
        proc = {
            "total": {"vol": vol, "nonvol": 0, "cpu_ticks": 0},
            "threads": {"flamewm": {"vol": vol, "nonvol": 0, "cpu_ticks": 0}},
        }
        return {
            "label": label,
            "status": "OK",
            "processes": {n: proc for n in wk.PROCESSES},
        }

    def test_median_over_runs_per_label(self) -> None:
        runs = [self._run("baseline", v) for v in (2, 9, 4)] + [self._run("current", 40)]
        summary = wk.summarize(runs)
        self.assertEqual(summary["baseline"]["flamewm"]["vol_median"], 4)
        self.assertEqual(summary["current"]["flamewm"]["vol_median"], 40)
        self.assertEqual(summary["baseline"]["flamewm"]["threads_vol_median"], {"flamewm": 4})

    def test_failed_runs_are_excluded(self) -> None:
        failed = {"label": "current", "status": "FAIL", "reason": "x"}
        summary = wk.summarize([self._run("current", 7), failed])
        self.assertEqual(summary["current"]["flamewm"]["runs"], 1)


if __name__ == "__main__":
    unittest.main()

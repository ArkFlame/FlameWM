"""Pure unit tests for the old-vs-new perf comparison helpers. No X server or display is touched.

Run with:
    python3 -I tools/test_flamewm_perf_compare.py
"""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))

import flamewm_perf_compare as perf  # noqa: E402

STAT = "4242 (flamewm shell) S 1 4242 4242 0 -1 4194560 100 0 0 0 " \
       "37 5 0 0 20 0 3 0 12345 0 0 0 0"  # utime=37 stime=5 -> 42 ticks; comm has a space
STATUS = """Name:\tflamewm
VmHWM:\t   20480 kB
VmRSS:\t   18000 kB
voluntary_ctxt_switches:\t1200
nonvoluntary_ctxt_switches:\t34
"""


class OrderTests(unittest.TestCase):
    def test_order_alternates_each_repetition(self) -> None:
        self.assertEqual(perf.order_for_rep(0), ("old", "new"))
        self.assertEqual(perf.order_for_rep(1), ("new", "old"))
        self.assertEqual(perf.order_for_rep(2), ("old", "new"))


class ParseTests(unittest.TestCase):
    def test_stat_ticks_sum_utime_and_stime_with_spaced_comm(self) -> None:
        self.assertEqual(perf.parse_stat_ticks(STAT), 42)

    def test_status_counters_include_both_switch_kinds_and_hwm(self) -> None:
        got = perf.parse_status_counters(STATUS)
        self.assertEqual(got, {"ctx_switches": 1234, "vmhwm_kb": 20480})

    def test_missing_switch_counter_is_none_not_zero(self) -> None:
        got = perf.parse_status_counters("VmHWM:\t10 kB\nvoluntary_ctxt_switches:\t5\n")
        self.assertIsNone(got["ctx_switches"])
        self.assertEqual(got["vmhwm_kb"], 10)


class StatsTests(unittest.TestCase):
    def test_nearest_rank_p95_on_five_values_is_the_maximum(self) -> None:
        self.assertEqual(perf.nearest_rank_p95([1.0, 2.0, 3.0, 4.0, 5.0]), 5.0)

    def test_nearest_rank_p95_on_twenty_values_is_the_19th(self) -> None:
        self.assertEqual(perf.nearest_rank_p95([float(v) for v in range(1, 21)]), 19.0)

    def test_summarize_is_order_independent(self) -> None:
        got = perf.summarize([5.0, 1.0, 3.0, 2.0, 4.0])
        self.assertEqual(got, {"n": 5, "min": 1.0, "median": 3.0, "p95": 5.0})

    def test_summarize_rejects_empty_sample(self) -> None:
        with self.assertRaises(ValueError):
            perf.summarize([])

    def test_ratio_is_none_when_baseline_is_zero(self) -> None:
        self.assertIsNone(perf.ratio(2.0, 0.0))
        self.assertAlmostEqual(perf.ratio(3.0, 2.0), 1.5)


def _set(label: str, minimized: int, reason: str = "") -> dict:
    return {"label": label, "reason": reason,
            "idle": {"cpu_s": {"flamewm": 0.5, "flamewm-shell": 0.25},
                     "ctx_switches": {"flamewm": 3, "flamewm-shell": 4},
                     "vmhwm_kb": {"flamewm": 100, "flamewm-shell": 200}},
            "start": {"cpu_s": {"flamewm": 1.0, "flamewm-shell": 1.0}, "wall_s": 9.0},
            "minimize": {"cpu_s": {"flamewm": 0.1, "flamewm-shell": 0.1}, "wall_s": 1.0,
                         "minimized": minimized, "requested": 10, "step": "ok"}}


class PhaseTests(unittest.TestCase):
    def test_incomplete_minimize_phase_is_dropped_while_other_phases_are_kept(self) -> None:
        sets = [_set("new", minimized=7, reason="minimize: only 7/10"), _set("old", minimized=10)]
        rows = perf.phase_rows(sets, "new")
        self.assertEqual(rows["minimize"], [])
        self.assertEqual(len(rows["idle"]), 1)
        self.assertEqual(len(rows["start"]), 1)

    def test_summary_is_none_for_label_without_completed_phase(self) -> None:
        sets = [_set("old", minimized=10), _set("new", minimized=7)]
        summary = perf.build_summary(sets)
        self.assertIsNone(summary["minimize_cpu_s"]["new"])
        self.assertIsNotNone(summary["minimize_cpu_s"]["old"])
        self.assertIsNone(summary["minimize_cpu_s"]["ratio"])
        self.assertAlmostEqual(summary["idle_cpu_s"]["ratio"], 1.0)


class MarkdownTests(unittest.TestCase):
    def test_table_has_every_metric_and_reports_problems(self) -> None:
        row = perf.summarize([1.0, 2.0])
        summary = {key: {"old": row, "new": row, "ratio": 1.0} for key, _ in perf.METRICS}
        text = perf.render_markdown(summary, reps=2, problems=["rep1-new: x"])
        for _, title in perf.METRICS:
            self.assertIn(title, text)
        self.assertIn("rep1-new: x", text)
        self.assertIn("X11 request and allocation counters were NOT collected", text)


if __name__ == "__main__":
    unittest.main()

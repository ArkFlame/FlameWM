"""Pure unit tests for the per-phase memory comparison helpers. No X server or display is touched.

Run with:
    python3 -I tools/test_flamewm_mem_phase_compare.py
"""

from __future__ import annotations

import contextlib
import io
import sys
import tempfile
import unittest
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))

import flamewm_mem_phase_compare as mpc  # noqa: E402
import flamewm_mem_sample as mem  # noqa: E402

MIB = 1024 * 1024


def proc(rss: int, pss: int, dirty: int, threads: int) -> dict:
    return {"rss": rss, "pss": pss, "private_dirty": dirty, "threads": threads}


def round_sample(wm_pss: int, shell_pss: int, xe_pss: int, wm_rss: int = 0, shell_rss: int = 0) -> dict:
    return {
        "flamewm": proc(wm_rss, wm_pss, 100, 3),
        "flamewm-shell": proc(shell_rss, shell_pss, 200, 5),
        "xephyr": proc(0, xe_pss, 50, 9),
    }


def rep_row(label: str, rep: int, phases: dict[str, list[dict] | None]) -> dict:
    """A run_set result: phases maps name -> samples (None marks NOT_RUN)."""
    out = {"label": label, "rep": rep, "phases": {}}
    for name in mpc.PHASES:
        samples = phases.get(name)
        if samples is None:
            out["phases"][name] = {"status": "NOT_RUN: test", "samples": []}
        else:
            out["phases"][name] = {"status": "ok", "samples": samples}
    out["reason"] = "" if all(v["status"] == "ok" for v in out["phases"].values()) else "partial"
    return out


def stats_for(pss_mib: float, rss_mib: float) -> dict:
    """A phase_stats()-shaped value with a single-rep median/p95 for the verdict tests."""
    def s(v: float) -> dict:
        return {"n": 1, "min": v, "median": v, "p95": v}
    ws = {m: s(0) for m in mpc.WS_METRICS}
    ws["pss"] = s(pss_mib * MIB)
    ws["rss"] = s(rss_mib * MIB)
    return {"wm_shell": ws, "xephyr": {m: s(0) for m in mpc.XE_METRICS}}


def full_agg(pss_mib: float, rss_mib: float) -> dict:
    return {p: stats_for(pss_mib, rss_mib) for p in mpc.PHASES}


class CombineTests(unittest.TestCase):
    def test_sums_wm_and_shell_and_excludes_xephyr(self) -> None:
        combined = mpc.combine_ws(round_sample(wm_pss=10, shell_pss=20, xe_pss=999, wm_rss=30, shell_rss=40))
        self.assertEqual(combined["pss"], 30)
        self.assertEqual(combined["rss"], 70)
        self.assertEqual(combined["private_dirty"], 300)
        self.assertEqual(combined["threads"], 8)

    def test_phase_rep_value_medians_samples_and_keeps_xephyr_line(self) -> None:
        samples = [round_sample(10, 10, 100), round_sample(30, 10, 300), round_sample(20, 10, 200)]
        value = mpc.phase_rep_value(samples)
        self.assertEqual(value["wm_shell"]["pss"], 30)  # median of 20, 40, 30
        self.assertEqual(value["xephyr"]["pss"], 200)   # median of 100, 300, 200, separate line
        even = mpc.phase_rep_value([round_sample(10, 0, 0), round_sample(30, 0, 0)])
        self.assertEqual(even["wm_shell"]["pss"], 20)


class PhaseStatsTests(unittest.TestCase):
    def test_empty_repetitions_give_none(self) -> None:
        self.assertIsNone(mpc.phase_stats([]))

    def test_median_and_nearest_rank_p95_across_reps(self) -> None:
        reps = [mpc.phase_rep_value([round_sample(v, 0, 0)]) for v in (1, 2, 3, 4, 100)]
        stats = mpc.phase_stats(reps)
        self.assertEqual(stats["wm_shell"]["pss"]["median"], 3)
        self.assertEqual(stats["wm_shell"]["pss"]["p95"], 100)
        self.assertEqual(stats["wm_shell"]["pss"]["n"], 5)


class AggregateTests(unittest.TestCase):
    def test_only_ok_phases_of_the_requested_label_count(self) -> None:
        sets = [
            rep_row("old", 0, {"cold": [round_sample(10, 0, 0)], "idle": None}),
            rep_row("new", 0, {"cold": [round_sample(99, 0, 0)]}),
            rep_row("old", 1, {"cold": [round_sample(30, 0, 0)], "idle": [round_sample(5, 0, 0)]}),
            {"label": "old", "rep": 2, "phases": {}, "reason": "restart"},
        ]
        agg = mpc.aggregate(sets, "old")
        self.assertEqual(agg["cold"]["wm_shell"]["pss"]["n"], 2)
        self.assertEqual(agg["cold"]["wm_shell"]["pss"]["median"], 20)
        self.assertEqual(agg["idle"]["wm_shell"]["pss"]["n"], 1)
        self.assertIsNone(agg["start"])
        self.assertEqual(mpc.aggregate(sets, "new")["cold"]["wm_shell"]["pss"]["median"], 99)


class VerdictTests(unittest.TestCase):
    TARGET = mem.TARGET_BYTES

    def test_target_constant_is_34_mib(self) -> None:
        self.assertEqual(self.TARGET, 35651584)
        self.assertEqual(mpc.mib_text(self.TARGET), "34.000 MiB")

    def test_all_phases_within(self) -> None:
        v = mpc.verdict(full_agg(20, 25))
        self.assertEqual(v["status"], "WITHIN")
        self.assertEqual(v["not_run_phases"], [])
        self.assertEqual(len(v["lines"]), len(mpc.PHASES))

    def test_equal_to_target_is_within(self) -> None:
        self.assertEqual(mpc.verdict(full_agg(34, 34))["status"], "WITHIN")

    def test_pss_above_target_is_over(self) -> None:
        agg = full_agg(20, 25)
        agg["drag"] = stats_for(34.5, 25)
        v = mpc.verdict(agg)
        self.assertEqual(v["status"], "OVER")
        self.assertIn("PSS median", [line for line in v["lines"] if line.startswith("drag")][0])
        self.assertIn("OVER", [line for line in v["lines"] if line.startswith("drag")][0])

    def test_rss_only_above_target_is_over_and_named(self) -> None:
        agg = full_agg(20, 25)
        agg["idle"] = stats_for(20, 40)
        v = mpc.verdict(agg)
        self.assertEqual(v["status"], "OVER")
        idle_line = [line for line in v["lines"] if line.startswith("idle")][0]
        self.assertIn("RSS median", idle_line)
        self.assertIn("OVER", idle_line)

    def test_no_phase_measured_is_not_run(self) -> None:
        v = mpc.verdict({p: None for p in mpc.PHASES})
        self.assertEqual(v["status"], "NOT_RUN")
        self.assertEqual(v["not_run_phases"], list(mpc.PHASES))

    def test_partial_marks_status_and_lists_missing_phases(self) -> None:
        agg = {p: None for p in mpc.PHASES}
        agg["cold"] = stats_for(20, 25)
        v = mpc.verdict(agg)
        self.assertEqual(v["status"], "WITHIN+PARTIAL")
        self.assertEqual(v["not_run_phases"], [p for p in mpc.PHASES if p != "cold"])


class RenderTests(unittest.TestCase):
    def test_markdown_has_verdict_per_label_and_xephyr_rows(self) -> None:
        sets = [rep_row("old", 0, {p: [round_sample(20, 0, 300, 25, 0)] for p in mpc.PHASES}),
                rep_row("new", 0, {p: [round_sample(10, 0, 300, 12, 0)] for p in mpc.PHASES})]
        summary = {label: mpc.aggregate(sets, label) for label in mpc.LABELS}
        verdicts = {label: mpc.verdict(summary[label]) for label in mpc.LABELS}
        text = mpc.render_markdown(summary, verdicts, reps=1, problems=[])
        self.assertIn("VERDICT old: WITHIN vs 34 MiB (35651584 B)", text)
        self.assertIn("VERDICT new: WITHIN vs 34 MiB (35651584 B)", text)
        self.assertIn("Xephyr (private, separate from the verdict)", text)
        self.assertIn("| cold | pss |", text)
        self.assertIn("Problems: none.", text)

    def test_missing_phase_renders_na_not_crash(self) -> None:
        sets = [rep_row("old", 0, {"cold": [round_sample(20, 0, 0)]})]
        summary = {label: mpc.aggregate(sets, label) for label in mpc.LABELS}
        verdicts = {label: mpc.verdict(summary[label]) for label in mpc.LABELS}
        text = mpc.render_markdown(summary, verdicts, reps=1, problems=["run incomplete"])
        self.assertIn("n/a", text)
        self.assertIn("VERDICT new: NOT_RUN", text)
        self.assertIn("run incomplete", text)


class OrderTests(unittest.TestCase):
    def test_order_alternates_each_repetition(self) -> None:
        self.assertEqual(mpc.perf.order_for_rep(0), ("old", "new"))
        self.assertEqual(mpc.perf.order_for_rep(1), ("new", "old"))
        self.assertEqual(mpc.perf.order_for_rep(2), ("old", "new"))


class IconArgumentTests(unittest.TestCase):
    def parse(self, *argv: str):
        return mpc.parse_args(list(argv))

    def rejects(self, *argv: str) -> None:
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as cm:
            self.parse(*argv)
        self.assertEqual(cm.exception.code, 2)

    def test_default_has_no_icon_scenario(self) -> None:
        args = self.parse()
        self.assertIsNone(args.icon_theme_dir)
        self.assertIsNone(args.synthetic_icons)
        self.assertEqual(mpc.icon_scenario(args), {"mode": "none"})
        self.assertEqual(mpc.scenario_text(mpc.icon_scenario(args)), "none (default, no icon theme staged)")

    def test_synthetic_icons_scenario(self) -> None:
        scenario = mpc.icon_scenario(self.parse("--synthetic-icons", "12"))
        self.assertEqual(scenario, {"mode": "synthetic", "name": mpc.SYNTHETIC_THEME, "files": 12})

    def test_icon_theme_dir_scenario_uses_dir_name(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            theme = Path(tmp) / "BigTheme"
            theme.mkdir()
            (theme / "index.theme").write_text("[Icon Theme]\nName=BigTheme\n", encoding="ascii")
            scenario = mpc.icon_scenario(self.parse("--icon-theme-dir", str(theme)))
        self.assertEqual(scenario, {"mode": "dir", "name": "BigTheme", "source": str(theme)})

    def test_icon_options_are_mutually_exclusive(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            self.rejects("--icon-theme-dir", tmp, "--synthetic-icons", "3")

    def test_synthetic_count_must_be_positive(self) -> None:
        self.rejects("--synthetic-icons", "0")
        self.rejects("--synthetic-icons", "-4")

    def test_icon_theme_dir_must_contain_index_theme(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            self.rejects("--icon-theme-dir", tmp)
        self.rejects("--icon-theme-dir", "/nonexistent/flamewm-theme")


class SyntheticThemeTests(unittest.TestCase):
    def test_generates_requested_file_count_with_index(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            icons = Path(tmp)
            theme = mpc.write_synthetic_theme(icons, 37)
            files = [p for p in theme.rglob("*") if p.is_file() and p.name != "index.theme"]
            self.assertEqual(len(files), 37)
            self.assertEqual({p.suffix for p in files}, {".png", ".svg"})
            index = (theme / "index.theme").read_text(encoding="ascii")
        self.assertIn(f"Name={mpc.SYNTHETIC_THEME}\n", index)
        self.assertIn("Inherits=hicolor", index)
        self.assertIn("Type=Scalable", index)

    def test_index_lists_every_directory_with_a_section(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            theme = mpc.write_synthetic_theme(Path(tmp), 1)
            index = (theme / "index.theme").read_text(encoding="ascii")
            on_disk = sorted(p.relative_to(theme).as_posix() for p in theme.glob("*/apps"))
            listed = index.split("Directories=")[1].split("\n")[0].split(",")
            self.assertEqual(sorted(listed), on_disk)
            self.assertEqual(len(listed), len(mpc.SYNTHETIC_SIZES) + 1)
            for name in listed:
                self.assertIn(f"[{name}]\n", index)
                self.assertTrue((theme / name).is_dir())

    def test_files_are_real_small_images(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            theme = mpc.write_synthetic_theme(Path(tmp), 9)
            pngs = sorted(theme.rglob("*.png"))
            svgs = sorted(theme.rglob("*.svg"))
            self.assertTrue(pngs and svgs)
            self.assertTrue(all(p.read_bytes().startswith(b"\x89PNG\r\n\x1a\n") for p in pngs))
            self.assertTrue(all(p.read_bytes().startswith(b"<svg") for p in svgs))


class StageIconScenarioTests(unittest.TestCase):
    def test_default_stages_nothing(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            run = Path(tmp)
            mpc.stage_icon_scenario(run, mpc.parse_args([]))
            self.assertEqual(list(run.iterdir()), [])

    def test_synthetic_stages_theme_and_config_name(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            run = Path(tmp)
            mpc.stage_icon_scenario(run, mpc.parse_args(["--synthetic-icons", "5"]))
            theme = run / "xdg-data" / "icons" / mpc.SYNTHETIC_THEME
            self.assertTrue((theme / "index.theme").is_file())
            kde = (run / "xdg-config" / "kdeglobals").read_text(encoding="utf-8")
            gtk = (run / "xdg-config" / "gtk-3.0" / "settings.ini").read_text(encoding="utf-8")
        self.assertEqual(kde, f"[Icons]\nTheme={mpc.SYNTHETIC_THEME}\n")
        self.assertIn(f"gtk-icon-theme-name={mpc.SYNTHETIC_THEME}", gtk)

    def test_icon_theme_dir_is_symlinked_and_named(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            src = Path(tmp) / "src" / "BigTheme"
            src.mkdir(parents=True)
            (src / "index.theme").write_text("[Icon Theme]\nName=BigTheme\n", encoding="ascii")
            run = Path(tmp) / "run"
            run.mkdir()
            mpc.stage_icon_scenario(run, mpc.parse_args(["--icon-theme-dir", str(src)]))
            link = run / "xdg-data" / "icons" / "BigTheme"
            self.assertTrue(link.is_symlink())
            self.assertEqual(link.resolve(), src.resolve())
            kde = (run / "xdg-config" / "kdeglobals").read_text(encoding="utf-8")
        self.assertEqual(kde, "[Icons]\nTheme=BigTheme\n")


if __name__ == "__main__":
    unittest.main(verbosity=2)

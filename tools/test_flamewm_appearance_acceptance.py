"""Pure unit tests for the appearance acceptance runner. No X server or display is touched.

Run with:
    python3 -I tools/test_flamewm_appearance_acceptance.py
"""

from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))

import flamewm_appearance_acceptance as ap  # noqa: E402

TREE_FIXTURE = """\
xwininfo: Window id: 0x41 (the root window) (has no name)

  Root window id: 0x41 (the root window) (has no name)
  Parent window id: 0x0 (none)
     5 children:
     0x400002 "FlameWM panel": ("flamewm" "FlameWM") 1280x44+0+756  +0+756
     0x400010 (has no name): ()  310x231+300+200  +300+200
        1 child:
        0x400011 "xclock": ("xclock" "XClock")  300x200+5+31  +305+231
     0x400020 "FlameWM start": ("flamewm" "FlameWM")  300x400+0+350  +0+350
     0x400030 (has no name): ()  1x1+-5+-5  +-5+-5
"""


class BlendAndPixelTests(unittest.TestCase):
    def test_blend_opacity_extremes(self) -> None:
        self.assertEqual(ap.blend((0, 0, 0), (255, 255, 255), 100), (255, 255, 255))
        self.assertEqual(ap.blend((10, 20, 30), (200, 100, 0), 0), (10, 20, 30))

    def test_blend_is_strictly_between_background_and_accent(self) -> None:
        bg, accent = (25, 27, 29), (0, 170, 0)
        mixed = ap.blend(bg, accent, 40)
        for b, a, m in zip(bg, accent, mixed):
            self.assertTrue(min(a, b) <= m <= max(a, b))
        self.assertNotEqual(mixed, accent)
        self.assertNotEqual(mixed, bg)

    def test_close_enough_rejects_wrong_color(self) -> None:
        self.assertTrue(ap.close_enough((51, 102, 153), (51, 102, 153), 2))
        self.assertFalse(ap.close_enough((51, 102, 153), (51, 102, 158), 2))
        self.assertFalse(ap.close_enough(None, (1, 2, 3), 2))

    def test_fraction_matching_requires_majority_for_taskbar_background(self) -> None:
        color = (51, 102, 153)
        background = [color] * 60 + [(255, 255, 255)] * 40
        self.assertGreaterEqual(ap.fraction_matching(background, color, 2), 0.5)
        mostly_wrong = [(255, 255, 255)] * 60 + [color] * 40
        self.assertLess(ap.fraction_matching(mostly_wrong, color, 2), 0.5)

    def test_dominant_color(self) -> None:
        self.assertEqual(ap.dominant_color([(1, 1, 1), (2, 2, 2), (2, 2, 2)]), (2, 2, 2))
        self.assertIsNone(ap.dominant_color([]))

    def test_pixel_diff_count_detects_theme_change_and_identity(self) -> None:
        a = [(0, 0, 0)] * 10
        b = [(0, 0, 0)] * 7 + [(255, 0, 0)] * 3
        self.assertEqual(ap.pixel_diff_count(a, a), 0)
        self.assertEqual(ap.pixel_diff_count(a, b), 3)
        with self.assertRaises(ValueError):
            ap.pixel_diff_count(a, b[:5])


class MonotonicTests(unittest.TestCase):
    def test_slide_up_accepts_monotonic_descent_to_final(self) -> None:
        self.assertTrue(ap.slide_up_ok([700, 640, 580, 520, 520]))

    def test_slide_up_rejects_too_few_samples_and_non_monotonic(self) -> None:
        self.assertFalse(ap.slide_up_ok([700, 520]))
        self.assertFalse(ap.slide_up_ok([700, 640, 660, 520]))
        self.assertFalse(ap.slide_up_ok([520, 520, 520, 520]))

    def test_slide_up_rejects_jump_without_intermediate_positions(self) -> None:
        self.assertFalse(ap.slide_up_ok([700, 520, 520, 520]))

    def test_no_intermediate_requires_constant_positions(self) -> None:
        self.assertTrue(ap.no_intermediate_ok([520, 520, 520]))
        self.assertFalse(ap.no_intermediate_ok([700, 640, 520]))
        self.assertFalse(ap.no_intermediate_ok([520, 520]))

    def test_minimize_width_narrowing_helper(self) -> None:
        self.assertTrue(ap.is_nonincreasing([300, 200, 120, 40]))
        self.assertFalse(ap.is_nonincreasing([300, 320, 120]))


class ParsingTests(unittest.TestCase):
    def test_parse_tree_absolute_coordinates_and_parents(self) -> None:
        nodes = ap.parse_tree(TREE_FIXTURE)
        client = ap.find_named(nodes, "xclock")
        self.assertEqual((client["w"], client["h"]), (300, 200))
        self.assertEqual((client["x"], client["y"]), (305, 231))
        frame = ap.parent_of(nodes, client)
        self.assertEqual((frame["x"], frame["y"], frame["w"], frame["h"]), (300, 200, 310, 231))

    def test_find_named_missing_returns_none(self) -> None:
        self.assertIsNone(ap.find_named(ap.parse_tree(TREE_FIXTURE), "FlameWM task menu"))

    def test_start_popup_found_with_geometry(self) -> None:
        popup = ap.find_named(ap.parse_tree(TREE_FIXTURE), "FlameWM start")
        self.assertEqual((popup["x"], popup["y"], popup["w"], popup["h"]), (0, 350, 300, 400))

    def test_parse_icon_geometry(self) -> None:
        text = "_NET_WM_ICON_GEOMETRY(CARDINAL) = 1210, 760, 40, 36\n"
        self.assertEqual(ap.parse_icon_geometry(text), (1210, 760, 40, 36))
        self.assertIsNone(ap.parse_icon_geometry("_NET_WM_ICON_GEOMETRY:  not set\n"))

    def test_map_state(self) -> None:
        self.assertEqual(ap.map_state("  Map State: IsUnMapped\n"), "IsUnMapped")
        self.assertIsNone(ap.map_state("no state here"))

    def test_settings_text_is_revisioned_key_value(self) -> None:
        text = ap.settings_text({"taskbarColor": "#336699", "taskbarOpacity": "100"})
        self.assertTrue(text.startswith("revision=2\n"))
        self.assertIn("taskbarColor=#336699\n", text)
        self.assertIn("taskbarOpacity=100\n", text)


class IdleAndSchemaTests(unittest.TestCase):
    def test_idle_verdict_thresholds(self) -> None:
        self.assertEqual(ap.idle_verdict(0.0, 0, True), ap.PASS)
        self.assertEqual(ap.idle_verdict(0.2, 0, True), ap.FAIL)
        self.assertEqual(ap.idle_verdict(0.0, 400, True), ap.FAIL)  # 250 ms timer = 20 wakeups/5 s
        self.assertEqual(ap.idle_verdict(0.0, 0, False), ap.FAIL)

    def test_compose_covers_every_id_once_and_defaults_not_run(self) -> None:
        doc = ap.compose_document({})
        self.assertEqual([e["id"] for e in doc["results"]], list(ap.IDS))
        self.assertTrue(all(e["status"] == ap.NOT_RUN for e in doc["results"]))
        self.assertEqual(doc["verdict"], "INCOMPLETE")
        self.assertEqual(doc["exit_code"], 0)
        self.assertEqual(doc["schema"], "flamewm-appearance-acceptance/1")

    def test_all_pass_is_the_only_all_scenarios_verdict(self) -> None:
        results = {sid: ap.entry(sid, ap.PASS, evidence=["x"]) for sid in ap.IDS}
        doc = ap.compose_document(results)
        self.assertEqual((doc["verdict"], doc["exit_code"]), ("ALL_SCENARIOS_PASS", 0))
        results["AP05"] = ap.entry("AP05", ap.NOT_RUN, "partial")
        self.assertNotEqual(ap.compose_document(results)["verdict"], "ALL_SCENARIOS_PASS")

    def test_any_fail_forces_exit_one(self) -> None:
        results = {sid: ap.entry(sid, ap.PASS) for sid in ap.IDS}
        results["AP09"] = ap.entry("AP09", ap.FAIL, "idle cpu")
        doc = ap.compose_document(results)
        self.assertEqual((doc["verdict"], doc["exit_code"]), ("FAIL", 1))

    def test_all_blocked_is_environment_blocked(self) -> None:
        results = {sid: ap.entry(sid, ap.ENV_BLOCKED, "no Xephyr") for sid in ap.IDS}
        doc = ap.compose_document(results)
        self.assertEqual((doc["verdict"], doc["exit_code"]), ("ENVIRONMENT_BLOCKED", 3))

    def test_entry_rejects_unknown_status(self) -> None:
        with self.assertRaises(ValueError):
            ap.entry("AP01", "MAYBE")


class RunDirGuardTests(unittest.TestCase):
    def setUp(self) -> None:
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.run = Path(temp.name) / "run"

    def test_outer_refuses_non_empty_run_dir(self) -> None:
        self.run.mkdir()
        (self.run / "stale.txt").write_text("x", encoding="ascii")
        with self.assertRaises(SystemExit):
            ap.validate_run_dir(self.run)

    def test_outer_accepts_missing_or_empty_run_dir(self) -> None:
        ap.validate_run_dir(self.run)
        self.run.mkdir()
        ap.validate_run_dir(self.run)

    def test_inner_accepts_only_the_outer_owner_marker(self) -> None:
        self.run.mkdir()
        (self.run / ap.base.RUN_OWNER_MARKER).write_text("owner-pid=1\n", encoding="ascii")
        ap.validate_inner_run_dir(self.run)

    def test_inner_refuses_empty_run_dir_so_marker_is_required(self) -> None:
        self.run.mkdir()
        with self.assertRaises(SystemExit):
            ap.validate_inner_run_dir(self.run)

    def test_inner_refuses_extra_content_beside_marker(self) -> None:
        self.run.mkdir()
        (self.run / ap.base.RUN_OWNER_MARKER).write_text("owner-pid=1\n", encoding="ascii")
        (self.run / "stale.txt").write_text("x", encoding="ascii")
        with self.assertRaises(SystemExit):
            ap.validate_inner_run_dir(self.run)


if __name__ == "__main__":
    unittest.main()

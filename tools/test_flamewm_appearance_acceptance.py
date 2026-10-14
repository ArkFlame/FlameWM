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


class StartAboveWindowsMetricTests(unittest.TestCase):
    """Pure pixel metrics behind AP06 stacking; the negative controls must fail."""

    def setUp(self) -> None:
        from PIL import Image

        self.Image = Image
        self.rest = (100, 300, 200, 100)
        self.client = (120, 250, 300, 200)
        self.popup_colour = (0x19, 0x1B, 0x1D)

    def frame(self, colour, size=(400, 400)):
        return self.Image.new("RGB", size, colour)

    def test_count_white_counts_only_client_white(self):
        img = self.frame(self.popup_colour)
        img.paste((255, 255, 255), (150, 320, 180, 360))
        self.assertEqual(ap.count_white(img, self.rest), 30 * 40)
        self.assertEqual(ap.count_white(img, (0, 0, 50, 50)), 0)

    def test_popup_on_top_has_no_client_white_in_its_area(self):
        img = self.frame(self.popup_colour)
        self.assertEqual(ap.count_client_white(img, self.rest, self.client), 0)

    def test_client_white_inside_the_popup_area_is_counted(self):
        # Negative control: the client face (white) shows where the popup should be.
        img = self.frame(self.popup_colour)
        img.paste((255, 255, 255), (150, 320, 180, 360))
        self.assertGreater(ap.count_client_white(img, self.rest, self.client), ap.WHITE_SLACK)

    def test_popup_region_outside_the_client_is_ignored(self):
        img = self.frame(self.popup_colour)
        img.paste((255, 255, 255), (0, 0, 50, 50))
        self.assertEqual(ap.count_client_white(img, self.rest, self.client), 0)

    def test_disjoint_region_never_counts(self):
        img = self.frame((255, 255, 255))
        self.assertEqual(ap.count_client_white(img, (0, 0, 10, 10), self.client), 0)

    def test_popup_top_finds_the_first_full_width_popup_row(self):
        img = self.frame((0x40, 0x40, 0x40))
        img.paste(self.popup_colour, (100, 350, 300, 400))
        self.assertEqual(ap.popup_top(img, (100, 300, 200, 100), 400), 350)

    def test_popup_top_ignores_a_thin_client_edge(self):
        img = self.frame((0x40, 0x40, 0x40))
        img.paste(self.popup_colour, (100, 320, 150, 321))  # 25% of the rest width
        self.assertIsNone(ap.popup_top(img, (100, 300, 200, 100), 400))

    def test_strip_changed_detects_popup_below_the_panel(self):
        settled = self.frame((0x10, 0x10, 0x10), size=(400, 44))
        self.assertEqual(ap.strip_changed(settled, settled, (100, 300)), 0)
        leaked = self.frame((0x10, 0x10, 0x10), size=(400, 44))
        leaked.paste((0x60, 0x60, 0x60), (150, 0, 200, 20))
        self.assertGreater(ap.strip_changed(leaked, settled, (100, 300)), 0)

    def test_rect_intersection(self):
        self.assertEqual(ap.rect_intersection((0, 0, 10, 10), (5, 5, 10, 10)), (5, 5, 5, 5))
        self.assertIsNone(ap.rect_intersection((0, 0, 10, 10), (10, 0, 5, 5)))


class ColourPickerMetricTests(unittest.TestCase):
    """Negative controls for the AP04 colour picker helpers on synthetic pixel grids."""

    W = 40
    H = 30

    def canvas(self, fill=(0, 0, 0)):
        return [fill] * (self.W * self.H)

    def paint(self, px, x0, y0, w, h, rgb):
        for y in range(y0, y0 + h):
            for x in range(x0, x0 + w):
                px[y * self.W + x] = rgb

    def test_colour_blobs_finds_only_exact_matches(self):
        px = self.canvas()
        self.paint(px, 2, 2, 8, 8, (0x3D, 0xAE, 0xE9))
        self.paint(px, 20, 2, 8, 8, (0x3D, 0xAE, 0xF0))  # 7 steps off: beyond tolerance 3
        self.paint(px, 2, 20, 2, 2, (0x3D, 0xAE, 0xE9))  # too small for min_px
        blobs = ap.colour_blobs(px, self.W, (0, 0, self.W, self.H), "#3DAEE9", min_px=40)
        self.assertEqual(blobs, [(2, 2, 9, 9, 64)])

    def test_preset_rows_needs_four_circles_on_one_band(self):
        px = self.canvas()
        for index, hex_value in enumerate(ap.PICKER_VIVID_PRESETS[:3]):
            rgb = tuple(int(hex_value[i:i + 2], 16) for i in (1, 3, 5))
            self.paint(px, 2 + index * 12, 2, 10, 10, rgb)
        self.assertEqual(ap.preset_rows(px, self.W, (0, 0, self.W, self.H)), [])

    def test_dot_row_selects_the_row_whose_seeded_circle_is_marked(self):
        px = self.canvas()
        seeded = tuple(int("336699"[i:i + 2], 16) for i in (0, 2, 4))
        self.paint(px, 2, 2, 12, 12, seeded)
        self.paint(px, 4, 4, 6, 6, (0xF1, 0xF2, 0xF3))  # selected dot at the circle centre
        rows = [{"y": 8, "circles": [(8, "#336699", None)]}, {"y": 20, "circles": [(8, "#336699", None)]}]
        self.assertEqual(ap.dot_row(px, self.W, rows, "#336699"), rows[0])
        px2 = self.canvas()
        self.paint(px2, 2, 2, 12, 12, seeded)
        self.assertIsNone(ap.dot_row(px2, self.W, rows[:1], "#336699"))

    def test_snapshot_value_reads_exact_key_only(self):
        text = "{'taskbarColor': <'#3DAEE9'>, 'startMenuColor': <'#191B1D'>}"
        self.assertEqual(ap.snapshot_value(text, "taskbarColor"), "#3DAEE9")
        self.assertEqual(ap.snapshot_value(text, "taskbar"), None)
        self.assertIsNone(ap.snapshot_value(None, "taskbarColor"))

    def test_hue_bins_separates_a_wheel_from_a_flat_circle(self):
        flat = self.canvas()
        self.paint(flat, 0, 0, 10, 10, (0x3D, 0xAE, 0xE9))
        self.assertEqual(ap.hue_bins(flat, self.W, (0, 0, 10, 10)), 1)
        wheel = self.canvas()
        for index, rgb in enumerate([(255, 0, 0), (255, 255, 0), (0, 255, 0), (0, 255, 255), (0, 0, 255), (255, 0, 255)]):
            self.paint(wheel, index * 2, 0, 2, 10, rgb)
        self.assertGreaterEqual(ap.hue_bins(wheel, self.W, (0, 0, 12, 10)), 6)


class XtestDisplayGuardTests(unittest.TestCase):
    """:94 is outside flamewm_xtest.py's :97..:119 range; the old harness ran it and every input was dropped."""

    def test_xtest_range_accepts_only_97_to_119(self) -> None:
        self.assertTrue(ap.xtest_display_ok(":97"))
        self.assertTrue(ap.xtest_display_ok(":119"))
        self.assertFalse(ap.xtest_display_ok(":94"))
        self.assertFalse(ap.xtest_display_ok(":120"))
        self.assertFalse(ap.xtest_display_ok("97"))
        self.assertFalse(ap.xtest_display_ok(":9x"))

    def test_parse_args_refuses_display_outside_xtest_range(self) -> None:
        import contextlib
        import io

        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            ap.parse_args(["--run-dir", "/tmp/unused-ap-run", "--display", ":94"])
        self.assertEqual(ap.parse_args(["--run-dir", "/tmp/unused-ap-run", "--display", ":97"]).display, ":97")

    def test_refused_xtest_call_raises_instead_of_silent_pass_through(self) -> None:
        from types import SimpleNamespace
        from unittest import mock

        session = ap.Session(SimpleNamespace(display=":97"), SimpleNamespace())
        with mock.patch.object(ap.base, "send_input", return_value=(2, "shell line")):
            with self.assertRaises(RuntimeError):
                session.input(click="10,10")
        with mock.patch.object(ap.base, "send_input", return_value=(0, "ok")):
            self.assertEqual(session.input(key="Super_L"), (0, "ok"))


class AccentLineAbsenceTests(unittest.TestCase):
    """AP03 asserts the taskbar top rows carry no accent line; the negative control must flag a real line."""

    ACCENT = (0x00, 0xAA, 0x00)
    BAR = (0x33, 0x66, 0x99)

    def test_top_rows_with_accent_line_are_flagged(self) -> None:
        width = 40
        strip = [self.BAR] * (width * 3)
        for x in range(width):
            strip[1 * width + x] = self.ACCENT  # a 1px accent line in row 1
        self.assertGreaterEqual(ap.accent_match_count(strip, self.ACCENT), width)

    def test_clean_top_rows_are_not_flagged(self) -> None:
        width = 40
        strip = [self.BAR] * (width * 3)
        self.assertEqual(ap.accent_match_count(strip, self.ACCENT), 0)


class OpenFadeSamplingTests(unittest.TestCase):
    RECT_BOX = (0, 0, 1, 1)

    def series(self, values: list[float]) -> list:
        return [(self.RECT_BOX, v) for v in values]

    def test_fade_with_mid_frames_inside_10_to_90_percent_passes(self) -> None:
        self.assertEqual(ap.open_style_fails("t", "fade", self.series([0, 95, 160, 228, 228, 228])), [])

    def test_fade_plateau_only_still_fails_without_retry(self) -> None:
        self.assertEqual(len(ap.open_style_fails("t", "fade", self.series([0, 228, 228]))), 1)

    def test_none_branch_rejects_intermediate_brightness(self) -> None:
        self.assertEqual(ap.open_style_fails("t", "none", self.series([228, 228, 228])), [])
        self.assertEqual(len(ap.open_style_fails("t", "none", self.series([0, 150, 228]))), 1)

    def test_open_fade_path_calls_native_sampler_not_screenshots(self) -> None:
        import inspect

        source = inspect.getsource(ap.style_matrix_checks)
        self.assertIn("sample_flight(s, 2.0, lambda img: (bright_bbox_pil(img), rect_brightness(img, open_rect)))", source)
        self.assertNotIn("timed_samples(s, tag, 4.0, open_rect)", source)


class FlightSamplingTests(unittest.TestCase):
    OVERLAY_TREE = TREE_FIXTURE.replace(
        "     0x400030 (has no name)",
        "     0x400040 (has no name): ()  120x60+400+300  +400+300\n     0x400030 (has no name)")

    def test_overlay_nodes_picks_new_nameless_top_level_not_frame(self) -> None:
        nodes = ap.parse_tree(self.OVERLAY_TREE)
        before = {"0x400002", "0x400010", "0x400020"}
        self.assertEqual([n["id"] for n in ap.overlay_nodes(nodes, before)], ["0x400040"])

    def test_overlay_nodes_ignores_windows_present_before_and_one_pixel_helpers(self) -> None:
        nodes = ap.parse_tree(self.OVERLAY_TREE)
        self.assertEqual(ap.overlay_nodes(nodes, {"0x400040"}), [])
        self.assertEqual(ap.overlay_nodes(ap.parse_tree(TREE_FIXTURE), set()), [])

    def test_flight_finished_needs_an_earlier_sighting_then_absence(self) -> None:
        self.assertFalse(ap.flight_finished(set(), set()))
        self.assertFalse(ap.flight_finished(set(), {"0x1"}))
        self.assertFalse(ap.flight_finished({"0x1"}, {"0x1"}))
        self.assertTrue(ap.flight_finished({"0x1"}, set()))

    def test_leftover_clients_flags_xclock_and_nameless_overlay_only(self) -> None:
        clean = "\n".join(line for line in TREE_FIXTURE.splitlines()
                          if "0x400010" not in line and "0x400011" not in line)
        self.assertEqual(ap.leftover_clients(ap.parse_tree(clean)), [])
        self.assertEqual([n["id"] for n in ap.leftover_clients(ap.parse_tree(TREE_FIXTURE))],
                         ["0x400010", "0x400011"])
        leftovers = ap.leftover_clients(ap.parse_tree(self.OVERLAY_TREE))
        self.assertEqual([n["id"] for n in leftovers], ["0x400010", "0x400011", "0x400040"])
        self.assertEqual([n["id"] for n in ap.leftover_clients(ap.parse_tree(clean.replace(
            '0x400020 "FlameWM start"', '0x400020 "xclock"')))], ["0x400020"])


class WaitUntilEmptyTests(unittest.TestCase):
    class FakeClock:
        def __init__(self) -> None:
            self.t = 0.0

        def now(self) -> float:
            return self.t

        def sleep(self, seconds: float) -> None:
            self.t += seconds

    def test_clean_sample_returns_without_sleeping(self) -> None:
        clock = self.FakeClock()
        self.assertEqual(ap.wait_until_empty(lambda: [], 3.0, clock.now, clock.sleep), [])
        self.assertEqual(clock.t, 0.0)

    def test_leftover_that_unmaps_is_waited_for(self) -> None:
        clock = self.FakeClock()
        samples = iter([["xclock"], ["xclock"], []])
        self.assertEqual(ap.wait_until_empty(lambda: next(samples), 3.0, clock.now, clock.sleep), [])
        self.assertAlmostEqual(clock.t, 0.2)

    def test_persistent_leftover_is_returned_after_the_bounded_timeout(self) -> None:
        clock = self.FakeClock()
        result = ap.wait_until_empty(lambda: ["xclock"], 3.0, clock.now, clock.sleep)
        self.assertEqual(result, ["xclock"])
        self.assertGreaterEqual(clock.t, 3.0)
        self.assertLess(clock.t, 3.2)


class StopCaseClientsTests(unittest.TestCase):
    def test_case_cleanup_terminates_the_started_client_and_clears_the_handle(self) -> None:
        import os
        with tempfile.TemporaryDirectory() as tmp:
            run = Path(tmp)
            (run / "logs").mkdir()
            ctx = ap.base.Ctx(run)
            session = ap.Session(ctx, None)
            session.tree = lambda label: []  # no X server: the window is already gone from the tree
            session.client = ctx.spawn("client-xclock", [sys.executable, "-c", "import time; time.sleep(60)"],
                                       dict(os.environ))
            proc = session.client.proc
            try:
                self.assertIsNone(proc.poll())
                ap.stop_case_clients(session)
                self.assertIsNotNone(proc.poll(), "case cleanup left the started client running")
                self.assertIsNone(session.client)
            finally:
                if proc.poll() is None:
                    proc.kill()
                    proc.wait(timeout=5)


if __name__ == "__main__":
    unittest.main()

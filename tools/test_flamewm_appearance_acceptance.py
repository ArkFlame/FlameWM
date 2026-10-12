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


if __name__ == "__main__":
    unittest.main()

from __future__ import annotations

import math
import sys
import tempfile
import unittest
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))

import flamewm_lock_acceptance as lk  # noqa: E402


def node(wid: str, x: int, y: int, w: int, h: int, parent: str | None = None, name: str = "") -> dict:
    return {"id": wid, "name": name, "parent": parent, "x": x, "y": y, "w": w, "h": h}


class CoversScreenTests(unittest.TestCase):
    def test_full_screen_at_origin_covers(self):
        self.assertTrue(lk.covers_screen(node("0x1", 0, 0, 1280, 800), 1280, 800))

    def test_offset_or_short_window_does_not_cover(self):
        self.assertFalse(lk.covers_screen(node("0x1", 1, 0, 1280, 800), 1280, 800))
        self.assertFalse(lk.covers_screen(node("0x1", 0, 0, 1280, 799), 1280, 800))

    def test_lock_window_ignores_partial_and_nested_windows(self):
        nodes = [node("0x1", 0, 0, 640, 400, parent=None),
                 node("0x2", 0, 0, 1280, 800, parent="0x9"),
                 node("0x3", 0, 0, 1280, 800, parent=None)]
        self.assertEqual(lk.lock_window(nodes, 1280, 800)["id"], "0x3")
        self.assertIsNone(lk.lock_window(nodes[:1], 1280, 800))


class RespawnTests(unittest.TestCase):
    def test_respawn_is_identified_by_pid_not_window_id(self):
        # X reuses window ids: the respawned lock can map under the killed lock's id.
        self.assertEqual(lk.respawned_pids([4001, 4002], 4001), [4002])
        self.assertEqual(lk.respawned_pids([4001], 4001), [])

    def test_lk10_to_lk13_are_registered_in_order(self):
        self.assertEqual(lk.IDS[-4:], ("LK10", "LK11", "LK12", "LK13"))
        self.assertEqual(len(lk.SCENARIOS), len(lk.IDS))
        self.assertIn("LK13", lk.TITLES)
        self.assertTrue(lk.FAKE_CONTROL_BIN.is_file())


class TopmostTests(unittest.TestCase):
    def test_first_root_child_is_topmost(self):
        # xwininfo -tree prints the root's children topmost first (checked natively by raising a window).
        nodes = [node("0xc", 0, 0, 5, 5), node("0xb", 0, 0, 10, 10, parent="0xc"), node("0xa", 0, 0, 10, 10)]
        self.assertTrue(lk.is_topmost(nodes, "0xc"))
        self.assertFalse(lk.is_topmost(nodes, "0xa"))

    def test_empty_tree_is_not_topmost(self):
        self.assertFalse(lk.is_topmost([], "0xa"))


class SignatureTests(unittest.TestCase):
    def test_signature_detects_wm_window_change_and_excludes_lock(self):
        base_nodes = [node("0xl", 0, 0, 1280, 800), node("0xw", 10, 10, 300, 200, name="xclock")]
        before = lk.tree_signature(base_nodes, {"0xl"})
        moved = [node("0xl", 0, 0, 1280, 800), node("0xw", 20, 10, 300, 200, name="xclock")]
        self.assertNotEqual(before, lk.tree_signature(moved, {"0xl"}))
        lock_changed = [node("0xl", 0, 0, 1280, 700), node("0xw", 10, 10, 300, 200, name="xclock")]
        self.assertEqual(before, lk.tree_signature(lock_changed, {"0xl"}))


class TextAndColourTests(unittest.TestCase):
    def test_failure_word_required(self):
        self.assertTrue(lk.failure_text_seen([("incorrect", 1, 2)]))
        self.assertFalse(lk.failure_text_seen([("password", 1, 2)]))
        self.assertFalse(lk.failure_text_seen([]))

    def test_background_must_be_breeze_dark(self):
        self.assertTrue(lk.background_ok([(0x23, 0x26, 0x29)] * 50))
        self.assertFalse(lk.background_ok([(0x00, 0x00, 0x00)] * 50))
        self.assertFalse(lk.background_ok([]))


class CustomisationTests(unittest.TestCase):
    def test_colour_must_dominate_outside_strip(self):
        blue = (0x33, 0x66, 0x99)
        self.assertTrue(lk.colour_dominates([blue] * 100, blue, 3))
        self.assertTrue(lk.colour_dominates([(0x36, 0x63, 0x9A)] * 100, blue, 3))
        self.assertFalse(lk.colour_dominates([(0x23, 0x26, 0x29)] * 100, blue, 3))
        # Majority right but a quarter of the strip wrong: the 95% floor rejects it.
        self.assertFalse(lk.colour_dominates([blue] * 75 + [(0, 0, 0)] * 25, blue, 3))

    def test_row_runs_group_consecutive_rows(self):
        self.assertEqual(lk.row_runs([5, 6, 7, 20, 21, 40]), [(5, 7), (20, 21), (40, 40)])
        self.assertEqual(lk.row_runs([]), [])

    def test_rows_with_finds_bright_text_rows(self):
        bright, dark = lk.TEXT_BRIGHT, (0x2A, 0x2E, 0x32)
        # 3 wide, 4 rows: only row 2 holds a bright pixel.
        pixels = [dark] * 6 + [dark, bright, dark] + [dark] * 3
        self.assertEqual(lk.rows_with(pixels, 3, lk.TEXT_BRIGHT, lk.TEXT_TOLERANCE), [2])

    def test_date_words_cover_weekday_month_and_year(self):
        words = lk.date_words(lk.datetime(2026, 10, 12))
        self.assertEqual(words, {"monday", "monday,", "october", "2026"})

    def test_pattern_quadrants_tile_the_generated_image(self):
        boxes = [box for box, _ in lk.PATTERN_QUADRANTS.values()]
        self.assertEqual(sorted(boxes), sorted([(0, 0, 160, 100), (160, 0, 320, 100),
                                                (0, 100, 160, 200), (160, 100, 320, 200)]))


class DocumentTests(unittest.TestCase):
    def test_lk06_not_run_blocks_all_pass(self):
        results = {sid: lk.entry(sid, lk.PASS, "") for sid in lk.IDS if sid != "LK06"}
        results["LK06"] = lk.entry("LK06", lk.NOT_RUN, "needs real credentials")
        doc = lk.compose_document(results)
        self.assertEqual(doc["verdict"], "INCOMPLETE")
        self.assertEqual(doc["exit_code"], 0)
        lk06_entry = next(e for e in doc["results"] if e["id"] == "LK06")
        self.assertEqual(lk06_entry["reason"], "needs real credentials")

    def test_fail_dominates(self):
        results = {"LK01": lk.entry("LK01", lk.FAIL, "x")}
        self.assertEqual(lk.compose_document(results)["exit_code"], 1)

    def test_missing_binary_is_environment_blocked_not_pass(self):
        with tempfile.TemporaryDirectory() as tmp:
            run = Path(tmp) / "run"
            code = lk.main(["--run-dir", str(run), "--lock-bin", str(Path(tmp) / "absent-lock")])
            self.assertEqual(code, 3)
            text = (run / "result.json").read_text(encoding="utf-8")
            self.assertIn('"verdict": "ENVIRONMENT_BLOCKED"', text)
            self.assertNotIn('"status": "PASS"', text)


def draw_reference_lock(width=701, height=438, clock_dy=0, skip=()):
    """Synthetic 701x438 unlocked lock: light field, white avatar ring, clock, four white power icons."""
    background, white, light = (0x23, 0x26, 0x29), (255, 255, 255), lk.LIGHT_FIELD
    pixels = [background] * (width * height)

    def put(x, y, colour):
        if 0 <= x < width and 0 <= y < height:
            pixels[y * width + x] = colour

    for y in range(253, 270):
        for x in range(271, 409):
            put(x, y, light)
        for x in range(413, 430):  # submit chevron box, light fill, 4 px right of the field
            put(x, y, light)
    for y in range(255, 268):  # caret: a narrow dark gap inside the field
        for x in range(300, 302):
            put(x, y, background)
    for y in range(height):
        for x in range(width):
            if 32 <= math.hypot(x + 0.5 - 350, y + 0.5 - 179) <= 34:
                put(x, y, white)
    for y in range(46 + clock_dy, 76 + clock_dy):
        for x in range(290, 410):
            put(x, y, white)
    for name, cx in zip(("power_sleep", "power_restart", "power_shutdown", "power_other"),
                        (245, 315, 386, 455)):
        if name in skip:
            continue
        for y in range(int(348 - 12), int(348 + 12)):
            for x in range(int(cx - 12), int(cx + 12)):
                if math.hypot(x + 0.5 - cx, y + 0.5 - 348) <= 11.5:
                    put(x, y, white)
    return pixels, width, height


class ReferenceLayoutTests(unittest.TestCase):
    def test_anchors_map_the_reference_canvas_identity_at_701x438(self):
        anchors = lk.reference_anchors(701, 438)
        self.assertEqual(anchors["password"], (271, 253, 138, 17))
        self.assertEqual(anchors["submit"], (413, 253, 17, 17))
        self.assertEqual(anchors["avatar"], (316, 145, 68, 68))
        self.assertEqual(anchors["power_shutdown"], (374.5, 336.5, 23, 23))

    def test_anchors_scale_and_centre_on_a_larger_output(self):
        s = min(1280 / 701, 800 / 438, 2.5)
        anchors = lk.reference_anchors(1280, 800)
        cx, cy = lk.anchor_centre(anchors["avatar"])
        ox, oy = (1280 - 701 * s) / 2, (800 - 438 * s) / 2
        self.assertAlmostEqual(cx, ox + 350 * s, places=6)
        self.assertAlmostEqual(cy, oy + 179 * s, places=6)
        self.assertAlmostEqual(lk.anchor_tolerance(1280, 800), 3 * s, places=9)

    def test_anchor_error_is_centre_distance(self):
        self.assertEqual(lk.anchor_error((10, 10, 20, 20), (10, 10, 20, 20)), 0)
        self.assertEqual(lk.anchor_error((25, 10, 20, 20), (10, 10, 20, 20)), 15)

    def test_negative_control_clock_offset_exceeds_tolerance(self):
        # +15 px on the clock anchor must be rejected at every scale the harness runs at.
        for w, h in ((701, 438), (1280, 800), (1920, 1080)):
            expected = lk.reference_anchors(w, h)["clock"]
            shifted = (expected[0], expected[1] + 15 * lk.reference_fit(w, h)[0], expected[2], expected[3])
            self.assertGreater(lk.anchor_error(shifted, expected), lk.anchor_tolerance(w, h))

    def test_synthetic_reference_lock_passes_and_clock_offset_fails(self):
        pixels, w, h = draw_reference_lock()
        ok, lines = lk.judge_reference_layout(lk.measure_reference_layout(pixels, w, h), w, h)
        self.assertTrue(ok, lines)
        pixels, w, h = draw_reference_lock(clock_dy=15)
        ok, lines = lk.judge_reference_layout(lk.measure_reference_layout(pixels, w, h), w, h)
        self.assertFalse(ok)
        self.assertTrue(any(line.startswith("clock:") and line.endswith("OUT") for line in lines), lines)

    def test_password_run_excludes_the_submit_box_and_spans_the_caret(self):
        pixels, w, h = draw_reference_lock()
        measured = lk.measure_reference_layout(pixels, w, h)["password"]
        self.assertIsNotNone(measured)
        self.assertLessEqual(abs(measured[0] - 271), 1)
        self.assertLessEqual(abs(measured[2] - 138), 1)

    def test_missing_power_icon_blob_fails(self):
        pixels, w, h = draw_reference_lock(skip=("power_restart",))
        measured = lk.measure_reference_layout(pixels, w, h)
        self.assertIsNone(measured["power_restart"])
        ok, lines = lk.judge_reference_layout(measured, w, h)
        self.assertFalse(ok)
        self.assertTrue(any(line.startswith("power_restart: no blob") for line in lines), lines)

    def test_coloured_avatar_disc_is_not_the_white_ring(self):
        pixels, w, h = draw_reference_lock()
        red = (0xEF, 0x44, 0x44)
        cx, cy = 350, 179
        for y in range(h):
            for x in range(w):
                if math.hypot(x + 0.5 - cx, y + 0.5 - cy) <= 34:
                    pixels[y * w + x] = red
        self.assertIsNone(lk.measure_avatar(pixels, w, h, lk.reference_anchors(w, h)["avatar"]))

    def test_dimmed_icon_has_lower_peak_brightness(self):
        pixels, w, h = draw_reference_lock()
        rect = lk.reference_anchors(w, h)["power_restart"]
        enabled = lk.icon_peak(pixels, w, h, rect)
        dimmed = [tuple(c // 3 for c in v) for v in pixels]
        self.assertLessEqual(lk.icon_peak(dimmed, w, h, rect) / enabled, lk.ICON_DIM_RATIO)


if __name__ == "__main__":
    unittest.main()

from __future__ import annotations

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

    def test_lk10_and_lk11_are_registered_after_lk09(self):
        self.assertEqual(lk.IDS[-2:], ("LK10", "LK11"))
        self.assertEqual(len(lk.SCENARIOS), len(lk.IDS))
        self.assertIn("LK11", lk.TITLES)


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

    def test_clock_and_username_recognised_after_layout_shift(self):
        # Default card: clock (tall), date (grey, not bright), username (short) below it.
        self.assertEqual(lk.clock_and_username([(53, 133), (186, 208)]), (True, True))
        # Date and username hidden: the clock moves down but is still the only text run.
        self.assertEqual(lk.clock_and_username([(99, 179)]), (True, False))
        self.assertEqual(lk.clock_and_username([]), (False, False))

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


if __name__ == "__main__":
    unittest.main()

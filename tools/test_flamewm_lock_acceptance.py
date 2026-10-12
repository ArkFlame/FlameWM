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


class DocumentTests(unittest.TestCase):
    def test_lk06_not_run_blocks_all_pass(self):
        results = {sid: lk.entry(sid, lk.PASS, "") for sid in lk.IDS if sid != "LK06"}
        results["LK06"] = lk.entry("LK06", lk.NOT_RUN, "needs real credentials")
        doc = lk.compose_document(results)
        self.assertEqual(doc["verdict"], "INCOMPLETE")
        self.assertEqual(doc["exit_code"], 0)
        self.assertEqual(doc["results"][-1]["reason"], "needs real credentials")

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

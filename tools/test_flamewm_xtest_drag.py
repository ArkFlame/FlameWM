"""Pure unit tests for flamewm_xtest --drag parsing, sequencing and the owned-display guard. No X server."""

from __future__ import annotations

import contextlib
import io
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))

import flamewm_xtest as xt  # noqa: E402


class RecordingBackend:
    def __init__(self, reject_at: int | None = None) -> None:
        self.events: list[tuple] = []
        self._reject_at = reject_at

    def _record(self, event: tuple) -> bool:
        index = len(self.events)
        self.events.append(event)
        return index != self._reject_at

    def fake_motion(self, x: int, y: int) -> bool:
        return self._record(("move", x, y))

    def fake_button(self, button: int, down: bool) -> bool:
        return self._record(("button", button, down))


class ParseDragTests(unittest.TestCase):
    def test_defaults(self) -> None:
        self.assertEqual(xt.parse_drag("10,20,300,40"), ((10, 20), (300, 40), 10, 300))

    def test_steps_and_hold(self) -> None:
        self.assertEqual(xt.parse_drag("1,2,3,4:5"), ((1, 2), (3, 4), 5, 300))
        self.assertEqual(xt.parse_drag("1,2,3,4:5:0"), ((1, 2), (3, 4), 5, 0))

    def test_rejects_malformed(self) -> None:
        for text in ["", "1,2,3", "1,2,3,4:0", "1,2,3,4:201", "1,2,3,4:5:60001", "1,2,3,4:5:6:7",
                     "-1,2,3,4", "a,b,c,d", "1,2,3,4:x", "1,2,3,4:"]:
            with self.subTest(text=text), self.assertRaises(ValueError):
                xt.parse_drag(text)


class DragEventsTests(unittest.TestCase):
    def test_press_equal_steps_hold_release(self) -> None:
        backend, sleeps = RecordingBackend(), []
        xt.drag_events(backend, (0, 0), (100, 40), 4, 500, sleep=sleeps.append)
        self.assertEqual(backend.events, [
            ("move", 0, 0), ("button", 1, True),
            ("move", 25, 10), ("move", 50, 20), ("move", 75, 30), ("move", 100, 40),
            ("button", 1, False)])
        self.assertEqual(sleeps[-1], 0.5)

    def test_keep_leaves_button_down(self) -> None:
        backend = RecordingBackend()
        xt.drag_events(backend, (5, 5), (9, 5), 1, 0, release=False, sleep=lambda _: None)
        self.assertEqual(backend.events[-1], ("move", 9, 5))
        self.assertNotIn(("button", 1, False), backend.events)

    def test_rejected_start_sends_no_press(self) -> None:
        backend = RecordingBackend(reject_at=0)
        with self.assertRaises(RuntimeError):
            xt.drag_events(backend, (0, 0), (9, 9), 2, 0, sleep=lambda _: None)
        self.assertEqual(backend.events, [("move", 0, 0)])

    def test_rejected_motion_still_releases(self) -> None:
        backend = RecordingBackend(reject_at=3)
        with self.assertRaises(RuntimeError):
            xt.drag_events(backend, (0, 0), (9, 9), 3, 0, sleep=lambda _: None)
        self.assertEqual(backend.events[-1], ("button", 1, False))


class OwnedDisplayGuardTests(unittest.TestCase):
    def run_main(self, argv: list[str]) -> tuple[int, str]:
        err = io.StringIO()
        with mock.patch.object(sys, "argv", ["flamewm_xtest.py", *argv]), contextlib.redirect_stderr(err), \
                contextlib.redirect_stdout(io.StringIO()):
            try:
                return xt.main(), err.getvalue()
            except SystemExit as exit_:
                return int(exit_.code), err.getvalue()

    def test_drag_refused_for_non_nested_display(self) -> None:
        code, _ = self.run_main(["--display", ":0", "--owner-token", "a" * 64, "--owner-marker", "/nonexistent",
                                 "--drag", "1,1,2,2"])
        self.assertEqual(code, 2)

    def test_drag_refused_on_marker_mismatch(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            marker = Path(tmp) / "marker"
            marker.write_text(":97\n" + "b" * 64 + "\n", encoding="ascii")
            code, err = self.run_main(["--display", ":97", "--owner-token", "a" * 64, "--owner-marker", str(marker),
                                       "--drag", "1,1,2,2"])
        self.assertEqual(code, 3)
        self.assertIn("INPUT_BLOCKED", err)

    def test_keep_requires_drag(self) -> None:
        code, _ = self.run_main(["--display", ":97", "--owner-token", "a" * 64, "--owner-marker", "/x",
                                 "--click", "1,1", "--drag-keep"])
        self.assertEqual(code, 2)


if __name__ == "__main__":
    unittest.main()

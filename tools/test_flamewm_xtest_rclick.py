"""Pure unit tests for flamewm_xtest --rclick sequencing and parsing. No X server is required."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))

import flamewm_xtest as xt  # noqa: E402

TOKEN = "a" * 64


class RecordingBackend:
    def __init__(self, reject: str | None = None) -> None:
        self.events: list[tuple] = []
        self._reject = reject

    def fake_motion(self, x: int, y: int) -> bool:
        self.events.append(("motion", x, y))
        return self._reject != "motion"

    def fake_button(self, button: int, down: bool) -> bool:
        self.events.append(("button", button, down))
        return self._reject != ("press" if down else "release")


class RClickEventsTests(unittest.TestCase):
    def test_motion_then_button3_press_and_release(self) -> None:
        backend = RecordingBackend()
        xt.rclick_events(backend, 120, 340)
        self.assertEqual(backend.events, [("motion", 120, 340), ("button", 3, True), ("button", 3, False)])

    def test_rejected_motion_sends_no_button(self) -> None:
        backend = RecordingBackend(reject="motion")
        with self.assertRaisesRegex(RuntimeError, "motion rejected"):
            xt.rclick_events(backend, 1, 2)
        self.assertEqual(backend.events, [("motion", 1, 2)])

    def test_rejected_press_sends_no_release(self) -> None:
        backend = RecordingBackend(reject="press")
        with self.assertRaisesRegex(RuntimeError, "press rejected"):
            xt.rclick_events(backend, 1, 2)
        self.assertNotIn(("button", 3, False), backend.events)

    def test_rejected_release_is_reported(self) -> None:
        backend = RecordingBackend(reject="release")
        with self.assertRaisesRegex(RuntimeError, "release rejected"):
            xt.rclick_events(backend, 1, 2)


class ParseRClickArgsTests(unittest.TestCase):
    def parse(self, *extra: str):
        argv = ["flamewm_xtest.py", "--display", ":97", "--owner-token", TOKEN,
                "--owner-marker", "/tmp/marker", *extra]
        with mock.patch.object(sys, "argv", argv):
            return xt.parse_args()

    def test_accepts_rclick_as_input_action(self) -> None:
        args = self.parse("--rclick", "10,20")
        self.assertEqual(args.rclick_xy, (10, 20))

    def test_rejects_malformed_rclick(self) -> None:
        for text in ["10", "10;20", "-1,2", "a,b", "1,2,3"]:
            with self.subTest(text=text), mock.patch("sys.stderr"), self.assertRaises(SystemExit):
                self.parse("--rclick", text)

    def test_rclick_cannot_run_alone_without_input_action(self) -> None:
        with mock.patch("sys.stderr"), self.assertRaises(SystemExit):
            self.parse("--display", ":97")

    def test_display_range_still_enforced(self) -> None:
        with mock.patch("sys.stderr"), self.assertRaises(SystemExit):
            self.parse_with_display(":96", "--rclick", "1,2")

    def parse_with_display(self, display: str, *extra: str):
        argv = ["flamewm_xtest.py", "--display", display, "--owner-token", TOKEN,
                "--owner-marker", "/tmp/marker", *extra]
        with mock.patch.object(sys, "argv", argv):
            return xt.parse_args()


if __name__ == "__main__":
    unittest.main()

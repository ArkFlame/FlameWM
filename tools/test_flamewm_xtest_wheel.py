"""Pure unit tests for flamewm_xtest --wheel parsing and sequencing. No X server is required."""

from __future__ import annotations

import contextlib
import io
import sys
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))

import flamewm_xtest as xt  # noqa: E402


class RecordingBackend:
    def __init__(self, reject_at: int | None = None) -> None:
        self.events: list[tuple[int, bool]] = []
        self._reject_at = reject_at

    def fake_button(self, button: int, down: bool) -> bool:
        index = len(self.events)
        self.events.append((button, down))
        return index != self._reject_at


class RecordingXtst:
    def __init__(self) -> None:
        self.calls: list[tuple] = []

    def XTestFakeButtonEvent(self, display, button, down, delay):
        self.calls.append((display, button, down, delay))
        return 1


class ParseWheelTests(unittest.TestCase):
    def test_up_and_down_default_to_one_step(self) -> None:
        self.assertEqual(xt.parse_wheel("up"), (4, 1))
        self.assertEqual(xt.parse_wheel("down"), (5, 1))

    def test_explicit_step_count(self) -> None:
        self.assertEqual(xt.parse_wheel("up:3"), (4, 3))
        self.assertEqual(xt.parse_wheel("down:2"), (5, 2))

    def test_upper_bound_accepted(self) -> None:
        self.assertEqual(xt.parse_wheel(f"down:{xt.MAX_WHEEL_STEPS}"), (5, xt.MAX_WHEEL_STEPS))

    def test_rejects_unknown_direction_and_case(self) -> None:
        for text in ["left", "right", "UP", "Down", "", ":3", "4"]:
            with self.subTest(text=text), self.assertRaises(ValueError):
                xt.parse_wheel(text)

    def test_rejects_bad_step_counts(self) -> None:
        for text in ["up:", "up:0", "up:-1", "up:+2", "up:abc", "up:1.5", f"up:{xt.MAX_WHEEL_STEPS + 1}", "up: 2"]:
            with self.subTest(text=text), self.assertRaises(ValueError):
                xt.parse_wheel(text)


class WheelClicksTests(unittest.TestCase):
    def test_each_step_is_press_then_release_on_the_given_button(self) -> None:
        backend = RecordingBackend()
        xt.wheel_clicks(backend, 4, 3)
        self.assertEqual(backend.events, [(4, True), (4, False)] * 3)

    def test_single_step_sends_one_pair(self) -> None:
        backend = RecordingBackend()
        xt.wheel_clicks(backend, 5, 1)
        self.assertEqual(backend.events, [(5, True), (5, False)])

    def test_rejected_press_stops_without_further_events(self) -> None:
        backend = RecordingBackend(reject_at=2)
        with self.assertRaises(RuntimeError):
            xt.wheel_clicks(backend, 4, 3)
        self.assertEqual(backend.events, [(4, True), (4, False), (4, True)])

    def test_x11_backend_routes_to_xtest_button_event(self) -> None:
        xtst = RecordingXtst()
        backend = xt.X11Backend(None, xtst, "display-handle")
        xt.wheel_clicks(backend, 5, 2)
        self.assertEqual(
            xtst.calls,
            [("display-handle", 5, 1, 0), ("display-handle", 5, 0, 0)] * 2,
        )


class ParseArgsWheelTests(unittest.TestCase):
    BASE = ["--display", ":97", "--owner-token", "a" * 64, "--owner-marker", "/nonexistent/marker"]

    def _parse(self, *extra: str):
        with mock.patch.object(sys, "argv", ["flamewm_xtest.py", *self.BASE, *extra]):
            return xt.parse_args()

    def test_wheel_alone_satisfies_request_requirement(self) -> None:
        args = self._parse("--wheel", "up:3")
        self.assertEqual(args.wheel_spec, (4, 3))

    def test_wheel_default_count_from_cli(self) -> None:
        args = self._parse("--wheel", "down")
        self.assertEqual(args.wheel_spec, (5, 1))

    def test_invalid_wheel_is_a_parser_error(self) -> None:
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as caught:
            self._parse("--wheel", "left:2")
        self.assertEqual(caught.exception.code, 2)


if __name__ == "__main__":
    unittest.main()

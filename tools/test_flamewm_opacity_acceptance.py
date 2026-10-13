#!/usr/bin/env python3
"""Unit tests (no X) for tools/flamewm_opacity_acceptance.py, including negative controls."""

from __future__ import annotations

import pathlib
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import flamewm_opacity_acceptance as acc  # noqa: E402

SOLID = (25, 27, 29)
BACKDROP = (120, 150, 200)


def ppm(width: int, height: int, rgb: tuple[int, int, int]) -> bytes:
    return b"P6\n%d %d\n255\n" % (width, height) + bytes(rgb) * (width * height)


class ParsePpmTests(unittest.TestCase):
    def test_reads_size_and_pixels(self) -> None:
        width, height, pixels = acc.parse_ppm(ppm(3, 2, BACKDROP))
        self.assertEqual((width, height), (3, 2))
        self.assertEqual(acc.pixel((width, height, pixels), 2, 1), BACKDROP)

    def test_rejects_short_and_non_p6_data(self) -> None:
        with self.assertRaises(ValueError):
            acc.parse_ppm(b"P5\n1 1\n255\n\x00")
        with self.assertRaises(ValueError):
            acc.parse_ppm(ppm(2, 2, SOLID)[:-1])

    def test_out_of_range_sample_is_an_error(self) -> None:
        image = acc.parse_ppm(ppm(2, 2, SOLID))
        with self.assertRaises(ValueError):
            acc.pixel(image, 2, 0)


class ExpectedBarTests(unittest.TestCase):
    def test_opacity_100_is_the_solid_taskbar_colour(self) -> None:
        self.assertEqual(acc.expected_bar_pixel(SOLID, BACKDROP, 100), SOLID)

    def test_opacity_0_is_the_backdrop(self) -> None:
        self.assertEqual(acc.expected_bar_pixel(SOLID, BACKDROP, 0), BACKDROP)

    def test_opacity_50_is_the_midpoint(self) -> None:
        mid = acc.expected_bar_pixel(SOLID, BACKDROP, 50)
        for got, fill, under in zip(mid, SOLID, BACKDROP):
            self.assertAlmostEqual(got, (fill + under) / 2, delta=1)


class CheckBarTests(unittest.TestCase):
    def test_backdrop_visible_at_opacity_0_passes(self) -> None:
        ok, _ = acc.check_bar((121, 149, 201), SOLID, BACKDROP, 0)
        self.assertTrue(ok)

    def test_negative_control_solid_bar_fails_opacity_0(self) -> None:
        # The reported defect: the bar still paints the solid taskbar colour at opacity 0.
        ok, _ = acc.check_bar(SOLID, SOLID, BACKDROP, 0)
        self.assertFalse(ok)

    def test_negative_control_backdrop_fails_opacity_100(self) -> None:
        ok, _ = acc.check_bar(BACKDROP, SOLID, BACKDROP, 100)
        self.assertFalse(ok)

    def test_midpoint_passes_within_eight(self) -> None:
        mid = acc.expected_bar_pixel(SOLID, BACKDROP, 50)
        ok, _ = acc.check_bar(tuple(c + 8 for c in mid), SOLID, BACKDROP, 50)
        self.assertTrue(ok)

    def test_negative_control_midpoint_nine_off_fails(self) -> None:
        mid = acc.expected_bar_pixel(SOLID, BACKDROP, 50)
        ok, _ = acc.check_bar(tuple(c + 9 for c in mid), SOLID, BACKDROP, 50)
        self.assertFalse(ok)

    def test_blur_is_not_checked_as_solid(self) -> None:
        self.assertFalse(acc.start_is_solid((209, 160, 176), SOLID))
        self.assertTrue(acc.start_is_solid(SOLID, SOLID))


class SnapshotReferenceTests(unittest.TestCase):
    def test_reads_last_logged_centre(self) -> None:
        import tempfile

        with tempfile.NamedTemporaryFile("w", suffix=".log", delete=False, encoding="utf-8") as handle:
            handle.write("debug backdrop capture ... centre_rgba=Some((1, 2, 3))\n")
            handle.write("debug backdrop capture ... centre_rgba=Some((39, 40, 47))\n")
            path = handle.name
        self.assertEqual(acc.snapshot_centre(path), (39, 40, 47))

    def test_negative_control_log_without_capture_is_an_error(self) -> None:
        import tempfile

        with tempfile.NamedTemporaryFile("w", suffix=".log", delete=False, encoding="utf-8") as handle:
            handle.write("debug backdrop paint no-backdrop\n")
            path = handle.name
        with self.assertRaises(ValueError):
            acc.snapshot_centre(path)


class ArgumentTests(unittest.TestCase):
    def test_opacity_outside_range_is_rejected(self) -> None:
        with self.assertRaises(SystemExit):
            acc.parse_args(["--display", ":128", "--taskbar-opacity", "101", "--bar-rect", "0,597,1350,44"])

    def test_display_pattern_accepts_nested_display_numbers(self) -> None:
        self.assertIsNotNone(acc.DISPLAY_RE.match(":128"))
        self.assertIsNone(acc.DISPLAY_RE.match("128; rm -rf"))


if __name__ == "__main__":
    unittest.main()

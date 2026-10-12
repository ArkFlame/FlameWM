"""Pure unit tests for the Magic Lamp bend acceptance metric. No X server or display is touched.

Run with:
    python3 -I tools/test_flamewm_lamp_acceptance.py
"""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))

import flamewm_lamp_acceptance as lamp  # noqa: E402
from PIL import Image  # noqa: E402

REST_W, REST_H = 300, 200
REST_BBOX = (400, 300, 699, 499)
ICON = (640.0, 780.0)
ROWS = 200


def frame(cx: float, cy: float, widths: list[int]) -> tuple[tuple[int, int, int, int], list[int]]:
    """A synthetic mid frame: rows of the given widths centred on (cx, cy)."""
    n, top_w = len(widths), max(widths)
    x0 = int(cx - top_w // 2)
    y0 = int(cy - n // 2)
    return (x0, y0, x0 + top_w - 1, y0 + n - 1), widths


def bent(strength: float, rows: int = ROWS, width: int = REST_W) -> list[int]:
    """Cubic lateral contraction: lower (dock-side) rows narrow faster; strength 0 is the identity."""
    return [round(width * (1 - strength * (i / (rows - 1)) ** 3)) for i in range(rows)]


def bent_series() -> list[tuple[tuple[int, int, int, int] | None, list[int]]]:
    return [frame(570 + k * 40, 420 + k * 120, bent(s)) for k, s in enumerate((0.2, 0.5, 0.7))]


class BendMetricTests(unittest.TestCase):
    def test_bent_cubic_passes(self) -> None:
        ok, why, detail = lamp.bend_verdict(REST_BBOX, bent_series(), ICON)
        self.assertTrue(ok, why)
        self.assertEqual(detail["bent_frames"], 3)
        self.assertGreaterEqual(detail["approach_steps"], 2)

    def test_rigid_scale_fails(self) -> None:
        # every row has the same width: the thirds never differ, so nothing bends
        series = [frame(570 + k * 40, 420 + k * 120, [REST_W - 40 * k] * ROWS) for k in range(3)]
        ok, why, _ = lamp.bend_verdict(REST_BBOX, series, ICON)
        self.assertFalse(ok)
        self.assertTrue(any("bend" in reason for reason in why), why)

    def test_linear_trapezoid_fails(self) -> None:
        # a linear taper narrows the thirds by a lot and is monotone, but it is a trapezoid, not a bend
        series = [frame(570 + k * 40, 420 + k * 120,
                        [round(REST_W - (REST_W * 0.5 * k / 3) * i / (ROWS - 1)) for i in range(ROWS)]) for k in range(3)]
        ok, why, _ = lamp.bend_verdict(REST_BBOX, series, ICON)
        self.assertFalse(ok, why)

    def test_empty_overlay_fails(self) -> None:
        ok, why, _ = lamp.bend_verdict(REST_BBOX, [(None, [])] * 6, ICON)
        self.assertFalse(ok)
        self.assertIn("fewer than 2 mid-flight frames", why[0])

    def test_static_frame_fails(self) -> None:
        static = [(REST_BBOX, [REST_W] * REST_H)] * 5
        ok, why, _ = lamp.bend_verdict(REST_BBOX, static, ICON)
        self.assertFalse(ok)
        self.assertIn("static or empty overlay", why[0])

    def test_no_approach_to_icon_fails(self) -> None:
        # bent rows, but the bbox drifts away from the taskbar icon
        series = [frame(570 - k * 40, 420 - k * 60, bent(s)) for k, s in enumerate((0.2, 0.5, 0.7))]
        ok, why, _ = lamp.bend_verdict(REST_BBOX, series, ICON)
        self.assertFalse(ok)
        self.assertTrue(any("approached the icon" in reason for reason in why), why)

    def test_single_bent_frame_fails(self) -> None:
        series = [frame(570, 420, bent(0.2)), frame(610, 540, [REST_W] * ROWS), frame(650, 660, [REST_W] * ROWS)]
        ok, why, _ = lamp.bend_verdict(REST_BBOX, series, ICON)
        self.assertFalse(ok)
        self.assertTrue(any("only 1 of 3" in reason for reason in why), why)

    def test_flared_at_dock_fails(self) -> None:
        # rows widen toward the dock side (a flare, the reverse of a genie neck): no dock-side narrowing
        flare = [round(200 + 100 * (i / (ROWS - 1)) ** 3) for i in range(ROWS)]
        series = [frame(570 + k * 40, 420 + k * 120, flare) for k in range(3)]
        ok, why, detail = lamp.bend_verdict(REST_BBOX, series, ICON)
        self.assertEqual(detail["bent_frames"], 0)
        self.assertFalse(ok)

    def test_widening_again_toward_the_dock_fails(self) -> None:
        flared = bent(0.7)
        flared[-60:] = [w + 70 for w in flared[-60:]]  # neck, then flaring out at the dock side: not a genie
        series = [frame(570, 420, flared), frame(610, 540, flared), frame(650, 660, bent(0.7))]
        ok, why, detail = lamp.bend_verdict(REST_BBOX, series, ICON)
        self.assertFalse(detail["frames"][0]["monotone"])
        self.assertEqual(detail["bent_frames"], 1)
        self.assertFalse(ok, why)


class MirrorTests(unittest.TestCase):
    MINIMIZE = [round(300 - 90 * (i / 20) ** 2) for i in range(21)]

    @staticmethod
    def minimize_series(widths: list[int]) -> list[tuple[float, int]]:
        """Minimize: distance to the icon falls from 1 (rest) to 0 (icon) as the widths run."""
        n = len(widths) - 1
        return [(1 - i / n, w) for i, w in enumerate(widths)]

    @staticmethod
    def restore_series(widths: list[int]) -> list[tuple[float, int]]:
        """Restore: distance rises from 0 to 1 while the widths run in the opposite order."""
        n = len(widths) - 1
        return [(i / n, w) for i, w in enumerate(widths)]

    def test_mirrored_restore_passes(self) -> None:
        ok, why, detail = lamp.mirror_verdict(self.minimize_series(self.MINIMIZE),
                                              self.restore_series(list(reversed(self.MINIMIZE))), REST_W)
        self.assertTrue(ok, why)
        self.assertLessEqual(detail["max_diff"], detail["tolerance"])

    def test_restore_that_is_not_the_reverse_fails(self) -> None:
        # restore keeps the rest width for the whole flight: no deformation to reverse
        ok, why, _ = lamp.mirror_verdict(self.minimize_series(self.MINIMIZE),
                                         self.restore_series([REST_W] * 21), REST_W)
        self.assertFalse(ok)
        self.assertIn("does not mirror", why[0])

    def test_restore_that_jumps_fails(self) -> None:
        jump = [300] * 10 + [120, 100, 90, 60, 40, 30, 20, 10, 5, 2, 1]  # snaps to full size, then collapses
        ok, why, _ = lamp.mirror_verdict(self.minimize_series(self.MINIMIZE), self.restore_series(jump), REST_W)
        self.assertFalse(ok)
        self.assertIn("does not mirror", why[0])

    def test_too_few_frames_fail(self) -> None:
        ok, why, _ = lamp.mirror_verdict([(1.0, 300), (0.5, 200), (0.0, 100)], [], REST_W)
        self.assertFalse(ok)
        self.assertIn(">= 3 mid frames", why[0])

    def test_two_frame_flights_cannot_pass(self) -> None:
        ok, why, _ = lamp.mirror_verdict([(1.0, 300), (0.0, 200)], [(0.0, 200), (1.0, 300)], REST_W)
        self.assertFalse(ok)
        self.assertIn(">= 3 mid frames", why[0])


class MaskProfileTests(unittest.TestCase):
    def test_row_span_of_bright_pixels(self) -> None:
        # 8x3 image: row 0 bright at x=1..4 (span 4), row 1 dark, row 2 bright at x=1 and x=7 (span 7);
        # grey (150) is not bright and must not widen the span
        img = Image.new("RGB", (8, 3), (0, 0, 0))
        for x in (1, 2, 4):
            img.putpixel((x, 0), (255, 255, 255))
        img.putpixel((3, 0), (0, 0, 0))
        img.putpixel((1, 2), (255, 255, 255))
        img.putpixel((7, 2), (255, 255, 255))
        for x in range(8):
            img.putpixel((x, 1), (0, 0, 0))
        img.putpixel((0, 2), (150, 150, 150))
        bbox, widths = lamp.mask_profile(img)
        self.assertEqual(bbox, (1, 0, 7, 2))
        self.assertEqual(widths, [4, 0, 7])

    def test_empty_image_has_no_bbox(self) -> None:
        self.assertEqual(lamp.mask_profile(Image.new("RGB", (4, 4), (20, 20, 20))), (None, []))


class ComposeTests(unittest.TestCase):
    def entries(self, status: str) -> dict[str, dict]:
        return {sid: lamp.entry(sid, status) for sid in lamp.IDS}

    def test_all_pass(self) -> None:
        doc = lamp.compose_document(self.entries(lamp.PASS))
        self.assertEqual((doc["verdict"], doc["exit_code"]), ("ALL_SCENARIOS_PASS", 0))

    def test_any_fail_is_exit_one(self) -> None:
        results = self.entries(lamp.PASS)
        results["LAMP01"] = lamp.entry("LAMP01", lamp.FAIL, "rigid")
        doc = lamp.compose_document(results)
        self.assertEqual((doc["verdict"], doc["exit_code"]), ("FAIL", 1))

    def test_missing_ids_are_not_run_not_pass(self) -> None:
        doc = lamp.compose_document({"LAMP01": lamp.entry("LAMP01", lamp.PASS)})
        self.assertEqual([e["status"] for e in doc["results"]], [lamp.PASS, lamp.NOT_RUN, lamp.NOT_RUN])
        self.assertEqual(doc["verdict"], "INCOMPLETE")

    def test_environment_blocked_is_exit_three(self) -> None:
        doc = lamp.compose_document(self.entries(lamp.ENV_BLOCKED))
        self.assertEqual((doc["verdict"], doc["exit_code"]), ("ENVIRONMENT_BLOCKED", 3))


if __name__ == "__main__":
    unittest.main()

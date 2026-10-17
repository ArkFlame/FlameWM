"""Pure unit tests for the context-menu/weather acceptance runner. No X server or display is touched.

Run with:
    python3 -I tools/test_flamewm_context_menu_weather_acceptance.py
"""

from __future__ import annotations

import argparse
import sys
import unittest
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))

import flamewm_context_menu_weather_acceptance as cm  # noqa: E402


def _args(**overrides: object) -> argparse.Namespace:
    names = ("task", "pager", "file", "folder", "sticky", "weather")
    values = {f"{name}_xy": None for name in names}
    values.update(overrides)
    return argparse.Namespace(**values)


class SessionOverrideTest(unittest.TestCase):
    def test_cli_xy_overrides_populate_session(self) -> None:
        args = _args(file_xy=(40, 40), folder_xy=(40, 125), sticky_xy=(640, 340), weather_xy=(1126, 768))
        session = cm.Session(ctx=object(), args=args)  # type: ignore[arg-type]
        self.assertEqual(session.overrides["file"], (40, 40))
        self.assertEqual(session.overrides["folder"], (40, 125))
        self.assertEqual(session.overrides["sticky"], (640, 340))
        self.assertEqual(session.overrides["weather"], (1126, 768))
        self.assertIsNone(session.overrides["task"])

    def test_override_wins_without_capture_or_ocr(self) -> None:
        session = cm.Session(ctx=object(), args=_args(task_xy=(69, 775)))  # type: ignore[arg-type]

        def no_capture(name: str) -> None:
            raise AssertionError("override must not trigger a screen capture")

        session.shot = no_capture  # type: ignore[method-assign]
        point, reason = session.locate("task", lambda t: False)
        self.assertEqual(point, (69, 775))
        self.assertEqual(reason, "override")


class OverrideMappedTest(unittest.TestCase):
    MAPPED = (
        'xwininfo: Window id: 0xa000db "FlameWM task menu"\n\n'
        "  Absolute upper-left X:  500\n  Map State: IsViewable\n  Override Redirect State: yes\n"
    )

    def test_real_xwininfo_lowercase_yes_counts_as_menu(self) -> None:
        self.assertTrue(cm.override_mapped(self.MAPPED))

    def test_unmapped_or_managed_window_is_not_a_menu(self) -> None:
        self.assertFalse(cm.override_mapped(self.MAPPED.replace("IsViewable", "IsUnmapped")))
        self.assertFalse(cm.override_mapped(self.MAPPED.replace("Override Redirect State: yes", "Override Redirect State: no")))


class MenuGeometryTest(unittest.TestCase):
    """Canonical menu height is 2 * MENU_PADDING + rows * ROW_HEIGHT (1 row = 41px, 2 = 72px, 3 = 103px)."""

    def test_canonical_heights_are_padding_plus_whole_rows(self) -> None:
        for rows, height in ((1, 41), (2, 72), (3, 103)):
            visual = cm.menu_visual({"id": "m", "x": 0, "y": 0, "w": 10, "h": height}, (5, 5),
                                    [(0, 0, 0)] * (10 * 200), 10, 200)
            self.assertTrue(visual["rows_exact"], f"{rows} rows at {height}px")

    def test_hover_row_starts_after_top_padding(self) -> None:
        visual = cm.menu_visual({"id": "m", "x": 0, "y": 0, "w": 10, "h": 72}, (5, cm.MENU_PADDING + 31 + 3),
                                [(0, 0, 0)] * (10 * 100), 10, 100)
        self.assertEqual(visual["hovered_row"], 1)


FRAME = cm.MENU_BORDER_RGB


def _canvas(width: int, height: int, fill: tuple[int, int, int]) -> list[tuple[int, int, int]]:
    return [fill] * (width * height)


def _paint(pixels, width, rect, colour) -> None:
    x, y, w, h = rect
    for row in range(y, y + h):
        for col in range(x, x + w):
            pixels[row * width + col] = colour


def _visual(menu_id: str, background, wallpaper, hover) -> dict:
    return {"menu_id": menu_id, "rect": [0, 0, 205, 41], "background_rgb": list(background),
            "border_rgb": list(FRAME), "rows_exact": True, "hover_rgb": list(hover),
            "wallpaper_rgb": list(wallpaper)}


class DesktopMenuFrameTest(unittest.TestCase):
    def test_desktop_frame_found_from_anchor_point(self) -> None:
        width, height = 400, 300
        pixels = _canvas(width, height, (200, 170, 190))
        _paint(pixels, width, (50, 40, 205, 165), (110, 90, 105))  # body
        _paint(pixels, width, (50, 40, 205, 1), FRAME)             # top edge
        _paint(pixels, width, (50, 204, 205, 1), FRAME)            # bottom edge
        _paint(pixels, width, (254, 40, 1, 165), FRAME)            # right edge
        rect = cm.desktop_menu_rect(pixels, width, height, (50, 40))
        self.assertEqual((rect["x"], rect["y"], rect["w"], rect["h"]), (50, 40, 205, 165))

    def test_no_frame_means_no_desktop_menu(self) -> None:
        pixels = _canvas(100, 100, (200, 170, 190))
        self.assertIsNone(cm.desktop_menu_rect(pixels, 100, 100, (10, 10)))


class OverlayModelTest(unittest.TestCase):
    def test_fit_line_recovers_exact_line(self) -> None:
        slope, intercept = cm.fit_line([0.0, 100.0, 200.0], [12.0, 64.0, 116.0])
        self.assertAlmostEqual(slope, 0.52)
        self.assertAlmostEqual(intercept, 12.0)

    def test_menus_on_one_overlay_line_pass(self) -> None:
        visuals = [_visual(f"m{i}", (round(0.5 * w + 10), round(0.5 * w + 10), round(0.5 * w + 10)), (w, w, w),
                           (round(0.5 * w + 40), round(0.5 * w + 40), round(0.5 * w + 40)))
                   for i, w in enumerate((40, 120, 200))]
        status, reason = cm.compare_visuals(visuals)
        self.assertEqual(status, cm.PASS, reason)

    def test_menu_off_the_shared_line_fails(self) -> None:
        visuals = [_visual(f"m{i}", (round(0.5 * w + 10),) * 3, (w, w, w), (round(0.5 * w + 40),) * 3)
                   for i, w in enumerate((40, 120, 200))]
        visuals[2]["background_rgb"] = [200, 200, 200]  # a menu filled as if it had no translucency
        status, reason = cm.compare_visuals(visuals)
        self.assertEqual(status, cm.FAIL)
        self.assertIn("fill differs", reason)

    def test_content_backed_menu_off_the_line_is_reported_not_gated(self) -> None:
        visuals = [_visual(f"m{i}", (round(0.5 * w + 10),) * 3, (w, w, w), (round(0.5 * w + 40),) * 3)
                   for i, w in enumerate((40, 120, 200))]
        content = _visual("sticky", (140, 136, 99), (236, 207, 169), (170, 166, 129))  # hover = fill + 30, as the others
        content["content_backed"] = True
        status, reason = cm.compare_visuals(visuals + [content])
        self.assertEqual(status, cm.PASS, reason)
        self.assertIn("not gated", reason)

    def test_no_hover_feedback_fails(self) -> None:
        visuals = [_visual(f"m{i}", (round(0.5 * w + 10),) * 3, (w, w, w), (round(0.5 * w + 10),) * 3)
                   for i, w in enumerate((40, 120, 200))]
        status, reason = cm.compare_visuals(visuals)
        self.assertEqual(status, cm.FAIL)
        self.assertIn("no hover feedback", reason)

    def test_accent_border_fails(self) -> None:
        visuals = [_visual(f"m{i}", (round(0.5 * w + 10),) * 3, (w, w, w), (round(0.5 * w + 40),) * 3)
                   for i, w in enumerate((40, 120, 200))]
        visuals[0]["border_rgb"] = [239, 64, 72]  # #ef4048 accent
        status, reason = cm.compare_visuals(visuals)
        self.assertEqual(status, cm.FAIL)
        self.assertIn("accent-coloured", reason)


if __name__ == "__main__":
    unittest.main()

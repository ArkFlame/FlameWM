#!/usr/bin/env python3
"""Native acceptance for the tray Wi-Fi, audio and calendar popups on a nested Xephyr display.

For each popup: click its tray button with XTest, confirm the popup window is mapped
(`xwininfo -id`), then sample its left padding strip in a root screenshot and check the
menu background:
- opacity 0: the strip is not one solid colour (frosted wallpaper shows through);
- opacity 100: every sample equals the menu colour (surfaceColor) within tolerance.

Only the given display is touched; displays :97 and :123-:133 are refused. Pure checks
(`parse_geometry`, `parse_map_state`, `strip_points`, `check_popup`) are unit tested in
tools/test_flamewm_tray_popups_acceptance.py without X.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import flamewm_opacity_acceptance as menu  # noqa: E402  (shared PPM/root/XTest helpers)

DISPLAY_RE = re.compile(r"^:(\d{1,3})(?:\.\d+)?$")
TOLERANCE = 6
POPUP_TITLES = {"wifi": "FlameWM network", "audio": "FlameWM audio", "calendar": "FlameWM calendar"}
GEOMETRY_RE = re.compile(r"Absolute upper-left X:\s*(-?\d+)\s*\n.*?Absolute upper-left Y:\s*(-?\d+)\s*\n.*?Width:\s*(\d+)\s*\n.*?Height:\s*(\d+)", re.S)


def display_allowed(display: str) -> bool:
    """Only nested test displays; never the user's :97 or the reserved :123-:133 range."""
    match = DISPLAY_RE.match(display)
    if not match:
        return False
    number = int(match.group(1))
    return number not in (97,) and not (123 <= number <= 133)


def parse_geometry(xwininfo_text: str) -> tuple[int, int, int, int]:
    """(x, y, width, height) from `xwininfo -id` output."""
    match = GEOMETRY_RE.search(xwininfo_text)
    if not match:
        raise ValueError("no geometry in xwininfo output")
    x, y, width, height = (int(v) for v in match.groups())
    return x, y, width, height


def parse_map_state(xwininfo_text: str) -> bool:
    return "Map State: IsViewable" in xwininfo_text


def find_window_id(tree_text: str, title: str) -> str | None:
    """Window id of the first `xwininfo -root -tree` line naming `title` exactly."""
    for line in tree_text.splitlines():
        match = re.match(r'\s*(0x[0-9a-fA-F]+) "([^"]*)"', line)
        if match and match.group(2) == title:
            return match.group(1)
    return None


def strip_points(rect: tuple[int, int, int, int], step: int = 6) -> list[tuple[int, int]]:
    """Points in the left padding strip of a popup (inside border and padding, no text)."""
    x, y, width, height = rect
    xs = [x + 4, x + 8]
    points = []
    for py in range(y + 16, y + height - 16, step):
        for px in xs:
            if px < x + width:
                points.append((px, py))
    return points


def colour_matches(observed: tuple[int, int, int], expected: tuple[int, int, int]) -> bool:
    return all(abs(a - b) <= TOLERANCE for a, b in zip(observed, expected))


def check_popup(samples: list[tuple[int, int, int]], opacity: int, menu_colour: tuple[int, int, int]) -> dict:
    """Pass rules: opacity 100 needs every sample equal to the menu colour; below 100 the strip must
    not be a single solid colour (backdrop visible)."""
    if not samples:
        return {"ok": False, "reason": "no samples"}
    distinct = sorted(set(samples))
    solid = all(colour_matches(s, samples[0]) for s in samples)
    if opacity >= 100:
        ok = all(colour_matches(s, menu_colour) for s in samples)
        reason = "solid menu colour" if ok else "pixels differ from menu colour"
    else:
        ok = not solid
        reason = "backdrop visible" if ok else "solid fill: backdrop not visible"
    return {"ok": ok, "reason": reason, "distinct": len(distinct), "solid": solid}


def xwininfo(window_id: str, display: str) -> str:
    return subprocess.run(
        ["xwininfo", "-display", display, "-id", window_id],
        check=True, capture_output=True, text=True, timeout=30,
    ).stdout


def root_tree(display: str) -> str:
    return subprocess.run(
        ["xwininfo", "-display", display, "-root", "-tree"],
        check=True, capture_output=True, text=True, timeout=30,
    ).stdout


def parse_point(text: str) -> tuple[int, int]:
    x, y = (int(v) for v in text.split(","))
    return x, y


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--display", required=True)
    parser.add_argument("--opacity", type=int, required=True, help="surfaceOpacity in effect (0 or 100)")
    parser.add_argument("--menu-color", required=True, help="surfaceColor as #RRGGBB")
    parser.add_argument("--wifi-button", required=True, help="x,y of the tray Wi-Fi icon")
    parser.add_argument("--audio-button", required=True, help="x,y of the tray volume icon")
    parser.add_argument("--calendar-button", required=True, help="x,y of the clock")
    parser.add_argument("--out", help="write the JSON result to this path")
    return parser.parse_args(argv)


def run(args: argparse.Namespace) -> dict:
    menu_colour = tuple(int(args.menu_color.lstrip("#")[i:i + 2], 16) for i in (0, 2, 4))
    buttons = {"wifi": args.wifi_button, "audio": args.audio_button, "calendar": args.calendar_button}
    result: dict = {"display": args.display, "opacity": args.opacity, "popups": {}}
    for name, button in buttons.items():
        x, y = parse_point(button)
        menu.click(args.display, x, y)
        time.sleep(2)
        window = find_window_id(root_tree(args.display), POPUP_TITLES[name])
        entry: dict = {"window": window, "mapped": False}
        if window:
            info = xwininfo(window, args.display)
            entry["mapped"] = parse_map_state(info)
            rect = parse_geometry(info)
            entry["rect"] = rect
            if entry["mapped"] and rect[2] > 1 and rect[3] > 1:
                image = menu.root_image(args.display)
                samples = [menu.pixel(image, px, py) for px, py in strip_points(rect)]
                entry.update(check_popup(samples, args.opacity, menu_colour))
            else:
                entry.update({"ok": False, "reason": "popup not mapped or zero-size"})
        else:
            entry.update({"ok": False, "reason": "popup window not found"})
        result["popups"][name] = entry
        # Dismiss before the next popup so each one is sampled alone.
        menu.click(args.display, 40, 40)
        time.sleep(1)
    result["ok"] = all(p.get("ok") for p in result["popups"].values())
    return result


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    if not display_allowed(args.display):
        print(f"refusing display {args.display!r}", file=sys.stderr)
        return 2
    result = run(args)
    text = json.dumps(result, indent=2)
    print(text)
    if args.out:
        with open(args.out, "w", encoding="utf-8") as handle:
            handle.write(text + "\n")
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

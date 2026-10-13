#!/usr/bin/env python3
"""Isolated native acceptance for the flamewm-lock screen (LK01..LK15).

Runs in a private Xephyr (display :90..:99), a private D-Bus session and private
XDG directories under --run-dir (must not exist or be empty). Reuses the appearance
runner's machinery (tree parsing, OCR, pixel helpers) and the settings runner's Ctx,
XTEST input and capture. Statuses: PASS, FAIL, NOT_RUN, ENVIRONMENT_BLOCKED.

  LK01 lock starts and covers the screen (xwininfo tree, mapped)
  LK02 grab effective: Super and Ctrl+Alt+Right via XTEST leave the WM tree unchanged
       and the lock window topmost
  LK03 wrong password + Return keeps it locked and OCR shows a failure word
  LK04 SIGTERM does not unlock (process alive, window still covers the screen)
  LK05 built-in background: the top-left strip of the lock screenshot matches the mean colour of
       assets/wallpapers/fitz-roy-laguna.webp cover-scaled to the screen (PIL reference)
  LK06 success path: always NOT_RUN ('needs real credentials'); never faked
  LK07 lockBackgroundColor=#336699: screen outside the card ~#336699, card still readable
  LK08 lockClockFormat=12h, lockShowDate=false, lockShowUsername=false: AM/PM seen by OCR,
       date words and the username label absent
  LK09 lockBackgroundImage=<generated PNG>: its quadrant colours show at the screen corners
  LK10 Super+L (XTEST chord, WM+shell running, no lock) starts the lock covering the screen in 5 s
  LK11 SIGKILL of the WM-started lock: a new lock window covers the screen again within 5 s
  LK12 reference layout: measured password field, avatar ring, clock, and power icon blobs sit
       within 3*s px of the reference anchors (701x438 canvas mapped onto the screen)
  LK13 power actions are real: a fake control plane (tools/flamewm_fake_control_plane.py) records
       Session calls from Sleep, Shut Down (+Cancel, +Escape, +confirm) and a dimmed Restart
  LK14 screensaver wake is consumed: after Ribbons start (1 idle minute), the first key only wakes
       the screen; the caret stays visible (no character reached the password field); the next key types
  LK15 screensaver: Ribbons frames change while active, and after displayOffMinutes the X server
       reports DPMS "Monitor is Off" (queried with xset q); a wake key turns the monitor back on

LK07..LK09 each restart the WM with their own seeded settings (no state shared between them).
LK10 and LK11 run on the WM-owned lock (the WM spawns it; the harness does not).
LK12 restarts with default settings. LK13 runs the lock without the WM so the fake owns the bus name.
LK14 and LK15 restart the WM with screensaver settings (1 idle minute; LK15 also sets a 1 minute display-off).

Exit status: 0 no FAIL, 1 any FAIL, 2 usage error, 3 environment blocked.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import math
import os
import re
import secrets
import shutil
import signal
import subprocess
import sys
import time
from collections import Counter
from datetime import datetime
from pathlib import Path

TOOLS =Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))

import flamewm_settings_acceptance as base  # noqa: E402
import flamewm_appearance_acceptance as appear  # noqa: E402
import flamewm_fake_control_plane as fake_plane  # noqa: E402

ROOT = base.ROOT
SCHEMA = "flamewm-lock-acceptance/1"
IDS = tuple(f"LK{n:02d}" for n in range(1, 18))
TITLES = {
    "LK01": "lock starts and covers the screen",
    "LK02": "grab effective: WM shortcuts do not act, lock stays topmost",
    "LK03": "wrong password keeps the lock and shows a failure message",
    "LK04": "SIGTERM does not unlock",
    "LK05": "built-in background: strip matches the laguna image mean",
    "LK06": "success path with real credentials",
    "LK07": "customised background colour fills the screen outside the card",
    "LK08": "clock 12h, date and username hidden",
    "LK09": "customised background image is cover-scaled behind the card",
    "LK10": "Super+L starts the lock",
    "LK11": "killed lock comes back",
    "LK12": "reference layout: anchors within 3*s px",
    "LK13": "power actions reach the control plane",
    "LK14": "screensaver wake key is consumed",
    "LK15": "Ribbons frames change; display off through DPMS",
    "LK16": "settings preview: no grab, exits on motion after grace; normal lock holds the grab",
    "LK17": "avatar: default ring with no letter; avatarImage shows a circle-clipped picture",
}
LOCK_APPEAR_SECONDS = 5.0
FAKE_CONTROL_BIN = TOOLS / "flamewm_fake_control_plane.py"
FAKE_PYTHON = "/usr/bin/python3"   # dbus-python lives in the system dist-packages
SESSION_CALL_SECONDS = 3.0
ICON_DIM_RATIO = 0.45              # disabled icon contrast <= 45% of enabled (38% opacity plus slack)
SAVER_IDLE_SECONDS = 65.0          # one idle minute to Ribbons, plus margin
SAVER_POLL_SECONDS = 150.0         # bounded wait for the Ribbons start and the DPMS transition
SAVER_FRAME_GAP = 1.5              # seconds between the two frames compared for motion
CARET_VISIBLE = 0.8                # caret rect white fraction when the field is empty (white caret)
CARET_HIDDEN = 0.6                 # caret rect white fraction once a character is typed
REF_W, REF_H, REF_MAX_SCALE = 701.0, 438.0, 2.5
BRIGHT_MIN = 180                   # max channel of a bright lock glyph or ring pixel
# Password field (ui/lock/lock.css): the lower-edge shade (black SHADE_ALPHA) sits under the glass,
# and the white glass (GLASS_ALPHA) sits over both. Both layers cover the field, so the field pixel is
# white*GLASS + (1-GLASS)*(1-SHADE)*background.
GLASS_ALPHA = 0.18
SHADE_ALPHA = 0.18
GLASS_TOLERANCE = 12
ANCHOR_BLOB_MIN_PIXELS = 6
FLAT_BACKGROUND = "#336699"          # solid background for the password glass sub-check
CHEVRON_INK_MIN = 235                # the chevron is pure white; the glass rim and fill sit below this
TEXT_TOLERANCE = 24
BACKGROUND_STRIP_REF = (200, 40)   # left strip (x, y) in reference px, above the clock: background only
PASS, FAIL = appear.PASS, appear.FAIL
NOT_RUN, ENV_BLOCKED = appear.NOT_RUN, appear.ENV_BLOCKED
STATUSES = (PASS, FAIL, NOT_RUN, ENV_BLOCKED)
LOCK_BIN_DEFAULT = ROOT / ".target/release/flamewm-lock"
LOCK_BIN_NAME = "flamewm-lock"
WM_BIN_DEFAULT = ROOT / ".target/release/flamewm"
BUILTIN_BACKGROUND = ROOT / "assets/wallpapers/fitz-roy-laguna.webp"   # lock default (document.rs)
DEFAULT_DIM_PERCENT = 25           # lockBackgroundDimPercent default (settings.rs)
BACKGROUND_TOLERANCE = 24          # per channel, mean colour of the strip (scaling differs slightly)
FAILURE_WORDS = {"incorrect", "failed", "wrong", "invalid"}
WRONG_PASSWORD_KEYS = ("x", "y", "z")
TEXT_BRIGHT = (0xEF, 0xF0, 0xF1)   # Breeze foreground: clock and username
CUSTOM_COLOUR = (0x33, 0x66, 0x99)
CUSTOM_COLOUR_TOLERANCE = 3
PATTERN_QUADRANTS = {               # generated 320x200 PNG; 4x cover-scale at 1280x800 keeps it exact
    "top-left": ((0, 0, 160, 100), (0xC0, 0x39, 0x2B)),
    "top-right": ((160, 0, 320, 100), (0x27, 0xAE, 0x60)),
    "bottom-left": ((0, 100, 160, 200), (0x29, 0x80, 0xB9)),
    "bottom-right": ((160, 100, 320, 200), (0xF1, 0xC4, 0x0F)),
}
PATTERN_TOLERANCE = 6


# ---------------------------------------------------------------- pure metrics

def covers_screen(node: dict, screen_w: int, screen_h: int) -> bool:
    """A window covers the screen when it sits at (0,0) with at least the screen size."""
    return node["x"] == 0 and node["y"] == 0 and node["w"] >= screen_w and node["h"] >= screen_h


def lock_window(nodes: list[dict], screen_w: int, screen_h: int) -> dict | None:
    """Largest root-level window covering the screen, or None."""
    candidates = [n for n in nodes if n["parent"] is None and covers_screen(n, screen_w, screen_h)]
    return max(candidates, key=lambda n: n["w"] * n["h"]) if candidates else None


def respawned_pids(pids: list[int], victim: int) -> list[int]:
    """WM-owned lock pids other than the killed one. X window ids are reused, so the pid is the identity."""
    return [pid for pid in pids if pid != victim]


def is_topmost(nodes: list[dict], window_id: str) -> bool:
    """`xwininfo -tree` lists the root's children topmost first (verified natively by raising a window)."""
    roots = [n["id"] for n in nodes if n["parent"] is None]
    return bool(roots) and roots[0] == window_id


def tree_signature(nodes: list[dict], exclude: set[str]) -> tuple:
    """Stable snapshot of every window except the lock's own (ids excluded)."""
    return tuple(sorted((n["id"], n["name"], n["x"], n["y"], n["w"], n["h"], str(n["parent"]))
                        for n in nodes if n["id"] not in exclude))


def failure_text_seen(words: list[tuple[str, int, int]]) -> bool:
    return any(word in FAILURE_WORDS for word, _, _ in words)


def mean_colour(pixels: list[tuple[int, int, int]]) -> tuple[float, float, float]:
    count = max(len(pixels), 1)
    return tuple(sum(p[i] for p in pixels) / count for i in range(3))


def background_matches(pixels: list[tuple[int, int, int]], expected: tuple[float, float, float]) -> bool:
    """Mean colour of the background strip within BACKGROUND_TOLERANCE on every channel."""
    return bool(pixels) and all(abs(got - want) <= BACKGROUND_TOLERANCE
                                for got, want in zip(mean_colour(pixels), expected))


def builtin_background_mean(width: int, height: int, box: tuple[int, int, int, int]) -> tuple[float, float, float] | None:
    """Mean colour of the built-in lock background over `box`, cover-scaled the way document.rs does it."""
    if base.Image is None or not BUILTIN_BACKGROUND.is_file():
        return None
    image = base.Image.open(BUILTIN_BACKGROUND).convert("RGB")
    scale = max(width / image.width, height / image.height)
    scaled_w = max(width, math.ceil(image.width * scale))
    scaled_h = max(height, math.ceil(image.height * scale))
    resample = getattr(base.Image, "Resampling", base.Image).BILINEAR
    scaled = image.resize((scaled_w, scaled_h), resample)
    left, top = (scaled_w - width) // 2, (scaled_h - height) // 2
    x0, y0, x1, y1 = box
    # The lock dims the background by DEFAULT_DIM_PERCENT (25%) after a blur that keeps the mean.
    dim_keep = 1.0 - DEFAULT_DIM_PERCENT / 100.0
    mean = mean_colour(list(scaled.crop((left + x0, top + y0, left + x1, top + y1)).getdata()))
    return tuple(channel * dim_keep for channel in mean)


def strip_box(width: int, height: int) -> tuple[int, int, int, int]:
    """Background-only strip above the clock band in screen pixels (matches background_strip)."""
    scale, ox, oy = reference_fit(width, height)
    strip_w, strip_h = BACKGROUND_STRIP_REF
    return (0, 0, int(ox + strip_w * scale), int(oy + strip_h * scale))


def frames_differ(first: list[tuple[int, int, int]], second: list[tuple[int, int, int]]) -> bool:
    """Ribbons animate: at least 2% of sampled pixels changed by more than a small tolerance."""
    sampled = list(zip(first[::7], second[::7]))
    changed = sum(1 for a, b in sampled if max(abs(a[i] - b[i]) for i in range(3)) > 24)
    return bool(sampled) and changed >= 0.02 * len(sampled)


def caret_white_fraction(pixels: list[tuple[int, int, int]], width: int, height: int) -> float:
    """Share of white pixels in the caret rectangle (the caret is white on the glass field)."""
    scale = reference_fit(width, height)[0]
    field = reference_anchors(width, height)["password"]
    inset = 10 * scale
    rect = (field[0] + inset, field[1] + field[3] * 0.25, max(1.5 * scale, 1.0), field[3] * 0.5)
    x0, y0, x1, y1 = _box(width, height, rect)
    area = [pixels[y * width + x] for y in range(y0, y1) for x in range(x0, x1)]
    return sum(1 for p in area if _white(p)) / max(len(area), 1)


def dpms_monitor_state(xset_text: str) -> str | None:
    """'on' or 'off' from `xset q` output, None when the server reports no DPMS monitor state."""
    for line in xset_text.splitlines():
        if "Monitor is" in line:
            return "off" if "Monitor is Off" in line else "on"
    return None


def colour_dominates(pixels: list[tuple[int, int, int]], rgb: tuple[int, int, int], tolerance: int) -> bool:
    """At least 95% of the pixels sit within tolerance of rgb, and the dominant colour does too."""
    return (appear.fraction_matching(pixels, rgb, tolerance) >= 0.95
            and appear.close_enough(appear.dominant_color(pixels), rgb, tolerance))


def row_runs(rows: list[int]) -> list[tuple[int, int]]:
    """Inclusive (first, last) runs of consecutive row indices."""
    runs: list[tuple[int, int]] = []
    for y in sorted(set(rows)):
        if runs and y == runs[-1][1] + 1:
            runs[-1] = (runs[-1][0], y)
        else:
            runs.append((y, y))
    return runs


def rows_with(pixels: list[tuple[int, int, int]], width: int, rgb: tuple[int, int, int], tolerance: int) -> list[int]:
    """Indices of the rows of a `width`-pixel-wide crop that contain a pixel near rgb."""
    return [y for y in range(len(pixels) // width)
            if any(appear.close_enough(p, rgb, tolerance) for p in pixels[y * width:(y + 1) * width])]


def date_words(now: datetime) -> set[str]:
    """Lower-cased OCR tokens of today's date line (weekday with and without comma, month, year)."""
    weekday = now.strftime("%A").lower()
    return {weekday, weekday + ",", now.strftime("%B").lower(), now.strftime("%Y")}


# ---------------------------------------------------------------- reference layout

def _centred(cx: float, cy: float, w: float, h: float) -> tuple[float, float, float, float]:
    return (cx - w / 2, cy - h / 2, w, h)


# Reference-canvas (701x438) rects as (x, y, w, h); mirrors crates/flamewm-lock/src/layout.rs.
REF_ANCHORS = {
    "clock": _centred(350, 61, 120, 30),
    "date": (270, 89, 160, 12),
    "avatar": _centred(350, 179, 68, 68),
    "username": _centred(350, 233, 160, 12),
    "password": (271, 253, 138, 17),
    "submit": (413, 253, 17, 17),
    "power_sleep": _centred(245, 348, 23, 23),
    "power_restart": _centred(315, 348, 23, 23),
    "power_shutdown": _centred(386, 348, 23, 23),
    "power_other": _centred(455, 348, 23, 23),
}
MEASURED_ANCHORS = ("password", "avatar", "clock", "power_sleep", "power_restart", "power_shutdown", "power_other")


def reference_fit(out_w: int, out_h: int) -> tuple[float, float, float]:
    """(s, ox, oy): uniform scale clamped to REF_MAX_SCALE and the centring offset."""
    s = min(out_w / REF_W, out_h / REF_H, REF_MAX_SCALE)
    return s, (out_w - REF_W * s) / 2, (out_h - REF_H * s) / 2


def reference_anchors(out_w: int, out_h: int) -> dict[str, tuple[float, float, float, float]]:
    """Expected (x, y, w, h) of every reference element in output pixels."""
    s, ox, oy = reference_fit(out_w, out_h)
    return {name: (ox + x * s, oy + y * s, w * s, h * s) for name, (x, y, w, h) in REF_ANCHORS.items()}


def anchor_centre(rect: tuple[float, float, float, float]) -> tuple[float, float]:
    x, y, w, h = rect
    return x + w / 2, y + h / 2


def anchor_error(measured: tuple[float, float, float, float], expected: tuple[float, float, float, float]) -> float:
    """Chebyshev distance in px between the centres of two (x, y, w, h) rects."""
    (mx, my), (ex, ey) = anchor_centre(measured), anchor_centre(expected)
    return max(abs(mx - ex), abs(my - ey))


def anchor_tolerance(out_w: int, out_h: int) -> float:
    return 3 * reference_fit(out_w, out_h)[0]


def _box(width: int, height: int, rect: tuple[float, float, float, float], pad: float = 0.0) -> tuple[int, int, int, int]:
    """Half-open pixel box (x0, y0, x1, y1) around rect, grown by pad and clipped to the image."""
    x, y, w, h = rect
    return (max(0, int(x - pad)), max(0, int(y - pad)),
            min(width, int(x + w + pad) + 1), min(height, int(y + h + pad) + 1))


def _points(pixels: list[tuple[int, int, int]], width: int, box: tuple[int, int, int, int], pred) -> list[tuple[int, int]]:
    x0, y0, x1, y1 = box
    return [(x, y) for y in range(y0, y1) for x in range(x0, x1) if pred(pixels[y * width + x])]


def _bbox(points: list[tuple[int, int]]) -> tuple[float, float, float, float] | None:
    if not points:
        return None
    xs, ys = [p[0] for p in points], [p[1] for p in points]
    return (float(min(xs)), float(min(ys)), float(max(xs) - min(xs) + 1), float(max(ys) - min(ys) + 1))


def _bright(p: tuple[int, int, int]) -> bool:
    return max(p) >= BRIGHT_MIN


def _white(p: tuple[int, int, int]) -> bool:
    """Every channel high: white text and the white ring, never a coloured avatar disc."""
    return min(p) >= BRIGHT_MIN


def glass_over(background: tuple[int, int, int]) -> tuple[float, float, float]:
    """Expected field colour over `background`: white glass over the black lower-edge shade."""
    return tuple(GLASS_ALPHA * 255 + (1 - GLASS_ALPHA) * (1 - SHADE_ALPHA) * c for c in background)


def is_glass_over(p: tuple[int, int, int], background: tuple[int, int, int]) -> bool:
    expected = glass_over(background)
    return all(abs(channel - want) <= GLASS_TOLERANCE for channel, want in zip(p, expected))


def measure_password(pixels, width, height, expected, s):
    """Bbox of the long light-fill run: per row, the longest run whose gaps are <= 2*s (the caret is a
    narrower gap; the submit button sits 5 reference px away and is a separate run). Rows holding >= 60%
    of the expected field width form the field."""
    # The background under each column is sampled 3 reference px above the field (outside the glass);
    # the blurred background changes slowly, so the column above is the local background.
    x0, y0, x1, y1 = _box(width, height, expected, 12 * s)
    above = max(0, int(expected[1] - 3 * s))
    hits = [(x, y) for y in range(y0, y1) for x in range(x0, x1)
            if is_glass_over(pixels[y * width + x], pixels[above * width + x])]
    by_row: dict[int, list[int]] = {}
    for x, y in hits:
        by_row.setdefault(y, []).append(x)
    min_run = 0.6 * expected[2]
    kept: dict[int, tuple[int, int]] = {}
    for y, xs in by_row.items():
        xs.sort()
        kept_run = _best_run(xs, 2 * s)
        if kept_run[1] - kept_run[0] + 1 >= min_run:
            kept[y] = kept_run
    if not kept:
        return None
    x0 = min(first for first, _ in kept.values())
    x1 = max(last for _, last in kept.values())
    y0, y1 = min(kept), max(kept)
    return (float(x0), float(y0), float(x1 - x0 + 1), float(y1 - y0 + 1))


def _best_run(xs: list[int], max_gap: float) -> tuple[int, int]:
    """Longest (first, last) run of sorted x positions with gaps no larger than max_gap."""
    best = current = (xs[0], xs[0])
    for x in xs[1:]:
        if x - current[1] > max_gap + 1:
            current = (x, x)
        else:
            current = (current[0], x)
        if current[1] - current[0] > best[1] - best[0]:
            best = current
    return best


def measure_avatar(pixels, width, height, expected):
    """Bbox of the bright ring: pixels 0.8R..1.2R from the expected centre (R = half the diameter)."""
    cx, cy = anchor_centre(expected)
    radius = expected[2] / 2
    box = _box(width, height, (cx - 1.5 * radius, cy - 1.5 * radius, 3 * radius, 3 * radius))
    ring = [p for p in _points(pixels, width, box, _white)
            if 0.8 * radius <= math.hypot(p[0] + 0.5 - cx, p[1] + 0.5 - cy) <= 1.2 * radius]
    return _bbox(ring)


def measure_clock(pixels, width, height, expected, avatar_top: float, s):
    """Bbox of the tallest bright row run above the avatar (the 31px clock outranges the 15px date)."""
    x, _, w, _ = expected
    box = (max(0, int(x - 40 * s)), 0, min(width, int(x + w + 40 * s) + 1), max(0, int(avatar_top - 2)))
    rows = sorted({y for _, y in _points(pixels, width, box, _white)})
    runs = row_runs(rows)
    if not runs:
        return None
    first, last = max(runs, key=lambda run: (run[1] - run[0], -run[0]))  # tie: the higher run
    return _bbox([p for p in _points(pixels, width, box, _white) if first <= p[1] <= last])


def measure_icon(pixels, width, height, expected, s):
    """Bbox of the bright blob at a power icon, searched in the icon box grown by 6 reference px."""
    points = _points(pixels, width, _box(width, height, expected, 6 * s), _bright)
    return _bbox(points) if len(points) >= ANCHOR_BLOB_MIN_PIXELS else None


def measure_reference_layout(pixels, width, height) -> dict[str, tuple[float, float, float, float] | None]:
    """Measured rect (or None when no blob is found) of every MEASURED_ANCHORS element."""
    s, _, _ = reference_fit(width, height)
    anchors = reference_anchors(width, height)
    measured = {"password": measure_password(pixels, width, height, anchors["password"], s),
                "avatar": measure_avatar(pixels, width, height, anchors["avatar"])}
    avatar_top = measured["avatar"][1] if measured["avatar"] else anchors["avatar"][1]
    measured["clock"] = measure_clock(pixels, width, height, anchors["clock"], avatar_top, s)
    for name in ("power_sleep", "power_restart", "power_shutdown", "power_other"):
        measured[name] = measure_icon(pixels, width, height, anchors[name], s)
    return measured


def judge_reference_layout(measured: dict, width: int, height: int, skip=()) -> tuple[bool, list[str]]:
    """(all anchors within tolerance, one observation line per anchor); `skip` names anchors judged elsewhere."""
    anchors = reference_anchors(width, height)
    tolerance = anchor_tolerance(width, height)
    ok, lines = True, []
    for name in MEASURED_ANCHORS:
        if name in skip:
            continue
        got = measured.get(name)
        if got is None:
            ok = False
            lines.append(f"{name}: no blob found, expected={_fmt(anchors[name])}")
            continue
        error = anchor_error(got, anchors[name])
        within = error <= tolerance
        ok = ok and within
        lines.append(f"{name}: measured={_fmt(got)} expected={_fmt(anchors[name])} "
                     f"err={error:.1f} tol={tolerance:.1f} {'ok' if within else 'OUT'}")
    return ok, lines


def _fmt(rect: tuple[float, float, float, float]) -> str:
    return "(" + ",".join(f"{v:.1f}" for v in rect) + ")"


def icon_peak(pixels, width, height, rect) -> int:
    """Brightest channel inside an icon rect."""
    x0, y0, x1, y1 = _box(width, height, rect)
    return max((max(pixels[y * width + x]) for y in range(y0, y1) for x in range(x0, x1)), default=0)


def icon_contrast(pixels, width, height, rect) -> int:
    """Peak brightness minus the median inside an icon rect. The background is most of the box, so
    the median cancels it and a 38% dimmed icon keeps about 38% of the enabled contrast."""
    x0, y0, x1, y1 = _box(width, height, rect)
    values = sorted(max(pixels[y * width + x]) for y in range(y0, y1) for x in range(x0, x1))
    return values[-1] - values[len(values) // 2] if values else 0


def text_band_counts(pixels, width, height, rect, s) -> int:
    """Bright pixels in a text anchor band, padded by 8 reference px."""
    return len(_points(pixels, width, _box(width, height, rect, 8 * s), _white))


# ---------------------------------------------------------------- results

def entry(sid: str, status: str, reason: str = "", evidence: list | None = None, commands: list | None = None) -> dict:
    if status not in STATUSES:
        raise ValueError(f"unknown status {status!r}")
    return {"id": sid, "title": TITLES[sid], "status": status, "reason": reason,
            "evidence": evidence or [], "command": " && ".join(commands or []) or "none"}


def compose_document(results: dict[str, dict]) -> dict:
    """Every LK ID appears once; PASS needs a PASS entry; missing IDs are NOT_RUN."""
    ordered = [results.get(sid, entry(sid, NOT_RUN, "not reached")) for sid in IDS]
    summary = {status: sum(1 for e in ordered if e["status"] == status) for status in STATUSES}
    if summary[FAIL]:
        verdict, code = "FAIL", 1
    elif summary[PASS] == len(IDS):
        verdict, code = "ALL_SCENARIOS_PASS", 0
    elif summary[ENV_BLOCKED] == len(IDS):
        verdict, code = "ENVIRONMENT_BLOCKED", 3
    else:
        verdict, code = "INCOMPLETE", 0
    return {"schema": SCHEMA, "results": ordered, "summary": summary, "verdict": verdict, "exit_code": code}


# ---------------------------------------------------------------- live session

class LockSession:
    def __init__(self, ctx: base.Ctx, args: argparse.Namespace) -> None:
        self.ctx, self.args = ctx, args
        self.lock: base.Owned | None = None
        self.shell: base.Owned | None = None

    def ensure_lock(self) -> bool:
        if self.lock is None or self.lock.proc.poll() is not None:
            self.lock = self.ctx.spawn("flamewm-lock", [self.args.lock_bin], self.ctx.app_env)
            time.sleep(self.args.settle_seconds)
        return self.lock.proc.poll() is None

    def tree(self, label: str) -> list[dict]:
        result = subprocess.run(["xwininfo", "-display", self.ctx.display, "-root", "-tree"],
                                env=self.ctx.host_env, capture_output=True, text=True, timeout=30, check=False)
        (self.ctx.run / "logs" / f"tree-{label}.txt").write_text(result.stdout + result.stderr, encoding="utf-8")
        return appear.parse_tree(result.stdout) if result.returncode == 0 else []

    def window(self, label: str) -> tuple[dict | None, list[dict]]:
        nodes = self.tree(label)
        return lock_window(nodes, self.args.screen_w, self.args.screen_h), nodes

    def shot(self, name: str) -> tuple[Path | None, list[tuple[int, int, int]] | None]:
        path = self.ctx.run / "shots" / f"{name}.png"
        ok, _ = base.capture(self.ctx, path)
        if not ok or base.Image is None:
            return None, None
        with base.Image.open(path) as image:
            return path, list(image.convert("RGB").getdata())

    def input(self, **kwargs) -> tuple[int, str]:
        kwargs.setdefault("click", None)
        kwargs.setdefault("key", None)
        return base.send_input(self.ctx, **kwargs)

    def crop(self, pixels: list[tuple[int, int, int]], rect: tuple[int, int, int, int]) -> list[tuple[int, int, int]]:
        """Crop (x, y, w, h) from a flat list captured at the screen size."""
        x0, y0, w, h = rect
        width = self.args.screen_w
        return [pixels[y * width + x] for y in range(y0, y0 + h) for x in range(x0, x0 + w)]

    def wm_lock_pids(self) -> list[int]:
        """Lock processes whose parent is the WM: the WM, not the harness, spawns the lock on Super+L and on respawn."""
        if self.ctx.wm is None:
            return []
        result = subprocess.run(["pgrep", "-P", str(self.ctx.wm.pid), "-x", LOCK_BIN_NAME],
                                capture_output=True, text=True, timeout=10, check=False)
        return [int(v) for v in result.stdout.split() if v.isdigit()]

    def wait_for_lock(self, ready) -> tuple[dict | None, list[dict]]:
        """Poll until ready(covering window or None) holds, bounded by LOCK_APPEAR_SECONDS."""
        deadline = time.monotonic() + LOCK_APPEAR_SECONDS
        while True:
            win, nodes = self.window("wait-lock")
            if ready(win) or time.monotonic() >= deadline:
                return win, nodes
            time.sleep(0.25)

    def reap_wm_locks(self) -> None:
        """Stops the WM, then SIGKILLs the WM-started locks: a lock outliving its WM would be left unsupervised."""
        pids = self.wm_lock_pids()
        self.ctx.stop(self.ctx.wm)
        for pid in pids:
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass

    def restart(self, values: dict[str, str]) -> bool:
        """Seed the settings, restart the WM so it serves them, then start a fresh lock instance."""
        seed_settings(self.ctx.run, values)
        self.ctx.stop(self.lock)
        self.lock = None
        if not base.restart_owned_wm(self.ctx, self.args, "flamewm-wm"):
            return False
        return self.ensure_lock()


def seed_settings(run: Path, values: dict[str, str]) -> None:
    # Same locations the appearance runner seeds: FLAMEWM_CONFIG_DIR (HOME/.config) and XDG_CONFIG_HOME.
    for root in (run / "home" / ".config", run / "xdg-config"):
        path = root / "flamewm" / "settings.toml"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(appear.settings_text(values), encoding="ascii")


def text_bands(s: LockSession, pixels: list[tuple[int, int, int]]) -> tuple[bool, bool]:
    """(clock present, username present): bright text inside the clock and username anchor bands."""
    w, h = s.args.screen_w, s.args.screen_h
    anchors = reference_anchors(w, h)
    scale = reference_fit(w, h)[0]
    clock = text_band_counts(pixels, w, h, anchors["clock"], scale)
    username = text_band_counts(pixels, w, h, anchors["username"], scale)
    return clock >= ANCHOR_BLOB_MIN_PIXELS, username >= ANCHOR_BLOB_MIN_PIXELS


def background_strip(s: LockSession, pixels: list[tuple[int, int, int]]) -> list[tuple[int, int, int]]:
    """Top-left corner above the clock band: only background is drawn there at any layout."""
    w, h = s.args.screen_w, s.args.screen_h
    scale, ox, oy = reference_fit(w, h)
    strip_w, strip_h = BACKGROUND_STRIP_REF
    return s.crop(pixels, (0, 0, int(ox + strip_w * scale), int(oy + strip_h * scale)))


def lk01(s: LockSession) -> dict:
    if not s.ensure_lock():
        return entry("LK01", FAIL, "flamewm-lock exited before the lock surface appeared")
    win, _ = s.window("LK01")
    if win is None:
        return entry("LK01", ENV_BLOCKED, "flamewm-lock exposes no native full-screen X surface")
    return entry("LK01", PASS, "", [f"window={win['id']} {win['w']}x{win['h']}+{win['x']}+{win['y']}"],
                 ["xwininfo -root -tree"])


def lk02(s: LockSession) -> dict:
    if not s.ensure_lock():
        return entry("LK02", FAIL, "flamewm-lock not running")
    win, before = s.window("LK02-before")
    if win is None:
        return entry("LK02", NOT_RUN, "no lock surface (LK01 did not observe one)")
    exclude = {win["id"]}
    snapshot_before = tree_signature(before, exclude)
    sent = []
    for label, status in (("Super_L", s.input(key="Super_L")[0]),
                          ("Ctrl+Alt+Right", s.input(chord="Ctrl+Alt+Right")[0])):
        sent.append(f"{label}={status}")
    time.sleep(s.args.settle_seconds)
    win_after, after = s.window("LK02-after")
    unchanged = tree_signature(after, exclude) == snapshot_before
    topmost = win_after is not None and is_topmost(after, win_after["id"])
    ok = unchanged and topmost and s.lock.proc.poll() is None
    reason = "" if ok else (f"tree changed={not unchanged} lock_topmost={topmost} "
                            f"alive={s.lock.proc.poll() is None}")
    return entry("LK02", PASS if ok else FAIL, reason, sent + [f"unchanged={unchanged}", f"topmost={topmost}"],
                 ["xwininfo -root -tree", "flamewm_xtest.py --key/--chord"])


def lk03(s: LockSession) -> dict:
    if not s.ensure_lock():
        return entry("LK03", FAIL, "flamewm-lock not running")
    for key in WRONG_PASSWORD_KEYS:
        s.input(key=key)
    s.input(key="Return")
    if shutil.which("tesseract") is None:
        time.sleep(s.args.settle_seconds)
        return entry("LK03", ENV_BLOCKED, "tesseract unavailable for failure-text OCR")
    # PAM answers after its failure delay and the lock polls the worker once a second: wait for the
    # message with a bounded poll instead of one fixed sleep (a slow PAM stack must not flake the check).
    deadline = time.monotonic() + 12.0
    time.sleep(1.0)
    words: list = []
    path = None
    win, nodes = None, []
    while True:
        win, nodes = s.window("LK03")
        path, _ = s.shot("LK03-failure")
        if path is None:
            return entry("LK03", ENV_BLOCKED, "capture failed")
        # The failure message sits just below the password field (reference layout), never above it.
        field_bottom = anchor_centre(reference_anchors(s.args.screen_w, s.args.screen_h)["password"])[1]
        words = ocr_message_words(path, s.args.screen_w, s.args.screen_h)
        words = [w for w in words if w[2] > field_bottom]
        if failure_text_seen(words) or time.monotonic() > deadline:
            break
        time.sleep(1.0)
    still_locked = win is not None and is_topmost(nodes, win["id"]) and s.lock.proc.poll() is None
    seen = failure_text_seen(words)
    ok = still_locked and seen
    reason = "" if ok else f"still_locked={still_locked} failure_text_seen={seen}"
    return entry("LK03", PASS if ok else FAIL, reason, [str(path), f"ocr_hits={words}"],
                 ["xdotool-style XTEST key x,y,z,Return", "tesseract"])


def lk04(s: LockSession) -> dict:
    if not s.ensure_lock():
        return entry("LK04", FAIL, "flamewm-lock not running")
    os.kill(s.lock.pid, signal.SIGTERM)
    time.sleep(s.args.settle_seconds)
    alive = s.lock.proc.poll() is None
    win, _ = s.window("LK04")
    ok = alive and win is not None
    reason = "" if ok else f"alive={alive} covering_window={win is not None}"
    return entry("LK04", PASS if ok else FAIL, reason, [f"alive={alive}"], ["kill -TERM <lock pid>"])


def lk05(s: LockSession) -> dict:
    if not s.ensure_lock():
        return entry("LK05", FAIL, "flamewm-lock not running")
    path, px = s.shot("LK05-look")
    if px is None:
        return entry("LK05", ENV_BLOCKED, "capture failed")
    w, h = s.args.screen_w, s.args.screen_h
    expected = builtin_background_mean(w, h, strip_box(w, h))
    if expected is None:
        return entry("LK05", ENV_BLOCKED, "PIL or the built-in background image is unavailable")
    strip = background_strip(s, px)
    got = mean_colour(strip)
    ok = background_matches(strip, expected)
    return entry("LK05", PASS if ok else FAIL,
                 "" if ok else f"strip mean {tuple(round(v) for v in got)} != laguna {tuple(round(v) for v in expected)}"
                 f" (tol {BACKGROUND_TOLERANCE})",
                 [str(path), f"strip_mean={tuple(round(v) for v in got)}",
                  f"laguna_mean={tuple(round(v) for v in expected)}"],
                 ["import -window root", "PIL cover-scaled fitz-roy-laguna.webp"])


def lk06(_: LockSession) -> dict:
    return entry("LK06", NOT_RUN, "needs real credentials")


def lk07(s: LockSession) -> dict:
    if not s.restart({"lockBackgroundColor": "#336699"}):
        return entry("LK07", FAIL, "WM or lock did not start with the seeded colour")
    path, px = s.shot("LK07-colour")
    if px is None:
        return entry("LK07", ENV_BLOCKED, "capture failed")
    outside = background_strip(s, px)
    # The default lockBackgroundDimPercent darkens the explicit colour too.
    dimmed = dimmed_colour(CUSTOM_COLOUR)
    colour_ok = colour_dominates(outside, dimmed, CUSTOM_COLOUR_TOLERANCE)
    clock_ok, _ = text_bands(s, px)
    ok = colour_ok and clock_ok
    reason = "" if ok else (f"outside_colour_ok={colour_ok} outside_dominant={appear.dominant_color(outside)} "
                            f"clock_readable={clock_ok}")
    return entry("LK07", PASS if ok else FAIL, reason,
                 [str(path), f"outside_dominant={appear.dominant_color(outside)}"],
                 ["xwininfo -root -tree", "import -window root"])


def lk08(s: LockSession) -> dict:
    if shutil.which("tesseract") is None:
        return entry("LK08", ENV_BLOCKED, "tesseract unavailable for date and AM/PM OCR")
    if not s.restart({}):
        return entry("LK08", FAIL, "WM or lock did not start with default settings")
    base_path, base_px = s.shot("LK08-default")
    if base_px is None:
        return entry("LK08", ENV_BLOCKED, "capture failed")
    base_clock, base_user = text_bands(s, base_px)
    base_date = appear.ocr_word_centres(base_path, date_words(datetime.now()))
    if not (base_clock and base_user and base_date):
        return entry("LK08", ENV_BLOCKED,
                     f"default probe inconclusive: clock={base_clock} username={base_user} date_ocr={base_date}")
    if not s.restart({"lockClockFormat": "12h", "lockShowDate": "false", "lockShowUsername": "false"}):
        return entry("LK08", FAIL, "WM or lock did not start with the seeded clock, date and username settings")
    path, px = s.shot("LK08-12h-no-date-no-user")
    if px is None:
        return entry("LK08", ENV_BLOCKED, "capture failed")
    clock_ok, user_seen = text_bands(s, px)
    ampm = appear.ocr_word_centres(path, {"am", "pm"})
    date_seen = appear.ocr_word_centres(path, date_words(datetime.now()))
    checks = {"clock_present": clock_ok, "ampm_ocr": bool(ampm), "date_absent": not date_seen,
              "username_absent": not user_seen}
    ok = all(checks.values())
    reason = "" if ok else "failed: " + ", ".join(name for name, passed in checks.items() if not passed)
    return entry("LK08", PASS if ok else FAIL, reason,
                 [str(path), f"ampm_ocr={ampm}", f"date_ocr_hits={date_seen}", f"username_seen={user_seen}",
                  f"default_date_ocr={base_date}"],
                 ["import -window root", "tesseract"])


def dimmed_colour(rgb: tuple[int, int, int]) -> tuple[int, int, int]:
    """Colour after the default lockBackgroundDimPercent (document.rs multiplies RGB by 1 - dim/100)."""
    keep = 1.0 - DEFAULT_DIM_PERCENT / 100.0
    return tuple(round(c * keep) for c in rgb)


def lk09(s: LockSession) -> dict:
    if base.Image is None:
        return entry("LK09", ENV_BLOCKED, "PIL unavailable to generate the background image")
    assets = s.ctx.run / "assets"
    assets.mkdir(parents=True, exist_ok=True)
    image_path = assets / "lock-pattern.png"
    canvas = base.Image.new("RGB", (320, 200))
    for box, rgb in PATTERN_QUADRANTS.values():
        canvas.paste(rgb, box)
    canvas.save(image_path, format="PNG")
    if not s.restart({"lockBackgroundImage": str(image_path)}):
        return entry("LK09", FAIL, "WM or lock did not start with the seeded background image")
    path, px = s.shot("LK09-image")
    if px is None:
        return entry("LK09", ENV_BLOCKED, "capture failed")
    w, h = s.args.screen_w, s.args.screen_h
    corners = {"top-left": (20, 20, 160, 120), "top-right": (w - 180, 20, 160, 120),
               "bottom-left": (20, h - 140, 160, 120), "bottom-right": (w - 180, h - 140, 160, 120)}
    seen, failed = [], []
    for name, rect in corners.items():
        # The lock dims the default-blurred background by DEFAULT_DIM_PERCENT; the quadrant colour is dimmed too.
        expected = dimmed_colour(PATTERN_QUADRANTS[name][1])
        dom = appear.dominant_color(s.crop(px, rect))
        seen.append(f"{name}={dom}")
        if not appear.close_enough(dom, expected, PATTERN_TOLERANCE):
            failed.append(f"{name} {dom} != {expected}")
    ok = not failed
    return entry("LK09", PASS if ok else FAIL, "" if ok else "; ".join(failed),
                 [str(path), str(image_path)] + seen, ["import -window root"])


def lk10(s: LockSession) -> dict:
    # Start from a WM and shell with no lock: stop any harness-owned lock left by LK07..LK09.
    s.ctx.stop(s.lock)
    s.lock = None
    if not base.restart_owned_wm(s.ctx, s.args, "flamewm-wm"):
        return entry("LK10", FAIL, "flamewm WM did not become ready")
    if s.shell is None or s.shell.proc.poll() is not None:
        s.shell = s.ctx.spawn("flamewm-shell", [s.args.shell_bin], dict(s.ctx.app_env, XDG_SESSION_TYPE="x11"))
    time.sleep(s.args.settle_seconds)
    if s.shell.proc.poll() is not None:
        return entry("LK10", FAIL, "flamewm-shell exited before the chord")
    before, _ = s.window("LK10-before")
    if before is not None:
        return entry("LK10", FAIL, "a lock window is already mapped before Super+L")
    status, _ = s.input(chord="Super+L")
    win, nodes = s.wait_for_lock(lambda covering: covering is not None)
    if win is None:
        return entry("LK10", FAIL, f"no covering lock window within {LOCK_APPEAR_SECONDS:g}s of Super+L",
                     [f"chord={status}"], ["flamewm_xtest.py --chord Super+L", "xwininfo -root -tree"])
    return entry("LK10", PASS, "", [f"chord={status}", f"window={win['id']} {win['w']}x{win['h']}",
                                    f"wm_lock_pids={s.wm_lock_pids()}"],
                 ["flamewm_xtest.py --chord Super+L", "xwininfo -root -tree"])


def lk11(s: LockSession) -> dict:
    before, _ = s.window("LK11-before")
    pids = s.wm_lock_pids()
    if before is None or not pids:
        return entry("LK11", NOT_RUN, "no WM-started lock to kill (LK10 did not observe one)")
    victim = pids[0]
    os.kill(victim, signal.SIGKILL)
    win, _ = s.wait_for_lock(lambda covering: covering is not None
                             and bool(respawned_pids(s.wm_lock_pids(), victim)))
    respawned = respawned_pids(s.wm_lock_pids(), victim)
    ok = win is not None and bool(respawned)
    reason = "" if ok else (f"no WM-owned respawn within {LOCK_APPEAR_SECONDS:g}s of SIGKILL" if not respawned
                            else "respawned lock did not cover the screen")
    return entry("LK11", PASS if ok else FAIL, reason,
                 [f"killed_pid={victim}", f"respawned_pids={respawned}",
                  f"covering_window={win and win['id']}"], ["kill -KILL <lock pid>", "xwininfo -root -tree"])


def measure_chevron_centre(pixels, width, height, circle, s) -> tuple[float, float] | None:
    """Ink-bbox centre of the white chevron inside the submit circle (inset 5 reference px to skip the rim)."""
    cx, cy, _, _ = circle
    inset = 5 * s
    x0, y0 = int(cx + inset), int(cy + inset)
    x1, y1 = int(cx + circle[2] - inset), int(cy + circle[3] - inset)
    ink = [(x, y) for y in range(y0, y1) for x in range(x0, x1) if min(pixels[y * width + x]) >= CHEVRON_INK_MIN]
    if len(ink) < ANCHOR_BLOB_MIN_PIXELS:
        return None
    xs, ys = [p[0] for p in ink], [p[1] for p in ink]
    return ((min(xs) + max(xs) + 1) / 2, (min(ys) + max(ys) + 1) / 2)


def measure_rim(pixels, width, height, expected, s) -> tuple[bool, str]:
    """Light 1 px rim at the field's top and left edges: the edge row/column must be brighter than the
    background 4 reference px outside it, and the brightest such row/column must sit within 3*s of the box edge."""
    x, y, w, h = expected
    tol = 3 * s
    x0, x1 = int(x + 10 * s), int(x + w - 10 * s)
    y0, y1 = int(y - 2 * s), int(y + 2 * s) + 1
    row = {yy: sum(sum(pixels[yy * width + xx]) for xx in range(x0, x1)) / max(x1 - x0, 1) for yy in range(y0, y1)}
    outside = int(y - 4 * s)
    base = sum(sum(pixels[outside * width + xx]) for xx in range(x0, x1)) / max(x1 - x0, 1)
    top_row = max(row, key=row.get)
    top_ok = row[top_row] - base >= 3 * 3 and abs(top_row + 0.5 - y) <= tol
    col0, col1 = int(x - 2 * s), int(x + 2 * s) + 1
    # The pill ends are semicircles: the straight left edge is the middle of the height only.
    ys0, ys1 = int(y + 0.35 * h), int(y + 0.65 * h)
    col = {xx: sum(sum(pixels[yy * width + xx]) for yy in range(ys0, ys1)) / max(ys1 - ys0, 1) for xx in range(col0, col1)}
    left_outside = int(x - 4 * s)
    base_col = sum(sum(pixels[yy * width + left_outside]) for yy in range(ys0, ys1)) / max(ys1 - ys0, 1)
    left_col = max(col, key=col.get)
    left_ok = col[left_col] - base_col >= 3 * 3 and abs(left_col + 0.5 - x) <= tol
    ok = top_ok and left_ok
    return ok, f"rim: top_row={top_row} (edge {y:.1f}) left_col={left_col} (edge {x:.1f}) tol={tol:.1f} {'ok' if ok else 'OUT'}"


def lk12(s: LockSession) -> dict:
    """Layout anchors on the photo background, the password glass on a flat background (exact two-layer blend),
    and the rim geometry on the photo. The flat background is a solid lockBackgroundColor (dimmed by the lock)."""
    s.reap_wm_locks()  # no WM-started lock may cover the screen while the default lock is measured
    if not s.restart({}):
        return entry("LK12", FAIL, "WM or lock did not start with default settings")
    path, px = s.shot("LK12-reference")
    if px is None:
        return entry("LK12", ENV_BLOCKED, "capture failed")
    w, h = s.args.screen_w, s.args.screen_h
    measured = measure_reference_layout(px, w, h)
    ok, lines = judge_reference_layout(measured, w, h, skip=("password",))  # glass: flat variant below
    anchors = reference_anchors(w, h)
    rim_ok, rim_line = measure_rim(px, w, h, anchors["password"], reference_fit(w, h)[0])
    ok = ok and rim_ok
    lines.append(rim_line)
    circle = anchors["submit"]
    centre = measure_chevron_centre(px, w, h, circle, reference_fit(w, h)[0])
    # Exact at scale 1 (0.5 px); scaled by the output scale otherwise.
    tolerance = max(0.5, 0.5 * reference_fit(w, h)[0])
    if centre is None:
        ok = False
        lines.append("chevron: no ink found in the submit circle")
    else:
        off = max(abs(centre[0] - anchor_centre(circle)[0]), abs(centre[1] - anchor_centre(circle)[1]))
        within = off <= tolerance
        ok = ok and within
        lines.append(f"chevron: ink_centre=({centre[0]:.1f},{centre[1]:.1f}) circle_centre="
                     f"({anchor_centre(circle)[0]:.1f},{anchor_centre(circle)[1]:.1f}) err={off:.2f} "
                     f"tol={tolerance:.2f} {'ok' if within else 'OUT'}")
    # Flat-background variant of the same layout: the glass is measured with the exact two-layer blend.
    if not s.restart({"lockBackgroundColor": FLAT_BACKGROUND}):
        ok = False
        lines.append("password(flat): lock did not start on the flat background")
    else:
        flat_path, flat = s.shot("LK12-flat")
        if flat is None:
            return entry("LK12", ENV_BLOCKED, "capture failed (flat)")
        field = measure_password(flat, w, h, anchors["password"], reference_fit(w, h)[0])
        if field is None:
            ok = False
            lines.append("password(flat): no glass field found")
        else:
            err = anchor_error(field, anchors["password"])
            within = err <= anchor_tolerance(w, h)
            ok = ok and within
            lines.append(f"password(flat): measured={_fmt(field)} expected={_fmt(anchors['password'])} "
                         f"err={err:.1f} tol={anchor_tolerance(w, h):.1f} {'ok' if within else 'OUT'}")
        # Negative control: the same detector over a field-free box must find no glass.
        shifted = (anchors["password"][0] + 300 * reference_fit(w, h)[0], anchors["password"][1],
                   anchors["password"][2], anchors["password"][3])
        if measure_password(flat, w, h, shifted, reference_fit(w, h)[0]) is not None:
            ok = False
            lines.append("password(flat): negative control found glass where there is none")
    return entry("LK12", PASS if ok else FAIL, "" if ok else "; ".join(l for l in lines if not l.endswith(" ok")),
                 [str(path)] + lines, ["import -window root", "measure_reference_layout", "measure_password(flat)"])


def start_control_plane(s: LockSession, label: str, caps: str) -> tuple[Path, base.Owned | None]:
    """Fake control plane owning com.arkflame.FlameWM1 on the private bus; None if it never became ready."""
    run = s.ctx.run / "control" / label
    run.mkdir(parents=True, exist_ok=True)
    report, ready = run / "session-calls.jsonl", run / "ready"
    report.unlink(missing_ok=True)
    ready.unlink(missing_ok=True)
    fake = s.ctx.spawn("fake-control-plane", [FAKE_PYTHON, "-I", str(FAKE_CONTROL_BIN), "--report", str(report),
                                              "--caps", caps, "--ready", str(ready)], s.ctx.app_env)
    deadline = time.monotonic() + 10.0
    while not ready.exists() and fake.proc.poll() is None and time.monotonic() < deadline:
        time.sleep(0.2)
    return report, (fake if ready.exists() else None)


def session_calls(report: Path) -> list[str]:
    if not report.exists():
        return []
    return [json.loads(line)["method"] for line in report.read_text(encoding="utf-8").splitlines() if line.strip()]


def wait_for_call(report: Path, method: str, seconds: float) -> bool:
    deadline = time.monotonic() + seconds
    while method not in session_calls(report):
        if time.monotonic() >= deadline:
            return False
        time.sleep(0.2)
    return True


def button_rect_px(width: int, height: int, ref_rect: tuple[float, float, float, float]) -> tuple[int, int, int, int]:
    """A reference-canvas rect (layout.rs) in screen pixels, as an OCR box."""
    scale, ox, oy = reference_fit(width, height)
    x, y, w, h = ref_rect
    return (int(ox + x * scale), int(oy + y * scale), int(ox + (x + w) * scale), int(oy + (y + h) * scale))


MESSAGE_REF = (271.0, 278.0, 138.0, 14.0)         # layout.rs lock-message: under the password field


def ocr_message_words(png: Path, width: int, height: int) -> list[tuple[str, int, int]]:
    """Words of the status line, OCR'd alone at 4x in single-line mode; centres in screen pixels."""
    tesseract = shutil.which("tesseract")
    if not tesseract or base.Image is None:
        return []
    box = button_rect_px(width, height, MESSAGE_REF)
    x0, y0, x1, y1 = box
    crop = png.with_name(png.stem + "-message.png")
    with base.Image.open(png) as image:
        image.convert("L").crop(box).resize(((x1 - x0) * 4, (y1 - y0) * 4)).save(crop)
    result = subprocess.run([tesseract, str(crop), "stdout", "--psm", "7"], capture_output=True,
                            text=True, timeout=60, check=False)
    centre_y = (y0 + y1) // 2
    return [(tok.strip(".,").lower(), (x0 + x1) // 2, centre_y) for tok in result.stdout.split() if tok.strip(".,")]


CONFIRM_OK_REF = (350.0, 395.0, 66.0, 16.0)       # layout.rs CONFIRM_OK
CONFIRM_CANCEL_REF = (425.0, 395.0, 56.0, 16.0)   # layout.rs CONFIRM_CANCEL


def ocr_button(png: Path, box: tuple[int, int, int, int]) -> str:
    """Text of one button: upscaled, single-line mode (tesseract misses labels in block mode)."""
    tesseract = shutil.which("tesseract")
    if not tesseract or base.Image is None:
        return ""
    scale = 4
    x0, y0, x1, y1 = box
    crop = png.with_name(png.stem + f"-{x0}-{y0}.png")
    with base.Image.open(png) as image:
        image.convert("L").crop(box).resize(((x1 - x0) * scale, (y1 - y0) * scale)).save(crop)
    result = subprocess.run([tesseract, str(crop), "stdout", "--psm", "8"], capture_output=True,
                            text=True, timeout=60, check=False)
    return "".join(ch for ch in result.stdout.lower() if ch.isalpha())


def ocr_strip_words(png: Path, width: int, height: int) -> list[tuple[str, int, int]]:
    """Confirm-strip labels as (word, x, y) at the button centres. Tesseract reads 'Shut Down' as
    'ShutDown' and 'Cancel' as 'cance', so tokens are canonicalised by prefix."""
    found: list[tuple[str, int, int]] = []
    ok_box = button_rect_px(width, height, CONFIRM_OK_REF)
    cancel_box = button_rect_px(width, height, CONFIRM_CANCEL_REF)
    cx, cy = (ok_box[0] + ok_box[2]) // 2, (ok_box[1] + ok_box[3]) // 2
    text = ocr_button(png, ok_box)
    if text.startswith("shut"):
        found += [("shut", cx, cy), ("down", cx, cy)]
    kx, ky = (cancel_box[0] + cancel_box[2]) // 2, (cancel_box[1] + cancel_box[3]) // 2
    if ocr_button(png, cancel_box).startswith("canc"):
        found.append(("cancel", kx, ky))
    return found


def start_lock_without_wm(s: LockSession) -> bool:
    """The lock alone on the private bus: a WM would own com.arkflame.FlameWM1 first."""
    s.reap_wm_locks()
    s.ctx.stop(s.lock)
    s.lock = s.ctx.spawn("flamewm-lock", [s.args.lock_bin], s.ctx.app_env)
    time.sleep(s.args.settle_seconds)
    return s.lock.proc.poll() is None


def lk13(s: LockSession) -> dict:
    if importlib.util.find_spec("dbus") is None or base.Image is None:
        return entry("LK13", ENV_BLOCKED, "dbus-python or PIL unavailable")
    if shutil.which("tesseract") is None:
        return entry("LK13", ENV_BLOCKED, "tesseract unavailable to locate the confirm strip")
    w, h = s.args.screen_w, s.args.screen_h
    anchors = reference_anchors(w, h)
    scale = reference_fit(w, h)[0]

    def centre(name: str) -> str:
        cx, cy = anchor_centre(anchors[name])
        return f"{round(cx)},{round(cy)}"

    def covering() -> bool:
        win, _ = s.window("LK13")
        return win is not None and s.lock.proc.poll() is None

    # Run A: every capability reported.
    report, fake = start_control_plane(s, "all", ",".join(fake_plane.ALL_CAPS))
    if fake is None or not start_lock_without_wm(s):
        return entry("LK13", FAIL, "fake control plane or lock (without WM) did not start")
    enabled_path, enabled_px = s.shot("LK13-enabled")
    if enabled_px is None:
        return entry("LK13", ENV_BLOCKED, "capture failed")
    enabled_peak = icon_contrast(enabled_px, w, h, anchors["power_restart"])
    observed: list[str] = []
    checks: dict[str, bool] = {}

    s.input(click=centre("power_sleep"))
    checks["sleep_suspends"] = wait_for_call(report, "Suspend", SESSION_CALL_SECONDS)
    observed.append(f"after Sleep calls={session_calls(report)}")

    s.input(click=centre("power_shutdown"))
    time.sleep(1.0)
    path, px = s.shot("LK13-confirm")
    words = ocr_strip_words(path, w, h) if path else []
    cancel = next((hit for hit in words if hit[0] == "cancel"), None)
    row = [hit for hit in words if cancel and hit[0] in {"shut", "down"} and abs(hit[2] - cancel[2]) <= 12 * scale]
    checks["confirm_strip_visible"] = cancel is not None and bool(row)
    observed.append(f"confirm OCR={words}")
    checks["shut_down_waits_for_confirm"] = "Shutdown" not in session_calls(report)

    if cancel is not None:
        s.input(click=f"{cancel[1]},{cancel[2]}")
        time.sleep(1.0)
        checks["cancel_records_nothing"] = session_calls(report) == ["Suspend"] and covering()

        s.input(click=centre("power_shutdown"))
        time.sleep(1.0)
        s.input(key="Escape")
        time.sleep(1.0)
        checks["escape_keeps_lock_and_calls_nothing"] = session_calls(report) == ["Suspend"] and covering()

        s.input(click=centre("power_shutdown"))
        time.sleep(1.0)
        again_path, _ = s.shot("LK13-confirm-again")
        again = ocr_strip_words(again_path, w, h) if again_path else []
        target = next((hit for hit in again if abs(hit[2] - cancel[2]) <= 12 * scale), None)
        if target is not None:
            s.input(click=f"{target[1]},{target[2]}")
        checks["confirm_records_shutdown"] = wait_for_call(report, "Shutdown", SESSION_CALL_SECONDS)
        observed.append(f"after confirm calls={session_calls(report)}")
    else:
        observed.append("no Cancel word found: confirm strip not visible")
    s.ctx.stop(fake)

    # Run B: reboot not reported, so Restart is dimmed and must record nothing.
    report_b, fake_b = start_control_plane(s, "no-reboot", "lock,logout,suspend,shutdown")
    if fake_b is None or not start_lock_without_wm(s):
        return entry("LK13", FAIL, "second fake control plane or lock did not start")
    _, dim_px = s.shot("LK13-no-reboot")
    if dim_px is None:
        return entry("LK13", ENV_BLOCKED, "capture failed")
    dim_peak = icon_contrast(dim_px, w, h, anchors["power_restart"])
    checks["restart_dimmed"] = enabled_peak > 0 and dim_peak <= ICON_DIM_RATIO * enabled_peak
    s.input(click=centre("power_restart"))
    time.sleep(SESSION_CALL_SECONDS)
    checks["disabled_restart_records_nothing"] = session_calls(report_b) == []
    observed.append(f"restart contrast enabled={enabled_peak} disabled={dim_peak} "
                    f"calls_b={session_calls(report_b)}")
    s.ctx.stop(fake_b)

    failed = [name for name, passed in checks.items() if not passed]
    reason = "" if not failed else "failed: " + ", ".join(failed) + "; " + "; ".join(observed)
    status = PASS if not failed else FAIL
    return entry("LK13", status, reason, [str(enabled_path), str(path)] + observed + [f"{k}={v}" for k, v in checks.items()],
                 ["fake_control_plane (dbus-python, private bus)", "flamewm_xtest.py --click/--key",
                  "tesseract", "xwininfo -root -tree"])


def xset_query(s: LockSession) -> str:
    result = subprocess.run(["xset", "-display", s.ctx.display, "q"], env=s.ctx.host_env,
                            capture_output=True, text=True, timeout=15, check=False)
    return result.stdout if result.returncode == 0 else ""


def wait_for_ribbons(s: LockSession, deadline: float) -> tuple[bool, Path | None]:
    """Polls two frames a short gap apart until the background changes: the Ribbons are running."""
    while time.monotonic() < deadline:
        first_path, first = s.shot("saver-frame-a")
        time.sleep(SAVER_FRAME_GAP)
        second_path, second = s.shot("saver-frame-b")
        if first is not None and second is not None and frames_differ(first, second):
            return True, second_path
        time.sleep(1.0)
    return False, None


def lk14(s: LockSession) -> dict:
    if not s.restart({"screensaverAfterLockMinutes": "1", "screensaverDisplayOffMinutes": "0"}):
        return entry("LK14", FAIL, "WM or lock did not start with the screensaver settings")
    w, h = s.args.screen_w, s.args.screen_h
    started = time.monotonic()
    time.sleep(max(0.0, SAVER_IDLE_SECONDS - (time.monotonic() - started)))
    active, frame_path = wait_for_ribbons(s, started + SAVER_POLL_SECONDS)
    if not active:
        return entry("LK14", FAIL, "Ribbons never became active after one idle minute")
    _, before = s.shot("LK14-ribbons")
    if before is None:
        return entry("LK14", ENV_BLOCKED, "capture failed")
    s.input(key="x")  # the wake key: consumed, never typed
    time.sleep(1.5)
    wake_path, after_wake = s.shot("LK14-after-wake")
    s.input(key="y")  # the next key is an ordinary one
    time.sleep(1.5)
    typed_path, typed = s.shot("LK14-after-typing")
    if after_wake is None or typed is None:
        return entry("LK14", ENV_BLOCKED, "capture failed")
    caret_before = caret_white_fraction(before, w, h)
    caret_after_wake = caret_white_fraction(after_wake, w, h)
    caret_typed = caret_white_fraction(typed, w, h)
    checks = {
        "caret_visible_before_wake": caret_before >= CARET_VISIBLE,
        "wake_key_consumed": caret_after_wake >= CARET_VISIBLE,
        "next_key_typed": caret_typed <= CARET_HIDDEN,
    }
    failed = [name for name, passed in checks.items() if not passed]
    reason = "" if not failed else "failed: " + ", ".join(failed)
    observed = [f"caret dark fraction before={caret_before:.2f} after_wake={caret_after_wake:.2f} "
                f"after_y={caret_typed:.2f}"]
    return entry("LK14", PASS if not failed else FAIL, reason,
                 [str(frame_path), str(wake_path), str(typed_path)] + observed
                 + [f"{k}={v}" for k, v in checks.items()],
                 ["flamewm_xtest.py --key x", "flamewm_xtest.py --key y", "caret rectangle from layout"])


def grab_status(display: str) -> tuple[int, int] | None:
    """Pointer and keyboard grab results from a separate X client; success is taken and released at once."""
    import ctypes
    import ctypes.util

    x11 = ctypes.CDLL(ctypes.util.find_library("X11") or "libX11.so.6")
    x11.XOpenDisplay.restype = ctypes.c_void_p
    x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x11.XDefaultRootWindow.restype = ctypes.c_ulong
    x11.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
    x11.XGrabPointer.restype = ctypes.c_int
    x11.XGrabPointer.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_uint,
                                 ctypes.c_int, ctypes.c_int, ctypes.c_ulong, ctypes.c_ulong, ctypes.c_ulong]
    x11.XGrabKeyboard.restype = ctypes.c_int
    x11.XGrabKeyboard.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_int, ctypes.c_int,
                                  ctypes.c_ulong]
    dpy = x11.XOpenDisplay(display.encode())
    if not dpy:
        return None
    root = x11.XDefaultRootWindow(dpy)
    pointer = x11.XGrabPointer(dpy, root, 0, 0, 1, 1, 0, 0, 0)
    if pointer == 0:
        x11.XUngrabPointer(dpy, 0)
    keyboard = x11.XGrabKeyboard(dpy, root, 0, 1, 1, 0)
    if keyboard == 0:
        x11.XUngrabKeyboard(dpy, 0)
    x11.XFlush(dpy)
    x11.XCloseDisplay(dpy)
    return pointer, keyboard


def stop_display_processes(display: str, name: str) -> list[int]:
    """SIGKILLs processes named `name` whose environment DISPLAY is this private display (other displays untouched)."""
    stopped = []
    for entry_name in os.listdir("/proc"):
        if not entry_name.isdigit():
            continue
        try:
            with open(f"/proc/{entry_name}/comm", encoding="utf-8") as handle:
                comm = handle.read().strip()
            with open(f"/proc/{entry_name}/environ", "rb") as handle:
                env = handle.read().split(b"\0")
        except OSError:
            continue
        if comm == name and f"DISPLAY={display}".encode() in env:
            try:
                os.kill(int(entry_name), signal.SIGKILL)
                stopped.append(int(entry_name))
            except ProcessLookupError:
                pass
    return stopped


def display_process_names(display: str) -> list[str]:
    names = []
    for entry_name in os.listdir("/proc"):
        if not entry_name.isdigit():
            continue
        try:
            with open(f"/proc/{entry_name}/environ", "rb") as handle:
                env = handle.read().split(b"\0")
            with open(f"/proc/{entry_name}/comm", encoding="utf-8") as handle:
                names.append(f"{entry_name}:{handle.read().strip()}")
        except OSError:
            continue
        if f"DISPLAY={display}".encode() not in env:
            names.pop()
    return names


def lk16(s: LockSession) -> dict:
    """Preview: covers the screen, holds no grab, and exits 0 on motion after the grace. Negative control:
    the normal lock refuses another client's keyboard grab. The WM is stopped for the grab probes, because
    its own root grabs (probed first, as the baseline) would make every probe fail regardless of the lock."""
    if not base.restart_owned_wm(s.ctx, s.args, "flamewm-wm"):
        return entry("LK16", FAIL, "flamewm WM did not start for the baseline probe")
    wm_baseline = grab_status(s.ctx.display)
    s.reap_wm_locks()  # stops the WM and any WM-started lock
    shells = stop_display_processes(s.ctx.display, "flamewm-shell")  # WM children survive the WM; they hold grabs
    locks = stop_display_processes(s.ctx.display, LOCK_BIN_NAME)  # a WM-started lock would hold the grabs
    time.sleep(0.5)
    left = display_process_names(s.ctx.display)
    preview =     preview = s.ctx.spawn("flamewm-lock-preview", [s.args.lock_bin, "--preview"], s.ctx.app_env)
    time.sleep(s.args.settle_seconds)
    win, _ = s.window("LK16-preview")
    path, _ = s.shot("LK16-preview")
    covers = win is not None
    preview_grabs = grab_status(s.ctx.display)
    # Motion is any pointer change; it is sent after the 400 ms grace has passed.
    s.input(move=f"{s.args.screen_w // 3},{s.args.screen_h // 3}")
    exited_at = None
    deadline = time.monotonic() + 3.0
    while time.monotonic() < deadline:
        if preview.proc.poll() is not None:
            exited_at = preview.proc.returncode
            break
        time.sleep(0.1)
    if preview.proc.poll() is None:
        s.ctx.stop(preview)
    normal = s.ctx.spawn("flamewm-lock", [s.args.lock_bin], s.ctx.app_env)
    time.sleep(s.args.settle_seconds)
    normal_grabs = grab_status(s.ctx.display)
    s.ctx.stop(normal)
    ok_preview = covers and preview_grabs == (0, 0) and exited_at == 0
    ok_control = normal_grabs is not None and normal_grabs[1] != 0
    ok = ok_preview and ok_control
    reason = "" if ok else (f"covers={covers} preview_grabs={preview_grabs} exit={exited_at} "
                            f"normal_grabs={normal_grabs}")
    evidence = [str(path), f"wm_baseline(pointer,keyboard)={wm_baseline}", f"stopped_shells={shells}", f"stopped_locks={locks}", f"left_on_display={left}", f"covers={covers}",
                f"preview_grabs(pointer,keyboard)={preview_grabs}", f"preview_exit={exited_at}",
                f"normal_grabs(pointer,keyboard)={normal_grabs}"]
    return entry("LK16", PASS if ok else FAIL, reason, evidence,
                 ["flamewm-lock --preview", "XTEST move after 400 ms", "XGrabPointer/XGrabKeyboard probe"])


AVATAR_FIXTURE_RGB = (200, 40, 160)


def write_avatar_fixture(path: Path) -> None:
    """Solid fixture colour with a white square off-centre (top-left quadrant), so the crop position is visible."""
    image = base.Image.new("RGB", (240, 240), AVATAR_FIXTURE_RGB)
    image.paste((255, 255, 255), (40, 40, 120, 120))
    image.save(path, "PNG")


def lk17(s: LockSession) -> dict:
    """Default avatar: the ring is drawn and no letter is drawn in it. With avatarImage: the picture fills the
    circle (centre shows the fixture colour), the box corners show the background, and the off-centre mark lands
    at the expected quarter-diameter offset."""
    if not s.restart({}):
        return entry("LK17", FAIL, "WM or lock did not start with default settings")
    w, h = s.args.screen_w, s.args.screen_h
    avatar = reference_anchors(w, h)["avatar"]
    _, default_px = s.shot("LK17-default")
    if default_px is None:
        return entry("LK17", ENV_BLOCKED, "capture failed")
    ring = measure_avatar(default_px, w, h, avatar)
    ring_ok = ring is not None and anchor_error(ring, avatar) <= anchor_tolerance(w, h)
    fixture = s.ctx.run / "avatar-fixture.png"
    write_avatar_fixture(fixture)
    if not s.restart({"avatarImage": str(fixture)}):
        return entry("LK17", FAIL, "lock did not start with the avatar fixture")
    path, px = s.shot("LK17-fixture")
    if px is None:
        return entry("LK17", ENV_BLOCKED, "capture failed")
    cx, cy = anchor_centre(avatar)
    x0, y0, size, _ = avatar
    diameter = 0.8 * size  # AVATAR_PICTURE_SCALE of the ring
    at = lambda x, y: px[int(round(y)) * w + int(round(x))]
    centre_ok = appear.close_enough(at(cx, cy), AVATAR_FIXTURE_RGB, 30)
    corner_ok = not appear.close_enough(at(x0 + 3, y0 + 3), AVATAR_FIXTURE_RGB, 30)
    mark_ok = appear.close_enough(at(cx - 0.25 * diameter, cy - 0.25 * diameter), (255, 255, 255), 40)
    ok = ring_ok and centre_ok and corner_ok and mark_ok
    reason = "" if ok else (f"ring_ok={ring_ok} centre_ok={centre_ok} corner_ok={corner_ok} mark_ok={mark_ok}")
    return entry("LK17", PASS if ok else FAIL, reason,
                 [str(path), f"ring={ring}", f"expected_avatar={_fmt(avatar)}"],
                 ["avatarImage=<generated 240x240 PNG>", "pixel probes in the avatar box"])


def lk15(s: LockSession) -> dict:
    if shutil.which("xset") is None:
        return entry("LK15", ENV_BLOCKED, "xset unavailable to query DPMS")
    if not s.restart({"screensaverAfterLockMinutes": "1", "screensaverDisplayOffMinutes": "1"}):
        return entry("LK15", FAIL, "WM or lock did not start with the display-off settings")
    started = time.monotonic()
    time.sleep(max(0.0, SAVER_IDLE_SECONDS - (time.monotonic() - started)))
    active, frame_path = wait_for_ribbons(s, started + SAVER_POLL_SECONDS)
    query_ribbons = xset_query(s)
    if dpms_monitor_state(query_ribbons) is None:
        return entry("LK15", ENV_BLOCKED, "X server reports no DPMS monitor state")
    off_deadline = time.monotonic() + SAVER_POLL_SECONDS
    state = dpms_monitor_state(query_ribbons)
    while state != "off" and time.monotonic() < off_deadline:
        time.sleep(2.0)
        state = dpms_monitor_state(xset_query(s))
    s.input(key="x")  # a wake key from display-off
    time.sleep(2.0)
    woke = dpms_monitor_state(xset_query(s))
    checks = {
        "ribbons_frames_change": active,
        "display_off_via_dpms": state == "off",
        "wake_turns_display_on": woke == "on",
    }
    failed = [name for name, passed in checks.items() if not passed]
    reason = "" if not failed else "failed: " + ", ".join(failed)
    observed = [f"monitor after display-off wait={state} after wake={woke}"]
    return entry("LK15", PASS if not failed else FAIL, reason,
                 [str(frame_path)] + observed + [f"{k}={v}" for k, v in checks.items()],
                 ["xset -display <private> q", "flamewm_xtest.py --key x"])


SCENARIOS = (lk01, lk02, lk03, lk04, lk05, lk06, lk07, lk08, lk09, lk10, lk11, lk12, lk13, lk14, lk15, lk16, lk17)


# ---------------------------------------------------------------- driver

def parse_args(argv: list[str]) -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--run-dir", required=True)
    p.add_argument("--geometry", default="1280x800")
    p.add_argument("--display", default=None)
    p.add_argument("--lock-bin", default=str(LOCK_BIN_DEFAULT))
    p.add_argument("--wm-bin", default=str(WM_BIN_DEFAULT))
    p.add_argument("--shell-bin", default=str(ROOT / ".target/release/flamewm-shell"))
    p.add_argument("--settle-seconds", type=float, default=3.0)
    args = p.parse_args(argv)
    if not re.fullmatch(r"\d+x\d+", args.geometry):
        p.error("--geometry must be WxH")
    args.screen_w, args.screen_h = (int(v) for v in args.geometry.split("x"))
    return args


def write_doc(run: Path, doc: dict) -> None:
    (run / "result.json").write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")


def run_inner(args: argparse.Namespace, run: Path) -> int:
    ctx = base.Ctx(run)
    results: dict[str, dict] = {}
    session: LockSession | None = None
    started = appear.utc_now()
    for sub in ("home", "xdg-config", "xdg-data", "xdg-cache", "xdg-runtime", "shots", "logs"):
        (run / sub).mkdir(parents=True, exist_ok=True)
    try:
        display = base.pick_display(args.display)
        if display is None:
            block(results, "no free display in :90..:99")
        else:
            ctx.display = display
            ctx.build_app_env()
            ctx.token = secrets.token_hex(32)
            ctx.marker.write_text(f"{ctx.display}\n{ctx.token}\n", encoding="ascii")
            os.chmod(ctx.marker, 0o600)
            xephyr = ctx.spawn("xephyr", ["Xephyr", ctx.display, "-screen", args.geometry, "-ac", "-noreset", "-br"],
                               ctx.host_env)
            if not base.wait_for_display(ctx, xephyr):
                block(results, f"Xephyr did not become ready on {ctx.display}")
            elif not base.restart_owned_wm(ctx, args, "flamewm-wm"):
                block(results, "flamewm WM did not become ready (needed for LK02 grab check)")
            else:
                session = LockSession(ctx, args)
                for scenario in SCENARIOS:
                    sid = scenario.__name__.upper()
                    try:
                        results[sid] = scenario(session)
                    except Exception as error:  # a crashed scenario is a FAIL, never a PASS
                        results[sid] = entry(sid, FAIL, f"scenario raised {type(error).__name__}: {error}")
    finally:
        if session is not None:
            session.reap_wm_locks()
        teardown = ctx.teardown()
        doc = compose_document(results)
        doc.update({"run_dir": str(run), "display": ctx.display, "geometry": args.geometry,
                    "lock_bin": str(Path(args.lock_bin).resolve()), "started_utc": started,
                    "finished_utc": appear.utc_now(), "teardown": teardown})
        if not teardown["ok"]:
            doc["verdict"], doc["exit_code"] = "FAIL", 1
        write_doc(run, doc)
    return doc["exit_code"]


def block(results: dict[str, dict], reason: str) -> None:
    for sid in IDS:
        results[sid] = entry(sid, ENV_BLOCKED, reason)


def main(argv: list[str] | None = None) -> int:
    raw = sys.argv[1:] if argv is None else list(argv)
    args = parse_args(raw)
    run = Path(args.run_dir).expanduser().absolute()
    inner = os.environ.get("FLAMEWM_LK_INNER") == "1"
    if inner:
        appear.validate_inner_run_dir(run)
    else:
        appear.validate_run_dir(run)
    missing = [name for name in ("Xephyr", "xwininfo", "import", "gdbus") if shutil.which(name) is None]
    lock_ok = os.access(args.lock_bin, os.X_OK) and Path(args.lock_bin).is_file()
    if missing or not os.environ.get("DISPLAY") or base.Image is None or not lock_ok:
        run.mkdir(parents=True, exist_ok=True)
        reason = "; ".join(filter(None, [
            f"missing tools: {missing}" if missing else "",
            "" if os.environ.get("DISPLAY") else "host DISPLAY unset",
            "" if base.Image is not None else "PIL unavailable",
            "" if lock_ok else f"lock binary missing or not executable: {args.lock_bin}",
        ]))
        results: dict[str, dict] = {}
        block(results, reason)
        write_doc(run, compose_document(results) | {"run_dir": str(run)})
        print(run / "result.json")
        return 3
    if not os.access(args.wm_bin, os.X_OK):
        print(f"error: binary not executable: {args.wm_bin}", file=sys.stderr)
        return 2
    run.mkdir(parents=True, exist_ok=True)
    os.chmod(run, 0o700)
    if inner:
        return run_inner(args, run)
    (run / base.RUN_OWNER_MARKER).write_text(f"owner-pid={os.getpid()}\ncreated={appear.utc_now()}\n", encoding="ascii")
    if shutil.which("dbus-run-session") is None:
        print("error: dbus-run-session not found on PATH", file=sys.stderr)
        return 3
    child = subprocess.Popen(["dbus-run-session", "--", sys.executable, "-I", str(Path(__file__).resolve()), *raw],
                             env=dict(os.environ, FLAMEWM_LK_INNER="1"))
    code = child.wait()
    print(run / "result.json")
    return code


if __name__ == "__main__":
    raise SystemExit(main())

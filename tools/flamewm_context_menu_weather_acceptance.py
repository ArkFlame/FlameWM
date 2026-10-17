#!/usr/bin/env python3
"""Native context-menu and weather-popup acceptance on a private Xephyr (X1, R1..R7, CMP, W1, W2).

Usage:
  tools/flamewm_context_menu_weather_acceptance.py --run-dir DIR [--geometry WxH] [--display :N]
      [--wm-bin PATH] [--shell-bin PATH] [--settings-bin PATH] [--desktop-bin PATH]
      [--client-bin NAME] [--settle-seconds S] [--dry-run]
      [--taskbar-empty-xy X,Y] [--task-xy X,Y] [--pager-xy X,Y] [--desktop-blank-xy X,Y]
      [--file-xy X,Y] [--folder-xy X,Y] [--sticky-xy X,Y] [--weather-xy X,Y]

What it does:
  * creates a fresh private run directory DIR (must not exist or be empty);
  * re-execs itself once under `dbus-run-session` when available;
  * starts an owned Xephyr on a free display in :90-:99 (:97-:99 preferred);
  * seeds the private HOME with ~/Desktop/acceptance-file.txt and ~/Desktop/acceptance-folder,
    a sticky note in XDG_STATE_HOME/sticky-notes.state, and settings with weatherLocation set;
  * starts the owned WM, Shell and desktop (flamewm-desktop), plus one owned client;
  * locates targets by --*-xy overrides or by tesseract OCR of a full-screen capture;
  * sends XTEST input only through tools/flamewm_xtest.py (owner token + marker);
  * counts mapped override-redirect windows that appeared after each right-click
    (`xwininfo -root -tree` + `xwininfo -id`); each menu step must show exactly one;
  * writes DIR/shots/*.png and DIR/result.json. Statuses: PASS, FAIL, NOT_RUN,
    ENVIRONMENT_BLOCKED. A PASS requires a measured window, pixel or OCR observation.

Exit status: 0 when every step PASSes, 1 when any step FAILs, 2 usage error,
3 environment blocked or incomplete (NOT_RUN/ENVIRONMENT_BLOCKED without a FAIL).
"""

from __future__ import annotations

import argparse
import json
import os
import re
import secrets
import shutil
import subprocess
import sys
import time
from collections import Counter
from pathlib import Path

from flamewm_harness_common import write_json

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))

import flamewm_appearance_acceptance as ap  # noqa: E402  (tree parser, settings writer, colour helpers)
import flamewm_settings_acceptance as base  # noqa: E402  (Ctx, capture, owned WM restart, send_input)

ROOT = base.ROOT
SCHEMA = "flamewm-context-menu-weather-acceptance/1"
PASS, FAIL, NOT_RUN, ENV_BLOCKED = base.STATUS_PASS, base.STATUS_FAIL, base.STATUS_NOT_RUN, base.STATUS_ENV_BLOCKED
ROW_HEIGHT = 31                  # menu row height (task requirement)
MENU_PADDING = 5                 # top and bottom container padding (crates/flamewm-ui-core/src/context_menu.rs)
TASKBAR_HEIGHT = ap.TASKBAR_HEIGHT
MENU_SETTLE_SECONDS = 1.0
MENU_PAINT_TIMEOUT_SECONDS = 8.0
MENU_POLL_SECONDS = 0.25
MENU_LABEL_PIXELS = 10
MENU_POINTER_SETTLE_SECONDS = 0.4
MENU_HOVER_SETTLE_SECONDS = 0.6
MENU_BORDER_RGB = (71, 75, 79)   # #474b4f menu frame (ui/shell/menu.css)
MENU_BORDER_TOLERANCE = 6
DESKTOP_MENU_WIDTH = 205         # canonical menu width (crates/flamewm-ui-core/src/context_menu.rs MENU_MIN_WIDTH)
COLOUR_TOLERANCE = 3
MODEL_TOLERANCE = 6              # max per-channel deviation from the shared wallpaper-overlay line
MIN_WALLPAPER_SPREAD = 80        # luminance spread across menus needed to fit the overlay line
WALLPAPER_RELATIVE = "assets/wallpapers/fitz-roy-dawn.webp"  # DEFAULT_WALLPAPER_PATH (flamewm-image-core)
STATE_FILE_NAME = "sticky-notes.state"  # crates/flamewm-desktop-core/src/sticky_persistence.rs
WEATHER_LOCATION = "Berlin"
FILE_NAME, FOLDER_NAME = "acceptance-file.txt", "acceptance-folder"
STICKY_TEXT = "FLAMEWM ACCEPTANCE NOTE"
STICKY_RECT = (520, 260, 240, 160)
DARK_DESKTOP_POINT = (300, 560)  # menu rect 300..505 x 560..725 lies on dark rock in the default wallpaper
OFFLINE_WORDS = {"offline", "unavailable", "unable", "error", "no", "unknown", "choose"}  # real words only, not "--"
STEP_TITLES = {
    "X1": "right-click replaces the previous menu (A gone after B opens)",
    "R1": "right-click empty taskbar",
    "R2": "right-click task entry",
    "R3": "right-click workspace pager",
    "R4": "right-click desktop blank",
    "R5": "right-click desktop file icon",
    "R6": "right-click desktop folder",
    "R7": "right-click sticky note",
    "R8": "right-click desktop blank over dark wallpaper",
    "CMP": "pixel comparison of menu rows across measured menus",
    "W1": "weather tray button opens popup",
    "W2": "weather popup opens offline (placeholder state)",
}
STEP_IDS = tuple(STEP_TITLES)


# ---------------------------------------------------------------- pure helpers (unit-tested)

def xy_arg(text: str) -> tuple[int, int]:
    match = re.fullmatch(r"(\d+),(\d+)", text)
    if not match:
        raise argparse.ArgumentTypeError(f"expected X,Y non-negative integers, got {text!r}")
    return int(match.group(1)), int(match.group(2))


def entry(sid: str, status: str, reason: str = "", evidence: dict | None = None) -> dict:
    return {"id": sid, "title": STEP_TITLES.get(sid, sid), "status": status, "reason": reason,
            "evidence": evidence or {}}


def verdict_of(results: dict[str, dict]) -> str:
    statuses = {r["status"] for r in results.values()}
    if FAIL in statuses:
        return FAIL
    if statuses == {PASS}:
        return PASS
    return "INCOMPLETE"


def exit_code_of(verdict: str) -> int:
    return {PASS: 0, FAIL: 1}.get(verdict, 3)


def override_mapped(xwininfo_text: str) -> bool:
    """True when `xwininfo -id` output reports a viewable override-redirect window.

    xwininfo prints lowercase `yes`/`no` for Override Redirect State (not `Yes`), so compare case-insensitively.
    """
    fields = {}
    for line in xwininfo_text.splitlines():
        key, sep, value = line.strip().partition(":")
        if sep:
            fields[key.strip()] = value.strip().lower()
    return fields.get("Override Redirect State") == "yes" and fields.get("Map State") == "isviewable"


def new_windows(before_ids: set[str], now: list[dict]) -> list[dict]:
    return [node for node in now if node["id"] not in before_ids]


def dominant(pixels: list[tuple[int, int, int]]) -> tuple[int, int, int] | None:
    return Counter(pixels).most_common(1)[0][0] if pixels else None


def crop(pixels: list[tuple[int, int, int]], screen_w: int, screen_h: int,
         rect: tuple[int, int, int, int]) -> list[tuple[int, int, int]]:
    x, y, w, h = rect
    x0, y0, x1, y1 = max(0, x), max(0, y), min(screen_w, x + w), min(screen_h, y + h)
    return [pixels[row * screen_w + col] for row in range(y0, y1) for col in range(x0, x1)]


def menu_visual(menu: dict, point: tuple[int, int], pixels, screen_w: int, screen_h: int) -> dict:
    x, y, w, h = menu["x"], menu["y"], menu["w"], menu["h"]
    # Fill from the top padding band: the hovered row (which can be the only row) must not set the fill.
    body = dominant(crop(pixels, screen_w, screen_h, (x + 6, y + 1, w - 12, MENU_PADDING - 2)))
    px, py = point
    inside = x <= px < x + w and y <= py < y + h
    row = (py - y - MENU_PADDING) // ROW_HEIGHT if inside else None
    if row is not None and not 0 <= row < (h - 2 * MENU_PADDING) // ROW_HEIGHT:
        row = None  # pointer on the container padding, not on a row
    hover = dominant(crop(pixels, screen_w, screen_h,
                          (x + 2, y + MENU_PADDING + row * ROW_HEIGHT + 2, w - 4, ROW_HEIGHT - 4))) if row is not None else None
    # 1px frame: sample the left-edge column at mid-height (menu.css border, #474b4f on every menu).
    border = dominant(crop(pixels, screen_w, screen_h, (x, y + h // 2, 1, 1)))
    return {"menu_id": menu["id"], "rect": [x, y, w, h], "background_rgb": list(body) if body else None,
            "border_rgb": list(border) if border else None,
            "rows_exact": (h - 2 * MENU_PADDING) % ROW_HEIGHT == 0, "row_height": ROW_HEIGHT, "hovered_row": row,
            "hover_rgb": list(hover) if hover else None}


def accent_hued(rgb: tuple[int, int, int]) -> bool:
    """True for the FlameWM accent family (#ef4048 red) that must never outline a menu."""
    r, g, b = rgb
    return r - max(g, b) > 60


def close_rgb(a: tuple[int, int, int], b: tuple[int, int, int], tolerance: int) -> bool:
    return max(abs(a[i] - b[i]) for i in range(3)) <= tolerance


def is_frame(pixel: tuple[int, int, int]) -> bool:
    return close_rgb(pixel, MENU_BORDER_RGB, MENU_BORDER_TOLERANCE)


def desktop_menu_rect(pixels, screen_w: int, screen_h: int, point: tuple[int, int]) -> dict | None:
    """Rect of a desktop-hosted menu, anchored top-left at POINT, from its 1px frame (no X window exists)."""
    px, py = point

    def frame_at(x: int, y: int) -> bool:
        return 0 <= x < screen_w and 0 <= y < screen_h and is_frame(pixels[y * screen_w + x])

    def edge_row(y: int) -> bool:  # a horizontal edge crosses three straight columns: one stray pixel is not an edge
        return all(frame_at(x, y) for x in (px + 40, px + 100, px + 150))

    def edge_col(x: int, mid: int) -> bool:
        return all(frame_at(x, y) for y in (mid - 3, mid, mid + 3))

    # Bottom edge in straight columns (corners blend), then the right edge at mid-height (also clear of corners).
    bottom = next((y for y in range(py + 8, min(screen_h, py + 400)) if edge_row(y)), None)
    if bottom is None:
        return None
    # Width is the canonical shell menu width (every menu kind is 205px, ui-core MENU_MIN_WIDTH); the right
    # frame must sit there. Searching for it wrongly locks onto stray frame-coloured pixels inside the menu.
    mid = (py + bottom) // 2
    right = px + DESKTOP_MENU_WIDTH - 1
    if right >= screen_w or not edge_col(right, mid):
        return None
    return {"id": f"desktop@{px},{py}", "x": px, "y": py, "w": DESKTOP_MENU_WIDTH, "h": bottom - py + 1}


def wallpaper_under(rect: dict, screen_w: int, screen_h: int) -> tuple[int, int, int] | None:
    """Mean of the default wallpaper cover-fitted to the screen, under RECT (blur ignored; a model input)."""
    if base.Image is None:
        return None
    path = ROOT / WALLPAPER_RELATIVE
    if not path.is_file():
        return None
    with base.Image.open(path) as source:
        natural = source.convert("RGB")
    nw, nh = natural.size
    if screen_w * nh >= screen_h * nw:
        sw, sh = screen_w, -(-nh * screen_w // nw)
    else:
        sw, sh = -(-nw * screen_h // nh), screen_h
    canvas = base.Image.new("RGB", (screen_w, screen_h))
    canvas.paste(natural.resize((sw, sh)), ((screen_w - sw) // 2, (screen_h - sh) // 2))
    return mean_under(list(canvas.getdata()), screen_w, screen_h, rect)


def fill_band(menu: dict) -> dict:
    """The unhovered top padding band the fill is sampled from; the backdrop it sees is this band's content."""
    return {"x": menu["x"] + 6, "y": menu["y"] + 1, "w": menu["w"] - 12, "h": MENU_PADDING - 2}


def mean_under(pixels, screen_w: int, screen_h: int, rect: dict) -> tuple[int, int, int] | None:
    area = crop(pixels, screen_w, screen_h, (rect["x"], rect["y"], rect["w"], rect["h"]))
    if not area:
        return None
    return tuple(round(sum(p[i] for p in area) / len(area)) for i in range(3))


def fit_line(xs: list[float], ys: list[float]) -> tuple[float, float]:
    mean_x, mean_y = sum(xs) / len(xs), sum(ys) / len(ys)
    sxx = sum((x - mean_x) ** 2 for x in xs)
    slope = sum((x - mean_x) * (y - mean_y) for x, y in zip(xs, ys)) / sxx if sxx else 0.0
    return slope, mean_y - slope * mean_x


def compare_visuals(visuals: list[dict]) -> tuple[str, str]:
    """Every menu is one translucent overlay over the wallpaper beneath it.

    Fill = k * wallpaper_mean + c with one (k, c) shared by all menus: menus on bright and dark wallpaper
    must fall on the same line (MODEL_TOLERANCE). The hovered-row delta (hover - fill) must match too.
    """
    wall = [v for v in visuals if not v.get("content_backed")]
    content = [v for v in visuals if v.get("content_backed")]
    if len(wall) < 3:
        return NOT_RUN, f"fewer than three wallpaper-backed menus measured ({len(wall)}); overlay line not fitted"
    if any(v["background_rgb"] is None or v.get("wallpaper_rgb") is None for v in visuals):
        return NOT_RUN, "menu fill or wallpaper under a menu could not be sampled"
    reasons = []
    lumas = [sum(v["wallpaper_rgb"]) / 3 for v in wall]
    if max(lumas) - min(lumas) < MIN_WALLPAPER_SPREAD:
        return NOT_RUN, "wallpaper under the menus is too uniform to separate the overlay line"
    # One line per channel, fitted on wallpaper-backed menus only; every menu is then scored against it.
    lines = [fit_line([float(v["wallpaper_rgb"][ch]) for v in wall], [float(v["background_rgb"][ch]) for v in wall])
             for ch in range(3)]
    for v in visuals:
        v["model_residual"] = round(max(abs(v["background_rgb"][ch] - (lines[ch][0] * v["wallpaper_rgb"][ch] + lines[ch][1]))
                                        for ch in range(3)), 2)
    worst = max(v["model_residual"] for v in wall)
    off_model = [v["menu_id"] for v in wall if v["model_residual"] > MODEL_TOLERANCE]
    if off_model:
        reasons.append(f"fill differs from the shared wallpaper overlay (> {MODEL_TOLERANCE}) for {off_model}")
    hovered = [v for v in visuals if v["hover_rgb"] is not None]
    if len(hovered) < 3:
        reasons.append(f"only {len(hovered)} menus had a hover measurement; hover not compared")
    elif all(v["hover_rgb"] == v["background_rgb"] for v in hovered):
        reasons.append("hovered row equals the fill for every menu: no hover feedback on pointer motion")
    else:
        # Hover is the same translucent accent layer everywhere: hover = (1 - a) * fill + a * accent,
        # so hover must be one line in fill across menus.
        for ch in range(3):
            xs = [float(v["background_rgb"][ch]) for v in hovered]
            ys = [float(v["hover_rgb"][ch]) for v in hovered]
            slope, intercept = fit_line(xs, ys)
            for v, x, y in zip(hovered, xs, ys):
                v["hover_residual"] = max(v.get("hover_residual", 0.0), round(abs(y - (slope * x + intercept)), 2))
        off_hover = [v["menu_id"] for v in hovered if v["hover_residual"] > MODEL_TOLERANCE]
        if off_hover:
            reasons.append(f"hover differs from the shared hover layer (> {MODEL_TOLERANCE}) for {off_hover}")
    accent = [v["menu_id"] for v in visuals if v.get("border_rgb") and accent_hued(tuple(v["border_rgb"]))]
    if accent:
        reasons.append(f"menu border is accent-coloured for {accent}")
    bad_rows = [f"{v['menu_id']} h={v['rect'][3]}" for v in visuals if not v["rows_exact"]]
    if bad_rows:
        reasons.append(f"menu height is not {MENU_PADDING}px padding plus whole {ROW_HEIGHT}px rows: {bad_rows}")
    if any(r.startswith(("fill differs", "hover differs", "hovered row", "menu border", "menu height"))
           for r in reasons):
        return FAIL, "; ".join(reasons)
    if reasons:
        return NOT_RUN, "; ".join(reasons)
    note = ""
    if content:
        note = (f"; content-backed {[v['menu_id'] for v in content]} measured, not gated: their backdrop blurs "
                f"icons/sticky note, residuals {[v['model_residual'] for v in content]}")
    return PASS, (f"{len(wall)} wallpaper-backed menus fit one overlay line (worst residual {worst:.1f}, "
                  f"tolerance {MODEL_TOLERANCE}); hover on one line; rows {ROW_HEIGHT}px{note}")


def sticky_state_text(output: str) -> str:
    x, y, w, h = STICKY_RECT
    fields = [b"acceptance-note".hex(), "0", output.encode().hex(), str(x), str(y), str(w), str(h),
              STICKY_TEXT.encode().hex(), "255", "246", "170", "0", "0", "0", "14", "0"]
    return "v2\t1\n" + "\t".join(fields) + "\n"


def xrandr_output_name(text: str) -> str | None:
    match = re.search(r"^(\S+) connected", text, re.MULTILINE)
    return match.group(1) if match else None


def ocr_locate(png: Path, predicate, region: tuple[int, int, int, int] | None = None,
               scale: int = 3, invert: bool = False) -> list[tuple[str, int, int]] | None:
    """Words on a full-screen capture whose lower-cased text satisfies PREDICATE; None when OCR is unavailable."""
    tesseract = shutil.which("tesseract")
    if not tesseract or base.Image is None:
        return None
    big = png.with_name(png.stem + "-ocr.png")
    with base.Image.open(png) as image:
        gray = image.convert("L").resize((image.width * scale, image.height * scale))
        # invert + 50% threshold: light text on dark becomes black on white
        (gray.point(lambda v: 255 if 255 - v > 127 else 0) if invert else gray).save(big)
    result = subprocess.run([tesseract, str(big), "stdout", "--psm", "11", "tsv"], capture_output=True,
                            text=True, timeout=120, check=False)
    found = []
    for line in result.stdout.splitlines()[1:]:
        cols = line.split("\t")
        if len(cols) < 12 or not cols[11].strip():
            continue
        text = cols[11].strip().lower()
        if not predicate(text):
            continue
        left, top, width, height = (int(v) for v in cols[6:10])
        cx, cy = (left + width // 2) // scale, (top + height // 2) // scale
        if region is None or (region[0] <= cx < region[0] + region[2] and region[1] <= cy < region[1] + region[3]):
            found.append((text, cx, cy))
    return found


def weather_token(text: str) -> bool:
    return re.fullmatch(r"-?\d{1,3}°[cf]?|-+°?[cf]?", text) is not None


# ---------------------------------------------------------------- session

class Session:
    """Owned Xephyr + WM + Shell + desktop + client; every spawned PID is tracked by base.Ctx."""

    def __init__(self, ctx: base.Ctx, args: argparse.Namespace) -> None:
        self.ctx, self.args = ctx, args
        self.shell: base.Owned | None = None
        self.desktop: base.Owned | None = None
        self.client: base.Owned | None = None
        # --*-xy overrides, keyed by the locate() names used in run_steps; None means OCR locates it.
        self.overrides: dict[str, tuple[int, int] | None] = {
            name: getattr(args, f"{name}_xy", None) for name in ("task", "pager", "file", "folder", "sticky", "weather")}

    def session_env(self, extra: dict[str, str] | None = None) -> dict[str, str]:
        return dict(self.ctx.app_env, XDG_SESSION_TYPE="x11", **(extra or {}))

    def start(self) -> bool:
        if not base.restart_owned_wm(self.ctx, self.args, "flamewm-wm"):
            return False
        self.shell = self.ctx.spawn("flamewm-shell", [self.args.shell_bin], self.session_env())
        self.desktop = self.ctx.spawn("flamewm-desktop", [self.args.desktop_bin], self.session_env())
        self.client = self.ctx.spawn("client", [self.args.client_bin, "-geometry", "300x200+300+200"], self.ctx.app_env)
        time.sleep(self.args.settle_seconds)
        return all(o.proc.poll() is None for o in (self.ctx.wm, self.shell, self.desktop, self.client))

    def restart_shell_offline(self) -> bool:
        """Restart only the Shell with an unreachable HTTP proxy so weather fetches fail."""
        self.ctx.stop(self.shell)
        dead = "http://127.0.0.1:9"
        self.shell = self.ctx.spawn("flamewm-shell-offline", [self.args.shell_bin],
                                    self.session_env({"http_proxy": dead, "https_proxy": dead, "all_proxy": dead,
                                                      "HTTP_PROXY": dead, "HTTPS_PROXY": dead, "ALL_PROXY": dead}))
        time.sleep(self.args.settle_seconds)
        return self.shell.proc.poll() is None

    def xtest(self, *flags: str) -> None:
        argv = [sys.executable, str(ROOT / "tools" / "flamewm_xtest.py"), "--display", self.ctx.display,
                "--owner-token", self.ctx.token, "--owner-marker", str(self.ctx.marker), *flags]
        recorded = [("<owner-token>" if part == self.ctx.token else part) for part in argv]
        result = subprocess.run(argv, env=self.ctx.host_env, capture_output=True, text=True, timeout=60, check=False)
        if result.returncode != 0:
            raise RuntimeError(f"XTEST refused on {self.ctx.display} (exit {result.returncode}): "
                               f"{base.shell(recorded)} {result.stderr.strip()[:200]}")

    def rclick(self, point: tuple[int, int]) -> None:
        # Real pointer motion first: the Shell routes a press by the hover target it tracked from motion.
        self.xtest("--move", f"{point[0]},{point[1]}")
        time.sleep(MENU_POINTER_SETTLE_SECONDS)
        self.xtest("--rclick", f"{point[0]},{point[1]}")

    def click(self, point: tuple[int, int]) -> None:
        self.xtest("--click", f"{point[0]},{point[1]}")

    def escape(self) -> None:
        self.xtest("--key", "Escape")
        time.sleep(0.5)

    def mapped_overrides(self) -> list[dict]:
        result = subprocess.run(["xwininfo", "-display", self.ctx.display, "-root", "-tree"],
                                env=self.ctx.host_env, capture_output=True, text=True, timeout=30, check=False)
        if result.returncode != 0:
            raise RuntimeError(f"xwininfo -root -tree failed: {result.stderr.strip()[:200]}")
        found = []
        for node in ap.parse_tree(result.stdout):
            if node["parent"] is not None or node["w"] <= 1 or node["h"] <= 1:
                continue  # top-level children of the root only
            info = subprocess.run(["xwininfo", "-display", self.ctx.display, "-id", node["id"]],
                                  env=self.ctx.host_env, capture_output=True, text=True, timeout=30, check=False)
            if override_mapped(info.stdout):
                found.append(node)
        return found

    def shot(self, name: str) -> tuple[Path | None, list[tuple[int, int, int]] | None]:
        path = self.ctx.run / "shots" / f"{name}.png"
        ok, _ = base.capture(self.ctx, path)
        if not ok or base.Image is None:
            return None, None
        with base.Image.open(path) as image:
            return path, list(image.convert("RGB").getdata())

    def locate(self, name: str, predicate, region=None) -> tuple[tuple[int, int] | None, str]:
        """Point of the first OCR word matching PREDICATE, or an override; reason explains a miss."""
        override = self.overrides.get(name)
        if override is not None:
            return override, "override"
        path, _ = self.shot(f"locate-{name}")
        if path is None:
            return None, "full-screen capture failed"
        words = ocr_locate(path, predicate, region)
        if words is None:
            return None, "tesseract unavailable; pass the --*-xy override"
        if not words:
            return None, f"no OCR match in the capture ({path.name})"
        _, x, y = words[0]
        return (x, y), f"OCR {words[0][0]!r}"


# ---------------------------------------------------------------- scenarios

def new_after(session: Session, before: set[str]) -> list[dict]:
    time.sleep(MENU_SETTLE_SECONDS)
    return new_windows(before, session.mapped_overrides())


def painted(menu: dict, pixels, screen_w: int, screen_h: int) -> bool:
    """True once the menu rect shows its light row labels: a mapped but unpainted popup has none."""
    body = crop(pixels, screen_w, screen_h, (menu["x"], menu["y"], menu["w"], menu["h"]))
    return sum(1 for r, g, b in body if min(r, g, b) > 150) >= MENU_LABEL_PIXELS


def await_menu(session: Session, sid: str, before: set[str]) -> tuple[list[dict], Path | None, list | None]:
    """Poll until one new override-redirect menu is mapped AND painted, then shoot it.

    Returns the last screenshot even on timeout, so a missing or blank menu is recorded as evidence.
    """
    args = session.args
    deadline = time.monotonic() + MENU_PAINT_TIMEOUT_SECONDS
    while True:
        menus = new_windows(before, session.mapped_overrides())
        path, pixels = session.shot(f"{sid}-menu")
        if pixels is not None and menus and painted(menus[0], pixels, args.screen_w, args.screen_h):
            return menus, path, pixels
        if time.monotonic() >= deadline:
            return menus, path, pixels
        time.sleep(MENU_POLL_SECONDS)


def menu_step(session: Session, sid: str, point: tuple[int, int] | None, where: str,
              desktop_hosted: bool = False) -> tuple[dict, dict | None]:
    """Escape, snapshot, right-click POINT, assert exactly one new mapped override-redirect window.

    `desktop_hosted` menus are drawn inside the FlameWM Desktop window (no child or override window),
    so zero new windows is a pixel-only result: NOT_RUN with the screenshot as evidence, not FAIL.
    """
    if point is None:
        return entry(sid, NOT_RUN, f"{where} not located; {where_hint(sid)}"), None
    args = session.args
    session.escape()
    before = {n["id"] for n in session.mapped_overrides()}
    _, pre_pixels = session.shot(f"{sid}-before")  # what the menu's backdrop blurs: its mean is the model input
    session.rclick(point)
    if desktop_hosted:
        # Drawn inside the FlameWM Desktop window: no X window to count, so measure the frame in pixels.
        time.sleep(MENU_SETTLE_SECONDS)
        path, pixels = session.shot(f"{sid}-menu")
        menus = []
    else:
        menus, path, pixels = await_menu(session, sid, before)
    evidence = {"point": list(point), "new_menu_ids": [m["id"] for m in menus], "screenshot": str(path or "")}
    if desktop_hosted:
        menu = desktop_menu_rect(pixels, args.screen_w, args.screen_h, point) if pixels is not None else None
        if menu is None:
            return entry(sid, NOT_RUN, "desktop menu frame not found in the screenshot (pixel-only)", evidence), None
        evidence["measured_by"] = "desktop frame scan (no X window)"
    else:
        if len(menus) != 1:
            return entry(sid, FAIL, f"expected exactly one mapped override-redirect menu, saw {len(menus)}", evidence), None
        if pixels is None:
            return entry(sid, NOT_RUN, "screenshot unavailable; menu pixels not measured", evidence), None
        menu = menus[0]
    # Real pointer motion onto the first row, then a fresh capture: hover must show without a click.
    row_point = (menu["x"] + menu["w"] // 2, menu["y"] + MENU_PADDING + ROW_HEIGHT // 2)
    # Leave the menu first so the move onto the row is a real enter (the right-click starts inside it).
    session.xtest("--move", f"{row_point[0]},{max(0, menu['y'] - 40)}")
    time.sleep(MENU_POINTER_SETTLE_SECONDS)
    session.xtest("--move", f"{row_point[0]},{row_point[1]}")
    time.sleep(MENU_HOVER_SETTLE_SECONDS)
    hover_path, hover_pixels = session.shot(f"{sid}-hover")
    if hover_pixels is None:
        return entry(sid, NOT_RUN, "hover screenshot unavailable; menu pixels not measured", evidence), None
    visual = menu_visual(menu, row_point, hover_pixels, args.screen_w, args.screen_h)
    # Desktop-hosted menus blur the desktop as drawn (wallpaper, icons, windows): use the pre-menu capture.
    # Taskbar/pager popups blur the wallpaper crop only, so use the wallpaper itself.
    band = fill_band(menu)
    if desktop_hosted and pre_pixels is not None:
        under = mean_under(pre_pixels, args.screen_w, args.screen_h, band)
    else:
        under = wallpaper_under(band, args.screen_w, args.screen_h)
    visual["wallpaper_rgb"] = list(under) if under is not None else None
    visual["backdrop_source"] = "pre-menu capture" if desktop_hosted else "wallpaper"
    evidence.update({"hover_point": list(row_point), "hover_screenshot": str(hover_path or ""), "visual": visual})
    return entry(sid, PASS, f"one menu {menu['id']} at {menu['x']},{menu['y']} {menu['w']}x{menu['h']}",
                 evidence), visual


def where_hint(sid: str) -> str:
    return {"R2": "task entry needs --task-xy", "R3": "pager needs --pager-xy (no OCR anchor)",
            "R5": f"icon label {FILE_NAME!r} not found; pass --file-xy",
            "R6": f"folder label {FOLDER_NAME!r} not found; pass --folder-xy",
            "R7": "sticky note text not found; pass --sticky-xy"}.get(sid, "pass the matching --*-xy override")


def x1_exclusive(session: Session) -> dict:
    point_a = (session.args.screen_w // 2, session.args.screen_h - TASKBAR_HEIGHT // 2)
    point_b = session.args.desktop_blank_point
    before = {n["id"] for n in session.mapped_overrides()}
    session.rclick(point_a)
    menu_a = new_after(session, before)
    if len(menu_a) != 1:
        return entry("X1", FAIL, f"menu A: expected one new menu, saw {len(menu_a)}", {"a_ids": [m["id"] for m in menu_a]})
    id_a = menu_a[0]["id"]
    before = {n["id"] for n in session.mapped_overrides()}
    session.rclick(point_b)
    menu_b = new_after(session, before)
    still_mapped = {n["id"] for n in session.mapped_overrides()}
    shot, _ = session.shot("X1-after-B")
    evidence = {"menu_a": id_a, "menu_b_ids": [m["id"] for m in menu_b], "screenshot": str(shot or "")}
    if id_a in still_mapped:
        return entry("X1", FAIL, f"menu A {id_a} is still mapped after menu B opened", evidence)
    if len(menu_b) > 1:
        return entry("X1", FAIL, f"menu B: expected at most one new menu, saw {len(menu_b)}", evidence)
    if menu_b:
        return entry("X1", PASS, f"menu A {id_a} gone after menu B {menu_b[0]['id']} opened", evidence)
    # Desktop-hosted B: no window to count, so check pixels: A's frame gone, B's frame drawn at its point.
    _, pixels = session.shot("X1-after-B-pixels")
    if pixels is None:
        return entry("X1", NOT_RUN, "screenshot unavailable; menu A/B pixels not checked", evidence)
    args = session.args
    a = menu_a[0]
    a_frame_drawn = is_frame(pixels[(a["y"] + a["h"] // 2) * args.screen_w + a["x"]])
    b = desktop_menu_rect(pixels, args.screen_w, args.screen_h, point_b)
    evidence["desktop_b_rect"] = None if b is None else [b["x"], b["y"], b["w"], b["h"]]
    if a_frame_drawn:
        return entry("X1", FAIL, f"menu A {id_a} frame still drawn after desktop menu B opened", evidence)
    if b is None:
        return entry("X1", NOT_RUN, f"menu A {id_a} frame gone; desktop menu B frame not found (pixel-only)", evidence)
    return entry("X1", PASS, f"menu A {id_a} frame gone; desktop menu B frame drawn at {point_b} "
                 f"({b['w']}x{b['h']}, pixel check)", evidence)


def weather_popup(session: Session, sid: str, point: tuple[int, int] | None) -> dict:
    if point is None:
        return entry(sid, NOT_RUN, "weather tray button not located; pass --weather-xy")
    session.escape()
    before = {n["id"] for n in session.mapped_overrides()}
    session.click(point)
    popups = new_after(session, before)
    path, _ = session.shot(f"{sid}-after-click")
    evidence = {"point": list(point), "popup_ids": [p["id"] for p in popups], "screenshot": str(path or "")}
    if not popups:
        return entry(sid, FAIL, "no new mapped override-redirect window after left-clicking the weather button", evidence)
    popup = popups[0]
    evidence["popup_rect"] = [popup["x"], popup["y"], popup["w"], popup["h"]]
    if path is not None and base.Image is not None:
        out = session.ctx.run / "shots" / f"{sid}-popup.png"
        with base.Image.open(path) as image:
            image.crop((popup["x"], popup["y"], popup["x"] + popup["w"], popup["y"] + popup["h"])).save(out)
        evidence["popup_png"] = str(out)
    session.escape()
    return entry(sid, PASS, f"popup {popup['id']} mapped at {popup['x']},{popup['y']} {popup['w']}x{popup['h']}", evidence)


def offline_text(session: Session, popup_png: str | None) -> tuple[str, str]:
    if not popup_png:
        return NOT_RUN, "popup capture missing; placeholder text not checked"
    # Light text on the dark popup: OCR reads it only inverted (plain 1x/3x garbles "Unknown").
    words = ocr_locate(Path(popup_png), lambda t: t.strip(".,:;!") in OFFLINE_WORDS, invert=True)
    if words is None:
        return NOT_RUN, "tesseract unavailable; placeholder text not checked"
    if not words:
        return FAIL, "popup maps but no offline/placeholder text found by OCR"
    return PASS, f"placeholder text {sorted({w for w, _, _ in words})}"


def run_steps(session: Session, results: dict[str, dict]) -> None:
    args = session.args
    screen_h = args.screen_h
    def guard(sid, fn):
        try:
            results[sid] = fn()
        except Exception as error:  # a crashed step is a FAIL with its reason, never a PASS
            results[sid] = entry(sid, FAIL, f"step raised {type(error).__name__}: {error}")

    guard("X1", lambda: x1_exclusive(session))
    visuals: list[dict] = []
    def menu(sid, point_fn, where, desktop_hosted=False, content_backed=False):
        # content_backed: the menu sits over icons or a sticky note, so its blurred backdrop is not wallpaper alone.
        def run():
            point, how = point_fn()
            result, visual = menu_step(session, sid, point, where, desktop_hosted)
            result["evidence"]["locate"] = how
            if visual is not None:
                visual["content_backed"] = content_backed
                visuals.append(visual)
            return result
        guard(sid, run)

    menu("R1", lambda: (args.taskbar_empty_point, "default taskbar centre"), "taskbar empty area")
    menu("R2", lambda: session.locate("task", lambda t: t == "xclock",
                                      (0, screen_h - TASKBAR_HEIGHT, args.screen_w, TASKBAR_HEIGHT)), "task entry")
    menu("R3", lambda: session.locate("pager", lambda t: False), "workspace pager")
    menu("R4", lambda: (args.desktop_blank_point, "default desktop point"), "desktop blank", desktop_hosted=True)
    menu("R5", lambda: session.locate("file", lambda t: t == FILE_NAME.lower()), "desktop file icon",
         desktop_hosted=True, content_backed=True)
    menu("R6", lambda: session.locate("folder", lambda t: t == FOLDER_NAME), "desktop folder",
         desktop_hosted=True, content_backed=True)
    menu("R7", lambda: session.locate("sticky", lambda t: t == "flamewm"), "sticky note",
         desktop_hosted=True, content_backed=True)
    # Same desktop menu over the darkest wallpaper area used here: its fill must land where the taskbar
    # menus (also over dark wallpaper) do, if both are the same translucent overlay.
    menu("R8", lambda: (DARK_DESKTOP_POINT, "fixed dark-wallpaper point"), "desktop blank over dark wallpaper",
         desktop_hosted=True)

    if len(visuals) >= 2:
        status, reason = compare_visuals(visuals)
        results["CMP"] = entry("CMP", status, reason, {"visuals": visuals})
    else:
        results["CMP"] = entry("CMP", NOT_RUN, f"fewer than two menus measured ({len(visuals)})")

    guard("W1", lambda: weather_popup(session, "W1", session.locate("weather", weather_token, (
        0, screen_h - TASKBAR_HEIGHT, args.screen_w, TASKBAR_HEIGHT))[0]))

    def w2():
        if not session.restart_shell_offline():
            return entry("W2", FAIL, "Shell exited when started with unreachable proxy env")
        point, _ = session.locate("weather", weather_token, (0, screen_h - TASKBAR_HEIGHT, args.screen_w, TASKBAR_HEIGHT))
        result = weather_popup(session, "W2", point)
        if result["status"] != PASS:
            return result
        text_status, text_reason = offline_text(session, result["evidence"].get("popup_png"))
        result["evidence"]["offline_text"] = text_reason
        result["evidence"]["caveat"] = ("offline is simulated by unreachable proxy env; the Shell honouring it "
                                        "is not verified, so network reachability is not measured")
        if text_status != PASS:
            return entry("W2", text_status, text_reason, result["evidence"])
        return entry("W2", PASS, f"popup opened; {text_reason}", result["evidence"])
    guard("W2", w2)


# ---------------------------------------------------------------- run orchestration

def utc_now() -> str:
    return time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())


def parse_args(argv: list[str]) -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--run-dir", required=True)
    p.add_argument("--geometry", default="1280x800")
    p.add_argument("--display", default=None, help="nested display in :97..:119 (XTEST input range)")
    p.add_argument("--wm-bin", default=str(ROOT / ".target/release/flamewm"))
    p.add_argument("--shell-bin", default=str(ROOT / ".target/release/flamewm-shell"))
    p.add_argument("--settings-bin", default=str(ROOT / ".target/release/flamewm-settings"))
    p.add_argument("--desktop-bin", default=str(ROOT / ".target/release/flamewm-desktop"))
    p.add_argument("--client-bin", default="xclock")
    p.add_argument("--settle-seconds", type=float, default=3.0)
    p.add_argument("--dry-run", action="store_true", help="print the step plan and exit; no X11, no processes")
    for flag in ("taskbar-empty", "task", "pager", "desktop-blank", "file", "folder", "sticky", "weather"):
        p.add_argument(f"--{flag}-xy", type=xy_arg, default=None, metavar="X,Y", help=f"override for {flag}")
    args = p.parse_args(argv)
    if not re.fullmatch(r"\d+x\d+", args.geometry):
        p.error("--geometry must be WxH")
    if args.display is not None and not ap.xtest_display_ok(args.display):
        p.error("--display must be in :97..:119 (XTEST input range); other displays cannot receive input")
    args.screen_w, args.screen_h = (int(v) for v in args.geometry.split("x"))
    args.taskbar_empty_point = args.taskbar_empty_xy or (args.screen_w // 2, args.screen_h - TASKBAR_HEIGHT // 2)
    args.desktop_blank_point = args.desktop_blank_xy or (args.screen_w * 3 // 4, args.screen_h // 4)
    return args


def validate_run_dir(run: Path) -> None:
    rel = run.resolve().relative_to(ROOT) if run.resolve().is_relative_to(ROOT) else None
    if rel is not None and any(str(rel).startswith(prefix) for prefix in base.FORBIDDEN_RUN_PREFIXES):
        raise SystemExit(f"error: run dir {run} is under a forbidden prefix")
    if run.exists() and (not run.is_dir() or any(run.iterdir())):
        raise SystemExit(f"error: run dir {run} must not exist or must be empty")


def seed_home(ctx: base.Ctx, output: str | None) -> None:
    desktop = ctx.run / "home" / "Desktop"
    desktop.mkdir(parents=True, exist_ok=True)
    (desktop / FILE_NAME).write_text("acceptance file\n", encoding="ascii")
    (desktop / FOLDER_NAME).mkdir(exist_ok=True)
    if output is not None:
        state = ctx.run / "xdg-state" / STATE_FILE_NAME
        state.parent.mkdir(parents=True, exist_ok=True)
        state.write_text(sticky_state_text(output), encoding="ascii")
    settings = {"taskbarShowWeather": "true", "weatherEnabled": "true", "weatherLocation": WEATHER_LOCATION}
    for root in (ctx.run / "home" / ".config", ctx.run / "xdg-config"):
        path = root / "flamewm" / "settings.toml"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(ap.settings_text(settings), encoding="ascii")


def run_inner(args: argparse.Namespace, run: Path) -> int:
    ctx = base.Ctx(run)
    results: dict[str, dict] = {}
    started = utc_now()
    for sub in ("home", "xdg-config", "xdg-data", "xdg-cache", "xdg-runtime", "xdg-state", "shots", "logs"):
        (run / sub).mkdir(parents=True, exist_ok=True)
    try:
        display = base.pick_display(args.display)
        if display is None or not ap.xtest_display_ok(display):
            block(results, f"no free XTEST-range display (picked {display})")
        else:
            ctx.display = display
            ctx.build_app_env()
            ctx.app_env["XDG_STATE_HOME"] = str(run / "xdg-state")
            ctx.token = secrets.token_hex(32)
            ctx.marker.write_text(f"{ctx.display}\n{ctx.token}\n", encoding="ascii")
            os.chmod(ctx.marker, 0o600)
            xephyr = ctx.spawn("xephyr", ["Xephyr", ctx.display, "-screen", args.geometry, "-ac", "-noreset", "-br"],
                               ctx.host_env)
            if not base.wait_for_display(ctx, xephyr):
                block(results, f"Xephyr did not become ready on {ctx.display}")
            else:
                randr = subprocess.run(["xrandr", "--query"], env=dict(ctx.host_env, DISPLAY=ctx.display),
                                       capture_output=True, text=True, timeout=30, check=False)
                output = xrandr_output_name(randr.stdout) if randr.returncode == 0 else None
                seed_home(ctx, output)
                session = Session(ctx, args)
                if not session.start():
                    block(results, "WM, Shell or desktop did not start or exited during settle")
                else:
                    run_steps(session, results)
    except Exception as error:
        reason = f"harness error {type(error).__name__}: {error}"
        for sid in STEP_IDS:
            results.setdefault(sid, entry(sid, NOT_RUN, reason))
    finally:
        teardown = ctx.teardown()
        for sid in STEP_IDS:
            results.setdefault(sid, entry(sid, NOT_RUN, "step did not run"))
        verdict = verdict_of(results) if teardown["ok"] else FAIL
        doc = {"schema": SCHEMA, "verdict": verdict, "exit_code": exit_code_of(verdict), "steps": results,
               "run_dir": str(run), "display": ctx.display, "geometry": args.geometry,
               "wm_bin": str(Path(args.wm_bin).resolve()), "shell_bin": str(Path(args.shell_bin).resolve()),
               "desktop_bin": str(Path(args.desktop_bin).resolve()), "started_utc": started,
               "finished_utc": utc_now(), "teardown": teardown}
        write_json(run / "result.json", doc)
    return doc["exit_code"]


def block(results: dict[str, dict], reason: str) -> None:
    for sid in STEP_IDS:
        results[sid] = entry(sid, ENV_BLOCKED, reason)


def plan(args: argparse.Namespace) -> dict:
    return {"dry_run": True, "schema": SCHEMA, "geometry": args.geometry,
            "steps": {sid: title for sid, title in STEP_TITLES.items()},
            "defaults": {"taskbar_empty": args.taskbar_empty_point, "desktop_blank": args.desktop_blank_point},
            "binaries": {"wm": args.wm_bin, "shell": args.shell_bin, "desktop": args.desktop_bin,
                         "settings": args.settings_bin},
            "run_dir": args.run_dir}


def main(argv: list[str] | None = None) -> int:
    raw = sys.argv[1:] if argv is None else list(argv)
    args = parse_args(raw)
    if args.dry_run:
        print(json.dumps(plan(args), indent=2))
        return 0
    run = Path(args.run_dir).expanduser().absolute()
    inner = os.environ.get("FLAMEWM_CM_INNER") == "1"
    if not inner:
        validate_run_dir(run)
    missing = [name for name in ("Xephyr", "xwininfo", "xdpyinfo", "import", "identify", "xrandr", "dbus-run-session")
               if shutil.which(name) is None] if not inner else []
    if missing or not os.environ.get("DISPLAY") or base.Image is None:
        run.mkdir(parents=True, exist_ok=True)
        reason = "; ".join(filter(None, [f"missing tools: {missing}" if missing else "",
                                         "" if os.environ.get("DISPLAY") else "host DISPLAY unset",
                                         "" if base.Image is not None else "PIL unavailable"]))
        results: dict[str, dict] = {}
        block(results, reason)
        doc = {"schema": SCHEMA, "verdict": ENV_BLOCKED, "exit_code": 3, "steps": results, "run_dir": str(run)}
        write_json(run / "result.json", doc)
        print(run / "result.json")
        return 3
    for binary in (args.wm_bin, args.shell_bin, args.desktop_bin, args.settings_bin):
        if not os.access(binary, os.X_OK):
            print(f"error: binary not executable: {binary}", file=sys.stderr)
            return 2
    run.mkdir(parents=True, exist_ok=True)
    os.chmod(run, 0o700)
    if inner:
        return run_inner(args, run)
    (run / base.RUN_OWNER_MARKER).write_text(f"owner-pid={os.getpid()}\ncreated={utc_now()}\n", encoding="ascii")
    child = subprocess.Popen(["dbus-run-session", "--", sys.executable, str(Path(__file__).resolve()), *raw],
                             env=dict(os.environ, FLAMEWM_CM_INNER="1"))
    code = child.wait()
    print(run / "result.json")
    return code


if __name__ == "__main__":
    raise SystemExit(main())

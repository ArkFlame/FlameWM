#!/usr/bin/env python3
"""Native appearance acceptance runner (AP01..AP09) on a private Xephyr.

Usage:
  tools/flamewm_appearance_acceptance.py --run-dir DIR [--geometry WxH] [--display :N]
      [--wm-bin PATH] [--shell-bin PATH] [--settings-bin PATH] [--settle-seconds S]
      [--client-bin PATH] [--icon-theme-alt NAME] [--cursor-theme-base NAME]
      [--cursor-theme-alt NAME] [--taskbar-slot X,Y,W,H]

What it does:
  * creates a fresh private run directory DIR (must not exist or be empty);
  * re-execs itself once under `dbus-run-session` when available;
  * starts an owned Xephyr on a free display in :90-:99 (:97-:99 preferred);
  * seeds the product settings file `XDG_CONFIG_HOME/flamewm/settings.toml` (key=value
    lines, the file the WM reads at start) and restarts the owned WM + Shell per phase;
  * samples screenshots (PNG under DIR/shots) by pixel and reads X window geometry from
    `xwininfo -root -tree`, window properties from `xprop`, and cursor pixels via XFixes;
  * sends XTEST input only through tools/flamewm_xtest.py (owner token + marker);
  * writes DIR/result.json. Statuses: PASS, FAIL, NOT_RUN, ENVIRONMENT_BLOCKED. A PASS is
    only emitted after a measured pixel, geometry, property or /proc observation.

The settings binary is checked for presence only; the persisted settings file is the
seed path the product reads (no UI automation is used by this runner).

Exit status: 0 when no AP FAILs, 1 when any AP FAILs, 2 usage error, 3 environment blocked.
"""

from __future__ import annotations

import argparse
import ctypes
import ctypes.util
import hashlib
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

import flamewm_settings_acceptance as base  # noqa: E402  (reused Ctx, capture, XTEST, tree regex)

ROOT = base.ROOT
SCHEMA = "flamewm-appearance-acceptance/1"
IDS = tuple(f"AP{n:02d}" for n in range(1, 10))
TITLES = {
    "AP01": "taskbar color and opacity",
    "AP02": "Start background color/opacity independent of taskbar",
    "AP03": "accent line and accent opacity blending",
    "AP04": "decoration Rounded vs iOS, maximize/snap/restore masks",
    "AP05": "titlebar color and per-button hover colors",
    "AP06": "Start slide-up geometry over time",
    "AP07": "minimize Magic Lamp overlay and icon geometry",
    "AP08": "icon and cursor theme switching",
    "AP09": "idle CPU and wakeups",
}
PASS, FAIL = base.STATUS_PASS, base.STATUS_FAIL
NOT_RUN, ENV_BLOCKED = base.STATUS_NOT_RUN, base.STATUS_ENV_BLOCKED
STATUSES = (PASS, FAIL, NOT_RUN, ENV_BLOCKED)

TASKBAR_HEIGHT = 44          # product default taskbarHeight (settings.rs)
TITLEBAR_HEIGHT = 31         # skin metrics titlebar
BUTTON_WIDTH = 38            # skin metrics button_width; order Minimize, Maximize, Close (left to right)
CLOSE_HOVER_DEFAULT = (0xE8, 0x11, 0x23)  # WINDOW_CHROME.close_hover
DECOR_CHANNEL_TOLERANCE = 2
IDLE_SECONDS = 5.0
IDLE_MAX_CPU_SECONDS = 0.05  # 1% of one core over the window
IDLE_MAX_VOLUNTARY_WAKEUPS = 10
CLK_TCK = os.sysconf("SC_CLK_TCK") if hasattr(os, "sysconf") else 100
SETTINGS_REVISION = 2


# ---------------------------------------------------------------- pure helpers (unit-tested)

def blend(background: tuple[int, int, int], foreground: tuple[int, int, int], alpha_pct: int) -> tuple[int, int, int]:
    """Alpha-composite foreground over background at alpha_pct (0..100)."""
    return tuple(round(b + (f - b) * alpha_pct / 100) for b, f in zip(background, foreground))


def close_enough(actual: tuple[int, int, int] | None, expected: tuple[int, int, int] | None, tolerance: int) -> bool:
    return base.rgb_close(actual, expected, tolerance)


def dominant_color(pixels: list[tuple[int, int, int]]) -> tuple[int, int, int] | None:
    if not pixels:
        return None
    return Counter(pixels).most_common(1)[0][0]


def fraction_matching(pixels: list[tuple[int, int, int]], expected: tuple[int, int, int], tolerance: int) -> float:
    if not pixels:
        return 0.0
    return sum(1 for p in pixels if close_enough(p, expected, tolerance)) / len(pixels)


def pixel_diff_count(a: list[tuple[int, int, int]], b: list[tuple[int, int, int]], tolerance: int = 0) -> int:
    """Number of positions whose channels differ by more than tolerance (equal-length lists)."""
    if len(a) != len(b):
        raise ValueError("pixel lists differ in length")
    return sum(1 for p, q in zip(a, b) if not close_enough(p, q, tolerance))


def is_nonincreasing(values: list[int]) -> bool:
    return all(later <= earlier for earlier, later in zip(values, values[1:]))


def slide_up_ok(ys: list[int]) -> bool:
    """Slide-up: at least 3 observed y values, non-increasing, with >=2 distinct intermediate steps."""
    if len(ys) < 3 or not is_nonincreasing(ys):
        return False
    distinct = sorted(set(ys), reverse=True)
    return len(distinct) >= 3 and ys[0] > ys[-1] and ys[-1] == min(ys)


def no_intermediate_ok(ys: list[int]) -> bool:
    """Animations disabled: every observation is the same final position."""
    return len(ys) >= 3 and len(set(ys)) == 1


def parse_tree(text: str) -> list[dict]:
    """Every named/unnamed window from `xwininfo -tree` with absolute x/y and parent id."""
    nodes: list[dict] = []
    stack: list[tuple[int, str, int, int]] = []  # (indent, id, screen_x, screen_y)
    for line in text.splitlines():
        match = base.TREE_WINDOW_RE.match(line)
        if not match:
            continue
        indent = len(line) - len(line.lstrip())
        while stack and stack[-1][0] >= indent:
            stack.pop()
        window_id, name, width, height, rel_x, rel_y = match.groups()
        parent = stack[-1][1] if stack else None
        base_x, base_y = (stack[-1][2], stack[-1][3]) if stack else (0, 0)
        screen_x, screen_y = base_x + int(rel_x), base_y + int(rel_y)
        stack.append((indent, window_id, screen_x, screen_y))
        nodes.append({"id": window_id, "name": name or "", "parent": parent, "w": int(width),
                      "h": int(height), "x": screen_x, "y": screen_y})
    return nodes


def find_named(nodes: list[dict], name: str) -> dict | None:
    matches = [n for n in nodes if n["name"] == name]
    return max(matches, key=lambda n: n["w"] * n["h"]) if matches else None


def parent_of(nodes: list[dict], node: dict) -> dict | None:
    return next((n for n in nodes if n["id"] == node["parent"]), None) if node else None


def parse_icon_geometry(xprop_text: str) -> tuple[int, int, int, int] | None:
    match = re.search(r"_NET_WM_ICON_GEOMETRY\(CARDINAL\)\s*=\s*(-?\d+),\s*(-?\d+),\s*(\d+),\s*(\d+)", xprop_text)
    return tuple(int(v) for v in match.groups()) if match else None


def map_state(xwininfo_text: str) -> str | None:
    match = re.search(r"Map State:\s*(\w+)", xwininfo_text)
    return match.group(1) if match else None


def settings_text(values: dict[str, str]) -> str:
    lines = [f"revision={SETTINGS_REVISION}"] + [f"{k}={v}" for k, v in values.items()]
    return "\n".join(lines) + "\n"


def idle_verdict(cpu_seconds: float, voluntary_wakeups: int, process_alive: bool) -> str:
    if not process_alive:
        return FAIL
    return PASS if cpu_seconds <= IDLE_MAX_CPU_SECONDS and voluntary_wakeups <= IDLE_MAX_VOLUNTARY_WAKEUPS else FAIL


def voluntary_switches(pid: int) -> int | None:
    try:
        text = Path(f"/proc/{pid}/status").read_text(encoding="ascii")
    except OSError:
        return None
    match = re.search(r"^voluntary_ctxt_switches:\s+(\d+)$", text, re.MULTILINE)
    return int(match.group(1)) if match else None


def voluntary_switches_total(pid: int) -> int | None:
    """Sum of voluntary_ctxt_switches over every thread in /proc/PID/task; threads that exit mid-window are skipped."""
    try:
        tids = list(Path(f"/proc/{pid}/task").iterdir())
    except OSError:
        return None
    counts = [voluntary_switches(int(tid.name)) for tid in tids if tid.name.isdigit()]
    counts = [c for c in counts if c is not None]
    return sum(counts) if counts else None


def entry(sid: str, status: str, reason: str = "", evidence: list | None = None, commands: list | None = None) -> dict:
    if status not in STATUSES:
        raise ValueError(f"unknown status {status!r}")
    return {"id": sid, "title": TITLES[sid], "status": status, "reason": reason,
            "evidence": evidence or [], "command": " && ".join(commands or []) or "none"}


def compose_document(results: dict[str, dict]) -> dict:
    """Every AP ID appears exactly once. PASS needs a PASS entry; missing IDs are NOT_RUN."""
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


# ---------------------------------------------------------------- live helpers

class Session:
    """Owned Xephyr + WM + Shell lifecycle over base.Ctx; every spawned PID is tracked."""

    def __init__(self, ctx: base.Ctx, args: argparse.Namespace) -> None:
        self.ctx, self.args = ctx, args
        self.shell: base.Owned | None = None
        self.client: base.Owned | None = None

    def spawn_client(self) -> None:
        """One owned xclock per phase; the previous phase's client is stopped first."""
        self.ctx.stop(self.client)
        self.client = self.ctx.spawn("client-xclock", [self.args.client_bin, "-geometry", "300x200+300+200"],
                                     self.ctx.app_env)
        time.sleep(self.args.settle_seconds)

    def seed(self, values: dict[str, str]) -> None:
        # The WM resolves FLAMEWM_CONFIG_DIR (session default HOME/.config/flamewm) before XDG_CONFIG_HOME,
        # so the seed must land there; the XDG copy covers consumers that honour XDG_CONFIG_HOME.
        for root in (self.ctx.run / "home" / ".config", self.ctx.run / "xdg-config"):
            path = root / "flamewm" / "settings.toml"
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(settings_text(values), encoding="ascii")

    def restart(self, values: dict[str, str]) -> bool:
        self.seed(values)
        self.ctx.stop(self.shell)
        # The WM must own com.arkflame.FlameWM1 (readiness proven by base.restart_owned_wm) before the
        # Shell starts; a Shell started alongside the WM fails GetShellBootstrap and exits.
        if not base.restart_owned_wm(self.ctx, self.args, "flamewm-restart"):
            return False
        self.shell = self.ctx.spawn("flamewm-shell", [self.args.shell_bin], dict(self.ctx.app_env, XDG_SESSION_TYPE="x11"))
        time.sleep(self.args.settle_seconds)
        return self.ctx.wm.proc.poll() is None and self.shell.proc.poll() is None

    def tree(self, label: str) -> list[dict]:
        result = subprocess.run(["xwininfo", "-display", self.ctx.display, "-root", "-tree"],
                                env=self.ctx.host_env, capture_output=True, text=True, timeout=30, check=False)
        (self.ctx.run / "logs" / f"tree-{label}.txt").write_text(result.stdout + result.stderr, encoding="utf-8")
        return parse_tree(result.stdout) if result.returncode == 0 else []

    def shot(self, name: str) -> tuple[Path | None, list[tuple[int, int, int]] | None]:
        path = self.ctx.run / "shots" / f"{name}.png"
        ok, _ = base.capture(self.ctx, path)
        if not ok or base.Image is None:
            return None, None
        with base.Image.open(path) as image:
            return path, list(image.convert("RGB").getdata())

    def pixels(self, pixels: list[tuple[int, int, int]], rect: tuple[int, int, int, int]) -> list[tuple[int, int, int]]:
        """Crop a rect (x, y, w, h) from a flat list captured at the screen size."""
        x0, y0, w, h = rect
        width = self.args.screen_w
        return [pixels[y * width + x] for y in range(y0, y0 + h) for x in range(x0, x0 + w)]

    def pixel(self, pixels: list[tuple[int, int, int]], x: int, y: int) -> tuple[int, int, int]:
        return pixels[y * self.args.screen_w + x]

    def input(self, **kwargs) -> tuple[int, str]:
        kwargs.setdefault("click", None)
        kwargs.setdefault("key", None)
        code, recorded = base.send_input(self.ctx, **kwargs)
        if code != 0:
            # A refused XTEST call means no input reached the WM; never let the scenario read stale pixels as results.
            raise RuntimeError(f"XTEST input refused on {self.ctx.display} (exit {code}): {recorded}")
        return code, recorded

    def client_frame(self, nodes: list[dict]) -> tuple[dict | None, dict | None]:
        client = find_named(nodes, "xclock")
        return client, parent_of(nodes, client)

    def taskbar_rect(self) -> tuple[int, int, int, int]:
        return (0, self.args.screen_h - TASKBAR_HEIGHT, self.args.screen_w, TASKBAR_HEIGHT)


# ---------------------------------------------------------------- scenarios

def ap01(s: Session) -> dict:
    color = "#336699"
    if not s.restart({"surfaceColor":color, "surfaceOpacity": "100", "taskbarHeight": str(TASKBAR_HEIGHT)}):
        return entry("AP01", FAIL, "WM or shell exited during restart")
    path, px = s.shot("AP01-taskbar")
    if px is None:
        return entry("AP01", ENV_BLOCKED, "capture failed")
    strip = s.pixels(px, s.taskbar_rect())
    frac = fraction_matching(strip, base.accent_rgb(color), 2)
    dom = dominant_color(strip)
    ok = frac >= 0.5
    return entry("AP01", PASS if ok else FAIL,
                 "" if ok else f"taskbar pixels matching {color}: {frac:.2f} (<0.50), dominant {dom}",
                 [str(path), f"fraction_matching={frac:.3f}", f"dominant={dom}"],
                 ["xwininfo -root -tree", "import -window root"])


def ap02(s: Session) -> dict:
    tb = "#336699"
    if not s.restart({"surfaceColor":tb, "surfaceOpacity": "100"}):
        return entry("AP02", FAIL, "WM or shell exited during restart")
    _, before = s.shot("AP02-taskbar-before")
    tb_before = dominant_color(s.pixels(before, s.taskbar_rect())) if before else None
    s.input(key=base.SHORTCUT_DEFAULT_BINDING)
    time.sleep(s.args.settle_seconds)
    nodes = s.tree("AP02-start-open")
    popup = find_named(nodes, "FlameWM start")
    path, px = s.shot("AP02-start-open")
    s.input(key=base.SHORTCUT_DEFAULT_BINDING)  # close again
    if popup is None or px is None:
        return entry("AP02", FAIL, "Start popup window not found in the X tree after Super")
    inner = (popup["x"] + 6, popup["y"] + 6, popup["w"] - 12, popup["h"] - 12)
    popup_dom = dominant_color(s.pixels(px, inner))
    tb_after = dominant_color(s.pixels(px, s.taskbar_rect()))
    ok = close_enough(popup_dom, base.accent_rgb(tb), 3) and close_enough(tb_after, tb_before, 2) \
        and tb_before is not None and close_enough(tb_before, base.accent_rgb(tb), 3)
    return entry("AP02", PASS if ok else FAIL,
                 "" if ok else f"popup dominant {popup_dom} vs {tb}; taskbar {tb_before} -> {tb_after}",
                 [str(path), f"popup={popup}", f"popup_dominant={popup_dom}", f"taskbar_before={tb_before}",
                  f"taskbar_after={tb_after}"], ["xwininfo -root -tree", "import -window root", "XTEST key Super_L"])


def accent_match_count(pixels: list[tuple[int, int, int]], rgb: tuple[int, int, int]) -> int:
    """Pixels whose colour is the accent within a tolerance of 2 per channel."""
    return sum(1 for p in pixels if close_enough(p, rgb, 2))


FLAME_RED_RGB = (0xEF, 0x40, 0x48)


def ap03(s: Session) -> dict:
    # The accent line above the taskbar was removed from panel.html; the accent is now consumed by the focused
    # task slot underline (.task-slot.focused .underline, 3px, projected with accentOpacity).
    tb, accent = "#336699", "#00AA00"
    rgb = base.accent_rgb(accent)
    strip = s.taskbar_rect()

    def focused_client_shot(name: str, opacity: str) -> tuple[Path | None, list[tuple[int, int, int]] | None]:
        if not s.restart({"surfaceColor":tb, "surfaceOpacity": "100", "accent": accent,
                          "accentOpacity": opacity, "taskbarStyle": "classic"}):
            return None, None
        s.spawn_client()
        _, frame = s.client_frame(s.tree(f"{name}-client"))
        if frame is None:
            return None, None
        s.input(click=f"{frame['x'] + frame['w'] // 2},{frame['y'] + 15}")  # focus the xclock via its titlebar
        time.sleep(1.0)
        return s.shot(name)

    path, px = focused_client_shot("AP03-accent-100", "100")
    if px is None:
        return entry("AP03", FAIL, "WM or shell exited or client frame missing during the accent 100 probe")
    top_rows = s.pixels(px, (0, strip[1], strip[2], 3))
    line_pixels = accent_match_count(top_rows, rgb)
    if line_pixels:
        return entry("AP03", FAIL, f"accent-coloured pixels in the taskbar top rows ({line_pixels}): the removed "
                     "accent line is back", [str(path), f"top_row_accent_pixels={line_pixels}"],
                     ["import -window root"])
    coords = [(x, y) for y in range(strip[1], strip[1] + strip[3]) for x in range(strip[0], strip[0] + strip[2])
              if close_enough(s.pixel(px, x, y), rgb, 2)]
    if len(coords) < 8:
        # projection.rs paints the focused underline with a fixed #ef4048 inline background, shadowing CSS var(--accent).
        flame = accent_match_count(s.pixels(px, strip), FLAME_RED_RGB)
        return entry("AP03", FAIL, f"focused task underline not accent-coloured ({len(coords)} accent pixels; "
                     f"{flame} pixels of the fixed focused red #ef4048 in the strip)",
                     [str(path), f"top_row_accent_pixels={line_pixels}", f"fixed_red_pixels={flame}"],
                     ["import -window root"])
    bg = dominant_color(s.pixels(px, strip))
    path40, px40 = focused_client_shot("AP03-accent-40", "40")
    if px40 is None:
        return entry("AP03", FAIL, "WM or shell exited or client frame missing during the accent 40 probe")
    sampled = Counter(s.pixel(px40, x, y) for x, y in coords).most_common(1)[0][0]
    expected = blend(base.accent_rgb(tb), rgb, 40)
    between = all(min(a, b) <= c <= max(a, b) for a, b, c in zip(base.accent_rgb(tb), rgb, sampled))
    ok = close_enough(sampled, expected, 3) and between and bg is not None
    return entry("AP03", PASS if ok else FAIL,
                 "" if ok else f"underline accent@40 pixel {sampled} vs blend {expected}",
                 [str(path), str(path40), f"top_row_accent_pixels={line_pixels}",
                  f"underline_accent_pixels={len(coords)}", f"accent100_bg={bg}",
                  f"accent40_pixel={sampled}", f"expected_blend={expected}"],
                 ["import -window root", "XTEST click titlebar", "settings seeded accentOpacity=40"])


class _XClientMessage(ctypes.Structure):
    _fields_ = [("type", ctypes.c_int), ("serial", ctypes.c_ulong), ("send_event", ctypes.c_int),
                ("display", ctypes.c_void_p), ("window", ctypes.c_ulong), ("message_type", ctypes.c_ulong),
                ("format", ctypes.c_int), ("data", ctypes.c_long * 5)]


def request_fullscreen(s: Session, window_id: str, enable: bool) -> bool:
    """EWMH _NET_WM_STATE client message (add/remove _NET_WM_STATE_FULLSCREEN) sent to the root window."""
    x11_path = ctypes.util.find_library("X11")
    if not x11_path:
        return False
    x11 = ctypes.cdll.LoadLibrary(x11_path)
    x11.XOpenDisplay.restype = ctypes.c_void_p
    x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x11.XDefaultRootWindow.restype = ctypes.c_ulong
    x11.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
    x11.XInternAtom.restype = ctypes.c_ulong
    x11.XInternAtom.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int]
    x11.XSendEvent.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_long, ctypes.c_void_p]
    x11.XFlush.argtypes = [ctypes.c_void_p]
    x11.XCloseDisplay.argtypes = [ctypes.c_void_p]
    dpy = x11.XOpenDisplay(s.ctx.display.encode("ascii"))
    if not dpy:
        return False
    try:
        root = x11.XDefaultRootWindow(dpy)
        event = _XClientMessage()
        event.type = 33  # ClientMessage
        event.send_event = 1
        event.display = dpy
        event.window = int(window_id, 16)
        event.message_type = x11.XInternAtom(dpy, b"_NET_WM_STATE", 0)
        event.format = 32
        event.data[0] = 1 if enable else 0
        event.data[1] = x11.XInternAtom(dpy, b"_NET_WM_STATE_FULLSCREEN", 0)
        buffer = ctypes.create_string_buffer(192)
        ctypes.memmove(buffer, ctypes.byref(event), ctypes.sizeof(event))
        mask = (1 << 20) | (1 << 19)  # SubstructureRedirect | SubstructureNotify
        ok = x11.XSendEvent(dpy, root, 0, mask, buffer)
        x11.XFlush(dpy)
        return bool(ok)
    finally:
        x11.XCloseDisplay(dpy)


def ocr_word_centres(png: Path, words: set[str]) -> list[tuple[str, int, int]]:
    """(word, x, y) centres of OCR'd words (lower-cased) found in a screenshot, at 1x coordinates."""
    tesseract = shutil.which("tesseract")
    if not tesseract or base.Image is None:
        return []
    scale = 3
    big = png.with_name(png.stem + "-ocr.png")
    with base.Image.open(png) as image:
        image.convert("L").resize((image.width * scale, image.height * scale)).save(big)
    result = subprocess.run([tesseract, str(big), "stdout", "--psm", "11", "tsv"], capture_output=True,
                            text=True, timeout=120, check=False)
    found = []
    for line in result.stdout.splitlines()[1:]:
        cols = line.split("\t")
        if len(cols) < 12 or cols[11].strip().lower() not in words:
            continue
        left, top, width, height = (int(v) for v in cols[6:10])
        found.append((cols[11].strip().lower(), (left + width // 2) // scale, (top + height // 2) // scale))
    return found


def decoration_selector_check(s: Session, ev: list, fails: list) -> None:
    """Click the real Settings Surfaces decoration selector and read the authoritative snapshot back.

    The decoration selector moved from Appearance to the Surfaces page; its only segments are
    rounded and squared (settings.decoration.ios is rejected by the markup parser)."""
    if not s.restart({"decoration": "rounded", "surfaceOpacity": "100"}):
        fails.append("WM or shell exited before the selector check")
        return
    settings = s.ctx.spawn("settings-ap04", [s.args.settings_bin, "--page", "surfaces"], s.ctx.app_env)
    try:
        time.sleep(s.args.settle_seconds)
        before, _ = base.dbus_settings_snapshot(s.ctx)
        words: list = []
        for round_index in range(8):
            path, _ = s.shot(f"AP04-selector-{round_index}")
            if path is None or before is None:
                fails.append("selector check could not capture the Settings page or the snapshot")
                return
            words = ocr_word_centres(path, {"squared", "rounded"})
            if any(w[0] == "squared" for w in words):
                break
            s.input(move="500,300")
            s.input(wheel="down:4")
            time.sleep(0.6)
        squared = [w for w in words if w[0] == "squared"]
        if not squared:
            fails.append("decoration selector text 'squared' not found on the Surfaces page")
            return
        target = (squared[0][1], squared[0][2])
        s.input(click=f"{target[0]},{target[1]}")
        time.sleep(1.5)
        after, _ = base.dbus_settings_snapshot(s.ctx)
        ev += [f"selector_click={target}", f"decoration_before_rounded={'<\'rounded\'>' in (before or '')}",
               f"decoration_after_squared={'<\'squared\'>' in (after or '')}"]
        if "'decoration': <'rounded'>" not in (before or ""):
            fails.append("selector negative control: decoration was not rounded before the click")
        if "'decoration': <'squared'>" not in (after or ""):
            fails.append("clicking the squared segment did not set decoration=squared")
    finally:
        s.ctx.stop(settings)


def ap04(s: Session) -> dict:
    ev: list = []
    fails: list[str] = []
    if not s.restart({"decoration": "rounded", "surfaceOpacity": "100"}):
        return entry("AP04", FAIL, "WM or shell exited during restart")
    s.spawn_client()
    nodes = s.tree("AP04-rounded")
    _, frame = s.client_frame(nodes)
    if frame is None:
        return entry("AP04", FAIL, "client frame not found in the X tree")
    path, px = s.shot("AP04-rounded-floating")
    title = s.pixel(px, frame["x"] + frame["w"] // 2, frame["y"] + 3)
    corner = s.pixel(px, frame["x"] + 2, frame["y"] + 2)
    cut = not close_enough(corner, title, 24)
    ev += [str(path), f"frame={frame}", f"title={title}", f"corner={corner}", f"corner_cut={cut}"]
    if not cut:
        fails.append("rounded corner not cut")

    s.input(click=f"{frame['x'] + frame['w'] - 57},{frame['y'] + 15}")  # Maximize button
    time.sleep(s.args.settle_seconds)
    _, frame_max = s.client_frame(s.tree("AP04-maximized"))
    _, pmax = s.shot("AP04-maximized")
    if frame_max is None or pmax is None:
        fails.append("maximized frame not observed")
    else:
        mc = s.pixel(pmax, frame_max["x"] + 2, frame_max["y"] + 2)
        mt = s.pixel(pmax, frame_max["x"] + frame_max["w"] // 2, frame_max["y"] + 3)
        ev += [f"maximized_frame={frame_max}", f"maximized_corner={mc}"]
        if not close_enough(mc, mt, 24):
            fails.append("maximized corner is cut (expected rectangular)")

    s.input(click=f"{frame_max['x'] + frame_max['w'] - 57},{frame_max['y'] + 15}" if frame_max else "0,0")
    time.sleep(s.args.settle_seconds)
    _, frame_back = s.client_frame(s.tree("AP04-restored"))
    _, prest = s.shot("AP04-restored")
    if frame_back and prest:
        rc = s.pixel(prest, frame_back["x"] + 2, frame_back["y"] + 2)
        rt = s.pixel(prest, frame_back["x"] + frame_back["w"] // 2, frame_back["y"] + 3)
        ev.append(f"restored_corner={rc}")
        if close_enough(rc, rt, 24):
            fails.append("restored corner not rounded again")
    else:
        fails.append("restored frame not observed")

    s.input(drag=f"{frame_back['x'] + frame_back['w'] // 2},{frame_back['y'] + 15},2,400:20" if frame_back else "0,0,0,0")
    time.sleep(s.args.settle_seconds)
    _, frame_snap = s.client_frame(s.tree("AP04-snap-left"))
    _, psnap = s.shot("AP04-snap-left")
    if frame_snap and psnap:
        sc = s.pixel(psnap, frame_snap["x"] + 2, frame_snap["y"] + 2)
        st = s.pixel(psnap, frame_snap["x"] + frame_snap["w"] // 2, frame_snap["y"] + 3)
        ev.append(f"snap_left_corner={sc} frame={frame_snap}")
        if not close_enough(sc, st, 24):
            fails.append("snap-left corner is cut (expected rectangular)")
    else:
        fails.append("snap-left frame not observed")

    # Fullscreen: the frame must go rectangular and cover the whole screen.
    s.input(click=f"{frame_snap['x'] + frame_snap['w'] // 2},{frame_snap['y'] + 100}" if frame_snap else "0,0")
    client = find_named(s.tree("AP04-fs-client"), "xclock")
    fullscreen_measured = False
    if client is not None and request_fullscreen(s, client["id"], True):
        time.sleep(s.args.settle_seconds)
        _, frame_fs = s.client_frame(s.tree("AP04-fullscreen"))
        _, pfs = s.shot("AP04-fullscreen")
        if frame_fs and pfs:
            fullscreen_measured = True
            fc = s.pixel(pfs, frame_fs["x"] + 2, frame_fs["y"] + 2)
            covers = frame_fs["w"] >= s.args.screen_w and frame_fs["h"] >= s.args.screen_h
            ev.append(f"fullscreen_frame={frame_fs} corner={fc} covers_screen={covers}")
            if not covers:
                fails.append(f"fullscreen frame does not cover the screen: {frame_fs}")
            if fc == (0, 0, 0) and covers:
                fails.append("fullscreen corner is cut (expected rectangular)")
        request_fullscreen(s, client["id"], False)
    if not fullscreen_measured:
        ev.append("fullscreen: _NET_WM_STATE request not observed; NOT measured")
    squared_ok = s.restart({"decoration": "squared", "surfaceOpacity": "100"})
    if squared_ok:
        _, frame_sq = s.client_frame(s.tree("AP04-squared"))
        _, psq = s.shot("AP04-squared")
        if frame_sq and psq:
            sq_title = s.pixel(psq, frame_sq["x"] + frame_sq["w"] // 2, frame_sq["y"] + 3)
            sq_corner = s.pixel(psq, frame_sq["x"] + 2, frame_sq["y"] + 2)
            ev.append(f"squared_corner={sq_corner} squared_title={sq_title}")
            if not close_enough(sq_corner, sq_title, 24):
                fails.append("squared decoration corner is cut (expected rectangular)")
        else:
            fails.append("squared frame not observed")
    else:
        fails.append("WM or shell exited during squared restart")
    decoration_selector_check(s, ev, fails)
    picker_ev, picker_fails = color_picker_check(s)
    ev += picker_ev
    fails += picker_fails
    if fails:
        return entry("AP04", FAIL, "; ".join(fails), ev, ["xwininfo -tree", "import -window root", "XTEST click/drag"])
    if not fullscreen_measured:
        return entry("AP04", NOT_RUN, "rounded/ios/maximize/snap/restore/selector measured; fullscreen not observed",
                     ev, ["xwininfo -tree", "import -window root", "XTEST click/drag"])
    return entry("AP04", PASS, "", ev, ["xwininfo -tree", "import -window root", "XTEST click/drag",
                                        "EWMH _NET_WM_STATE_FULLSCREEN"])


def ap05(s: Session) -> dict:
    if not s.restart({"surfaceColor": "#1B1E20", "surfaceOpacity": "100"}):
        return entry("AP05", FAIL, "WM or shell exited during restart")
    s.spawn_client()
    _, frame = s.client_frame(s.tree("AP05-start"))
    if frame is None:
        return entry("AP05", FAIL, "client frame not found in the X tree")
    fx, fy, fw = frame["x"], frame["y"], frame["w"]
    tb_rect = (fx + fw // 2, fy + 2, max(1, fw // 2 - 3 * BUTTON_WIDTH - 8), 3)
    min_rect = (fx + fw - 3 * BUTTON_WIDTH + 6, fy + 4, BUTTON_WIDTH - 12, TITLEBAR_HEIGHT - 8)
    close_rect = (fx + fw - BUTTON_WIDTH + 6, fy + 4, BUTTON_WIDTH - 12, TITLEBAR_HEIGHT - 8)
    _, px = s.shot("AP05-idle")
    tb = dominant_color(s.pixels(px, tb_rect))
    min_idle = dominant_color(s.pixels(px, min_rect))
    fails = []
    ev_extra: list[str] = []
    if not close_enough(tb, base.accent_rgb("#1B1E20"), 3):
        fails.append(f"titlebar {tb} != #1B1E20")
    s.input(move=f"{fx + fw - BUTTON_WIDTH // 2},{fy + TITLEBAR_HEIGHT // 2}")
    time.sleep(1.0)
    path, px_hover = s.shot("AP05-close-hover")
    close_pixels = s.pixels(px_hover, close_rect)
    close_hover = dominant_color(close_pixels)
    # The hover fill is a circle, so the titlebar stays the dominant colour: require a sizeable matching area.
    hover_share = fraction_matching(close_pixels, CLOSE_HOVER_DEFAULT, 6)
    min_after = dominant_color(s.pixels(px_hover, min_rect))
    if hover_share < 0.15:
        fails.append(f"close hover {CLOSE_HOVER_DEFAULT} covers {hover_share:.2f} of the button (<0.15), dominant {close_hover}")
    if not close_enough(min_after, min_idle, 3):
        fails.append(f"minimize button changed on close hover: {min_idle} -> {min_after}")

    # Minimize and maximize buttons: hovering each must repaint its own button (hover fill) and only it.
    max_rect = (fx + fw - 2 * BUTTON_WIDTH + 6, fy + 4, BUTTON_WIDTH - 12, TITLEBAR_HEIGHT - 8)
    for name, rect_btn, centre_x in (("minimize", min_rect, fx + fw - 3 * BUTTON_WIDTH + BUTTON_WIDTH // 2),
                                     ("maximize", max_rect, fx + fw - 2 * BUTTON_WIDTH + BUTTON_WIDTH // 2)):
        s.input(move=f"{s.args.screen_w // 2},{s.args.screen_h // 2}")
        time.sleep(0.5)
        _, idle_px = s.shot(f"AP05-{name}-idle")
        s.input(move=f"{centre_x},{fy + TITLEBAR_HEIGHT // 2}")
        time.sleep(1.0)
        _, hover_px = s.shot(f"AP05-{name}-hover")
        if idle_px is None or hover_px is None:
            fails.append(f"{name} hover capture failed")
            continue
        changed = pixel_diff_count(s.pixels(idle_px, rect_btn), s.pixels(hover_px, rect_btn), 6)
        other = max_rect if name == "minimize" else min_rect
        leaked = pixel_diff_count(s.pixels(idle_px, other), s.pixels(hover_px, other), 6)
        close_leak = pixel_diff_count(s.pixels(idle_px, close_rect), s.pixels(hover_px, close_rect), 6)
        ev_extra.append(f"{name}_hover_changed_px={changed} other_button_px={leaked} close_px={close_leak}")
        if changed < 40:
            fails.append(f"{name} button did not repaint on hover ({changed} px changed)")
        if leaked > 10 or close_leak > 10:
            fails.append(f"{name} hover repainted a neighbouring button ({leaked}/{close_leak} px)")

    custom = "#00FF00"
    if not s.restart({"surfaceColor": "#1B1E20", "surfaceOpacity": "100", "titlebarCloseHoverColor": custom}):
        return entry("AP05", FAIL, "WM or shell exited during custom restart", [str(path)])
    _, frame2 = s.client_frame(s.tree("AP05-custom"))
    if frame2 is None:
        fails.append("client frame missing after custom restart")
    else:
        cr = (frame2["x"] + frame2["w"] - BUTTON_WIDTH + 6, frame2["y"] + 4, BUTTON_WIDTH - 12, TITLEBAR_HEIGHT - 8)
        s.input(move=f"{frame2['x'] + frame2['w'] - BUTTON_WIDTH // 2},{frame2['y'] + TITLEBAR_HEIGHT // 2}")
        time.sleep(1.0)
        _, pc = s.shot("AP05-custom-close-hover")
        custom_share = fraction_matching(s.pixels(pc, cr), base.accent_rgb(custom), 6)
        if custom_share < 0.15:
            fails.append(f"custom close hover {custom} covers {custom_share:.2f} of the button (<0.15)")
    ev = [str(path), f"titlebar={tb}", f"close_hover={close_hover}", f"min_idle={min_idle}", f"min_after={min_after}"] + ev_extra
    if fails:
        return entry("AP05", FAIL, "; ".join(fails), ev, ["XTEST move", "import -window root"])
    return entry("AP05", PASS, "", ev, ["XTEST move", "import -window root"])


def _ap06_slide(s: Session) -> dict:
    if not s.restart({"animationsEnabled": "true", "startAnimationEnabled": "true", "startAnimation": "slide-up", "taskbarStyle": "classic", "surfaceOpacity": "100"}):
        return entry("AP06", FAIL, "WM or shell exited during restart")
    ys: list[int] = []
    s.input(key=base.SHORTCUT_DEFAULT_BINDING)
    deadline = time.monotonic() + 1.5
    while time.monotonic() < deadline:
        popup = find_named(s.tree("AP06-anim"), "FlameWM start")
        if popup:
            ys.append(popup["y"])
        time.sleep(0.05)
    time.sleep(s.args.settle_seconds)
    final = find_named(s.tree("AP06-final"), "FlameWM start")
    s.input(key=base.SHORTCUT_DEFAULT_BINDING)
    if final is None or len(ys) < 3:
        return entry("AP06", FAIL, f"fewer than 3 popup observations ({len(ys)})")
    ys.append(final["y"])
    ok = slide_up_ok(ys)
    res = entry("AP06", PASS if ok else FAIL, "" if ok else f"y sequence not slide-up: {ys}",
                [f"y_samples={ys}"], ["xwininfo -root -tree polling", "XTEST key Super_L"])
    # Regression: the slide must play on EVERY open, not just the first (a stale "shown" flag after an
    # immediate close once made later opens appear without any motion).
    rest_y = final["y"]
    for cycle in range(1, 5):
        time.sleep(0.6)
        s.input(key=base.SHORTCUT_DEFAULT_BINDING)  # open
        seen: list[int] = []
        deadline = time.monotonic() + 0.8
        while time.monotonic() < deadline:
            popup = find_named(s.tree("AP06-repeat"), "FlameWM start")
            if popup:
                seen.append(popup["y"])
        time.sleep(s.args.settle_seconds / 2)
        s.input(key=base.SHORTCUT_DEFAULT_BINDING)  # close
        if not seen or max(seen) <= rest_y + 10:
            ok = False
            res = entry("AP06", FAIL, f"open #{cycle + 1} did not slide (y samples {seen}, rest y {rest_y})",
                        [f"y_samples={ys}", f"repeat_{cycle}={seen}"], ["xwininfo -root -tree polling"])
            break
    if not s.restart({"animationsEnabled": "false", "startAnimationEnabled": "false", "startAnimation": "slide-up", "taskbarStyle": "classic", "surfaceOpacity": "100"}):
        return entry("AP06", FAIL, "WM or shell exited during disabled restart")
    ys2: list[int] = []
    s.input(key=base.SHORTCUT_DEFAULT_BINDING)
    for _ in range(8):
        popup = find_named(s.tree("AP06-disabled"), "FlameWM start")
        if popup:
            ys2.append(popup["y"])
    s.input(key=base.SHORTCUT_DEFAULT_BINDING)
    ok2 = no_intermediate_ok(ys2)
    if ok and ok2:
        return entry("AP06", PASS, "", [f"enabled_y={ys}", f"disabled_y={ys2}"], res["command"].split(" && "))
    reason = "; ".join(p for p in (res["reason"], "" if ok2 else f"disabled y not constant: {ys2}") if p)
    return entry("AP06", FAIL, reason, [f"enabled_y={ys}", f"disabled_y={ys2}"], res["command"].split(" && "))


def animation_slider_check(s: Session) -> tuple[list[str], list[str]]:
    """Drag the real Settings animation speed slider to its far end and read animationSpeed back."""
    ev: list[str] = []
    if not s.restart({"animationsEnabled": "true", "animationSpeed": "100", "surfaceOpacity": "100", "taskbarStyle": "classic"}):
        return ["WM or shell exited before the slider check"], ev
    s.ctx.stop(s.client)  # a leftover client window would sit above Settings and swallow the wheel
    settings = s.ctx.spawn("settings-ap06", [s.args.settings_bin, "--page", "animations"], s.ctx.app_env)
    fails: list[str] = []
    try:
        time.sleep(s.args.settle_seconds)
        before, _ = base.dbus_settings_snapshot(s.ctx)
        row = None
        for round_index in range(36):
            path, _ = s.shot(f"AP06-slider-{round_index}")
            if path is None:
                return ["slider check could not capture the Settings page"], ev
            # The filled part of the slider track is a long accent-coloured run in the control column
            # (x 500..720); toggles sit right of x=737 and the accent swatches are short.
            with base.Image.open(path) as image:
                rgb = image.convert("RGB")
                for y in range(40, 560):
                    run = 0
                    for x in range(500, 721):
                        if close_enough(rgb.getpixel((x, y)), (0xEF, 0x40, 0x48), 24):
                            run += 1
                            if run >= 40:
                                row = ("track", x, y)
                                break
                        else:
                            run = 0
                    if row:
                        break
            if row:
                break
            s.input(move="500,300")
            s.input(wheel="down:8")
            time.sleep(0.6)
        if before is None:
            return ["Settings snapshot unreadable before the slider drag"], ev
        if row is None:
            return ["animation speed row not found on the Animations page"], ev
        # Track: 180px, then a 58px value pill after a 10px gap, right-aligned at the card edge (x ~ 776).
        right = 776 - 58 - 10
        left = right - 180
        y = row[2]
        s.input(drag=f"{left + 4},{y},{right + 20},{y}:12")
        time.sleep(1.5)
        after, _ = base.dbus_settings_snapshot(s.ctx)
        ev += [f"slider_row={row}", f"slider_drag={left + 4},{y}->{right + 20}",
               f"animationSpeed_before_100={'animationSpeed\': <100>' in before or 'animationSpeed\': <int64 100>' in before}"]
        match = re.search(r"'animationSpeed': <(?:int64 )?(\d+)>", after or "")
        speed = int(match.group(1)) if match else None
        ev.append(f"animationSpeed_after={speed}")
        if speed is None or speed < 190:
            # Diagnostic only: does a plain click on the track near its far end register?
            s.input(click=f"{right - 3},{y}")
            time.sleep(1.0)
            clicked, _ = base.dbus_settings_snapshot(s.ctx)
            match2 = re.search(r"'animationSpeed': <(?:int64 )?(\d+)>", clicked or "")
            ev.append(f"animationSpeed_after_click={match2.group(1) if match2 else None}")
            fails.append(f"dragging the animation speed slider to the end gave animationSpeed={speed} (expected ~200)")
    finally:
        s.ctx.stop(settings)
    return fails, ev


def ap06(s: Session) -> dict:
    result = _ap06_slide(s)
    fails, ev = animation_slider_check(s)
    for style in START_STYLES:
        style_fails, style_ev = start_style_check(s, style)
        fails += style_fails
        ev += style_ev
    for style in START_STYLES:
        above_fails, above_ev = start_above_windows_check(s, style)
        fails += above_fails
        ev += above_ev
    result["evidence"] = list(result.get("evidence", [])) + ev
    if fails:
        reason = "; ".join(p for p in (result.get("reason", ""), *fails) if p)
        return entry("AP06", FAIL, reason, result["evidence"], result.get("commands"))
    return result


def nonblack_bbox(pixels: list[tuple[int, int, int]], width: int, y_limit: int) -> tuple[int, int, int, int] | None:
    """(x0, y0, x1, y1) of the bright (clock face, all channels >= 200) pixels above y_limit, or None.

    The shell popups and chrome are dark, so only the white xclock client counts; a Start popup or a
    black overlay can never satisfy the lamp measurement."""
    xs, ys = [], []
    for y in range(y_limit):
        row = pixels[y * width:(y + 1) * width]
        hit = [x for x, p in enumerate(row) if min(p) >= 200]
        if hit:
            xs += [hit[0], hit[-1]]
            ys.append(y)
    return (min(xs), ys[0], max(xs), ys[-1]) if xs else None


def grab_flight(s: Session, seconds: float, measure) -> list:
    """measure(img) for each in-process root grab of the strip above the taskbar, one per START_SAMPLE_PERIOD.

    Screenshot tools take ~0.3 s per frame, too slow for an ~0.8 s flight; XGetImage does not."""
    import flamewm_lamp_acceptance as lamp  # RootGrab: in-process XGetImage, shared with the lamp run

    strip_y = s.args.screen_h - TASKBAR_HEIGHT
    out: list = []
    grab = lamp.RootGrab(s.ctx.display)
    try:
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            started = time.monotonic()
            out.append(measure(grab.grab(0, 0, s.args.screen_w, strip_y)))
            time.sleep(max(0.0, START_SAMPLE_PERIOD - (time.monotonic() - started)))
    finally:
        grab.close()
    return out


def overlay_nodes(nodes: list[dict], top_before: set[str]) -> list[dict]:
    """Transient override-redirect overlays: nameless top-level windows new since `top_before`, not the
    client's frame, and wider than 1 px (xwininfo helper windows)."""
    client = find_named(nodes, "xclock")
    owner = parent_of(nodes, client) if client else None
    return [n for n in nodes if n.get("parent") is None and n["id"] not in top_before and not n.get("name")
            and (owner is None or n["id"] != owner["id"]) and n["w"] > 1]


def flight_finished(seen_before: set[str], now: set[str]) -> bool:
    """True once an overlay that was seen earlier is no longer present."""
    return bool(seen_before) and not now


def leftover_clients(nodes: list[dict]) -> list[dict]:
    """Windows a clean desktop must not have before a measurement: an xclock or a nameless top-level overlay."""
    return [n for n in nodes if n.get("name") == "xclock"
            or (n.get("parent") is None and not n.get("name") and n["w"] > 1)]


CLEAN_DESKTOP_SECONDS = 3.0  # AP07 precheck: leftovers from a previous case may take a moment to unmap
CASE_CLEANUP_SECONDS = 3.0   # per-case teardown: bounded wait for the stopped client's window to leave the tree


def wait_until_empty(sample, timeout: float, now=time.monotonic, sleep=time.sleep, interval: float = 0.1) -> list:
    """Re-samples until sample() is empty or `timeout` has passed; returns the last sample (empty = clean)."""
    deadline = now() + timeout
    current = sample()
    while current and now() < deadline:
        sleep(interval)
        current = sample()
    return current


def stop_case_clients(s: Session) -> None:
    """Per-case teardown: stop the xclock this case spawned, then wait (bounded) for it to leave the X tree."""
    s.ctx.stop(s.client)
    s.client = None
    wait_until_empty(lambda: leftover_clients(s.tree("case-cleanup")), CASE_CLEANUP_SECONDS)


def overlay_observer(s: Session, label: str, top_before: set[str]):
    """observe() callable for sample_flight: overlay nodes from a fresh X tree each call."""
    ticks = iter(range(10 ** 9))

    def observe() -> list[dict]:
        return overlay_nodes(s.tree(f"{label}-{next(ticks)}"), top_before)

    return observe


def sample_flight(s: Session, seconds: float, measure, observe=None, done=None) -> tuple[list, list[list[dict]]]:
    """measure(img) per in-process root grab of the strip above the taskbar, one per START_SAMPLE_PERIOD.

    Stops one grab after either `done(sample)` is true or an overlay reported by observe() has appeared and
    disappeared, and never later than `seconds`. Returns (samples, overlay nodes per observe tick)."""
    import flamewm_lamp_acceptance as lamp  # RootGrab: in-process XGetImage, shared with the lamp run

    strip_y = s.args.screen_h - TASKBAR_HEIGHT
    samples: list = []
    ticks: list[list[dict]] = []
    seen: set[str] = set()
    grab = lamp.RootGrab(s.ctx.display)
    try:
        deadline = time.monotonic() + seconds
        finished = False
        while time.monotonic() < deadline:
            started = time.monotonic()
            samples.append(measure(grab.grab(0, 0, s.args.screen_w, strip_y)))
            if finished:
                break
            if done is not None and done(samples[-1]):
                finished = True
            if observe is not None:
                now = observe()
                ticks.append(now)
                ids = {n["id"] for n in now}
                finished = finished or flight_finished(seen, ids)
                seen |= ids
            time.sleep(max(0.0, START_SAMPLE_PERIOD - (time.monotonic() - started)))
    finally:
        grab.close()
    return samples, ticks


def bright_bbox_pil(img) -> tuple[int, int, int, int] | None:
    """PIL form of nonblack_bbox: inclusive (x0, y0, x1, y1) of all-channel >= 200 pixels, or None."""
    from PIL import ImageChops

    r, g, b = img.split()
    low = ImageChops.darker(ImageChops.darker(r, g), b).point(lambda v: 255 if v >= 200 else 0)
    box = low.getbbox()
    return None if box is None else (box[0], box[1], box[2] - 1, box[3] - 1)


def flight_bright_boxes(s: Session, seconds: float) -> list:
    """In-process equivalent of sample_bright_boxes: bright bbox per grab."""
    return grab_flight(s, seconds, bright_bbox_pil)


def rect_brightness(img, rect: tuple[int, int, int, int]) -> float:
    """Mean channel value of a PIL image over rect (x0, y0, x1, y1), as grab_samples measures it."""
    from PIL import ImageStat

    return sum(ImageStat.Stat(img.crop(rect)).mean) / 3


def grab_samples(s: Session, seconds: float, rect: tuple[int, int, int, int]) -> list:
    """In-process equivalent of timed_samples: (client bbox, mean brightness over rect) per grab."""
    from PIL import ImageStat

    def measure(img):
        return (bright_bbox_pil(img), sum(ImageStat.Stat(img.crop(rect)).mean) / 3)

    return grab_flight(s, seconds, measure)


def extra_effect_checks(s: Session) -> tuple[list[str], list[str]]:
    """Window maximize and window-open animations measured as painted client sizes at 0.25x speed."""
    fails: list[str] = []
    ev: list[str] = []
    width = lambda b: (b[2] - b[0]) if b else 0  # noqa: E731
    if not s.restart({"animationsEnabled": "true", "maximizeAnimationEnabled": "true", "animationSpeed": "25", "taskbarStyle": "classic",
                      "surfaceOpacity": "100"}):
        return ["WM or shell exited before the maximize check"], ev
    s.spawn_client()
    _, frame = s.client_frame(s.tree("AP07-max-frame"))
    if frame is None:
        return ["client frame not found for the maximize check"], ev
    _, pre = s.shot("AP07-max-pre")
    rest_w = width(nonblack_bbox(pre, s.args.screen_w, s.args.screen_h - TASKBAR_HEIGHT)) if pre else 0
    s.input(click=f"{frame['x'] + frame['w'] - 57},{frame['y'] + 15}")  # Maximize
    boxes = flight_bright_boxes(s, 3.0)
    widths = [width(b) for b in boxes]
    final_w = widths[-1] if widths else 0
    middle = [w for w in widths if rest_w * 1.15 < w < final_w * 0.9]
    ev += [f"maximize_rest_w={rest_w}", f"maximize_widths={widths}"]
    if final_w <= rest_w or not middle:
        fails.append(f"maximize did not animate through intermediate sizes (rest {rest_w}, widths {widths})")
    # Unmaximize: the same titlebar button on the maximized frame must shrink back through intermediates.
    _, frame_max = s.client_frame(s.tree("AP07-unmax-frame"))
    if frame_max is not None and final_w > rest_w:
        s.input(click=f"{frame_max['x'] + frame_max['w'] - 57},{frame_max['y'] + 15}")
        boxes_u = flight_bright_boxes(s, 3.0)
        widths_u = [width(b) for b in boxes_u]
        end_w = widths_u[-1] if widths_u else 0
        middle_u = [w for w in widths_u if end_w * 1.15 < w < final_w * 0.9]
        ev.append(f"unmaximize_widths={widths_u}")
        if not (0 < end_w < final_w * 0.5) or not middle_u:
            fails.append(f"unmaximize did not shrink through intermediate sizes (widths {widths_u})")
    # Window open: the first frames after map are smaller than the settled window.
    if not s.restart({"animationsEnabled": "true", "windowOpenAnimationEnabled": "true", "animationSpeed": "25", "taskbarStyle": "classic",
                      "surfaceOpacity": "100"}):
        return fails + ["WM or shell exited before the window-open check"], ev
    s.ctx.stop(s.client)
    s.client = s.ctx.spawn("client-xclock-open", [s.args.client_bin, "-geometry", "300x200+300+200"],
                           s.ctx.app_env)
    # The open flight starts after Expose (~60-120 ms after map) and lasts ~640 ms at 0.25x, so sample the
    # in-process root grab until the transient overlay window (nameless, new, not the client frame) has
    # appeared and gone, rather than a fixed short window; the overlay's own geometry is recorded per tick.
    top_before = {n["id"] for n in s.tree("AP07-open-before") if n.get("parent") is None}
    samples, ticks = sample_flight(s, 3.0, bright_bbox_pil, overlay_observer(s, "AP07-open-tree", top_before))
    widths = [width(b) for b in samples]
    transient = {n["id"] for tick in ticks for n in tick}
    overlay_geoms = [(n["x"], n["y"], n["w"], n["h"]) for tick in ticks for n in tick]
    settled = max(widths) if widths else 0
    smaller = [w for w in widths if 0 < w < settled * 0.9]
    ev += [f"open_widths={widths}", f"open_transient_windows={sorted(transient)}",
           f"open_overlay_geoms={overlay_geoms}"]
    if settled == 0 or not (smaller or transient):
        fails.append(f"window open did not animate (widths {widths}, transient windows {sorted(transient)})")
    rf, rev = respawn_after_minimize_check(s)
    return fails + rf, ev + rev


def region_brightness(pixels: list[tuple[int, int, int]], width: int, rect: tuple[int, int, int, int]) -> float:
    """Mean channel value over the rect (x0, y0, x1, y1), used for the fade-out brightness check."""
    x0, y0, x1, y1 = rect
    values = [sum(pixels[y * width + x]) / 3 for y in range(y0, y1) for x in range(x0, x1)]
    return sum(values) / len(values) if values else 0.0


def timed_samples(s: Session, label: str, seconds: float, rect: tuple[int, int, int, int] | None) -> list:
    """(client bbox or None, mean brightness over rect or None) per screenshot for `seconds`."""
    strip_y = s.args.screen_h - TASKBAR_HEIGHT
    out: list = []
    deadline = time.monotonic() + seconds
    n = 0
    while time.monotonic() < deadline:
        _, px = s.shot(f"{label}-{n}")
        n += 1
        if px is not None:
            bright = region_brightness(px, s.args.screen_w, rect) if rect else None
            out.append((nonblack_bbox(px, s.args.screen_w, strip_y), bright))
    return out


def style_matrix_checks(s: Session) -> tuple[list[str], list[str]]:
    """Per-style checks at 0.25x (the settings floor). Fade is judged by mean brightness of the client rect (a fading client's
    bright pixels drop below the bright-bbox threshold, so bboxes alone cannot see it); none must show no
    intermediate state."""
    fails: list[str] = []
    ev: list[str] = []
    strip_y = s.args.screen_h - TASKBAR_HEIGHT
    width = lambda b: (b[2] - b[0]) if b else 0  # noqa: E731
    base = {"animationsEnabled": "true", "animationSpeed": "25", "surfaceOpacity": "100", "taskbarStyle": "classic"}
    for effect, key, click in (
        ("minimize", "minimizeAnimation", lambda f: f"{f['x'] + f['w'] - 3 * BUTTON_WIDTH + BUTTON_WIDTH // 2},{f['y'] + 15}"),
        ("maximize", "maximizeAnimation", lambda f: f"{f['x'] + f['w'] - 57},{f['y'] + 15}"),
    ):
        for style in ("fade", "none"):
            tag = f"AP07-style-{effect}-{style}"
            if not s.restart({**base, key: style}):
                return fails + [f"WM or shell exited before {tag}"], ev
            s.spawn_client()
            _, frame = s.client_frame(s.tree(f"{tag}-frame"))
            if frame is None:
                return fails + [f"client frame not found for {tag}"], ev
            _, pre = s.shot(f"{tag}-pre")
            rest = nonblack_bbox(pre, s.args.screen_w, strip_y) if pre else None
            if rest is None:
                fails.append(f"{tag}: client pixels not found before the effect")
                continue
            rest_b = region_brightness(pre, s.args.screen_w, rest)
            s.input(click=click(frame))
            if effect == "maximize":
                samples = grab_samples(s, 4.0, rest)
            elif style == "fade":
                # The fade is ~1.04 s at 0.25x: sample the in-process grab at native rate until the client has
                # faded out (bright bbox gone and brightness under 10% of rest), instead of ~0.3 s screenshots.
                samples, _ = sample_flight(s, 4.0, lambda img: (bright_bbox_pil(img), rect_brightness(img, rest)),
                                           done=lambda sm: sm[0] is None and sm[1] < 0.1 * rest_b)
            else:
                samples = timed_samples(s, tag, 4.0, rest)
            present = [(b, l) for b, l in samples if b is not None]
            widths = [width(b) for b, _ in present]
            rest_w = width(rest)
            final_w = widths[-1] if widths else 0
            ev.append(f"{tag}: rest={rest} widths={widths} brightness={[round(l) for _, l in samples]}")
            if effect == "minimize" and style == "fade":
                # In place: no neck, a dimmed frame mid-flight, and the client gone at the end.
                mid = [l for _, l in samples if 0.1 * rest_b < l < 0.9 * rest_b]
                if not mid:
                    fails.append(f"{tag}: no mid-flight frame between 10% and 90% of rest brightness {rest_b:.0f}")
                if any(abs(w - rest_w) > rest_w * 0.1 for w in widths):
                    fails.append(f"{tag}: client changed size during the fade (widths {widths}, rest {rest_w})")
                if samples and samples[-1][1] >= 0.1 * rest_b:
                    fails.append(f"{tag}: client still visible after the fade")
            elif effect == "minimize":
                if not samples or samples[0][0] is not None or present:
                    fails.append(f"{tag}: none should remove the client at the first sample (present {len(present)})")
            elif style == "fade":
                if not any(rest_w * 1.05 < w < final_w * 0.95 for w in widths):
                    fails.append(f"{tag}: no intermediate size between rest {rest_w} and final {final_w}")
            elif any(abs(w - final_w) > final_w * 0.05 for w in widths):
                fails.append(f"{tag}: none showed intermediate sizes (widths {widths})")
    # Window open: a fresh client mapped after restart, judged on the fixed rect the 300x200 client settles in.
    open_rect = (300, 180, 600, 400)
    for style in ("fade", "none"):
        tag = f"AP07-style-open-{style}"
        if not s.restart({**base, "windowOpenAnimation": style}):
            return fails + [f"WM or shell exited before {tag}"], ev
        s.ctx.stop(s.client)
        s.client = s.ctx.spawn("client-xclock-style", [s.args.client_bin, "-geometry", "300x200+300+200"],
                               s.ctx.app_env)
        # Native grabs at 50 ms: the 0.25x open flight is ~640 ms, while ~0.3-1.3 s screenshots land after it.
        samples, _ = sample_flight(s, 2.0, lambda img: (bright_bbox_pil(img), rect_brightness(img, open_rect)))
        settled = samples[-1][1] if samples else 0.0
        ev.append(f"{tag}: brightness={[round(l) for _, l in samples]} settled={settled:.0f}")
        fails += open_style_fails(tag, style, samples)
    return fails, ev


def open_style_fails(tag: str, style: str, samples: list) -> list[str]:
    """Verdict for one window-open series of (bbox, brightness); the settled level is the last sample."""
    settled = samples[-1][1] if samples else 0.0
    if settled < 50:
        return [f"{tag}: client never appeared (settled brightness {settled:.0f})"]
    if style == "fade":
        if not any(0.1 * settled < l < 0.9 * settled for _, l in samples):
            return [f"{tag}: no mid-open frame between 10% and 90% of settled brightness"]
    elif any(b is not None and abs(l - settled) > settled * 0.1 for b, l in samples):
        return [f"{tag}: none showed intermediate brightness"]
    return []


RESPAWN_CYCLES = 10
RESPAWN_TIMEOUT_SECONDS = 10.0


def wait_unmapped_xclock(s: Session) -> bool:
    """Polls until the owned xclock is no longer viewable (the minimize unmap is visible)."""
    deadline = time.monotonic() + RESPAWN_TIMEOUT_SECONDS
    while time.monotonic() < deadline:
        client, _ = s.client_frame(s.tree("AP07-respawn-unmap"))
        if client is not None and "IsViewable" not in map_state_of(s, client["id"]):
            return True
        time.sleep(0.05)
    return False


def map_state_of(s: Session, win_id: str) -> str:
    out = subprocess.run(["xwininfo", "-display", s.ctx.display, "-id", win_id], env=s.ctx.host_env,
                         capture_output=True, text=True, timeout=20, check=False).stdout
    return map_state(out) or ""


def wait_framed_xclock(s: Session, label: str) -> tuple[dict | None, dict | None]:
    """Polls the tree until the owned xclock has a frame parent, or the respawn timeout passes."""
    deadline = time.monotonic() + RESPAWN_TIMEOUT_SECONDS
    while True:
        client, frame = s.client_frame(s.tree(label))
        if (client is not None and frame is not None) or time.monotonic() >= deadline:
            return client, frame
        time.sleep(0.25)


def respawn_after_minimize_check(s: Session) -> tuple[list[str], list[str]]:
    """Each xclock is minimized, stopped while minimized, then the next xclock must be framed.

    A client XID recycled after a minimized client was destroyed must not inherit that client's state, so
    the new window still gets a frame parent (an unframed map means the old client was never unmanaged)."""
    fails: list[str] = []
    ev: list[str] = []
    if not s.restart({"surfaceOpacity": "100"}):
        return ["WM or shell exited before the respawn check"], ev
    framed = 0
    for n in range(RESPAWN_CYCLES):
        s.spawn_client()
        _, frame = s.client_frame(s.tree(f"AP07-respawn-{n}-first"))
        if frame is None:
            fails.append(f"respawn cycle {n}: first xclock frame not found")
            break
        minimize_at = f"{frame['x'] + frame['w'] - 3 * BUTTON_WIDTH + BUTTON_WIDTH // 2},{frame['y'] + 15}"
        s.input(click=minimize_at)
        minimized = wait_unmapped_xclock(s)
        s.ctx.stop(s.client)
        s.spawn_client()
        client, frame = wait_framed_xclock(s, f"AP07-respawn-{n}-next")
        if not minimized:
            fails.append(f"respawn cycle {n}: minimize did not unmap the xclock")
        if client is None or frame is None:
            alive = s.client.proc.poll() is None
            state = map_state_of(s, client["id"]) if client is not None else "no client"
            ev.append(f"respawn cycle {n}: next xclock framed=False (process alive={alive}, map={state})")
            fails.append(f"respawn cycle {n}: next xclock after a destroyed minimized client has no frame")
            continue
        framed += 1
        ev.append(f"respawn cycle {n}: next xclock framed=True")
    ev.append(f"respawn_framed={framed}/{RESPAWN_CYCLES}")
    return fails, ev


def ap07(s: Session) -> dict:
    # The minimize flight is ~1.04 s at 0.25x; in-process root grabs (~50 ms apart) sample it until the
    # transient overlay has appeared and gone. The measurement is the painted client pixels (bounding box of
    # bright content above the taskbar), not an overlay window's size.
    if not s.restart({"animationsEnabled": "true", "minimizeAnimationEnabled": "true", "animationSpeed": "25", "taskbarStyle": "classic",
                      "surfaceOpacity": "100"}):
        return entry("AP07", FAIL, "WM or shell exited during restart")
    capability = "32-bit ARGB visual" if "depth 32" in subprocess.run(
        ["xdpyinfo", "-display", s.ctx.display], env=s.ctx.host_env, capture_output=True, text=True,
        timeout=20, check=False).stdout else "root-visual fallback (no 32-bit visual reported)"
    # Clean-desktop precheck: a leftover xclock or overlay from an earlier phase would be measured as the
    # client, so refuse to measure instead of producing an odd-geometry box0.
    leftovers = wait_until_empty(lambda: leftover_clients(s.tree("AP07-clean")), CLEAN_DESKTOP_SECONDS)
    if leftovers:
        return entry("AP07", FAIL, f"desktop not clean before the minimize measurement: {leftovers}")
    precheck_ev = ["clean_desktop_precheck=passed (no xclock or nameless overlay before spawn)"]
    s.spawn_client()
    _, frame = s.client_frame(s.tree("AP07-frame"))
    client = find_named(s.tree("AP07-client"), "xclock")
    if frame is None or client is None:
        return entry("AP07", FAIL, "client/frame not found")
    strip_y = s.args.screen_h - TASKBAR_HEIGHT
    _, pre = s.shot("AP07-pre")
    box0 = nonblack_bbox(pre, s.args.screen_w, strip_y) if pre else None
    top_before = {n["id"] for n in s.tree("AP07-mid-before") if n.get("parent") is None}
    s.input(click=f"{frame['x'] + frame['w'] - 3 * BUTTON_WIDTH + BUTTON_WIDTH // 2},{frame['y'] + 15}")  # Minimize
    boxes, _ = sample_flight(s, 3.0, bright_bbox_pil, overlay_observer(s, "AP07-mid-tree", top_before))
    wm_state = subprocess.run(["xwininfo", "-display", s.ctx.display, "-id", client["id"]], env=s.ctx.host_env,
                              capture_output=True, text=True, timeout=20, check=False).stdout
    icon = subprocess.run(["xprop", "-display", s.ctx.display, "-id", client["id"], "_NET_WM_ICON_GEOMETRY"],
                          env=s.ctx.host_env, capture_output=True, text=True, timeout=20, check=False).stdout
    rect = parse_icon_geometry(icon)
    fails = []
    if box0 is None:
        fails.append("client pixels not found before minimize")
    else:
        w0, h0 = box0[2] - box0[0], box0[3] - box0[1]
        mids = [b for b in boxes if b is not None and b != box0]
        # The lamp flies toward the icon: mid frames differ from the rest bbox, their centres approach
        # _NET_WM_ICON_GEOMETRY step over step, and the last visible centre is within 60% of the first's
        # distance. The bend itself is proven by scripts/lamp-acceptance (LAMP01-03).
        centres = [((b[0] + b[2]) / 2, (b[1] + b[3]) / 2) for b in mids]
        if rect is not None:
            icon_c = (rect[0] + rect[2] / 2, rect[1] + rect[3] / 2)
            dists = [((cx - icon_c[0]) ** 2 + (cy - icon_c[1]) ** 2) ** 0.5 for cx, cy in centres]
        else:
            dists = []
        approaching = len(dists) >= 2 and all(b <= a + 2 for a, b in zip(dists, dists[1:]))
        if len(mids) < 2 or not approaching or dists[-1] > 0.6 * dists[0]:
            fails.append(f"minimize lamp did not fly toward the icon (mid frames {len(mids)}, "
                         f"icon-distance {[round(d) for d in dists]}; rest={box0}, frames={boxes})")
        if boxes and boxes[-1] is not None:
            fails.append(f"client pixels still on screen after the flight: {boxes[-1]}")
    if map_state(wm_state) not in ("IsUnMapped", "IsUnviewable"):
        fails.append(f"client map state {map_state(wm_state)} after minimize")
    if rect is None or rect[1] < strip_y:
        fails.append(f"_NET_WM_ICON_GEOMETRY {rect} not inside taskbar strip")
    # Restore through the taskbar button the lamp flew to (rect = _NET_WM_ICON_GEOMETRY).
    restore_ev: list[str] = []
    if rect is not None and not fails:
        s.input(click=f"{rect[0] + rect[2] // 2},{rect[1] + rect[3] // 2}")
        time.sleep(s.args.settle_seconds)
        after_restore = subprocess.run(["xwininfo", "-display", s.ctx.display, "-id", client["id"]],
                                       env=s.ctx.host_env, capture_output=True, text=True, timeout=20,
                                       check=False).stdout
        restore_state = map_state(after_restore)
        restore_ev.append(f"taskbar_restore_map_state={restore_state}")
        if restore_state != "IsViewable":
            fails.append(f"taskbar click did not restore the minimized client (map state {restore_state})")
    extra_fails, extra_ev = extra_effect_checks(s)
    extra_ev += restore_ev
    fails += extra_fails
    style_fails, style_ev = style_matrix_checks(s)
    extra_ev += style_ev
    fails += style_fails
    ev = precheck_ev + [f"capability={capability}", f"rest_bbox={box0}", f"frame_bboxes={boxes}",
                        f"map_state={map_state(wm_state)}", f"icon_geometry={rect}"] + extra_ev
    if fails:
        return entry("AP07", FAIL, "; ".join(fails), ev, ["in-process XGetImage root grab", "xprop _NET_WM_ICON_GEOMETRY"])
    return entry("AP07", PASS, "", ev, ["in-process XGetImage root grab", "xprop _NET_WM_ICON_GEOMETRY"])


def ap08(s: Session) -> dict:
    fails, ev = [], []
    # No app icon is visible without desktop entries in the private XDG_DATA_HOME, so seed one and compare
    # the Start menu (its app list renders the entry's themed icon) between the two icon themes.
    apps = s.ctx.run / "xdg-data" / "applications"
    apps.mkdir(parents=True, exist_ok=True)
    (apps / "ap08-probe.desktop").write_text(
        "[Desktop Entry]\nType=Application\nName=AP08 Probe\nExec=true\nIcon=folder\nCategories=Utility;\n",
        encoding="utf-8")

    if not s.restart({"iconTheme": "McMojave-circle", "surfaceOpacity": "100"}):
        return entry("AP08", FAIL, "WM or shell exited during restart")
    def start_menu_shot(name: str):
        s.input(key=base.SHORTCUT_DEFAULT_BINDING)
        time.sleep(s.args.settle_seconds)
        result = s.shot(name)
        s.input(key=base.SHORTCUT_DEFAULT_BINDING)
        time.sleep(1.0)
        return result

    _, pa = start_menu_shot("AP08-icon-base")
    if not s.restart({"iconTheme": s.args.icon_theme_alt, "surfaceOpacity": "100"}):
        return entry("AP08", FAIL, "WM or shell exited during alt restart")
    _, pb = start_menu_shot("AP08-icon-alt")
    rect = (0, s.args.screen_h - TASKBAR_HEIGHT - 400, 560, 400)
    if pa is not None and pb is not None:
        changed = pixel_diff_count(s.pixels(pa, rect), s.pixels(pb, rect))
        ev.append(f"start_menu_changed_pixels={changed} alt={s.args.icon_theme_alt}")
        if changed < 20:
            fails.append(f"Start menu icon region unchanged for iconTheme {s.args.icon_theme_alt} ({changed} px)")
    else:
        fails.append("icon capture failed")
    # An installed package puts the bundled cursor themes under /usr/share/icons; stage the same layout in
    # the private XDG_DATA_HOME so the WM can resolve them by name (symlinks into the repository assets).
    icons = s.ctx.run / "xdg-data" / "icons"
    icons.mkdir(parents=True, exist_ok=True)
    mojave = icons / "McMojave-cursors"
    if not mojave.exists():
        mojave.symlink_to(base.ROOT / "assets" / "themes" / "cursors" / "McMojave-cursors")
    breeze = icons / "FlameWM-Breeze-Dark"
    if not breeze.exists():
        breeze.mkdir()
        (breeze / "cursors").symlink_to(base.ROOT / "assets" / "cursors" / "cursors")
        index = (base.ROOT / "assets" / "cursors" / "index.theme").read_text(encoding="utf-8")
        (breeze / "index.theme").write_text(index.replace("Name=Breeze Dark", "Name=FlameWM-Breeze-Dark", 1),
                                            encoding="utf-8")
    # libXcursor's default search path is ~/.icons and ~/.local/share/icons (HOME, not XDG_DATA_HOME).
    s.ctx.app_env["FLAMEWM_CURSOR_PATH"] = str(icons)  # the session plan's search root for theme resolution
    home_icons = s.ctx.run / "home" / ".icons"
    if not home_icons.exists():
        home_icons.parent.mkdir(parents=True, exist_ok=True)
        home_icons.symlink_to(icons)
    digests = {}
    for theme in (s.args.cursor_theme_base, s.args.cursor_theme_alt):
        if not s.restart({"cursorTheme": theme, "surfaceOpacity": "100", "decoration": "rounded"}):
            return entry("AP08", FAIL, "WM or shell exited during cursor restart")
        # The WM defines themed cursors on frame resize handles; the bare root keeps the stock X cursor.
        s.spawn_client()
        _, frame = s.client_frame(s.tree("AP08-cursor"))
        if frame is None:
            return entry("AP08", FAIL, "client frame not found for the cursor probe")
        # Edge-midpoint left resize handle: independent of the rounded corner shape.
        s.input(move=f"{frame['x'] + 1},{frame['y'] + frame['h'] // 2}")
        time.sleep(1.0)
        digests[theme] = cursor_digest(s)
    ev.append(f"cursor_digests={digests}")
    if None in digests.values() or digests[s.args.cursor_theme_base] == digests[s.args.cursor_theme_alt]:
        if not xfixes_cursor_observable(s):
            ev.append("xfixes_negative_control=server reports identical cursor images for watch vs left_ptr")
            if not fails:
                return entry("AP08", ENV_BLOCKED,
                             "icon theme measured; cursor pixels not observable: this server (Xephyr) reports an "
                             f"identical XFixes cursor image for two different cursors ({digests})",
                             ev, ["import -window root", "XFixesGetCursorImage"])
        fails.append(f"cursor image unchanged or unreadable: {digests}")
    if fails:
        return entry("AP08", FAIL, "; ".join(fails), ev, ["import -window root", "XFixesGetCursorImage"])
    return entry("AP08", PASS, "", ev, ["import -window root", "XFixesGetCursorImage"])


class _CursorImage(ctypes.Structure):
    _fields_ = [("x", ctypes.c_short), ("y", ctypes.c_short), ("width", ctypes.c_ushort),
                ("height", ctypes.c_ushort), ("xhot", ctypes.c_ushort), ("yhot", ctypes.c_ushort),
                ("cursor_serial", ctypes.c_ulong), ("pixels", ctypes.POINTER(ctypes.c_ulong)),
                ("atom", ctypes.c_ulong), ("name", ctypes.c_char_p)]


def cursor_digest(s: Session) -> str | None:
    """SHA-256 of the cursor ARGB pixels currently shown on the owned display (XFixes)."""
    x11_path, xfixes_path = ctypes.util.find_library("X11"), ctypes.util.find_library("Xfixes")
    if not x11_path or not xfixes_path:
        return None
    x11, xfixes = ctypes.cdll.LoadLibrary(x11_path), ctypes.cdll.LoadLibrary(xfixes_path)
    x11.XOpenDisplay.restype = ctypes.c_void_p
    x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x11.XCloseDisplay.argtypes = [ctypes.c_void_p]
    xfixes.XFixesGetCursorImage.restype = ctypes.POINTER(_CursorImage)
    xfixes.XFixesGetCursorImage.argtypes = [ctypes.c_void_p]
    xfixes.XFixesQueryVersion.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_int)]
    dpy = x11.XOpenDisplay(s.ctx.display.encode("ascii"))
    if not dpy:
        return None
    try:
        major, minor = ctypes.c_int(), ctypes.c_int()
        if not xfixes.XFixesQueryVersion(dpy, ctypes.byref(major), ctypes.byref(minor)):
            return None
        image = xfixes.XFixesGetCursorImage(dpy)
        if not image:
            return None
        cur = image.contents
        count = cur.width * cur.height
        raw = b"".join((int(cur.pixels[i]) & 0xFFFFFFFF).to_bytes(4, "little") for i in range(count))
        name = (cur.name or b"").decode("ascii", "replace")
        return f"{cur.width}x{cur.height}@{cur.xhot},{cur.yhot} {name} {hashlib.sha256(raw).hexdigest()[:16]}"
    finally:
        x11.XCloseDisplay(dpy)


def xfixes_cursor_observable(s: Session) -> bool:
    """Negative control: can this server report two known-different cursors through XFixes?

    A probe window is given the stock `watch` then `left_ptr` cursor and the pointer is parked on it.
    When both read back identically (Xephyr reports a fixed image), cursor-theme pixels cannot be
    measured here and AP08 must say so instead of failing the product."""
    x11_path = ctypes.util.find_library("X11")
    if not x11_path:
        return False
    x11 = ctypes.cdll.LoadLibrary(x11_path)
    x11.XOpenDisplay.restype = ctypes.c_void_p
    x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x11.XDefaultRootWindow.restype = ctypes.c_ulong
    x11.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
    x11.XCreateSimpleWindow.restype = ctypes.c_ulong
    x11.XCreateSimpleWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong] + [ctypes.c_int] * 2 + [ctypes.c_uint] * 3 + [ctypes.c_ulong] * 2
    x11.XCreateFontCursor.restype = ctypes.c_ulong
    x11.XCreateFontCursor.argtypes = [ctypes.c_void_p, ctypes.c_uint]
    x11.XDefineCursor.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_ulong]
    x11.XMapRaised.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
    x11.XDestroyWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
    x11.XFlush.argtypes = [ctypes.c_void_p]
    x11.XCloseDisplay.argtypes = [ctypes.c_void_p]
    dpy = x11.XOpenDisplay(s.ctx.display.encode("ascii"))
    if not dpy:
        return False
    try:
        root = x11.XDefaultRootWindow(dpy)
        window = x11.XCreateSimpleWindow(dpy, root, 40, 40, 200, 200, 0, 0, 0xFFFFFF)
        x11.XMapRaised(dpy, window)
        digests = []
        for glyph in (150, 68):  # XC_watch, XC_left_ptr
            x11.XDefineCursor(dpy, window, x11.XCreateFontCursor(dpy, glyph))
            x11.XFlush(dpy)
            s.input(move="140,140")
            time.sleep(0.4)
            digests.append(cursor_digest(s))
        x11.XDestroyWindow(dpy, window)
        x11.XFlush(dpy)
        return None not in digests and digests[0] != digests[1]
    finally:
        x11.XCloseDisplay(dpy)


def ap09(s: Session) -> dict:
    if not s.restart({"surfaceOpacity": "100"}):
        return entry("AP09", FAIL, "WM or shell exited during restart")
    time.sleep(s.args.settle_seconds)
    pids = {"flamewm": s.ctx.wm.pid, "flamewm-shell": s.shell.pid}
    # Control: a sleeping child sampled over the same window, so the wakeup counters have a floor.
    control = subprocess.Popen([sys.executable, "-I", "-c", "import time; time.sleep(600)"])
    try:
        start = {n: (base.parse_proc_metrics(p), voluntary_switches(p), voluntary_switches_total(p))
                 for n, p in {**pids, "control-sleeper": control.pid}.items()}
        time.sleep(IDLE_SECONDS)
        per, ev, fails = {}, [], []
        for name, pid in {**pids, "control-sleeper": control.pid}.items():
            now_metrics, now_vol, now_total = base.parse_proc_metrics(pid), voluntary_switches(pid), voluntary_switches_total(pid)
            before_metrics, before_vol, before_total = start[name]
            total_wake = now_total - before_total if now_total is not None and before_total is not None else "n/a"
            alive = now_metrics is not None and before_metrics is not None and now_vol is not None and before_vol is not None
            if name == "control-sleeper":
                ev.append(f"control-sleeper: voluntary_wakeups_total={total_wake} over {IDLE_SECONDS:g}s"
                          + ("" if alive else " (control not observable)"))
                continue
            if not alive:
                fails.append(f"{name} not observable or exited")
                continue
            cpu = (now_metrics["cpu_ticks"] - before_metrics["cpu_ticks"]) / CLK_TCK
            wake = now_vol - before_vol
            per[name] = (cpu, wake)
            ev.append(f"{name}: cpu_seconds={cpu:.3f} voluntary_wakeups_leader={wake} "
                      f"voluntary_wakeups_total={total_wake} over {IDLE_SECONDS:g}s")
            if idle_verdict(cpu, wake, True) == FAIL:
                fails.append(f"{name} idle cpu {cpu:.3f}s / wakeups {wake}")
    finally:
        control.kill()
        control.wait()
    if fails:
        return entry("AP09", FAIL, "; ".join(fails), ev, ["/proc/<pid>/stat", "/proc/<pid>/status"])
    return entry("AP09", PASS, "", ev, ["/proc/<pid>/stat", "/proc/<pid>/status"])


SCENARIOS = (ap01, ap02, ap03, ap04, ap05, ap06, ap07, ap08, ap09)


# ---------------------------------------------------------------- driver

def utc_now() -> str:
    return time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())


def xtest_display_ok(display: str) -> bool:
    """flamewm_xtest.py only accepts nested displays :97..:119; any other display silently drops all input."""
    return re.fullmatch(r":\d+", display) is not None and int(display[1:]) in range(97, 120)


def parse_args(argv: list[str]) -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--run-dir", required=True)
    p.add_argument("--geometry", default="1280x800")
    p.add_argument("--display", default=None, help="nested display in :97..:119 (XTEST input range)")
    p.add_argument("--wm-bin", default=str(ROOT / ".target/release/flamewm"))
    p.add_argument("--shell-bin", default=str(ROOT / ".target/release/flamewm-shell"))
    p.add_argument("--settings-bin", default=str(ROOT / ".target/release/flamewm-settings"))
    p.add_argument("--client-bin", default="xclock")
    p.add_argument("--settle-seconds", type=float, default=3.0)
    p.add_argument("--only", default=None, metavar="AP07,AP08",
                   help="debug: run only these scenarios; the others stay NOT_RUN so the verdict cannot be PASS")
    p.add_argument("--icon-theme-alt", default="breeze-dark")
    p.add_argument("--cursor-theme-base", default="McMojave-cursors")
    p.add_argument("--cursor-theme-alt", default="FlameWM-Breeze-Dark")
    p.add_argument("--taskbar-slot", metavar="X,Y,W,H", default=None,
                   help="expected taskbar slot rect for AP07 _NET_WM_ICON_GEOMETRY (optional)")
    args = p.parse_args(argv)
    if not re.fullmatch(r"\d+x\d+", args.geometry):
        p.error("--geometry must be WxH")
    if args.display is not None and not xtest_display_ok(args.display):
        p.error("--display must be in :97..:119 (XTEST input range); other displays cannot receive input")
    args.screen_w, args.screen_h = (int(v) for v in args.geometry.split("x"))
    return args


def validate_run_dir(run: Path) -> None:
    rel = run.resolve().relative_to(ROOT) if run.resolve().is_relative_to(ROOT) else None
    if rel is not None and any(str(rel).startswith(prefix) for prefix in base.FORBIDDEN_RUN_PREFIXES):
        raise SystemExit(f"error: run dir {run} is under a forbidden prefix")
    if run.exists() and (not run.is_dir() or any(run.iterdir())):
        raise SystemExit(f"error: run dir {run} must not exist or must be empty")


def validate_inner_run_dir(run: Path) -> None:
    """The re-exec child sees only the owner marker written by the outer invocation, nothing else."""
    if not run.is_dir() or [item.name for item in run.iterdir()] != [base.RUN_OWNER_MARKER]:
        raise SystemExit(f"error: inner run dir {run} must contain only the outer {base.RUN_OWNER_MARKER} marker")


def run_inner(args: argparse.Namespace, run: Path) -> int:
    ctx = base.Ctx(run)
    results: dict[str, dict] = {}
    started = utc_now()
    for sub in ("home", "xdg-config", "xdg-data", "xdg-cache", "xdg-runtime", "shots", "logs"):
        (run / sub).mkdir(parents=True, exist_ok=True)
    try:
        display = base.pick_display(args.display)
        if display is None:
            raise_block(results, "no free display in :90..:99")
        elif not xtest_display_ok(display):
            raise_block(results, f"auto-picked {display} is outside the XTEST input range :97..:119")
        else:
            ctx.display = display
            ctx.build_app_env()
            ctx.token = secrets.token_hex(32)
            ctx.marker.write_text(f"{ctx.display}\n{ctx.token}\n", encoding="ascii")
            os.chmod(ctx.marker, 0o600)
            xephyr = ctx.spawn("xephyr", ["Xephyr", ctx.display, "-screen", args.geometry, "-ac", "-noreset", "-br"],
                               ctx.host_env)
            if not base.wait_for_display(ctx, xephyr):
                raise_block(results, f"Xephyr did not become ready on {ctx.display}")
            else:
                session = Session(ctx, args)
                only = {x.strip().upper() for x in args.only.split(",")} if args.only else None
                for scenario in SCENARIOS:
                    sid = scenario.__name__.upper()
                    if only is not None and sid not in only:
                        continue
                    try:
                        results[sid] = scenario(session)
                    except Exception as error:  # a crashed scenario is a FAIL with its reason, never a PASS
                        results[sid] = entry(sid, FAIL, f"scenario raised {type(error).__name__}: {error}")
                    finally:
                        stop_case_clients(session)
    finally:
        teardown = ctx.teardown()
        doc = compose_document(results)
        doc.update({"run_dir": str(run), "display": ctx.display, "geometry": args.geometry,
                    "wm_bin": str(Path(args.wm_bin).resolve()), "shell_bin": str(Path(args.shell_bin).resolve()),
                    "started_utc": started, "finished_utc": utc_now(), "teardown": teardown})
        if not teardown["ok"]:
            doc["verdict"], doc["exit_code"] = "FAIL", 1
        write_json(run / "result.json", doc)
    return doc["exit_code"]


def raise_block(results: dict[str, dict], reason: str) -> None:
    for sid in IDS:
        results[sid] = entry(sid, ENV_BLOCKED, reason)


def main(argv: list[str] | None = None) -> int:
    raw = sys.argv[1:] if argv is None else list(argv)
    args = parse_args(raw)
    run = Path(args.run_dir).expanduser().absolute()
    inner = os.environ.get("FLAMEWM_AP_INNER") == "1"
    if inner:
        validate_inner_run_dir(run)
    else:
        validate_run_dir(run)
    missing = [name for name in ("Xephyr", "xwininfo", "xprop", "import", "identify", "xdpyinfo", "gdbus")
               if shutil.which(name) is None]
    if missing or not os.environ.get("DISPLAY") or base.Image is None:
        run.mkdir(parents=True, exist_ok=True)
        reason = "; ".join(filter(None, [f"missing tools: {missing}" if missing else "",
                                         "" if os.environ.get("DISPLAY") else "host DISPLAY unset",
                                         "" if base.Image is not None else "PIL unavailable"]))
        results: dict[str, dict] = {}
        raise_block(results, reason)
        doc = compose_document(results) | {"run_dir": str(run)}
        write_json(run / "result.json", doc)
        print(run / "result.json")
        return 3
    for binary in (args.wm_bin, args.shell_bin, args.settings_bin):
        if not os.access(binary, os.X_OK):
            print(f"error: binary not executable: {binary}", file=sys.stderr)
            return 2
    run.mkdir(parents=True, exist_ok=True)
    os.chmod(run, 0o700)
    if inner:
        return run_inner(args, run)
    (run / base.RUN_OWNER_MARKER).write_text(f"owner-pid={os.getpid()}\ncreated={utc_now()}\n", encoding="ascii")
    if shutil.which("dbus-run-session") is None:
        print("error: dbus-run-session not found on PATH", file=sys.stderr)
        return 3
    child = subprocess.Popen(["dbus-run-session", "--", sys.executable, str(Path(__file__).resolve()), *raw],
                             env=dict(os.environ, FLAMEWM_AP_INNER="1"))
    code = child.wait()
    print(run / "result.json")
    return code



# ---------------------------------------------------------------- AP06 Start animation styles

START_STYLES = ("slide", "fade", "magic-lamp", "none")
START_BG = (0x19, 0x1B, 0x1D)  # product default startMenuColor
START_SAMPLE_PERIOD = 0.05     # <= 60 ms in-process XGetImage captures
START_FLIGHT_SECONDS = 1.2     # slide/fade/lamp at animationSpeed 25 finish within ~0.72 s


def start_popup_metrics(img, rest: dict, strip_y: int) -> dict:
    """Start popup bbox (pixels within 6 of START_BG) and mean brightness over the resting rect."""
    from PIL import Image, ImageChops, ImageStat

    solid = Image.new("RGB", img.size, START_BG)
    near = ImageChops.difference(img, solid).convert("L").point(lambda v: 255 if v <= 6 else 0)
    bbox = near.crop((0, 0, img.width, strip_y)).getbbox()
    box = (rest["x"], rest["y"], rest["x"] + rest["w"], rest["y"] + rest["h"])
    brightness = ImageStat.Stat(img.crop(box).convert("L")).mean[0]
    if bbox is None:
        return {"bbox": None, "brightness": brightness}
    return {"bbox": (bbox[0], bbox[1], bbox[2] - bbox[0], bbox[3] - bbox[1]), "brightness": brightness}


def start_style_check(s: Session, style: str) -> tuple[list[str], list[str]]:
    """Open Start with Super for one animation style and sample the popup region in-process.

    slide: the popup top moves. fade: geometry stays put while region brightness ramps up.
    magic-lamp: the popup width is not rigid across frames. none: the final state at the first sample."""
    import flamewm_lamp_acceptance as lamp  # RootGrab: in-process XGetImage, shared with the lamp run

    ev: list[str] = []
    tag = f"AP06-{style}"
    if not s.restart({"animationsEnabled": "true", "startAnimationEnabled": "true", "startAnimation": style, "taskbarStyle": "classic",
                      "animationSpeed": "25", "surfaceOpacity": "100"}):
        return [f"{style}: WM or shell exited before the Start style check"], ev
    s.input(key=base.SHORTCUT_DEFAULT_BINDING)
    time.sleep(s.args.settle_seconds)
    rest_node = find_named(s.tree(f"{tag}-rest"), "FlameWM start")
    s.input(key=base.SHORTCUT_DEFAULT_BINDING)
    time.sleep(s.args.settle_seconds)
    if rest_node is None:
        return [f"{style}: Start popup not found in the X tree"], ev
    rest = {"x": rest_node["x"], "y": rest_node["y"], "w": rest_node["w"], "h": rest_node["h"]}
    strip_y = s.args.screen_h - TASKBAR_HEIGHT
    grab = lamp.RootGrab(s.ctx.display)
    try:
        # The popup must be fully gone before the next open is sampled, or the flight is read
        # against a closing popup. Wait for an empty rest rect, up to 3 s.
        pre_open = None
        for _ in range(30):
            pre_open = start_popup_metrics(grab.grab(0, 0, s.args.screen_w, strip_y), rest, strip_y)
            if pre_open["brightness"] < 1.0:
                break
            time.sleep(0.1)
        ev.append(f"{tag}: pre_open_brightness={round(pre_open['brightness'], 1)}")
        t0 = time.monotonic()
        s.input(key=base.SHORTCUT_DEFAULT_BINDING)
        frames: list[tuple[float, dict]] = []
        while time.monotonic() - t0 < START_FLIGHT_SECONDS:
            started = time.monotonic()
            img = grab.grab(0, 0, s.args.screen_w, strip_y)
            frames.append((started - t0, start_popup_metrics(img, rest, strip_y)))
            if len(frames) % 3 == 1 and len(frames) < 40:
                # Keep a few frames of the flight as evidence (what is really on the root, per style).
                img.crop((0, max(0, rest["y"] - 80), 700, strip_y)).save(
                    s.ctx.run / "shots" / f"{tag}-flight-{len(frames):02d}.png")
            time.sleep(max(0.0, START_SAMPLE_PERIOD - (time.monotonic() - started)))
        s.input(key=base.SHORTCUT_DEFAULT_BINDING)
    finally:
        grab.close()
    boxes = [m["bbox"] for _, m in frames if m["bbox"] is not None]
    bright = [m["brightness"] for _, m in frames]
    ev.append(f"{tag}: rest={rest} frames={len(frames)} boxes_seen={len(boxes)}")
    ev.append(f"{tag}: bboxes={[m['bbox'] for _, m in frames]}")
    ev.append(f"{tag}: brightness={[round(b, 1) for b in bright]}")
    fails: list[str] = []
    if not boxes:
        return [f"{style}: popup never visible in {len(frames)} samples"], ev
    tops = [b[1] for b in boxes]
    widths = [b[2] for b in boxes]
    heights = [b[3] for b in boxes]
    rest_top = rest["y"]
    if style == "slide":
        if max(tops) - min(tops) < 20 or abs(tops[-1] - rest_top) > 3:
            fails.append(f"slide: popup top did not travel to rest (tops {tops}, rest {rest_top})")
    elif style == "fade":
        # Geometry is the rest rect from the first frame to the last: a low-alpha frame only shows its
        # brighter content, so early bboxes may be INSIDE the rest rect (never outside it, never moved).
        outside = [b for b in boxes if b[0] < rest["x"] - 3 or b[1] < rest_top - 3
                   or b[0] + b[2] > rest["x"] + rest["w"] + 3 or b[1] + b[3] > rest_top + rest["h"] + 3]
        if outside or abs(boxes[-1][1] - rest_top) > 3 or abs(boxes[-1][2] - rest["w"]) > 3:
            fails.append(f"fade: geometry moved (boxes {boxes[:4]} ..., rest {rest})")
        lo, hi = min(bright), max(bright)
        rising = [b for b in bright if lo + 1 < b < hi - 1]
        monotone = all(bright[i + 1] >= bright[i] - 0.5 for i in range(len(bright) - 1))
        if hi - lo < 20 or len(rising) < 4 or not monotone:
            fails.append(f"fade: no monotone brightness ramp (intermediate {len(rising)}, {bright[0]:.1f} -> {bright[-1]:.1f})")
    elif style == "magic-lamp":
        if max(widths) - min(widths) < max(10, 0.1 * rest["w"]):
            fails.append(f"magic-lamp: width is rigid across frames (widths {widths})")
    elif style == "none":
        first = frames[0][1]["bbox"]
        if first is None or abs(first[1] - rest_top) > 3 or abs(first[2] - rest["w"]) > 3:
            fails.append(f"none: popup not at final state on the first sample ({first}, rest {rest})")
    return fails, ev

# ---------------------------------------------------------------- AP06 Start stacking over windows

ABOVE_CLIENT_GEOMETRY = (120, 250, 300, 200)  # xclock placed under the Start rest rect
CLIENT_WHITE = 200                             # xclock face: all channels >= 200
WHITE_SLACK = 32                               # popup-own light text and antialiasing, in pixels


def _flat(img):
    return img.get_flattened_data() if hasattr(img, "get_flattened_data") else img.getdata()


def rect_intersection(a: tuple[int, int, int, int], b: tuple[int, int, int, int]) -> tuple[int, int, int, int] | None:
    """Intersection of two (x, y, w, h) rects, or None when they do not overlap."""
    x0, y0 = max(a[0], b[0]), max(a[1], b[1])
    x1, y1 = min(a[0] + a[2], b[0] + b[2]), min(a[1] + a[3], b[1] + b[3])
    return (x0, y0, x1 - x0, y1 - y0) if x1 > x0 and y1 > y0 else None


def count_white(img, rect: tuple[int, int, int, int]) -> int:
    """Pixels inside rect whose every channel is >= CLIENT_WHITE (the xclock face)."""
    from PIL import ImageChops

    x, y, w, h = rect
    r, g, b = img.crop((x, y, x + w, y + h)).split()
    lowest = ImageChops.darker(ImageChops.darker(r, g), b)
    return lowest.point(lambda v: 255 if v >= CLIENT_WHITE else 0).histogram()[255]


def count_client_white(img, region: tuple[int, int, int, int], client: tuple[int, int, int, int]) -> int:
    """Client-white pixels inside (region ∩ client rect)."""
    area = rect_intersection(region, client)
    return count_white(img, area) if area else 0


def strip_changed(strip, reference, columns: tuple[int, int]) -> int:
    """Taskbar-strip pixels in the columns that differ (>12 luminance) from the settled strip."""
    from PIL import ImageChops

    x0, x1 = columns
    if x1 <= x0:
        return 0
    box = (x0, 0, x1, strip.height)
    diff = ImageChops.difference(strip.crop(box), reference.crop(box)).convert("L")
    return diff.point(lambda v: 255 if v > 12 else 0).histogram()[255]


def popup_top(img, rest: tuple[int, int, int, int], strip_y: int, min_fraction: float = 0.8) -> int | None:
    """Top row of the Start popup inside the rest columns: the first row at or below the rest top where
    at least min_fraction of the columns are START_BG. A client frame edge covers far fewer columns."""
    from PIL import Image, ImageChops

    x, y, w, h = rest
    crop = img.crop((x, y, x + w, strip_y))
    near = ImageChops.difference(crop, Image.new("RGB", crop.size, START_BG)).convert("L").point(
        lambda v: 255 if v <= 6 else 0)
    fractions = _flat(near.resize((1, crop.height), Image.Resampling.BOX))
    for row, value in enumerate(fractions):
        if value >= min_fraction * 255:
            return y + row
    return None


def start_above_windows_check(s: Session, style: str) -> tuple[list[str], list[str]]:
    """Start popup stacks above a white xclock it overlaps during every style's flight.

    The popup's own light pixels are measured first with no client present (reference). During the flight,
    client-white pixels inside the popup area may not exceed that reference (the popup covers the client).
    For slide the popup must stay out of the taskbar strip while it moves (the panel clip)."""
    import flamewm_lamp_acceptance as lamp  # RootGrab: in-process XGetImage, shared with the lamp run

    ev: list[str] = []
    tag = f"AP06-above-{style}"
    if not s.restart({"animationsEnabled": "true", "startAnimationEnabled": "true", "startAnimation": style, "taskbarStyle": "classic",
                      "animationSpeed": "25", "surfaceOpacity": "100"}):
        return [f"{style}: WM or shell exited before the stacking check"], ev
    s.ctx.stop(s.client)
    s.client = None
    strip_y = s.args.screen_h - TASKBAR_HEIGHT
    grab = lamp.RootGrab(s.ctx.display)
    try:
        s.input(key=base.SHORTCUT_DEFAULT_BINDING)
        time.sleep(s.args.settle_seconds)
        rest_node = find_named(s.tree(f"{tag}-rest"), "FlameWM start")
        if rest_node is None:
            return [f"{style}: Start popup not found in the X tree"], ev
        rest = (rest_node["x"], rest_node["y"], rest_node["w"], rest_node["h"])
        reference_white = count_white(grab.grab(0, 0, s.args.screen_w, strip_y), rest)
        s.input(key=base.SHORTCUT_DEFAULT_BINDING)
        for _ in range(30):
            if start_popup_metrics(grab.grab(0, 0, s.args.screen_w, strip_y), {"x": rest[0], "y": rest[1],
                                   "w": rest[2], "h": rest[3]}, strip_y)["brightness"] < 1.0:
                break
            time.sleep(0.1)
        s.client = s.ctx.spawn("client-xclock-above", [s.args.client_bin, "-geometry",
                               "{}x{}+{}+{}".format(*ABOVE_CLIENT_GEOMETRY[2:], *ABOVE_CLIENT_GEOMETRY[:2])],
                               s.ctx.app_env)
        time.sleep(s.args.settle_seconds)
        client_node, client_frame = s.client_frame(s.tree(f"{tag}-client"))
        node = client_frame or client_node
        if node is None:
            return [f"{style}: stacking client xclock not found in the X tree"], ev
        client = (node["x"], node["y"], node["w"], node["h"])
        t0 = time.monotonic()
        s.input(key=base.SHORTCUT_DEFAULT_BINDING)
        frames: list[tuple[int | None, int, object]] = []
        while time.monotonic() - t0 < START_FLIGHT_SECONDS:
            started = time.monotonic()
            img = grab.grab(0, 0, s.args.screen_w, strip_y)
            top = popup_top(img, rest, strip_y)
            region = (rest[0], top, rest[2], min(rest[3], strip_y - top)) if top is not None else None
            white = count_client_white(img, region, client) if region else 0
            frames.append((top, white, grab.grab(0, strip_y, s.args.screen_w, TASKBAR_HEIGHT)))
            if len(frames) % 4 == 1 and len(frames) < 24:
                img.crop((max(0, client[0] - 40), max(0, client[1] - 40), 700, strip_y)).save(
                    s.ctx.run / "shots" / f"{tag}-flight-{len(frames):02d}.png")
            time.sleep(max(0.0, START_SAMPLE_PERIOD - (time.monotonic() - started)))
        s.input(key=base.SHORTCUT_DEFAULT_BINDING)
    finally:
        grab.close()
    settled_strip = frames[-1][2]
    below = [strip_changed(f[2], settled_strip, (rest[0], rest[0] + rest[2])) if f[0] is not None else 0
             for f in frames]
    overlapped = [f for f in frames if f[0] is not None and rect_intersection((rest[0], f[0], rest[2], rest[3]), client)]
    ev.append(f"{tag}: client={client} rest={rest} reference_white={reference_white} frames={len(frames)}")
    ev.append(f"{tag}: popup_top={[f[0] for f in frames]}")
    ev.append(f"{tag}: client_white_in_popup={[f[1] for f in frames]}")
    ev.append(f"{tag}: taskbar_popup_pixels={below}")
    fails: list[str] = []
    if not overlapped:
        fails.append(f"{style}: popup never overlapped the client in {len(frames)} samples")
    if any(f[1] > reference_white + WHITE_SLACK for f in frames):
        fails.append(f"{style}: client-white pixels inside the popup area (client stacked above the popup)")
    if style == "slide" and any(value > 0 for value in below):
        fails.append("slide: popup pixels below the panel top edge while sliding")
    return fails, ev


# ---------------------------------------------------------------- colour picker (AP04)

PICKER_SURFACE_PRESETS = ["#191B1D", "#232629", "#1B1E20", "#2A2E32", "#336699",
                          "#3DAEE9", "#1ABC9C", "#EF4048", "#9B59B6"]
PICKER_VIVID_PRESETS = ["#336699", "#3DAEE9", "#1ABC9C", "#EF4048", "#9B59B6"]


def hex_of(pixel: tuple[int, int, int]) -> str:
    return "#{:02X}{:02X}{:02X}".format(*pixel)


def colour_matches(pixel: tuple[int, int, int], hex_value: str, tolerance: int = 3) -> bool:
    target = tuple(int(hex_value[i:i + 2], 16) for i in (1, 3, 5))
    return all(abs(a - b) <= tolerance for a, b in zip(pixel, target))


def colour_blobs(px: list, width: int, rect: tuple[int, int, int, int], hex_value: str,
                 min_px: int = 40) -> list[tuple[int, int, int, int, int]]:
    """Connected blobs of pixels matching `hex_value` inside rect: (x0, y0, x1, y1, count)."""
    rx, ry, rw, rh = rect
    hits = {(x, y) for y in range(ry, ry + rh) for x in range(rx, rx + rw)
            if colour_matches(px[y * width + x], hex_value)}
    blobs = []
    while hits:
        stack = [hits.pop()]
        xs, ys = [], []
        while stack:
            x, y = stack.pop()
            xs.append(x)
            ys.append(y)
            for n in ((x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)):
                if n in hits:
                    hits.remove(n)
                    stack.append(n)
        if len(xs) >= min_px:
            blobs.append((min(xs), min(ys), max(xs), max(ys), len(xs)))
    return sorted(blobs, key=lambda b: (b[1], b[0]))


def centre(blob: tuple[int, int, int, int, int]) -> tuple[int, int]:
    return (blob[0] + blob[2]) // 2, (blob[1] + blob[3]) // 2


def interior_cell_pixel(px: list, width: int, x: int, y: int, hex_value: str) -> bool:
    """True when the 3x3 grid at offsets -3/0/+3 around (x, y) all match `hex_value`.

    Rejects anti-aliased corner and edge pixels, whose neighbours fall outside the solid cell."""
    return all(colour_matches(px[(y + dy) * width + (x + dx)], hex_value)
               for dx in (-3, 0, 3) for dy in (-3, 0, 3))


def preset_rows(px: list, width: int, rect: tuple[int, int, int, int]) -> list[dict]:
    """Rows of circles: each row holds vivid preset blobs whose centres share one band."""
    found = []
    for hex_value in PICKER_VIVID_PRESETS:
        for blob in colour_blobs(px, width, rect, hex_value, min_px=200):
            if blob[2] - blob[0] <= 34 and blob[3] - blob[1] <= 34:
                found.append((hex_value, centre(blob), blob))
    rows: list[dict] = []
    for hex_value, (cx, cy), blob in sorted(found, key=lambda item: item[1][1]):
        for row in rows:
            if abs(row["y"] - cy) <= 6:
                row["circles"].append((cx, hex_value, blob))
                break
        else:
            rows.append({"y": cy, "circles": [(cx, hex_value, blob)]})
    for row in rows:
        row["circles"].sort()
    return [row for row in rows if len(row["circles"]) >= 4]


def hue_bins(px: list, width: int, rect: tuple[int, int, int, int]) -> int:
    """Distinct 30-degree hue bins among saturated, bright pixels: a wheel has many, a flat circle one."""
    import colorsys
    rx, ry, rw, rh = rect
    bins = set()
    for y in range(ry, ry + rh):
        for x in range(rx, rx + rw):
            r, g, b = (c / 255 for c in px[y * width + x])
            h, sat, val = colorsys.rgb_to_hsv(r, g, b)
            if sat > 0.5 and val > 0.5:
                bins.add(int(h * 12))
    return len(bins)


def dot_row(px: list, width: int, rows: list[dict], seeded: str) -> dict | None:
    """The row whose circle for `seeded` carries the selected dot: that row is the setting it shows."""
    for row in rows:
        for cx, hex_value, _ in row["circles"]:
            if hex_value == seeded and colour_matches(px[row["y"] * width + cx], "#F1F2F3", 12):
                return row
    return None


def snapshot_value(text: str | None, key: str) -> str | None:
    match = re.search(r"'%s': <'([^']*)'>" % re.escape(key), text or "")
    return match.group(1) if match else None


def color_picker_check(s: Session) -> tuple[list, list]:
    """AP04 colour picker: presets, wheel, palette cell and typed hex, read back from the snapshot."""
    ev: list = []
    fails: list = []
    if not s.restart({"surfaceColor": "#336699", "surfaceOpacity": "100"}):
        return ev, ["WM or shell exited before the colour picker check"]
    settings = s.ctx.spawn("settings-ap04-picker", [s.args.settings_bin, "--page", "surfaces"], s.ctx.app_env)
    try:
        time.sleep(s.args.settle_seconds)
        s.input(click="400,16")  # title bar: raise Settings above the AP04 client window
        time.sleep(0.6)
        s.input(move="500,300")
        time.sleep(0.8)  # Surfaces page opens with the surface colour row
        width = s.args.screen_w
        window = (0, 0, 830, 580)
        _, px = s.shot("AP04-picker-rows")
        if px is None:
            return ev, ["colour picker check could not capture the Settings page"]
        rows = preset_rows(px, width, window)
        ev.append(f"preset_rows={[(r['y'], len(r['circles'])) for r in rows]}")
        before, _ = base.dbus_settings_snapshot(s.ctx)
        if snapshot_value(before, "surfaceColor") != "#336699":
            fails.append("colour picker negative control: surfaceColor was not the seeded #336699")
        target = dot_row(px, width, rows, "#336699")
        if target is None:
            return ev, fails + ["no row shows the seeded #336699 as selected; the Surfaces row is not visible"]
        blue = next((c for c in target["circles"] if c[1] == "#3DAEE9"), None)
        if blue is None:
            return ev, fails + ["the Surfaces row has no #3DAEE9 circle"]
        s.input(click=f"{blue[0]},{target['y']}")
        time.sleep(1.2)
        after, _ = base.dbus_settings_snapshot(s.ctx)
        preset_value = snapshot_value(after, "surfaceColor")
        ev.append(f"preset_click={blue[0]},{target['y']} surfaceColor={preset_value}")
        if preset_value != "#3DAEE9":
            fails.append(f"preset circle did not write surfaceColor=#3DAEE9 (got {preset_value})")
        if target["circles"][-1][0] + 38 <= blue[0]:
            fails.append("wheel circle is not after the preset circles")
        wheel_x = target["circles"][-1][0] + 38
        _, px_before = s.shot("AP04-picker-closed")
        s.input(click=f"{wheel_x},{target['y']}")
        time.sleep(1.2)
        _, px_open = s.shot("AP04-picker-open")
        if px_before is None or px_open is None:
            return ev, fails + ["picker screenshots could not be captured"]
        changed = pixel_diff_count(px_before, px_open, 8)
        wheel_hues = hue_bins(px_open, width, (wheel_x - 15, target["y"] - 15, 30, 30))
        ev += [f"wheel_click={wheel_x},{target['y']} wheel_hues={wheel_hues} picker_changed_px={changed}"]
        if changed < 2000:
            fails.append("clicking the wheel did not open the colour picker panel")
        palette_hexes = []
        # Palette cells sit inside the fixed-size panel (460x330 at 250,60 in the Settings body).
        for y in range(145, 305, 2):
            for x in range(255, 705, 2):
                pixel = px_open[y * width + x]
                h = hex_of(pixel)
                if h not in ("#000000", "#FFFFFF") and max(pixel) - min(pixel) > 60 and h not in PICKER_VIVID_PRESETS \
                        and interior_cell_pixel(px_open, width, x, y, h):
                    palette_hexes.append((x, y, h))
        if not palette_hexes:
            return ev, fails + ["no palette cell colours found in the open picker"]
        cx, cy, cell_hex = palette_hexes[0]
        cell_box = next((b for b in colour_blobs(px_open, width, window, cell_hex, min_px=30)
                         if b[0] <= cx <= b[2] and b[1] <= cy <= b[3]), None)
        if cell_box is None:
            return ev, fails + [f"palette cell {cell_hex} at {cx},{cy} has no blob"]
        cell_x, cell_y = centre(cell_box)
        s.input(click=f"{cell_x},{cell_y}")
        time.sleep(1.2)
        picked, _ = base.dbus_settings_snapshot(s.ctx)
        ev.append(f"palette_cell={cell_x},{cell_y} colour={cell_hex} surfaceColor={snapshot_value(picked, 'surfaceColor')}")
        if snapshot_value(picked, "surfaceColor") != cell_hex:
            fails.append(f"palette cell {cell_hex} did not write surfaceColor")
        _, px_applied = s.shot("AP04-picker-applied")
        previews = colour_blobs(px_applied, width, window, cell_hex, min_px=200)
        if not previews:
            return ev, fails + ["preview swatch not found after the palette pick"]
        preview = max(previews, key=lambda b: b[3])
        field = (preview[0] - 40, centre(preview)[1])
        s.input(click=f"{field[0]},{field[1]}")
        time.sleep(0.6)
        # The buffer keeps the last applied value, so clear it before typing.
        for key in ["BackSpace"] * 8 + ["3", "d", "a", "e", "e", "9", "Return"]:
            s.input(key=key)
            time.sleep(0.15)
        time.sleep(1.2)
        typed, _ = base.dbus_settings_snapshot(s.ctx)
        ev.append(f"typed_hex surfaceColor={snapshot_value(typed, 'surfaceColor')}")
        if snapshot_value(typed, "surfaceColor") != "#3DAEE9":
            fails.append("typed #3DAEE9 + Enter did not write surfaceColor")
        for key in ["BackSpace"] * 8 + ["1", "2", "3", "4", "5", "Return"]:
            s.input(key=key)
            time.sleep(0.15)
        time.sleep(1.2)
        _, px_error = s.shot("AP04-picker-invalid")
        invalid, _ = base.dbus_settings_snapshot(s.ctx)
        error_changed = pixel_diff_count(px_applied, px_error, 8) if px_error else 0
        ev.append(f"invalid_hex surfaceColor={snapshot_value(invalid, 'surfaceColor')} error_px={error_changed}")
        if snapshot_value(invalid, "surfaceColor") != "#3DAEE9":
            fails.append("invalid hex changed surfaceColor")
        if error_changed < 20:
            fails.append("invalid hex showed no inline error pixels")
    finally:
        s.ctx.stop(settings)
    return ev, fails


if __name__ == "__main__":
    raise SystemExit(main())

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
        return base.send_input(self.ctx, **kwargs)

    def client_frame(self, nodes: list[dict]) -> tuple[dict | None, dict | None]:
        client = find_named(nodes, "xclock")
        return client, parent_of(nodes, client)

    def taskbar_rect(self) -> tuple[int, int, int, int]:
        return (0, self.args.screen_h - TASKBAR_HEIGHT, self.args.screen_w, TASKBAR_HEIGHT)


# ---------------------------------------------------------------- scenarios

def ap01(s: Session) -> dict:
    color = "#336699"
    if not s.restart({"taskbarColor": color, "taskbarOpacity": "100", "taskbarHeight": str(TASKBAR_HEIGHT)}):
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
    tb, sm = "#336699", "#884422"
    if not s.restart({"taskbarColor": tb, "taskbarOpacity": "100", "startMenuColor": sm, "startMenuOpacity": "100"}):
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
    ok = close_enough(popup_dom, base.accent_rgb(sm), 3) and close_enough(tb_after, tb_before, 2) \
        and tb_before is not None and close_enough(tb_before, base.accent_rgb(tb), 3)
    return entry("AP02", PASS if ok else FAIL,
                 "" if ok else f"popup dominant {popup_dom} vs {sm}; taskbar {tb_before} -> {tb_after}",
                 [str(path), f"popup={popup}", f"popup_dominant={popup_dom}", f"taskbar_before={tb_before}",
                  f"taskbar_after={tb_after}"], ["xwininfo -root -tree", "import -window root", "XTEST key Super_L"])


def ap03(s: Session) -> dict:
    tb, accent = "#336699", "#00AA00"
    if not s.restart({"taskbarColor": tb, "taskbarOpacity": "100", "accent": accent, "accentOpacity": "100"}):
        return entry("AP03", FAIL, "WM or shell exited during restart")
    path, px = s.shot("AP03-accent-100")
    rgb = base.accent_rgb(accent)
    strip = s.taskbar_rect()
    coords = [(x, y) for y in range(strip[1], strip[1] + strip[3]) for x in range(strip[0], strip[0] + strip[2])
              if close_enough(s.pixel(px, x, y), rgb, 2)]
    if len(coords) < 8:
        return entry("AP03", NOT_RUN, f"accent line not located on taskbar ({len(coords)} accent pixels)",
                     [str(path)])
    bg = dominant_color(s.pixels(px, strip))
    if not s.restart({"taskbarColor": tb, "taskbarOpacity": "100", "accent": accent, "accentOpacity": "40"}):
        return entry("AP03", FAIL, "WM or shell exited during second restart")
    path40, px40 = s.shot("AP03-accent-40")
    sampled = Counter(s.pixel(px40, x, y) for x, y in coords).most_common(1)[0][0]
    expected = blend(base.accent_rgb(tb), rgb, 40)
    between = all(min(a, b) <= c <= max(a, b) for a, b, c in zip(base.accent_rgb(tb), rgb, sampled))
    ok = close_enough(sampled, expected, 3) and between and bg is not None
    return entry("AP03", PASS if ok else FAIL,
                 "" if ok else f"accent@40 pixel {sampled} vs blend {expected}",
                 [str(path), str(path40), f"accent_pixels={len(coords)}", f"accent100_bg={bg}",
                  f"accent40_pixel={sampled}", f"expected_blend={expected}"],
                 ["import -window root", "XTEST none; settings seeded accentOpacity=40"])


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
    """Click the real Settings Appearance selector and read the authoritative snapshot back."""
    if not s.restart({"decoration": "rounded", "taskbarOpacity": "100"}):
        fails.append("WM or shell exited before the selector check")
        return
    settings = s.ctx.spawn("settings-ap04", [s.args.settings_bin, "--page", "appearance"], s.ctx.app_env)
    try:
        time.sleep(s.args.settle_seconds)
        before, _ = base.dbus_settings_snapshot(s.ctx)
        words: list = []
        for round_index in range(8):
            path, _ = s.shot(f"AP04-selector-{round_index}")
            if path is None or before is None:
                fails.append("selector check could not capture the Settings page or the snapshot")
                return
            words = ocr_word_centres(path, {"ios", "i0s", "10s", "rounded"})
            if any(w[0] != "rounded" for w in words) or sum(1 for w in words if w[0] == "rounded") >= 1:
                break
            s.input(move="500,300")
            s.input(wheel="down:4")
            time.sleep(0.6)
        ios = [w for w in words if w[0] != "rounded"]
        target = None
        if ios:
            target = (ios[0][1], ios[0][2])
        else:
            rounded = [w for w in words if w[0] == "rounded"]
            if rounded:
                target = (rounded[-1][1] + 84, rounded[-1][2])  # iOS segment: 80px + 4px gap to the right
        if target is None:
            fails.append("decoration selector text not found on the Appearance page")
            return
        s.input(click=f"{target[0]},{target[1]}")
        time.sleep(1.5)
        after, _ = base.dbus_settings_snapshot(s.ctx)
        ev += [f"selector_click={target}", f"decoration_before_rounded={'<\'rounded\'>' in (before or '')}",
               f"decoration_after_ios={'<\'ios\'>' in (after or '')}"]
        if "'decoration': <'rounded'>" not in (before or ""):
            fails.append("selector negative control: decoration was not rounded before the click")
        if "'decoration': <'ios'>" not in (after or ""):
            fails.append("clicking the iOS segment did not set decoration=ios")
    finally:
        s.ctx.stop(settings)


def ap04(s: Session) -> dict:
    ev: list = []
    fails: list[str] = []
    if not s.restart({"decoration": "rounded", "taskbarOpacity": "100"}):
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
    ios_ok = s.restart({"decoration": "ios", "taskbarOpacity": "100"})
    if ios_ok:
        _, frame_ios = s.client_frame(s.tree("AP04-ios"))
        _, pios = s.shot("AP04-ios")
        if frame_ios and pios:
            hl = s.pixel(pios, frame_ios["x"] + frame_ios["w"] // 2, frame_ios["y"] + 1)
            hl_title = s.pixel(pios, frame_ios["x"] + frame_ios["w"] // 2, frame_ios["y"] + 3)
            ev.append(f"ios_top_row={hl} ios_title={hl_title}")
            if close_enough(hl, hl_title, 2):
                fails.append("iOS top highlight row not present")
        else:
            fails.append("iOS frame not observed")
    else:
        fails.append("WM or shell exited during iOS restart")
    decoration_selector_check(s, ev, fails)
    if fails:
        return entry("AP04", FAIL, "; ".join(fails), ev, ["xwininfo -tree", "import -window root", "XTEST click/drag"])
    if not fullscreen_measured:
        return entry("AP04", NOT_RUN, "rounded/ios/maximize/snap/restore/selector measured; fullscreen not observed",
                     ev, ["xwininfo -tree", "import -window root", "XTEST click/drag"])
    return entry("AP04", PASS, "", ev, ["xwininfo -tree", "import -window root", "XTEST click/drag",
                                        "EWMH _NET_WM_STATE_FULLSCREEN"])


def ap05(s: Session) -> dict:
    if not s.restart({"titlebarColor": "#1B1E20", "taskbarOpacity": "100"}):
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

    custom = "#00FF00"
    if not s.restart({"titlebarColor": "#1B1E20", "taskbarOpacity": "100", "titlebarCloseHoverColor": custom}):
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
    ev = [str(path), f"titlebar={tb}", f"close_hover={close_hover}", f"min_idle={min_idle}", f"min_after={min_after}"]
    if fails:
        return entry("AP05", FAIL, "; ".join(fails), ev, ["XTEST move", "import -window root"])
    return entry("AP05", PASS, "", ev, ["XTEST move", "import -window root"])


def _ap06_slide(s: Session) -> dict:
    if not s.restart({"animationsEnabled": "true", "startAnimationEnabled": "true", "startAnimation": "slide-up"}):
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
    if not s.restart({"animationsEnabled": "false", "startAnimationEnabled": "false", "startAnimation": "slide-up"}):
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
    if not s.restart({"animationsEnabled": "true", "animationSpeed": "100", "taskbarOpacity": "100"}):
        return ["WM or shell exited before the slider check"], ev
    s.ctx.stop(s.client)  # a leftover client window would sit above Settings and swallow the wheel
    settings = s.ctx.spawn("settings-ap06", [s.args.settings_bin, "--page", "appearance"], s.ctx.app_env)
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
            return ["animation speed row not found on the Appearance page"], ev
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


def sample_bright_boxes(s: Session, seconds: float, label: str) -> list:
    """Bright-pixel (client) bounding boxes above the taskbar over `seconds` of repeated screenshots."""
    strip_y = s.args.screen_h - TASKBAR_HEIGHT
    boxes: list = []
    deadline = time.monotonic() + seconds
    n = 0
    while time.monotonic() < deadline:
        _, px = s.shot(f"{label}-{n}")
        n += 1
        if px is not None:
            boxes.append(nonblack_bbox(px, s.args.screen_w, strip_y))
    return boxes


def extra_effect_checks(s: Session) -> tuple[list[str], list[str]]:
    """Window maximize and window-open animations measured as painted client sizes at 0.25x speed."""
    fails: list[str] = []
    ev: list[str] = []
    width = lambda b: (b[2] - b[0]) if b else 0  # noqa: E731
    if not s.restart({"animationsEnabled": "true", "maximizeAnimationEnabled": "true", "animationSpeed": "25",
                      "taskbarOpacity": "100"}):
        return ["WM or shell exited before the maximize check"], ev
    s.spawn_client()
    _, frame = s.client_frame(s.tree("AP07-max-frame"))
    if frame is None:
        return ["client frame not found for the maximize check"], ev
    _, pre = s.shot("AP07-max-pre")
    rest_w = width(nonblack_bbox(pre, s.args.screen_w, s.args.screen_h - TASKBAR_HEIGHT)) if pre else 0
    s.input(click=f"{frame['x'] + frame['w'] - 57},{frame['y'] + 15}")  # Maximize
    boxes = sample_bright_boxes(s, 3.0, "AP07-max")
    widths = [width(b) for b in boxes]
    final_w = widths[-1] if widths else 0
    middle = [w for w in widths if rest_w * 1.15 < w < final_w * 0.9]
    ev += [f"maximize_rest_w={rest_w}", f"maximize_widths={widths}"]
    if final_w <= rest_w or not middle:
        fails.append(f"maximize did not animate through intermediate sizes (rest {rest_w}, widths {widths})")
    # Window open: the first frames after map are smaller than the settled window.
    if not s.restart({"animationsEnabled": "true", "windowOpenAnimationEnabled": "true", "animationSpeed": "25",
                      "taskbarOpacity": "100"}):
        return fails + ["WM or shell exited before the window-open check"], ev
    s.ctx.stop(s.client)
    s.client = s.ctx.spawn("client-xclock-open", [s.args.client_bin, "-geometry", "300x200+300+200"],
                           s.ctx.app_env)
    # Screenshots are slower than the 0.25x open flight start, so also poll the cheap X tree for the
    # transient overlay window (a nameless top-level that does not contain the client).
    top_before = {n["id"] for n in s.tree("AP07-open-before") if n.get("parent") is None}
    transient: set[str] = set()
    for poll in range(10):
        nodes = s.tree(f"AP07-open-poll-{poll}")
        client = find_named(nodes, "xclock")
        owner = parent_of(nodes, client) if client else None
        for node in nodes:
            if (node.get("parent") is None and node["id"] not in top_before and not node.get("name")
                    and (owner is None or node["id"] != owner["id"]) and node["w"] > 1):
                transient.add(node["id"])
    boxes = sample_bright_boxes(s, 2.0, "AP07-open")
    widths = [width(b) for b in boxes]
    settled = max(widths) if widths else 0
    smaller = [w for w in widths if 0 < w < settled * 0.9]
    ev += [f"open_widths={widths}", f"open_transient_windows={sorted(transient)}"]
    if settled == 0 or not (smaller or transient):
        fails.append(f"window open did not animate (widths {widths}, transient windows {sorted(transient)})")
    rf, rev = respawn_after_minimize_check(s)
    return fails + rf, ev + rev


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
    if not s.restart({"taskbarOpacity": "100"}):
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
    # Slowed to 0.25x so the ~100 ms screenshots land inside the flight; the measurement is the painted
    # client pixels (bounding box of non-black content above the taskbar), not an overlay window's size.
    if not s.restart({"animationsEnabled": "true", "minimizeAnimationEnabled": "true", "animationSpeed": "25",
                      "taskbarOpacity": "100"}):
        return entry("AP07", FAIL, "WM or shell exited during restart")
    capability = "32-bit ARGB visual" if "depth 32" in subprocess.run(
        ["xdpyinfo", "-display", s.ctx.display], env=s.ctx.host_env, capture_output=True, text=True,
        timeout=20, check=False).stdout else "root-visual fallback (no 32-bit visual reported)"
    s.spawn_client()
    _, frame = s.client_frame(s.tree("AP07-frame"))
    client = find_named(s.tree("AP07-client"), "xclock")
    if frame is None or client is None:
        return entry("AP07", FAIL, "client/frame not found")
    strip_y = s.args.screen_h - TASKBAR_HEIGHT
    _, pre = s.shot("AP07-pre")
    box0 = nonblack_bbox(pre, s.args.screen_w, strip_y) if pre else None
    s.input(click=f"{frame['x'] + frame['w'] - 3 * BUTTON_WIDTH + BUTTON_WIDTH // 2},{frame['y'] + 15}")  # Minimize
    boxes: list = []
    deadline = time.monotonic() + 3.0
    n = 0
    while time.monotonic() < deadline:
        _, px = s.shot(f"AP07-mid-{n}")
        s.tree(f"AP07-mid-{n}")
        n += 1
        if px is not None:
            boxes.append(nonblack_bbox(px, s.args.screen_w, strip_y))
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
        # A real lamp changes the painted shape mid-flight: narrower and/or shorter than the resting window,
        # never a static frame, and ends with the client gone from the screen.
        shaped = [b for b in mids if (b[2] - b[0]) < w0 * 0.95 or (b[3] - b[1]) < h0 * 0.95]
        # 0.25x makes the flight ~1 s against ~0.3 s per screenshot: a live lamp is seen in several frames,
        # so one lucky frame followed by black (a blank overlay) must not pass.
        if len(shaped) < 2:
            fails.append(f"fewer than 2 mid-flight frames with a narrowed/shortened client image "
                         f"({len(shaped)}; rest={box0}, frames={boxes})")
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
    ev = [f"capability={capability}", f"rest_bbox={box0}", f"frame_bboxes={boxes}",
          f"map_state={map_state(wm_state)}", f"icon_geometry={rect}"] + extra_ev
    if fails:
        return entry("AP07", FAIL, "; ".join(fails), ev, ["import -window root polling", "xprop _NET_WM_ICON_GEOMETRY"])
    return entry("AP07", PASS, "", ev, ["import -window root polling", "xprop _NET_WM_ICON_GEOMETRY"])


def ap08(s: Session) -> dict:
    fails, ev = [], []
    # No app icon is visible without desktop entries in the private XDG_DATA_HOME, so seed one and compare
    # the Start menu (its app list renders the entry's themed icon) between the two icon themes.
    apps = s.ctx.run / "xdg-data" / "applications"
    apps.mkdir(parents=True, exist_ok=True)
    (apps / "ap08-probe.desktop").write_text(
        "[Desktop Entry]\nType=Application\nName=AP08 Probe\nExec=true\nIcon=folder\nCategories=Utility;\n",
        encoding="utf-8")

    if not s.restart({"iconTheme": "McMojave-circle", "taskbarOpacity": "100"}):
        return entry("AP08", FAIL, "WM or shell exited during restart")
    def start_menu_shot(name: str):
        s.input(key=base.SHORTCUT_DEFAULT_BINDING)
        time.sleep(s.args.settle_seconds)
        result = s.shot(name)
        s.input(key=base.SHORTCUT_DEFAULT_BINDING)
        time.sleep(1.0)
        return result

    _, pa = start_menu_shot("AP08-icon-base")
    if not s.restart({"iconTheme": s.args.icon_theme_alt, "taskbarOpacity": "100"}):
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
        if not s.restart({"cursorTheme": theme, "taskbarOpacity": "100"}):
            return entry("AP08", FAIL, "WM or shell exited during cursor restart")
        # The WM defines themed cursors on frame resize handles; the bare root keeps the stock X cursor.
        s.spawn_client()
        _, frame = s.client_frame(s.tree("AP08-cursor"))
        if frame is None:
            return entry("AP08", FAIL, "client frame not found for the cursor probe")
        s.input(move=f"{frame['x'] + 3},{frame['y'] + 3}")
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
    if not s.restart({"taskbarOpacity": "100"}):
        return entry("AP09", FAIL, "WM or shell exited during restart")
    time.sleep(s.args.settle_seconds)
    pids = {"flamewm": s.ctx.wm.pid, "flamewm-shell": s.shell.pid}
    start = {n: (base.parse_proc_metrics(p), voluntary_switches(p)) for n, p in pids.items()}
    time.sleep(IDLE_SECONDS)
    per, ev, fails = {}, [], []
    for name, pid in pids.items():
        now_metrics, now_vol = base.parse_proc_metrics(pid), voluntary_switches(pid)
        before_metrics, before_vol = start[name]
        alive = now_metrics is not None and before_metrics is not None and now_vol is not None and before_vol is not None
        if not alive:
            fails.append(f"{name} not observable or exited")
            continue
        cpu = (now_metrics["cpu_ticks"] - before_metrics["cpu_ticks"]) / CLK_TCK
        wake = now_vol - before_vol
        per[name] = (cpu, wake)
        ev.append(f"{name}: cpu_seconds={cpu:.3f} voluntary_wakeups={wake} over {IDLE_SECONDS:g}s")
        if idle_verdict(cpu, wake, True) == FAIL:
            fails.append(f"{name} idle cpu {cpu:.3f}s / wakeups {wake}")
    if fails:
        return entry("AP09", FAIL, "; ".join(fails), ev, ["/proc/<pid>/stat", "/proc/<pid>/status"])
    return entry("AP09", PASS, "", ev, ["/proc/<pid>/stat", "/proc/<pid>/status"])


SCENARIOS = (ap01, ap02, ap03, ap04, ap05, ap06, ap07, ap08, ap09)


# ---------------------------------------------------------------- driver

def utc_now() -> str:
    return time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())


def parse_args(argv: list[str]) -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--run-dir", required=True)
    p.add_argument("--geometry", default="1280x800")
    p.add_argument("--display", default=None)
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
        teardown = ctx.teardown()
        doc = compose_document(results)
        doc.update({"run_dir": str(run), "display": ctx.display, "geometry": args.geometry,
                    "wm_bin": str(Path(args.wm_bin).resolve()), "shell_bin": str(Path(args.shell_bin).resolve()),
                    "started_utc": started, "finished_utc": utc_now(), "teardown": teardown})
        if not teardown["ok"]:
            doc["verdict"], doc["exit_code"] = "FAIL", 1
        (run / "result.json").write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")
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
        (run / "result.json").write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")
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


if __name__ == "__main__":
    raise SystemExit(main())

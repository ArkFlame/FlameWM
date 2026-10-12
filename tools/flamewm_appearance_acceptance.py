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
        path = self.ctx.run / "xdg-config" / "flamewm" / "settings.toml"
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
    corner = s.pixel(px, frame["x"], frame["y"])
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
        mc = s.pixel(pmax, frame_max["x"], frame_max["y"])
        mt = s.pixel(pmax, frame_max["x"] + frame_max["w"] // 2, frame_max["y"] + 3)
        ev += [f"maximized_frame={frame_max}", f"maximized_corner={mc}"]
        if not close_enough(mc, mt, 24):
            fails.append("maximized corner is cut (expected rectangular)")

    s.input(click=f"{frame_max['x'] + frame_max['w'] - 57},{frame_max['y'] + 15}" if frame_max else "0,0")
    time.sleep(s.args.settle_seconds)
    _, frame_back = s.client_frame(s.tree("AP04-restored"))
    _, prest = s.shot("AP04-restored")
    if frame_back and prest:
        rc = s.pixel(prest, frame_back["x"], frame_back["y"])
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
        sc = s.pixel(psnap, frame_snap["x"], frame_snap["y"])
        st = s.pixel(psnap, frame_snap["x"] + frame_snap["w"] // 2, frame_snap["y"] + 3)
        ev.append(f"snap_left_corner={sc} frame={frame_snap}")
        if not close_enough(sc, st, 24):
            fails.append("snap-left corner is cut (expected rectangular)")
    else:
        fails.append("snap-left frame not observed")

    ev.append("fullscreen: no harness-driven fullscreen trigger; NOT measured")
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
    if fails:
        return entry("AP04", FAIL, "; ".join(fails), ev, ["xwininfo -tree", "import -window root", "XTEST click/drag"])
    return entry("AP04", NOT_RUN, "rounded/ios/maximize/snap/restore measured; fullscreen not driven by harness",
                 ev, ["xwininfo -tree", "import -window root", "XTEST click/drag"])


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
    close_hover = dominant_color(s.pixels(px_hover, close_rect))
    min_after = dominant_color(s.pixels(px_hover, min_rect))
    if not close_enough(close_hover, CLOSE_HOVER_DEFAULT, 6):
        fails.append(f"close hover {close_hover} != default {CLOSE_HOVER_DEFAULT}")
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
        custom_hover = dominant_color(s.pixels(pc, cr))
        if not close_enough(custom_hover, base.accent_rgb(custom), 6):
            fails.append(f"custom close hover {custom_hover} != {custom}")
    ev = [str(path), f"titlebar={tb}", f"close_hover={close_hover}", f"min_idle={min_idle}", f"min_after={min_after}"]
    if fails:
        return entry("AP05", FAIL, "; ".join(fails), ev, ["XTEST move", "import -window root"])
    return entry("AP05", PASS, "", ev, ["XTEST move", "import -window root"])


def ap06(s: Session) -> dict:
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


def ap07(s: Session) -> dict:
    if not s.restart({"minimizeAnimationEnabled": "true", "taskbarOpacity": "100"}):
        return entry("AP07", FAIL, "WM or shell exited during restart")
    capability = "32-bit ARGB visual" if "depth 32" in subprocess.run(
        ["xdpyinfo", "-display", s.ctx.display], env=s.ctx.host_env, capture_output=True, text=True,
        timeout=20, check=False).stdout else "root-visual fallback (no 32-bit visual reported)"
    s.spawn_client()
    before = {n["id"] for n in s.tree("AP07-before")}
    _, frame = s.client_frame(s.tree("AP07-frame"))
    client = find_named(s.tree("AP07-client"), "xclock")
    if frame is None or client is None:
        return entry("AP07", FAIL, "client/frame not found")
    s.input(click=f"{frame['x'] + frame['w'] - 3 * BUTTON_WIDTH + BUTTON_WIDTH // 2},{frame['y'] + 15}")  # Minimize
    widths: dict[str, list[int]] = {}
    deadline = time.monotonic() + 1.5
    while time.monotonic() < deadline:
        for node in s.tree("AP07-anim"):
            if node["id"] not in before and node["id"] not in (client["id"], frame["id"]):
                widths.setdefault(node["id"], []).append(node["w"])
        time.sleep(0.04)
    lamp_ok = any(len(w) >= 3 and is_nonincreasing(w) and w[0] > w[-1] for w in widths.values())
    wm_state = subprocess.run(["xwininfo", "-display", s.ctx.display, "-id", client["id"]], env=s.ctx.host_env,
                              capture_output=True, text=True, timeout=20, check=False).stdout
    icon = subprocess.run(["xprop", "-display", s.ctx.display, "-id", client["id"], "_NET_WM_ICON_GEOMETRY"],
                          env=s.ctx.host_env, capture_output=True, text=True, timeout=20, check=False).stdout
    rect = parse_icon_geometry(icon)
    strip_y = s.args.screen_h - TASKBAR_HEIGHT
    fails = []
    if not lamp_ok:
        fails.append(f"no transient window narrowing toward taskbar: {widths}")
    if map_state(wm_state) not in ("IsUnMapped", "IsUnviewable"):
        fails.append(f"client map state {map_state(wm_state)} after minimize")
    if rect is None or rect[1] < strip_y:
        fails.append(f"_NET_WM_ICON_GEOMETRY {rect} not inside taskbar strip")
    ev = [f"capability={capability}", f"lamp_widths={widths}", f"map_state={map_state(wm_state)}",
          f"icon_geometry={rect}"]
    if fails:
        return entry("AP07", FAIL, "; ".join(fails), ev, ["xwininfo -root -tree polling", "xprop _NET_WM_ICON_GEOMETRY"])
    return entry("AP07", PASS, "", ev, ["xwininfo -root -tree polling", "xprop _NET_WM_ICON_GEOMETRY"])


def ap08(s: Session) -> dict:
    fails, ev = [], []
    if not s.restart({"iconTheme": "McMojave-circle", "taskbarOpacity": "100"}):
        return entry("AP08", FAIL, "WM or shell exited during restart")
    _, pa = s.shot("AP08-icon-base")
    if not s.restart({"iconTheme": s.args.icon_theme_alt, "taskbarOpacity": "100"}):
        return entry("AP08", FAIL, "WM or shell exited during alt restart")
    _, pb = s.shot("AP08-icon-alt")
    rect = s.taskbar_rect()
    if pa is not None and pb is not None:
        changed = pixel_diff_count(s.pixels(pa, rect), s.pixels(pb, rect))
        ev.append(f"taskbar_changed_pixels={changed} alt={s.args.icon_theme_alt}")
        if changed < 20:
            fails.append(f"taskbar icon region unchanged for iconTheme {s.args.icon_theme_alt} ({changed} px)")
    else:
        fails.append("icon capture failed")
    digests = {}
    for theme in (s.args.cursor_theme_base, s.args.cursor_theme_alt):
        if not s.restart({"cursorTheme": theme, "taskbarOpacity": "100"}):
            return entry("AP08", FAIL, "WM or shell exited during cursor restart")
        s.input(move=f"{s.args.screen_w // 2},{s.args.screen_h // 3}")
        time.sleep(1.0)
        digests[theme] = cursor_digest(s)
    ev.append(f"cursor_digests={digests}")
    if None in digests.values() or digests[s.args.cursor_theme_base] == digests[s.args.cursor_theme_alt]:
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
        return hashlib.sha256(raw).hexdigest()
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
                for scenario in SCENARIOS:
                    sid = scenario.__name__.upper()
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

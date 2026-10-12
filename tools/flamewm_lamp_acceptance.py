#!/usr/bin/env python3
"""Native Magic Lamp bend acceptance (LAMP01..LAMP03) on a private Xephyr.

Usage:
  tools/flamewm_lamp_acceptance.py --run-dir DIR [--geometry WxH] [--display :N]
      [--wm-bin PATH] [--shell-bin PATH] [--settle-seconds S]

What it does:
  * reuses the appearance runner's Session (owned Xephyr, WM, Shell, private HOME/XDG/DBus, XTEST input);
  * starts a 40px black/white checkerboard client (this file, `--checker-client`, python-Xlib, WM_NAME and
    WM_CLASS 'LampCheckers') and minimizes it at animationSpeed=25 (0.25x);
  * samples screenshots during the flight and measures, per frame, the bright pixels above the taskbar:
    bounding box plus per-row widths (span of bright pixels in each row);
  * LAMP01 bend: dock-side (bottom) rows narrower than top rows by more than max(6 px, 8% of bbox width),
    monotone narrowing toward the dock side, a non-linear profile (a linear taper is a trapezoid, not a bend),
    in >= 2 mid frames, and the bbox centre approaches the _NET_WM_ICON_GEOMETRY centre across >= 2 steps;
  * LAMP02 restore reverses: restore flight widths, reversed, resampled and compared to the minimize widths;
  * LAMP03 ends at icon / frame removed: last visible frame at the icon, client and frame unmapped after
    minimize, frame mapped again after the taskbar restore.
  * writes DIR/result.json. Statuses: PASS, FAIL, NOT_RUN, ENVIRONMENT_BLOCKED. Fails closed.

Exit status: 0 when all LAMP IDs PASS, 1 when any FAILs, 2 usage error, 3 environment blocked.
"""

from __future__ import annotations

import argparse
import ctypes
import ctypes.util
import json
import math
import os
import re
import secrets
import shutil
import subprocess
import sys
import time
from pathlib import Path

from PIL import Image, ImageChops

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))

import flamewm_appearance_acceptance as ap  # noqa: E402  (Session, nonblack_bbox, map helpers, run-dir checks)
import flamewm_settings_acceptance as base  # noqa: E402

ROOT = base.ROOT
SCHEMA = "flamewm-lamp-acceptance/1"
IDS = ("LAMP01", "LAMP02", "LAMP03")
TITLES = {
    "LAMP01": "Magic Lamp bend: dock-side rows contract first, not a rigid scale",
    "LAMP02": "Magic Lamp restore reverses the minimize deformation",
    "LAMP03": "Magic Lamp ends at the taskbar icon and the frame is removed",
}
PASS, FAIL = base.STATUS_PASS, base.STATUS_FAIL
NOT_RUN, ENV_BLOCKED = base.STATUS_NOT_RUN, base.STATUS_ENV_BLOCKED
STATUSES = (PASS, FAIL, NOT_RUN, ENV_BLOCKED)

FLIGHT_SECONDS = 3.0
SAMPLE_PERIOD = 0.05  # in-process XGetImage capture: ~15-25 frames per ~1 s flight
ICON_END_TOLERANCE_PX = 80.0   # last visible frame centre to icon centre (LAMP03)
# Interior phases only: the first/last visible frame is set by the visibility edge (a 1-35 px sliver
# sampled at a random phase), so comparing the endpoints measures sampling, not the deformation.
MIRROR_PHASES = (0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875)
MIRROR_MIN_FRAMES = 3  # fewer frames would resample into a trivially passing line
CHECKER_SQUARE = 40
CHECKER_COLS, CHECKER_ROWS = 8, 6


# ---------------------------------------------------------------- pure metric (no X server)

def mask_profile(img: Image.Image) -> tuple[tuple[int, int, int, int] | None, list[int]]:
    """Bright (all channels >= 200) mask of an RGB image: inclusive bbox and per-row span (last - first + 1).

    Rows inside the bbox without bright pixels count as 0. Uses PIL only, so one frame costs a few ms."""
    r, g, b = img.split()
    mask = ImageChops.darker(ImageChops.darker(r, g), b).point(lambda v: 255 if v >= 200 else 0)
    box = mask.getbbox()
    if box is None:
        return None, []
    x0, y0, x1, y1 = box  # PIL boxes are end-exclusive
    widths: list[int] = []
    for y in range(y0, y1):
        row = mask.crop((x0, y, x1, y + 1)).getbbox()
        widths.append(row[2] - row[0] if row else 0)
    return (x0, y0, x1 - 1, y1 - 1), widths


def linear_residual(values: list[int]) -> float:
    """Largest absolute deviation of the values from their least-squares line over the row index."""
    n = len(values)
    if n < 3:
        return 0.0
    mx, my = (n - 1) / 2, sum(values) / n
    var = sum((i - mx) ** 2 for i in range(n))
    slope = sum((i - mx) * (v - my) for i, v in enumerate(values)) / var
    return max(abs(v - (my + slope * (i - mx))) for i, v in enumerate(values))


def bend_frame(bbox: tuple[int, int, int, int], widths: list[int]) -> dict:
    """Bend test for one mid-flight frame.

    A genie narrows toward the dock but its widest part is not necessarily the very first rows (the
    titlebar is narrower than the checker area just below it), so the criterion is peak width versus
    the dock-side third, plus a profile that is not a straight taper (a linear taper is a trapezoid)."""
    k = max(1, len(widths) // 3)
    win = max(5, len(widths) // 12)
    smooth = [sum(widths[max(0, i - win // 2):i + win // 2 + 1]) / len(widths[max(0, i - win // 2):i + win // 2 + 1])
              for i in range(len(widths))]
    peak = max(smooth) if smooth else 0.0
    bottom = sum(widths[-k:]) / k
    top = sum(widths[:k]) / k
    bbox_w = bbox[2] - bbox[0] + 1
    drop = peak - bottom
    threshold = max(6.0, 0.08 * bbox_w)
    residual = linear_residual(widths)
    residual_threshold = max(3.0, 0.04 * bbox_w)
    # A genie's widest part is above the dock: a peak inside the dock-side third is a flare, not a neck.
    peak_in_upper_part = smooth.index(peak) < 2 * len(widths) // 3 if smooth else False
    # Informational: never wider than the narrowest point so far (+tol) AFTER the peak.
    tol = max(2.0, 0.04 * bbox_w)
    running_min, monotone = float("inf"), True
    for value in smooth[smooth.index(peak):] if smooth else []:
        if value > running_min + tol:
            monotone = False
            break
        running_min = min(running_min, value)
    return {"bbox": list(bbox), "top_third": round(top, 1), "bottom_third": round(bottom, 1),
            "peak": round(peak, 1), "drop": round(drop, 1), "threshold": round(threshold, 1),
            "monotone": monotone, "linear_residual": round(residual, 1),
            "peak_row": smooth.index(peak) if smooth else None,
            "bent": drop > threshold and residual > residual_threshold and monotone and peak_in_upper_part}


def bend_verdict(rest_bbox: tuple[int, int, int, int] | None, frames: list[tuple[tuple[int, int, int, int] | None, list[int]]],
                 icon_centre: tuple[float, float] | None) -> tuple[bool, list[str], dict]:
    """frames: (bbox, row widths) in time order. Rigid scale, static, empty overlay and linear taper all fail."""
    mids = [(b, w) for b, w in frames if b is not None and b != rest_bbox]
    detail: dict = {"rest_bbox": list(rest_bbox) if rest_bbox else None, "mid_frames": len(mids)}
    if len(mids) < 2:
        return False, [f"fewer than 2 mid-flight frames ({len(mids)}); static or empty overlay"], detail
    per = [bend_frame(b, w) for b, w in mids]
    detail["frames"] = per
    bent = sum(1 for f in per if f["bent"])
    detail["bent_frames"] = bent
    reasons: list[str] = []
    if bent < 2:
        reasons.append(f"only {bent} of {len(mids)} mid frames bend (dock-side rows narrower by > max(6px, 8%), "
                       f"monotone, non-linear; need >= 2)")
    steps = 0
    if icon_centre is not None:
        dist = [math.hypot((b[0] + b[2]) / 2 - icon_centre[0], (b[1] + b[3]) / 2 - icon_centre[1]) for b, _ in mids]
        steps = sum(1 for a, c in zip(dist, dist[1:]) if c < a - 1.0)
        detail["icon_distances"] = [round(d, 1) for d in dist]
        if steps < 2:
            reasons.append(f"bbox approached the icon in {steps} steps (need >= 2)")
    else:
        reasons.append("no _NET_WM_ICON_GEOMETRY centre to approach")
    detail["approach_steps"] = steps
    return not reasons, reasons, detail


def value_at(points: list[tuple[float, float]], phase: float) -> float:
    """Piecewise-linear width over sorted (phase, width) points; phases outside the samples clamp."""
    if phase <= points[0][0]:
        return points[0][1]
    for (p0, v0), (p1, v1) in zip(points, points[1:]):
        if phase <= p1:
            return v0 if p1 == p0 else v0 + (v1 - v0) * (phase - p0) / (p1 - p0)
    return points[-1][1]


def mirror_verdict(min_series: list[tuple[float, int]], restore_series: list[tuple[float, int]],
                   rest_width: int) -> tuple[bool, list[str], dict]:
    """Mirror check on flight progress: each series is (distance of the bbox centre to the icon, bbox width).

    Progress is distance / (largest distance seen in either flight), so 1 is the rest position and 0 the icon.
    The restore flight reverses the minimize flight: its width at progress x must equal the minimize width at
    progress x, within max(6px, 8% of the rest width) at every interior progress. Frame timestamps are not used:
    at 50 ms cadence a sampling phase of one frame moves a steep part of the lamp by 50+ px, so the time
    axis measures the sampler, not the deformation. No direction-only fallback."""
    if len(min_series) < MIRROR_MIN_FRAMES or len(restore_series) < MIRROR_MIN_FRAMES:
        return False, [f"restore mirror needs >= {MIRROR_MIN_FRAMES} mid frames per flight (minimize "
                       f"{len(min_series)}, restore {len(restore_series)})"], {}
    ref = max(d for d, _ in min_series + restore_series)
    if ref <= 0:
        return False, ["mirror flights have no distance to the icon"], {}
    a = sorted((d / ref, float(w)) for d, w in min_series)
    b = sorted((d / ref, float(w)) for d, w in restore_series)
    pairs = [(round(value_at(a, x), 1), round(value_at(b, x), 1)) for x in MIRROR_PHASES]
    tol = max(6.0, 0.08 * rest_width)
    diffs = [abs(m - r) for m, r in pairs]
    detail = {"progress_grid": list(MIRROR_PHASES), "minimize": [m for m, _ in pairs],
              "restore_at_same_progress": [r for _, r in pairs], "tolerance": round(tol, 1),
              "max_diff": round(max(diffs), 1)}
    if max(diffs) > tol:
        return False, [f"restore does not mirror minimize: max diff {max(diffs):.1f} px > tolerance {tol:.1f} px"], detail
    return True, [], detail


def centre_distance(box: tuple[int, int, int, int], icon_centre: tuple[float, float]) -> float:
    return math.hypot((box[0] + box[2]) / 2 - icon_centre[0], (box[1] + box[3]) / 2 - icon_centre[1])


def entry(sid: str, status: str, reason: str = "", evidence: list | None = None, commands: list | None = None) -> dict:
    if status not in STATUSES:
        raise ValueError(f"unknown status {status!r}")
    return {"id": sid, "title": TITLES[sid], "status": status, "reason": reason,
            "evidence": evidence or [], "command": " && ".join(commands or []) or "none"}


def compose_document(results: dict[str, dict]) -> dict:
    """Every LAMP ID appears exactly once. PASS needs a PASS entry; missing IDs are NOT_RUN."""
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


# ---------------------------------------------------------------- checker client (runs inside the private display)

def checker_client() -> int:
    """40px black/white checkerboard window via ctypes on libX11 (stdlib only: runs under `python3 -I`).

    Redraws on every Expose; runs until terminated by the owner."""
    import ctypes
    import ctypes.util

    vp, ul, ui, it = ctypes.c_void_p, ctypes.c_ulong, ctypes.c_uint, ctypes.c_int
    xlib = ctypes.cdll.LoadLibrary(ctypes.util.find_library("X11") or "libX11.so.6")
    sigs = {
        "XOpenDisplay": (vp, [ctypes.c_char_p]),
        "XDefaultScreen": (it, [vp]),
        "XRootWindow": (ul, [vp, it]),
        "XBlackPixel": (ul, [vp, it]),
        "XWhitePixel": (ul, [vp, it]),
        "XCreateSimpleWindow": (ul, [vp, ul, it, it, ui, ui, ui, ul, ul]),
        "XStoreName": (it, [vp, ul, ctypes.c_char_p]),
        "XInternAtom": (ul, [vp, ctypes.c_char_p, it]),
        "XChangeProperty": (it, [vp, ul, ul, ul, it, it, ctypes.c_char_p, it]),
        "XSelectInput": (it, [vp, ul, ctypes.c_long]),
        "XMapWindow": (it, [vp, ul]),
        "XCreateGC": (vp, [vp, ul, ctypes.c_ulong, vp]),
        "XSetForeground": (it, [vp, vp, ul]),
        "XFillRectangle": (it, [vp, ul, vp, it, it, ui, ui]),
        "XFlush": (it, [vp]),
        "XNextEvent": (it, [vp, ctypes.c_char_p]),
    }
    for name, (restype, argtypes) in sigs.items():
        fn = getattr(xlib, name)
        fn.restype, fn.argtypes = restype, argtypes
    expose_event, exposure_mask, xa_string = 12, 1 << 15, 31
    dpy = xlib.XOpenDisplay(None)
    if not dpy:
        return 3
    screen = xlib.XDefaultScreen(dpy)
    root = xlib.XRootWindow(dpy, screen)
    width, height = CHECKER_COLS * CHECKER_SQUARE, CHECKER_ROWS * CHECKER_SQUARE
    win = xlib.XCreateSimpleWindow(dpy, root, 300, 200, width, height, 0, xlib.XBlackPixel(dpy, screen),
                                   xlib.XBlackPixel(dpy, screen))
    xlib.XStoreName(dpy, win, b"LampCheckers")
    wm_class = b"lampcheckers\0LampCheckers\0"
    xlib.XChangeProperty(dpy, win, xlib.XInternAtom(dpy, b"WM_CLASS", 0), xa_string, 8, 0, wm_class, len(wm_class))
    xlib.XSelectInput(dpy, win, exposure_mask)
    gc = xlib.XCreateGC(dpy, win, 0, None)
    white, black = xlib.XWhitePixel(dpy, screen), xlib.XBlackPixel(dpy, screen)
    xlib.XMapWindow(dpy, win)
    xlib.XFlush(dpy)
    event = ctypes.create_string_buffer(256)
    while True:
        xlib.XNextEvent(dpy, event)
        if ctypes.c_int.from_buffer(event).value != expose_event:
            continue
        for row in range(CHECKER_ROWS):
            for col in range(CHECKER_COLS):
                xlib.XSetForeground(dpy, gc, white if (row + col) % 2 == 0 else black)
                xlib.XFillRectangle(dpy, win, gc, col * CHECKER_SQUARE, row * CHECKER_SQUARE,
                                    CHECKER_SQUARE, CHECKER_SQUARE)
        xlib.XFlush(dpy)


# ---------------------------------------------------------------- live scenarios

class RootGrab:
    """In-process XGetImage of the owned display root (ctypes on libX11); no `import` subprocess per frame."""

    class _XImage(ctypes.Structure):
        _fields_ = [("width", ctypes.c_int), ("height", ctypes.c_int), ("xoffset", ctypes.c_int),
                    ("format", ctypes.c_int), ("data", ctypes.c_void_p), ("byte_order", ctypes.c_int),
                    ("bitmap_unit", ctypes.c_int), ("bitmap_bit_order", ctypes.c_int), ("bitmap_pad", ctypes.c_int),
                    ("depth", ctypes.c_int), ("bytes_per_line", ctypes.c_int), ("bits_per_pixel", ctypes.c_int)]

    def __init__(self, display: str) -> None:
        vp, ul, it, uint = ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_uint
        ximage_p = ctypes.POINTER(self._XImage)
        xlib = ctypes.cdll.LoadLibrary(ctypes.util.find_library("X11") or "libX11.so.6")
        sigs = {
            "XOpenDisplay": (vp, [ctypes.c_char_p]),
            "XCloseDisplay": (it, [vp]),
            "XDefaultRootWindow": (ul, [vp]),
            "XGetImage": (ximage_p, [vp, ul, it, it, uint, uint, ul, it]),
            "XDestroyImage": (it, [ximage_p]),
        }
        for name, (restype, argtypes) in sigs.items():
            fn = getattr(xlib, name)
            fn.restype, fn.argtypes = restype, argtypes
        self._xlib = xlib
        self._dpy = xlib.XOpenDisplay(display.encode("ascii"))
        if not self._dpy:
            raise OSError(f"XOpenDisplay({display}) failed")
        self._root = xlib.XDefaultRootWindow(self._dpy)

    def grab(self, x: int, y: int, w: int, h: int) -> Image.Image:
        all_planes, zpixmap = (1 << 64) - 1, 2
        ptr = self._xlib.XGetImage(self._dpy, self._root, x, y, w, h, all_planes, zpixmap)
        if not ptr:
            raise RuntimeError("XGetImage returned NULL")
        try:
            img = ptr.contents
            if img.bits_per_pixel != 32:
                raise RuntimeError(f"unsupported bits_per_pixel {img.bits_per_pixel}")
            raw = ctypes.string_at(img.data, img.bytes_per_line * img.height)
            return Image.frombytes("RGB", (img.width, img.height), raw, "raw", "BGRX", img.bytes_per_line, 1)
        finally:
            self._xlib.XDestroyImage(ptr)

    def close(self) -> None:
        self._xlib.XCloseDisplay(self._dpy)


def sample_flight(s: ap.Session, grab: RootGrab, label: str) -> list[dict]:
    """In-process captures every SAMPLE_PERIOD for FLIGHT_SECONDS; each frame keeps its time, bbox and row widths.

    Only the region above the taskbar strip is captured; visible frames keep a PNG crop of the bright bbox."""
    strip_y = s.args.screen_h - ap.TASKBAR_HEIGHT
    frames: list[dict] = []
    deadline = time.monotonic() + FLIGHT_SECONDS
    n = 0
    while time.monotonic() < deadline:
        started = time.monotonic()
        img = grab.grab(0, 0, s.args.screen_w, strip_y)
        bbox, widths = mask_profile(img)
        shot = None
        if bbox is not None:
            shot = s.ctx.run / "shots" / f"{label}-{n}.png"
            img.crop((bbox[0], bbox[1], bbox[2] + 1, bbox[3] + 1)).save(shot)
        frames.append({"t": started, "bbox": bbox, "widths": widths, "shot": str(shot) if shot else None})
        n += 1
        time.sleep(max(0.0, SAMPLE_PERIOD - (time.monotonic() - started)))
    return frames


def icon_centre_of(s: ap.Session, win_id: str) -> tuple[tuple[int, int, int, int] | None, tuple[float, float] | None]:
    text = subprocess.run(["xprop", "-display", s.ctx.display, "-id", win_id, "_NET_WM_ICON_GEOMETRY"],
                          env=s.ctx.host_env, capture_output=True, text=True, timeout=20, check=False).stdout
    rect = ap.parse_icon_geometry(text)
    if rect is None:
        return None, None
    return rect, (rect[0] + rect[2] / 2, rect[1] + rect[3] / 2)


def run_lamp(s: ap.Session, grab: RootGrab) -> dict[str, dict]:
    settings = {"animationsEnabled": "true", "minimizeAnimationEnabled": "true", "animationSpeed": "25",
                "taskbarOpacity": "100"}
    if not s.restart(settings):
        return {sid: entry(sid, FAIL, "WM or shell exited during restart") for sid in IDS}
    client_proc = s.ctx.spawn("client-lamp", [sys.executable, "-I", str(Path(__file__).resolve()), "--checker-client"],
                              s.ctx.app_env)
    s.client = client_proc
    time.sleep(s.args.settle_seconds)
    client, frame = None, None
    deadline = time.monotonic() + 10.0
    while time.monotonic() < deadline:
        nodes = s.tree("LAMP-frame")
        client = ap.find_named(nodes, "LampCheckers")
        frame = ap.parent_of(nodes, client) if client else None
        if frame is not None:
            break
        time.sleep(0.25)
    if client is None or frame is None:
        return {sid: entry(sid, FAIL, "checker client/frame not found") for sid in IDS}
    strip_y = s.args.screen_h - ap.TASKBAR_HEIGHT
    rest_img = grab.grab(0, 0, s.args.screen_w, strip_y)
    rest_path = s.ctx.run / "shots" / "LAMP-rest.png"
    rest_img.save(rest_path)
    rest_bbox, _ = mask_profile(rest_img)
    if rest_bbox is None:
        return {sid: entry(sid, FAIL, "checker pixels not found before minimize", [str(rest_path)]) for sid in IDS}
    rest_width = rest_bbox[2] - rest_bbox[0] + 1

    s.input(click=f"{frame['x'] + frame['w'] - 3 * ap.BUTTON_WIDTH + ap.BUTTON_WIDTH // 2},{frame['y'] + 15}")
    mins = sample_flight(s, grab, "LAMP-min")
    icon_rect, icon_centre = icon_centre_of(s, client["id"])
    min_frames = [(f["bbox"], f["widths"]) for f in mins]
    ok1, why1, detail1 = bend_verdict(rest_bbox, min_frames, icon_centre)
    min_shots = [f["shot"] for f in mins if f["shot"]]
    ev1 = [str(rest_path), f"rest_bbox={rest_bbox}", f"icon_geometry={icon_rect}", f"detail={json.dumps(detail1)}"] + min_shots
    results: dict[str, dict] = {}
    results["LAMP01"] = entry("LAMP01", PASS if ok1 else FAIL, "; ".join(why1), ev1,
                              ["XGetImage root (in-process)", "xprop _NET_WM_ICON_GEOMETRY"])

    # LAMP03 (minimize half): the flight ends at the icon with the client and its frame unmapped.
    fails3: list[str] = []
    last_mid = [f for f in mins if f["bbox"] is not None and f["bbox"] != rest_bbox]
    if mins and mins[-1]["bbox"] is not None:
        fails3.append(f"client pixels still on screen after the flight: {mins[-1]['bbox']}")
    if icon_rect is None or icon_rect[1] < strip_y:
        fails3.append(f"_NET_WM_ICON_GEOMETRY {icon_rect} not inside taskbar strip")
    if last_mid and icon_centre is not None:
        def dist(b: tuple[int, int, int, int]) -> float:
            return math.hypot((b[0] + b[2]) / 2 - icon_centre[0], (b[1] + b[3]) / 2 - icon_centre[1])

        first_d, end_d = dist(last_mid[0]["bbox"]), dist(last_mid[-1]["bbox"])
        # Screenshots are ~0.3 s apart in a ~1 s flight, so the last SAMPLED frame is rarely at the icon;
        # the flight must still converge on it (closer than the first sampled frame, within half the
        # distance of the first) and finish with the client and frame unmapped (checked below).
        if not (end_d < first_d and end_d <= 0.75 * first_d):
            fails3.append(f"flight does not converge on the icon (first {first_d:.1f}px, last visible {end_d:.1f}px)")
    elif not last_mid:
        fails3.append("no visible mid-flight frame to check the end against the icon")
    unmapped_client = ap.map_state(subprocess.run(["xwininfo", "-display", s.ctx.display, "-id", client["id"]],
                                                  env=s.ctx.host_env, capture_output=True, text=True, timeout=20,
                                                  check=False).stdout)
    unmapped_frame = ap.map_state(subprocess.run(["xwininfo", "-display", s.ctx.display, "-id", frame["id"]],
                                                 env=s.ctx.host_env, capture_output=True, text=True, timeout=20,
                                                 check=False).stdout)
    if unmapped_client not in ("IsUnMapped", "IsUnviewable") or unmapped_frame not in ("IsUnMapped", "IsUnviewable"):
        fails3.append(f"client/frame map states after minimize: {unmapped_client}/{unmapped_frame}")
    ev3 = [f"client_map={unmapped_client}", f"frame_map={unmapped_frame}", f"last_mid_bbox={last_mid[-1]['bbox'] if last_mid else None}"]

    # Restore through the taskbar icon; LAMP02 compares the restore flight with the minimize flight.
    if icon_centre is None:
        results["LAMP02"] = entry("LAMP02", FAIL, "no taskbar icon geometry to restore through", ev1)
        results["LAMP03"] = entry("LAMP03", FAIL, "; ".join(fails3), ev3)
        return results
    s.input(click=f"{int(icon_centre[0])},{int(icon_centre[1])}")
    ress = sample_flight(s, grab, "LAMP-res")
    restored_client = ap.map_state(subprocess.run(["xwininfo", "-display", s.ctx.display, "-id", client["id"]],
                                                  env=s.ctx.host_env, capture_output=True, text=True, timeout=20,
                                                  check=False).stdout)
    restored_frame = ap.map_state(subprocess.run(["xwininfo", "-display", s.ctx.display, "-id", frame["id"]],
                                                 env=s.ctx.host_env, capture_output=True, text=True, timeout=20,
                                                 check=False).stdout)
    min_w = [(centre_distance(f["bbox"], icon_centre), f["bbox"][2] - f["bbox"][0] + 1)
             for f in mins if f["bbox"] and f["bbox"] != rest_bbox]
    res_w = [(centre_distance(f["bbox"], icon_centre), f["bbox"][2] - f["bbox"][0] + 1)
             for f in ress if f["bbox"] and f["bbox"] != rest_bbox]
    ok2, why2, detail2 = mirror_verdict(min_w, res_w, rest_width)
    fails2 = list(why2)
    if restored_client != "IsViewable":
        fails2.append(f"taskbar click did not restore the client (map state {restored_client})")
    ev2 = [f"minimize_widths={[w for _, w in min_w]}", f"restore_widths={[w for _, w in res_w]}",
           f"detail={json.dumps(detail2)}", f"restored_client_map={restored_client}"] + [f["shot"] for f in ress if f["shot"]]
    results["LAMP02"] = entry("LAMP02", FAIL if fails2 else PASS, "; ".join(fails2), ev2,
                              ["XGetImage root (in-process)", "flamewm_xtest taskbar click"])
    if restored_frame != "IsViewable":
        fails3.append(f"frame not mapped after taskbar restore (map state {restored_frame})")
    ev3.append(f"restored_frame_map={restored_frame}")
    results["LAMP03"] = entry("LAMP03", FAIL if fails3 else PASS, "; ".join(fails3), ev3,
                              ["xwininfo -id", "xprop _NET_WM_ICON_GEOMETRY"])
    return results


# ---------------------------------------------------------------- driver

def parse_args(argv: list[str]) -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--run-dir", required=True)
    p.add_argument("--geometry", default="1280x800")
    p.add_argument("--display", default=None)
    p.add_argument("--wm-bin", default=str(ROOT / ".target/release/flamewm"))
    p.add_argument("--shell-bin", default=str(ROOT / ".target/release/flamewm-shell"))
    p.add_argument("--settle-seconds", type=float, default=3.0)
    args = p.parse_args(argv)
    if not re.fullmatch(r"\d+x\d+", args.geometry):
        p.error("--geometry must be WxH")
    args.screen_w, args.screen_h = (int(v) for v in args.geometry.split("x"))
    return args


def run_inner(args: argparse.Namespace, run: Path) -> int:
    ctx = base.Ctx(run)
    results: dict[str, dict] = {}
    started = ap.utc_now()
    for sub in ("home", "xdg-config", "xdg-data", "xdg-cache", "xdg-runtime", "shots", "logs"):
        (run / sub).mkdir(parents=True, exist_ok=True)
    try:
        display = base.pick_display(args.display)
        if display is None:
            results = {sid: entry(sid, ENV_BLOCKED, "no free display in :90..:99") for sid in IDS}
        else:
            ctx.display = display
            ctx.build_app_env()
            ctx.token = secrets.token_hex(32)
            ctx.marker.write_text(f"{ctx.display}\n{ctx.token}\n", encoding="ascii")
            os.chmod(ctx.marker, 0o600)
            xephyr = ctx.spawn("xephyr", ["Xephyr", ctx.display, "-screen", args.geometry, "-ac", "-noreset", "-br"],
                               ctx.host_env)
            if not base.wait_for_display(ctx, xephyr):
                results = {sid: entry(sid, ENV_BLOCKED, f"Xephyr did not become ready on {ctx.display}") for sid in IDS}
            else:
                grab = None
                try:
                    grab = RootGrab(ctx.display)
                    results = run_lamp(ap.Session(ctx, args), grab)
                except OSError as error:
                    results = {sid: entry(sid, ENV_BLOCKED, f"in-process capture unavailable: {error}") for sid in IDS}
                except Exception as error:  # a crashed scenario is a FAIL with its reason, never a PASS
                    results = {sid: entry(sid, FAIL, f"scenario raised {type(error).__name__}: {error}") for sid in IDS}
                finally:
                    if grab is not None:
                        grab.close()
    finally:
        teardown = ctx.teardown()
        doc = compose_document(results)
        doc.update({"run_dir": str(run), "display": ctx.display, "geometry": args.geometry,
                    "wm_bin": str(Path(args.wm_bin).resolve()), "shell_bin": str(Path(args.shell_bin).resolve()),
                    "started_utc": started, "finished_utc": ap.utc_now(), "teardown": teardown})
        if not teardown["ok"]:
            doc["verdict"], doc["exit_code"] = "FAIL", 1
        (run / "result.json").write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")
    return doc["exit_code"]


def main(argv: list[str] | None = None) -> int:
    raw = sys.argv[1:] if argv is None else list(argv)
    if raw == ["--checker-client"]:
        return checker_client()
    args = parse_args(raw)
    run = Path(args.run_dir).expanduser().absolute()
    inner = os.environ.get("FLAMEWM_LAMP_INNER") == "1"
    if inner:
        ap.validate_inner_run_dir(run)
    else:
        ap.validate_run_dir(run)
    missing = [name for name in ("Xephyr", "xwininfo", "xprop", "import", "identify", "xdpyinfo", "gdbus")
               if shutil.which(name) is None]
    if missing or not os.environ.get("DISPLAY") or base.Image is None:
        run.mkdir(parents=True, exist_ok=True)
        reason = "; ".join(filter(None, [f"missing tools: {missing}" if missing else "",
                                         "" if os.environ.get("DISPLAY") else "host DISPLAY unset",
                                         "" if base.Image is not None else "PIL unavailable"]))
        doc = compose_document({sid: entry(sid, ENV_BLOCKED, reason) for sid in IDS}) | {"run_dir": str(run)}
        (run / "result.json").write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")
        print(run / "result.json")
        return 3
    for binary in (args.wm_bin, args.shell_bin):
        if not os.access(binary, os.X_OK):
            print(f"error: binary not executable: {binary}", file=sys.stderr)
            return 2
    run.mkdir(parents=True, exist_ok=True)
    os.chmod(run, 0o700)
    if inner:
        return run_inner(args, run)
    (run / base.RUN_OWNER_MARKER).write_text(f"owner-pid={os.getpid()}\ncreated={ap.utc_now()}\n", encoding="ascii")
    if shutil.which("dbus-run-session") is None:
        print("error: dbus-run-session not found on PATH", file=sys.stderr)
        return 3
    child = subprocess.Popen(["dbus-run-session", "--", sys.executable, str(Path(__file__).resolve()), *raw],
                             env=dict(os.environ, FLAMEWM_LAMP_INNER="1"))
    code = child.wait()
    print(run / "result.json")
    return code


if __name__ == "__main__":
    raise SystemExit(main())

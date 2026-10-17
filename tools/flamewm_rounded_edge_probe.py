#!/usr/bin/env python3
"""V7 A12 pixel probe: rounded edges are anti-aliased (no binary step).

Starts a private Xvfb, runs `flamewm-lock --preview` on a flat lockBackgroundColor (seeded
through tools/flamewm_lock_acceptance.py), screenshots the root and samples the lock
password glass field corner and the submit (enter) circle. Along each rounded edge the
partial-alpha samples must produce at least MIN_INTERMEDIATE distinct RGB values strictly
between the background colour and the fill colour.

Usage:
  flamewm_rounded_edge_probe.py --run-dir <new dir> --shot-dir <dir> --lock-bin <flamewm-lock> --wm-bin <flamewm> [--label after]

Exit: 0 PASS, 1 FAIL, 3 ENVIRONMENT_BLOCKED. Settings toggle: see result.json (NOT_RUN unless probed).
"""

from __future__ import annotations

import argparse
import math
import os
import shutil
import subprocess
import sys
import tempfile
import time
from collections import Counter
from pathlib import Path

from flamewm_harness_common import write_json

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))
import flamewm_osd_audio_acceptance as base  # noqa: E402
import flamewm_lock_acceptance as lock  # noqa: E402

MIN_INTERMEDIATE = 2
CHANNEL_TOL = 3
RGB = tuple[int, int, int]


def dist(a: RGB, b: RGB) -> int:
    return max(abs(a[0] - b[0]), abs(a[1] - b[1]), abs(a[2] - b[2]))


def between(p: RGB, a: RGB, b: RGB) -> bool:
    return all(min(a[i], b[i]) - CHANNEL_TOL <= p[i] <= max(a[i], b[i]) + CHANNEL_TOL for i in range(3))


def radial_profile(px, width: int, centre: tuple[float, float], radius: float, angle: float,
                   reach: float) -> list[RGB]:
    """Pixel colours along one ray from radius+reach (outside) to radius-reach (inside)."""
    cx, cy = centre
    out: list[RGB] = []
    last = None
    steps = int(2 * reach / 0.25)
    for i in range(steps + 1):
        r = radius + reach - i * 0.25
        x = int(math.floor(cx + r * math.cos(math.radians(angle))))
        y = int(math.floor(cy + r * math.sin(math.radians(angle))))
        if (x, y) == last:
            continue
        last = (x, y)
        out.append(px[y * width + x])
    return out


def runs_of(colours: list[RGB]) -> list[tuple[RGB, int]]:
    runs: list[tuple[RGB, int]] = []
    for c in colours:
        if runs and dist(c, runs[-1][0]) <= CHANNEL_TOL:
            runs[-1] = (runs[-1][0], runs[-1][1] + 1)
        else:
            runs.append((c, 1))
    return runs


def edge_probe(px, width: int, centre: tuple[float, float], radius: float, angles: tuple[float, float],
               reach: float) -> dict:
    """Anti-aliasing along a rounded edge. For every ray across the edge, a binary step gives two
    plateaus with nothing between them; a blended edge gives at least one short run whose colour lies
    strictly between its neighbouring plateaus. Counts the distinct intermediate colours found."""
    lo, hi = angles
    seen: Counter = Counter()
    rays_with_blend = 0
    rays = 0
    for deg10 in range(int(lo * 10), int(hi * 10) + 1, 5):
        rays += 1
        runs = runs_of(radial_profile(px, width, centre, radius, deg10 / 10.0, reach))
        blended = False
        for i in range(1, len(runs) - 1):
            colour, length = runs[i]
            a, b = runs[i - 1][0], runs[i + 1][0]
            if length <= 4 and dist(a, b) > 2 * CHANNEL_TOL and between(colour, a, b) \
                    and dist(colour, a) > CHANNEL_TOL and dist(colour, b) > CHANNEL_TOL:
                seen[colour] += 1
                blended = True
        rays_with_blend += blended
    return {"rays": rays, "rays_with_blend": rays_with_blend, "intermediate_values": len(seen),
            "intermediate_px": sum(seen.values()),
            "top_values": [("#%02x%02x%02x" % c, n) for c, n in seen.most_common(4)]}


def probe_target(px, width: int, name: str, rect, radius: float, angles, s: float) -> dict:
    x, y, w, h = rect
    centre = (x + w / 2, y + h / 2) if name == "submit" else (x + radius, y + radius)
    result = edge_probe(px, width, centre, radius, angles, 1.5 * s)
    result.update({"target": name, "rect": [round(v, 1) for v in rect], "radius": round(radius, 2)})
    result["ok"] = result["intermediate_values"] >= MIN_INTERMEDIATE
    return result


def find_pill(image) -> tuple[int, int, int, int] | None:
    """Bounding box of the red 'on' toggle pill (saturated red) in the Settings window."""
    width, height = image.size
    px = image.load()
    xs, ys = [], []
    # Right-hand control column of the Desktop page (the sidebar icons are excluded).
    for y in range(height // 4, height):
        for x in range(width // 2, width):
            r, g, b = px[x, y]
            if r > 180 and g < 110 and b < 110:
                xs.append(x)
                ys.append(y)
    if len(xs) < 200:
        return None
    x0, x1, y0, y1 = min(xs), max(xs), min(ys), max(ys)
    w, h = x1 - x0 + 1, y1 - y0 + 1
    if w > 3 * h or h > 40:
        return None
    return (x0, y0, w, h)


def inner(args: argparse.Namespace, run: Path) -> int:
    doc: dict = {"started_utc": base.utc_now(), "run_dir": str(run), "label": args.label}
    procs = base.Procs()
    runtime: Path | None = None
    display = base.pick_display(args.display)
    shots = Path(args.shot_dir)
    shots.mkdir(parents=True, exist_ok=True)
    logs = run / "logs"
    logs.mkdir(parents=True, exist_ok=True)
    for sub in ("home", "xdg-config", "xdg-data", "xdg-cache"):
        (run / sub).mkdir(parents=True, exist_ok=True)
    verdict, code = "ENVIRONMENT_BLOCKED", 3
    try:
        if display is None:
            raise base.Blocked("no free private display in the B15 range")
        runtime = Path(tempfile.mkdtemp(prefix="fwmv7a12-", dir="/tmp"))
        os.chmod(runtime, 0o700)
        env = base.app_env(display, runtime, run)
        env["USER"] = "flamewm-probe"
        doc["display"] = display
        lock_bin = Path(args.lock_bin)
        if not os.access(lock_bin, os.X_OK):
            raise base.Blocked(f"lock binary not executable: {lock_bin}")
        base_env = {"PATH": env["PATH"], "HOME": env["HOME"]}
        procs.spawn("xvfb", ["Xvfb", display, "-screen", "0", "1280x800x24", "-ac", "-noreset", "-nolisten", "tcp"],
                    base_env, logs / "xvfb.log")
        if not base.wait_until(lambda: (base.X_SOCKET_DIR / f"X{display[1:]}").exists(), 20):
            raise base.Blocked(f"Xvfb did not create socket for {display}")
        lock.seed_settings(run, {"lockBackgroundColor": lock.FLAT_BACKGROUND})
        # The preview reads lockBackgroundColor from the WM settings snapshot, so the WM must run.
        wm = Path(args.wm_bin)
        if not os.access(wm, os.X_OK):
            raise base.Blocked(f"WM binary not executable: {wm}")
        procs.spawn("flamewm", [str(wm)], env, logs / "flamewm.log")
        time.sleep(3)
        procs.spawn("lock-preview", [str(lock_bin), "--preview"], env, logs / "lock-preview.log")
        time.sleep(4)
        shot = shots / f"a12-{args.label}-lock-flat.png"
        grab = subprocess.run(["import", "-display", display, "-window", "root", str(shot)], env=base_env,
                              capture_output=True, text=True, timeout=30, check=False)
        if grab.returncode != 0:
            raise base.Blocked(f"import failed: {grab.stderr.strip()}")
        from PIL import Image

        image = Image.open(shot).convert("RGB")
        width, height = image.size
        px = list(image.getdata())
        s, _, _ = lock.reference_fit(width, height)
        anchors = lock.reference_anchors(width, height)
        # The photo background is not flat here, so the glass detector (flat-background only) may miss
        # the field; the layout anchor is used for the corner and the measured box is reported.
        field = lock.measure_password(px, width, height, anchors["password"], s)
        doc["password_measured"] = None if field is None else [round(v, 1) for v in field]
        doc["screen"] = [width, height]
        doc["scale"] = round(s, 3)
        doc["screenshot"] = str(shot)
        checks = []
        fx, fy, fw, fh = anchors["password"]
        checks.append(probe_target(px, width, "password", (fx, fy, fw, fh), fh / 2, (180.0, 270.0), s))
        sx, sy, sw_, sh_ = anchors["submit"]
        checks.append(probe_target(px, width, "submit", (sx, sy, sw_, sh_), sw_ / 2, (0.0, 360.0), s))
        doc["checks"] = checks
        # Settings toggle on the Desktop page: the lock covers the screen, so stop it first.
        lock_proc = next(p for name, p in procs.owned if name == "lock-preview")
        lock_proc.terminate()
        lock_proc.wait(timeout=5)
        settings_bin = Path(args.settings_bin)
        if not os.access(settings_bin, os.X_OK):
            raise base.Blocked(f"settings binary not executable: {settings_bin}")
        procs.spawn("settings", [str(settings_bin), "--page", "desktop"], env, logs / "settings.log")
        time.sleep(5)
        shot2 = shots / f"a12-{args.label}-settings-desktop.png"
        grab = subprocess.run(["import", "-display", display, "-window", "root", str(shot2)], env=base_env,
                              capture_output=True, text=True, timeout=30, check=False)
        if grab.returncode != 0:
            raise base.Blocked(f"import failed (settings): {grab.stderr.strip()}")
        doc["settings_screenshot"] = str(shot2)
        toggle = find_pill(Image.open(shot2).convert("RGB"))
        if toggle is None:
            checks.append({"target": "settings-toggle", "ok": False, "reason": "no red toggle pill found"})
        else:
            tx, ty, tw, th = toggle
            checks.append(probe_target(list(Image.open(shot2).convert("RGB").getdata()), width, "toggle",
                                       (tx, ty, tw, th), th / 2, (90.0, 270.0), s))
        doc["toggle_bbox"] = toggle
        all_ok = all(c.get("ok") for c in checks)
        verdict, code = ("PASS", 0) if all_ok else ("FAIL", 1)
    except base.Blocked as error:
        doc["reason"] = str(error)
    finally:
        teardown = procs.teardown()
        if runtime is not None:
            shutil.rmtree(runtime, ignore_errors=True)
        doc["teardown"] = teardown
        if not teardown["ok"]:
            verdict, code = "FAIL", 1
        doc["verdict"], doc["exit_code"] = verdict, code
        doc["finished_utc"] = base.utc_now()
        write_json(run / "result.json", doc)
    return code


def parse_args(argv: list[str]) -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--run-dir", required=True, help="new or empty directory for logs and result.json")
    p.add_argument("--shot-dir", required=True, help="directory for screenshots (scratchpad)")
    p.add_argument("--lock-bin", required=True, help="flamewm-lock binary to probe")
    p.add_argument("--wm-bin", required=True, help="flamewm binary providing the settings snapshot")
    p.add_argument("--settings-bin", default=None, help="flamewm-settings binary; default: next to --wm-bin")
    p.add_argument("--label", default="after", help="screenshot/result label, e.g. after or before")
    p.add_argument("--display", default=None, help="private display in the B15 range; default: first free")
    return p.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    raw = sys.argv[1:] if argv is None else list(argv)
    args = parse_args(raw)
    if args.settings_bin is None:
        args.settings_bin = str(Path(args.wm_bin).with_name("flamewm-settings"))
    run = Path(args.run_dir).expanduser().absolute()
    if os.environ.get("FLAMEWM_A12_INNER") == "1":
        return inner(args, run)
    run.mkdir(parents=True, exist_ok=True)
    if shutil.which("dbus-run-session") is None:
        print("error: dbus-run-session not found on PATH", file=sys.stderr)
        return 3
    child = subprocess.Popen(["dbus-run-session", "--", sys.executable, "-I", str(Path(__file__).resolve()), *raw],
                             env=dict(os.environ, FLAMEWM_A12_INNER="1"))
    code = child.wait()
    print(run / "result.json")
    return code


if __name__ == "__main__":
    raise SystemExit(main())

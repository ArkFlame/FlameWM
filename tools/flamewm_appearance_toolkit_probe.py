#!/usr/bin/env python3
"""Probe whether the session appearance plan changes real Qt apps (KCalc, System Settings).

For each app and case this starts a private Xvfb display, launches the app
under an isolated HOME/XDG tree with only the environment the plan emits, finds
the app's largest window, and samples a menubar pixel. Cases:

  control    no FlameWM overlay at all (host-neutral baseline)
  xdg-only   owned kdeglobals + XDG_CONFIG_DIRS, no QT_QPA_PLATFORMTHEME
  dark       full plan (Dark): XDG_CONFIG_DIRS + QT_QPA_PLATFORMTHEME=kde
  light      full plan (Light)

Dolphin is sampled in its folder view (Breeze [Colors:View] BackgroundNormal
20,22,24 dark / 255,255,255 light) instead of the menubar.

An app is SUPPORTED only when its dark pixel matches its Breeze background
(KCalc/System Settings: 32,35,38 dark, 239,240,241 light; Dolphin: view colors
above), its light pixel matches, and the two differ. Anything else is reported
as observed-but-not-supported.

Usage: flamewm_appearance_toolkit_probe.py --plan-bin <appearance_plan> --work <dir>
"""

import argparse
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

from PIL import Image

BREEZE_BACKGROUND = {"dark": (32, 35, 38), "light": (239, 240, 241)}
BREEZE_VIEW_BACKGROUND = {"dark": (20, 22, 24), "light": (255, 255, 255)}
MENUBAR_SAMPLE = (320, 4)
SETTLE_SECONDS = 8
# app -> (binary, window title substring, sample (x, y) in window, expected Breeze colors)
APPS = {
    "kcalc": ("kcalc", "KCalc", MENUBAR_SAMPLE, BREEZE_BACKGROUND),
    "systemsettings": ("systemsettings", "System Settings", MENUBAR_SAMPLE, BREEZE_BACKGROUND),
    # Empty HOME gives an empty folder view; (500, 150) sits in the view below the infobar, above the centered text.
    "dolphin": ("dolphin", "Dolphin", (500, 150), BREEZE_VIEW_BACKGROUND),
}
WINDOW_LINE = re.compile(r"(0x[0-9a-f]+) \"([^\"]*)\":.*?(\d+)x(\d+)\+")


def find_window(display: str, env: dict, title: str):
    tree = subprocess.run(
        ["xwininfo", "-display", display, "-root", "-tree"],
        check=True,
        capture_output=True,
        text=True,
        env=env,
    ).stdout
    candidates = [
        (int(width) * int(height), window_id)
        for window_id, name, width, height in WINDOW_LINE.findall(tree)
        if title in name and int(width) >= 100 and int(height) >= 100
    ]
    return max(candidates)[1] if candidates else None


def plan_environment(plan_bin: str, config_home: Path, mode: str) -> dict:
    out = subprocess.run(
        [plan_bin, str(config_home), mode],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    return dict(line.split("=", 1) for line in out.splitlines() if "=" in line)


def run_case(args, app: str, case: str, display: str) -> dict:
    binary, title, sample, _expected = APPS[app]
    root = Path(args.work) / app / case
    shutil.rmtree(root, ignore_errors=True)
    config_home = root / ".config"
    (root / ".local/share").mkdir(parents=True)
    config_home.mkdir(parents=True)
    runtime = root / "runtime"
    runtime.mkdir(mode=0o700)
    env = {
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "DISPLAY": display,
        "HOME": str(root),
        "XDG_CONFIG_HOME": str(config_home),
        "XDG_DATA_HOME": str(root / ".local/share"),
        "XDG_RUNTIME_DIR": str(runtime),
        "LANG": "C.UTF-8",
    }
    plan_env = {}
    if case in ("xdg-only", "dark", "light"):
        mode = "dark" if case == "xdg-only" else case
        plan_env = plan_environment(args.plan_bin, config_home, mode)
        env["XDG_CONFIG_DIRS"] = plan_env["XDG_CONFIG_DIRS"]
        if case != "xdg-only" and plan_env.get("QT_QPA_PLATFORMTHEME"):
            env["QT_QPA_PLATFORMTHEME"] = plan_env["QT_QPA_PLATFORMTHEME"]
    proc = subprocess.Popen(
        ["dbus-run-session", "--", binary],
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )
    try:
        window = None
        deadline = time.monotonic() + SETTLE_SECONDS * 3
        while window is None and time.monotonic() < deadline:
            time.sleep(0.5)
            window = find_window(display, env, title)
        if window is None:
            return {"app": app, "case": case, "error": "no window mapped"}
        time.sleep(SETTLE_SECONDS)
        png = root / "window.png"
        subprocess.run(
            ["import", "-display", display, "-window", window, str(png)],
            check=True,
            env=env,
        )
        image = Image.open(png).convert("RGB")
        pixel = image.getpixel((min(sample[0], image.width - 1), min(sample[1], image.height - 1)))
    finally:
        os.killpg(proc.pid, signal.SIGTERM)
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(proc.pid, signal.SIGKILL)
            proc.wait()
    return {
        "app": app,
        "case": case,
        "xdg_config_dirs": env.get("XDG_CONFIG_DIRS", ""),
        "qt_qpa_platformtheme": env.get("QT_QPA_PLATFORMTHEME", ""),
        "sample_rgb": list(pixel),
    }


def verdict(app: str, rows: dict) -> dict:
    expected = APPS[app][3]
    dark = rows.get("dark", {}).get("sample_rgb")
    light = rows.get("light", {}).get("sample_rgb")
    supported = (
        dark is not None
        and light is not None
        and tuple(dark) == expected["dark"]
        and tuple(light) == expected["light"]
        and dark != light
    )
    return {"app": app, "supported": supported}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--plan-bin", required=True)
    parser.add_argument("--work", required=True)
    parser.add_argument("--display", default=":95")
    parser.add_argument("--screen", default="1024x768x24")
    parser.add_argument("--apps", default="kcalc,systemsettings")
    args = parser.parse_args()
    Path(args.work).mkdir(parents=True, exist_ok=True)
    xvfb = subprocess.Popen(
        ["Xvfb", args.display, "-screen", "0", args.screen, "-nolisten", "tcp"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    cases, verdicts = [], []
    try:
        time.sleep(2)
        for app in args.apps.split(","):
            rows = {}
            for case in ("control", "xdg-only", "dark", "light"):
                row = run_case(args, app, case, args.display)
                cases.append(row)
                rows[case] = row
            verdicts.append(verdict(app, rows))
    finally:
        xvfb.terminate()
        xvfb.wait(timeout=10)
    print(json.dumps({"cases": cases, "verdicts": verdicts}, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())

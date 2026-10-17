#!/usr/bin/env python3
"""Native tray-popup acceptance on a private Xephyr (P1..P5): audio, network, battery, calendar, weather.

Usage:
  tools/flamewm_popups_theme_acceptance.py --run-dir DIR --display :N --wm-bin PATH --shell-bin PATH
      --settings-bin PATH --desktop-bin PATH [--geometry WxH] [--settle-seconds S]
      [--tray-xy NAME=X,Y ...] [--frames N] [--frame-ms MS]

What it does (reuses the owned Xephyr/WM/Shell/desktop session of flamewm_context_menu_weather_acceptance):
  * clicks each tray item with XTEST, one at a time, and dismisses the popup before the next;
  * captures root screenshots every --frame-ms while the popup is sliding in (mid-slide shots);
  * checks the popup window is mapped, sits above the taskbar top edge, and that the previous
    popup window is gone before the next one opens;
  * writes DIR/shots/P-<name>-slide-NN.png, DIR/shots/P-<name>-final.png and DIR/result.json.

Battery live watts needs a UPower seam; the repo has none (crates/flamewm-integrations-linux/src/power.rs
hard-codes org.freedesktop.UPower, no env override), so P4 reports NOT_RUN unless one is supplied.

Exit status: 0 all PASS, 1 any FAIL, 2 usage error, 3 environment blocked or incomplete.
"""

from __future__ import annotations

import argparse
import json
import os
import secrets
import subprocess
import sys
import time
from pathlib import Path

from flamewm_harness_common import write_json

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))

import flamewm_context_menu_weather_acceptance as cmw  # noqa: E402
import flamewm_settings_acceptance as base  # noqa: E402
import flamewm_tray_popups_acceptance as tray  # noqa: E402  (xwininfo/root-tree helpers, display rule)

SCHEMA = "flamewm-popups-theme-acceptance/1"
PASS, FAIL, NOT_RUN = "PASS", "FAIL", "NOT_RUN"
# Tray items as laid out on a 1280x800 Xephyr screen with the default taskbar (read from R1-before.png).
DEFAULT_TRAY = {"weather": (1126, 768), "audio": (1126, 768), "network": (1156, 775),
                "battery": (1186, 775), "calendar": (1238, 768)}
TITLES = {"audio": "FlameWM audio", "network": "FlameWM network", "battery": "FlameWM battery",
          "calendar": "FlameWM calendar", "weather": "FlameWM weather"}
TASKBAR_TOP = 752  # taskbar occupies y>=752 on the 800px-high screen (see R1-before.png)
ORDER = ["weather", "audio", "network", "battery", "calendar"]


def capture_frame(display: str, out: Path) -> None:
    subprocess.run(["import", "-display", display, "-window", "root", str(out)],
                   check=True, capture_output=True, timeout=30)


def popup_info(display: str) -> tuple[dict[str, str | None], dict[str, dict]]:
    """Window ids for each popup title currently in the root tree, plus geometry for mapped ones."""
    tree = tray.root_tree(display)
    windows: dict[str, str | None] = {}
    geometry: dict[str, dict] = {}
    for name, title in TITLES.items():
        window = tray.find_window_id(tree, title)
        windows[name] = window
        if window:
            text = tray.xwininfo(window, display)
            if tray.parse_map_state(text):
                x, y, w, h = tray.parse_geometry(text)
                geometry[name] = {"x": x, "y": y, "w": w, "h": h}
    return windows, geometry


def judge(name: str, geometry: dict | None, previous_gone: bool) -> dict:
    if geometry is None:
        return {"status": FAIL, "reason": "popup not mapped"}
    bottom = geometry["y"] + geometry["h"]
    if geometry["w"] <= 1 or geometry["h"] <= 1:
        return {"status": FAIL, "reason": "zero-size popup", "geometry": geometry}
    if bottom > TASKBAR_TOP + 2:
        return {"status": FAIL, "reason": f"popup bottom {bottom} overlaps taskbar top {TASKBAR_TOP}",
                "geometry": geometry}
    if not previous_gone:
        return {"status": FAIL, "reason": "previous popup still mapped", "geometry": geometry}
    return {"status": PASS, "reason": f"mapped above taskbar (bottom {bottom}), previous popup closed",
            "geometry": geometry}


def run_popups(session: cmw.Session, args: argparse.Namespace, shots: Path) -> dict[str, dict]:
    display = session.ctx.display
    results: dict[str, dict] = {}
    previous_name: str | None = None
    for name in ORDER:
        if name not in args.tray:
            results[name] = {"status": NOT_RUN, "reason": "no tray position (pass --tray-xy)"}
            continue
        if name == "battery":
            results[name] = {"status": NOT_RUN, "reason": "no UPower seam; live watts not checked"}
            continue
        x, y = args.tray[name]
        session.xtest("--click", f"{x},{y}")
        frame_paths = []
        started = time.monotonic()
        for index in range(args.frames):
            target = time.monotonic() - started
            delay = index * args.frame_ms / 1000.0 - target
            if delay > 0:
                time.sleep(delay)
            path = shots / f"P-{name}-slide-{index:02d}.png"
            capture_frame(display, path)
            frame_paths.append(path.name)
        time.sleep(args.settle_seconds)
        _, geometry = popup_info(display)
        final = shots / f"P-{name}-final.png"
        capture_frame(display, final)
        # The previous popup must be gone (dismissed on its own click) once this one is open.
        previous_gone = previous_name is None or previous_name not in geometry
        verdict = judge(name, geometry.get(name), previous_gone)
        verdict.update({"frames": frame_paths, "final": final.name})
        results[name] = verdict
        session.xtest("--click", "40,40")  # dismiss by clicking the desktop
        time.sleep(1.0)
        previous_name = name
    return results


def parse_args(argv: list[str]) -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--run-dir", required=True)
    p.add_argument("--display", required=True)
    p.add_argument("--geometry", default="1280x800")
    p.add_argument("--wm-bin", required=True)
    p.add_argument("--shell-bin", required=True)
    p.add_argument("--settings-bin", required=True)
    p.add_argument("--desktop-bin", required=True)
    p.add_argument("--client-bin", default="xclock")
    p.add_argument("--settle-seconds", type=float, default=2.0)
    p.add_argument("--frames", type=int, default=12)
    p.add_argument("--frame-ms", type=int, default=50)
    p.add_argument("--tray-xy", action="append", default=[], metavar="NAME=X,Y")
    args = p.parse_args(argv)
    args.tray = dict(DEFAULT_TRAY)
    for item in args.tray_xy:
        name, _, xy = item.partition("=")
        args.tray[name] = cmw.xy_arg(xy)
    return args


def run(args: argparse.Namespace) -> int:
    run_dir = Path(args.run_dir)
    if not tray.display_allowed(args.display):
        print(f"refusing display {args.display!r}", file=sys.stderr)
        return 2
    if run_dir.exists() and any(run_dir.iterdir()):
        print(f"run dir must be new or empty: {run_dir}", file=sys.stderr)
        return 2
    for sub in ("home", "xdg-config", "xdg-data", "xdg-cache", "xdg-runtime", "xdg-state", "shots", "logs"):
        (run_dir / sub).mkdir(parents=True, exist_ok=True)
    shots = run_dir / "shots"
    ctx = base.Ctx(run_dir)
    results: dict[str, dict] = {}
    try:
        ctx.display = args.display
        ctx.build_app_env()
        ctx.app_env["XDG_STATE_HOME"] = str(run_dir / "xdg-state")
        ctx.token = secrets.token_hex(32)
        ctx.marker.write_text(f"{ctx.display}\n{ctx.token}\n", encoding="ascii")
        os.chmod(ctx.marker, 0o600)
        xephyr = ctx.spawn("xephyr", ["Xephyr", ctx.display, "-screen", args.geometry, "-ac", "-noreset", "-br"],
                           ctx.host_env)
        if not base.wait_for_display(ctx, xephyr):
            raise RuntimeError(f"Xephyr did not become ready on {ctx.display}")
        cmw.seed_home(ctx, None)
        session = cmw.Session(ctx, args)
        if not session.start():
            raise RuntimeError("WM, Shell or desktop did not start")
        results = run_popups(session, args, shots)
    except Exception as error:  # record the blocker instead of hiding it
        results.setdefault("harness", {"status": NOT_RUN, "reason": f"{type(error).__name__}: {error}"})
    finally:
        teardown = ctx.teardown()
    ok_all = bool(results) and all(r["status"] == PASS for r in results.values())
    verdict = "PASS" if ok_all else ("FAIL" if any(r["status"] == FAIL for r in results.values()) else "INCOMPLETE")
    doc = {"schema": SCHEMA, "verdict": verdict, "popups": results, "display": args.display,
           "teardown": teardown}
    write_json(run_dir / "result.json", doc)
    print(json.dumps(doc, indent=2))
    return 0 if verdict == "PASS" else (1 if verdict == "FAIL" else 3)


def main(argv: list[str] | None = None) -> int:
    return run(parse_args(sys.argv[1:] if argv is None else list(argv)))


if __name__ == "__main__":
    raise SystemExit(main())

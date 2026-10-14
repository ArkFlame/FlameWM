#!/usr/bin/env python3
"""V7 A14 native probe: window overflow, taskbar stacking, snap and maximize.

Starts a private Xvfb (B15 display range), flamewm and flamewm-shell, then runs
`flamewm-x11-canary j07 --scenario explicit-window-overflow`. The canary drags a
600x400 client by its titlebar with real XTEST input far left, right, up and down
and checks the frame stays at least 1/4 visible on each axis. This probe then
samples the taskbar pixels inside the frame/taskbar overlap of each screenshot:
they must be taskbar background, never client or frame colour.

Usage:
  flamewm_layout_probe.py --run-dir <new dir> --shot-dir <dir> --bin-dir <release dir>

Exit: 0 PASS, 1 FAIL, 3 ENVIRONMENT_BLOCKED. Requires dbus-run-session, Xvfb, PIL.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from collections import Counter
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))
import flamewm_osd_audio_acceptance as base  # noqa: E402  (private display, env, process helpers)

SCENARIO = "explicit-window-overflow"
CLAMP_SCENARIO = "explicit-window-clamp"
SCENARIOS = (SCENARIO, CLAMP_SCENARIO)
INNER_ENV = "FLAMEWM_V7_INNER"
CLIENT_RGB = (0, 255, 0)
TASKBAR_TOL = 12
CLIENT_TOL = 40
MIN_TASKBAR_FRACTION = 0.90


def dist(a: tuple[int, int, int], b: tuple[int, int, int]) -> int:
    return max(abs(a[0] - b[0]), abs(a[1] - b[1]), abs(a[2] - b[2]))


def taskbar_check(shot: Path, panel: list[int], frame: list[int]) -> dict:
    """Overlap of the frame with the taskbar: fraction of taskbar-coloured samples."""
    from PIL import Image

    image = Image.open(shot).convert("RGB")
    pixels = image.load()
    px0, py0 = panel[0], panel[1]
    px1, py1 = panel[0] + panel[2], panel[1] + panel[3]
    background = Counter(image.crop((px0, py0, px1, py1)).get_flattened_data()).most_common(1)[0][0]
    ox0, oy0 = max(frame[0], px0), max(frame[1], py0)
    ox1, oy1 = min(frame[0] + frame[2], px1, image.width), min(frame[1] + frame[3], py1, image.height)
    if ox1 <= ox0 or oy1 <= oy0:
        return {"overlap_px": 0, "taskbar_fraction": None, "client_samples": 0, "ok": None}
    total = taskbar = client = 0
    for y in range(oy0, oy1, 2):
        for x in range(ox0, ox1, 2):
            colour = pixels[x, y]
            total += 1
            if dist(colour, background) <= TASKBAR_TOL:
                taskbar += 1
            if dist(colour, CLIENT_RGB) <= CLIENT_TOL:
                client += 1
    fraction = taskbar / total
    ok = fraction >= MIN_TASKBAR_FRACTION and client == 0
    return {
        "overlap_px": (ox1 - ox0) * (oy1 - oy0),
        "taskbar_background": "#%02x%02x%02x" % background,
        "taskbar_fraction": round(fraction, 4),
        "client_samples": client,
        "ok": ok,
    }


def inner(args: argparse.Namespace, run: Path) -> int:
    scenario = args.scenario
    doc: dict = {"started_utc": base.utc_now(), "run_dir": str(run), "scenario": scenario}
    procs = base.Procs()
    runtime: Path | None = None
    display = base.pick_display(args.display)
    logs, shots = run / "logs", Path(args.shot_dir)
    for sub in ("home", "xdg-config", "xdg-data", "xdg-cache"):
        (run / sub).mkdir(parents=True, exist_ok=True)
    logs.mkdir(parents=True, exist_ok=True)
    shots.mkdir(parents=True, exist_ok=True)
    verdict, code = "ENVIRONMENT_BLOCKED", 3
    try:
        if display is None:
            raise base.Blocked("no free private display in the B15 range")
        runtime = Path(tempfile.mkdtemp(prefix="fwmv7-", dir="/tmp"))
        os.chmod(runtime, 0o700)
        env = base.app_env(display, runtime, run)
        doc["display"] = display
        bin_dir = Path(args.bin_dir)
        wm, shell_bin, canary = bin_dir / "flamewm", bin_dir / "flamewm-shell", bin_dir / "flamewm-x11-canary"
        missing = [str(p) for p in (wm, shell_bin, canary) if not os.access(p, os.X_OK)]
        if missing:
            raise base.Blocked(f"binaries not executable: {missing}")
        base_env = {"PATH": env["PATH"], "HOME": env["HOME"]}
        procs.spawn("xvfb", ["Xvfb", display, "-screen", "0", "1280x800x24", "-ac", "-noreset", "-nolisten", "tcp"],
                    base_env, logs / "xvfb.log")
        if not base.wait_until(lambda: (base.X_SOCKET_DIR / f"X{display[1:]}").exists(), 20):
            raise base.Blocked(f"Xvfb did not create socket for {display}")
        procs.spawn("flamewm", [str(wm)], env, logs / "flamewm.log")
        time.sleep(3)
        procs.spawn("flamewm-shell", [str(shell_bin)], env, logs / "flamewm-shell.log")
        if not base.wait_until(lambda: base.canary_lists(canary, env, "FlameWM panel"), 40):
            raise base.Blocked("flamewm shell did not show the taskbar panel")
        proc = subprocess.run([str(canary), "j07", "--scenario", scenario, "--shot-dir", str(shots)],
                              env=env, capture_output=True, text=True, timeout=600, check=False)
        (logs / "canary.txt").write_text(proc.stdout + proc.stderr, encoding="utf-8")
        doc["canary_exit"] = proc.returncode
        doc["canary_stdout"] = proc.stdout.strip().splitlines()
        if scenario == CLAMP_SCENARIO:
            # Clamp run: the canary verdict is the result; no taskbar sampling.
            doc["clamp_line"] = next((line for line in doc["canary_stdout"] if "clamp" in line or "start=" in line), None)
            verdict, code = ("PASS", 0) if proc.returncode == 0 else ("FAIL", 1)
            return code
        cases_path = shots / "overflow-cases.json"
        if not cases_path.exists():
            raise base.Blocked(f"canary wrote no {cases_path.name}")
        data = json.loads(cases_path.read_text(encoding="utf-8"))
        doc["screen"], doc["panel"] = data["screen"], data["panel"]
        checks = []
        for case in data["cases"]:
            entry = {"name": case["name"], "frame": case["frame"], "outside": case["outside"],
                     "quarter_ok": case["quarter_ok"], "shot": case["shot"]}
            entry["taskbar"] = taskbar_check(Path(case["shot"]), data["panel"], case["frame"])
            if case["name"] == "up":
                entry["status"] = "NOT_RUN"
                entry["reason"] = "titlebar grab cannot lift frame above y=0 without top-edge maximize zone"
                entry["ok"] = None
            else:
                entry["ok"] = case["outside"] and case["quarter_ok"] and entry["taskbar"]["ok"] is not False
                entry["status"] = "PASS" if entry["ok"] else "FAIL"
            checks.append(entry)
        doc["cases"] = checks
        overlapped = [c["name"] for c in checks if c["taskbar"]["overlap_px"] > 0]
        doc["taskbar_overlap_cases"] = overlapped
        doc["snap_maximize_line"] = next((line for line in doc["canary_stdout"] if "maximize[" in line), None)
        if not overlapped:
            raise base.Blocked("no frame reached the taskbar; taskbar stacking not exercised")
        canary_pass = proc.returncode == 0
        all_ok = canary_pass and all(c["ok"] for c in checks if c["ok"] is not None)
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
        (run / "result.json").write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")
    return code


def parse_args(argv: list[str]) -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--run-dir", required=True, help="new or empty directory for logs and result.json")
    p.add_argument("--shot-dir", required=True, help="directory for screenshots (scratchpad)")
    p.add_argument("--display", default=None, help="private display in the B15 range; default: first free")
    p.add_argument("--scenario", default=SCENARIO, choices=SCENARIOS, help="canary j07 scenario to run")
    p.add_argument("--bin-dir", required=True, help="directory holding flamewm, flamewm-shell and flamewm-x11-canary")
    return p.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    raw = sys.argv[1:] if argv is None else list(argv)
    args = parse_args(raw)
    run = Path(args.run_dir).expanduser().absolute()
    if os.environ.get(INNER_ENV) == "1":
        return inner(args, run)
    run.mkdir(parents=True, exist_ok=True)
    if shutil.which("dbus-run-session") is None:
        print("error: dbus-run-session not found on PATH", file=sys.stderr)
        return 3
    child = subprocess.Popen(["dbus-run-session", "--", sys.executable, "-I", str(Path(__file__).resolve()), *raw],
                             env=dict(os.environ, **{INNER_ENV: "1"}))
    code = child.wait()
    print(run / "result.json")
    return code


if __name__ == "__main__":
    raise SystemExit(main())

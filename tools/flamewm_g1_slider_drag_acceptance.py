#!/usr/bin/env python3
"""Native proof for audio slider value changes by real XTEST drags (G1).

Reuses the private stack of tools/flamewm_osd_audio_acceptance.py: Xvfb on a private display
in the B15 range (default :103), a private D-Bus session, private pipewire + pipewire-pulse,
the private null sink and the QMMP/Firefox streams. The host audio is never addressed.

Checks, each read back from the PipeWire node (pw-dump channelVolumes / mute):
  devices-drag-inside    press the Devices slider thumb, drag inside the popup to ~25%
  devices-drag-outside   press the thumb at 25%, move inside to ~60%, then move beyond the
                         popup LEFT edge and release there; value must clamp to 0 and the
                         popup must stay open (a lost release leaves the value at ~60)
  devices-mute           click the slot-1 mute button; mute flag must flip
  apps-drag-inside       Applications tab: QMMP row (OCR) thumb dragged to ~25%
Each change also records whether the volume OSD card was viewable within 1 s.

Statuses: PASS, FAIL, ENVIRONMENT_BLOCKED. Exit: 0 PASS, 1 FAIL, 3 environment blocked.
The runtime directory is a short path under /tmp (AF_UNIX path limit).
"""

from __future__ import annotations

import argparse
import ctypes
import ctypes.util
import json
import math
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from flamewm_harness_common import write_json

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))
import flamewm_osd_audio_acceptance as osd  # noqa: E402  (private stack helpers)

INNER_ENV = "FLAMEWM_G1_INNER"
DEFAULT_BIN = "/tmp/claude-1000/-run-media-linsaftw-SDD-GitHub-ArkFlame-flamewm-webrender/3ab0e413-61b5-459f-af23-4f1e67d93c05/scratchpad/final2/release"
DEFAULT_DISPLAY = ":103"
SCREEN = (1280, 800)
POPUP_TITLE = "FlameWM audio"
OSD_TITLE = "FlameWM volume"
PANEL_TITLE = "FlameWM panel"
# Popup-relative template geometry (ui/shell/audio.html, flamewm-shell taskbar/status/audio.rs).
TRACK_X, TRACK_W = 58.0, 350.0
LIST_TOP, GROUP_H = 92, 84
SLOT1_LANE_CY = 31 + 6          # lane top 31 in group, thumb/track centre 6 px down
SLOT1_MUTE_CENTRE = (72, 154)   # mute button 28x28 at (58, 48) inside group 1
APPS_TAB = (280, 70)
TOLERANCE = 2.0                 # percent, covers snapping and cube-root rounding
PRESS_BUTTON = 1


def utc_now() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


class Xtest:
    """Real XTEST pointer injection through libX11/libXtst (no SendEvent)."""

    def __init__(self, display: str) -> None:
        self.x11 = ctypes.CDLL(ctypes.util.find_library("X11"))
        self.xtst = ctypes.CDLL(ctypes.util.find_library("Xtst"))
        self.x11.XOpenDisplay.restype = ctypes.c_void_p
        self.x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
        self.xtst.XTestFakeMotionEvent.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_ulong]
        self.xtst.XTestFakeButtonEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
        self.dpy = self.x11.XOpenDisplay(display.encode())
        if not self.dpy:
            raise osd.Blocked(f"XOpenDisplay failed for {display}")

    def motion(self, x: int, y: int) -> None:
        self.xtst.XTestFakeMotionEvent(self.dpy, -1, int(x), int(y), 0)
        self.x11.XFlush(self.dpy)

    def button(self, button: int, down: bool) -> None:
        self.xtst.XTestFakeButtonEvent(self.dpy, button, 1 if down else 0, 0)
        self.x11.XFlush(self.dpy)

    def drag(self, start: tuple[int, int], waypoints: list[tuple[int, int]], steps: int = 14) -> None:
        """Press at start, move through each waypoint in steps, release at the last point."""
        self.motion(*start)
        time.sleep(0.1)
        self.button(PRESS_BUTTON, True)
        time.sleep(0.1)
        cur = start
        for wx, wy in waypoints:
            for i in range(1, steps + 1):
                self.motion(round(cur[0] + (wx - cur[0]) * i / steps), round(cur[1] + (wy - cur[1]) * i / steps))
                time.sleep(0.02)
            cur = (wx, wy)
        time.sleep(0.1)
        self.button(PRESS_BUTTON, False)

    def click(self, x: int, y: int) -> None:
        self.motion(x, y)
        time.sleep(0.1)
        self.button(PRESS_BUTTON, True)
        time.sleep(0.05)
        self.button(PRESS_BUTTON, False)


def list_windows(canary: Path, env: dict[str, str]) -> list[dict]:
    out = subprocess.run([str(canary), "list"], env=env, capture_output=True, text=True, timeout=20, check=False)
    pattern = re.compile(r'^win id=(\d+) parent=(\d+) geom=(\d+)x(\d+)\+(-?\d+)\+(-?\d+) map=(\w+) name="(.*?)" class=', re.M)
    return [
        {"id": int(m[0]), "w": int(m[2]), "h": int(m[3]), "x": int(m[4]), "y": int(m[5]), "map": m[6], "name": m[7]}
        for m in pattern.findall(out.stdout)
    ]


def viewable(canary: Path, env: dict[str, str], title: str) -> dict | None:
    for win in list_windows(canary, env):
        if win["name"] == title and win["map"] == "viewable":
            return win
    return None


def wait_for(predicate, timeout: float, interval: float = 0.05):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        value = predicate()
        if value:
            return value
        time.sleep(interval)
    return None


def pw_dump(pw_env: dict[str, str]) -> list:
    out = subprocess.run(["pw-dump"], env=pw_env, capture_output=True, text=True, timeout=20, check=False)
    try:
        return json.loads(out.stdout)
    except json.JSONDecodeError:
        return []


def node_volume(pw_env: dict[str, str], match) -> dict | None:
    """Percent and mute of the first matching node. Percent is 100 * cbrt(channel) (pulse mapping),
    from channelVolumes when PipeWire reports them, else from the node `volume` (a suspended stream
    reports an empty channelVolumes list and volume 1.0 = 100%)."""
    for obj in pw_dump(pw_env):
        info = obj.get("info") if isinstance(obj, dict) else None
        if not info or not match(info.get("props", {})):
            continue
        for param in info.get("params", {}).get("Props", []) or []:
            if "channelVolumes" in param or "volume" in param:
                channels = param.get("channelVolumes") or []
                mute = bool(param.get("mute", False))
                if channels:
                    percent = 100.0 * (sum(channels) / len(channels)) ** (1 / 3)
                elif "volume" in param:
                    percent = 100.0 * float(param["volume"]) ** (1 / 3)
                else:
                    percent = None
                return {"id": int(obj["id"]), "percent": round(percent, 2) if percent is not None else None,
                        "channels": channels, "volume": param.get("volume"), "mute": mute}
        return {"id": int(obj["id"]), "percent": None, "channels": None, "mute": None}
    return None


def sink_volume(pw_env: dict[str, str]) -> dict | None:
    return node_volume(pw_env, lambda p: p.get("node.name") == osd.SINK_NAME)


def stream_volume(pw_env: dict[str, str], app: str) -> dict | None:
    return node_volume(pw_env, lambda p: p.get("application.name") == app and "Stream" in str(p.get("media.class", "")))


def settle(read, target: float | None, timeout: float = 3.0) -> dict | None:
    """Poll the node until its percent is within TOLERANCE of target (or timeout); return last read."""
    last = {"value": None}

    def ok() -> bool:
        last["value"] = read()
        if target is None or last["value"] is None or last["value"].get("percent") is None:
            return False
        return abs(last["value"]["percent"] - target) <= TOLERANCE

    wait_for(ok, timeout, 0.1)
    return last["value"] if last["value"] is not None else read()


def osd_seen(canary: Path, env: dict[str, str], timeout: float = 1.0) -> bool:
    return bool(wait_for(lambda: viewable(canary, env, OSD_TITLE) is not None, timeout, 0.05))


def shot(env: dict[str, str], shots: Path, name: str) -> str | None:
    out = shots / name
    status = subprocess.run(["import", "-window", "root", str(out)], env=env, capture_output=True, check=False)
    return str(out) if status.returncode == 0 else None


def ocr_row(env: dict[str, str], png: str, popup: dict, word: str, run: Path, name: str) -> int | None:
    """Slot number (1-based) whose label row contains `word` in the popup crop, via tesseract TSV."""
    crop = run / f"{name}-popup.png"
    big = run / f"{name}-popup-ocr.png"
    geometry = f"{popup['w']}x{popup['h']}+{popup['x']}+{popup['y']}"
    subprocess.run(["convert", png, "-crop", geometry, "+repage", str(crop)], check=False)
    subprocess.run(["convert", str(crop), "-colorspace", "Gray", "-resize", "300%", str(big)], check=False)
    out = subprocess.run(["tesseract", str(big), "stdout", "--psm", "11", "tsv"], capture_output=True, text=True, check=False)
    for line in out.stdout.splitlines()[1:]:
        cols = line.split("\t")
        if len(cols) < 12 or cols[11].strip().lower() != word:
            continue
        y = int(cols[7]) // 3
        return (y - LIST_TOP) // GROUP_H + 1
    return None


def thumb_x(popup: dict, percent: float) -> int:
    return round(popup["x"] + TRACK_X + TRACK_W * percent / 100.0)


def inner(args: argparse.Namespace, run: Path) -> int:
    shots = Path(args.shot_dir)
    shots.mkdir(parents=True, exist_ok=True)
    logs = run / "logs"
    logs.mkdir(parents=True, exist_ok=True)
    for sub in ("home", "xdg-config/pipewire/pipewire.conf.d", "xdg-data", "xdg-cache"):
        (run / sub).mkdir(parents=True, exist_ok=True)
    (run / "xdg-config/pipewire/pipewire.conf.d/50-b15-null-sink.conf").write_text(osd.PIPEWIRE_CONF_DROPIN, encoding="utf-8")
    checks: list[dict] = []
    doc: dict = {"started_utc": utc_now(), "run_dir": str(run), "display": args.display, "checks": checks}
    procs = osd.Procs()
    runtime: Path | None = None
    verdict, code = "ENVIRONMENT_BLOCKED", 3
    try:
        if args.display not in osd.CANDIDATE_DISPLAYS:
            raise osd.Blocked(f"{args.display} outside the private display range")
        if osd.pick_display(args.display) is None:
            raise osd.Blocked(f"{args.display} is in use")
        runtime = Path(tempfile.mkdtemp(prefix="fwmg1-", dir="/tmp"))
        os.chmod(runtime, 0o700)
        env = osd.app_env(args.display, runtime, run)
        base_env = {"PATH": env["PATH"], "HOME": env["HOME"]}
        bin_dir = Path(args.bin_dir)
        wm, shell_bin, canary = bin_dir / "flamewm", bin_dir / "flamewm-shell", bin_dir / "flamewm-x11-canary"
        missing = [str(p) for p in (wm, shell_bin, canary) if not os.access(p, os.X_OK)]
        if missing:
            raise osd.Blocked(f"binaries not executable: {missing}")

        procs.spawn("xvfb", ["Xvfb", args.display, "-screen", "0", "1280x800x24", "-ac", "-noreset", "-nolisten", "tcp"],
                    base_env, logs / "xvfb.log")
        if not osd.wait_until(lambda: (osd.X_SOCKET_DIR / f"X{args.display[1:]}").exists(), 20):
            raise osd.Blocked("Xvfb socket did not appear")
        pw_env = {"PATH": env["PATH"], "XDG_RUNTIME_DIR": str(runtime), "PIPEWIRE_RUNTIME_DIR": str(runtime),
                  "XDG_CONFIG_HOME": env["XDG_CONFIG_HOME"], "HOME": env["HOME"]}
        procs.spawn("pipewire", ["/usr/bin/pipewire"], pw_env, logs / "pipewire.log")
        if not osd.wait_until(lambda: (runtime / "pipewire-0").exists(), 20):
            raise osd.Blocked("private pipewire socket did not appear")
        procs.spawn("pipewire-pulse", ["/usr/bin/pipewire-pulse"], pw_env, logs / "pipewire-pulse.log")
        if not osd.wait_until(lambda: (runtime / "pulse" / "native").exists(), 20):
            raise osd.Blocked("private pipewire-pulse socket did not appear")
        subprocess.run(["pw-metadata", "-n", "default", "0", "default.audio.sink",
                        json.dumps({"name": osd.SINK_NAME}), "Spa:String:JSON"],
                       env=pw_env, capture_output=True, text=True, timeout=20, check=False)

        procs.spawn("flamewm", [str(wm)], env, logs / "flamewm.log")
        time.sleep(3)
        procs.spawn("flamewm-shell", [str(shell_bin)], env, logs / "flamewm-shell.log")
        if not osd.wait_until(lambda: viewable(canary, env, PANEL_TITLE) is not None, 40):
            raise osd.Blocked("flamewm shell did not show the taskbar panel")
        streams = osd.write_streams(run)
        for app, icon in osd.STREAMS:
            props = json.dumps({"application.name": app, "application.icon_name": icon, "media.name": "Playback"})
            procs.spawn(f"stream-{app.lower()}", ["pw-cat", "--playback", "-P", props, str(streams)], pw_env,
                        logs / f"stream-{app.lower()}.log")
        snap = {"text": None}

        def ready() -> bool:
            snap["text"] = osd.audio_snapshot(env)
            return osd.snapshot_ready(snap["text"])

        if not osd.wait_until(ready, 30, 0.5):
            raise osd.Blocked("WM snapshot lacks the private sink as default or the QMMP/Firefox streams")
        (run / "audio-snapshot.txt").write_text(snap["text"] or "", encoding="utf-8")

        xt = Xtest(args.display)
        # Known start: 60% on the private sink, then open the audio popup from the taskbar.
        sink_id = osd.sink_node_id(pw_env)
        if sink_id is None:
            raise osd.Blocked("private sink node not visible to pw-dump")
        osd.set_sink_percent(pw_env, sink_id, 60)
        settle(lambda: sink_volume(pw_env), 60, 4)
        wait_for(lambda: viewable(canary, env, OSD_TITLE) is None, 3, 0.1)
        panel = viewable(canary, env, PANEL_TITLE)
        xt.click(panel["x"] + panel["w"] - 120, panel["y"] + panel["h"] // 2)
        popup = wait_for(lambda: viewable(canary, env, POPUP_TITLE), 5)
        if popup is None:
            raise osd.Blocked("audio popup did not open from the taskbar button")
        time.sleep(0.6)
        popup = viewable(canary, env, POPUP_TITLE) or popup
        doc["popup_open"] = popup
        cy = popup["y"] + LIST_TOP + SLOT1_LANE_CY
        shot(env, shots, "g1-00-popup-open.png")

        def sink_check(name: str, waypoints: list[tuple[int, int]], target: float, expect_open: bool) -> dict:
            before = sink_volume(pw_env)
            start = (thumb_x(popup, before["percent"]), cy)
            shot(env, shots, f"g1-{name}-before.png")
            xt.drag(start, waypoints)
            after = settle(lambda: sink_volume(pw_env), target)
            seen = osd_seen(canary, env)
            still_open = viewable(canary, env, POPUP_TITLE) is not None
            shot(env, shots, f"g1-{name}-after.png")
            ok = (after["percent"] is not None and abs(after["percent"] - target) <= TOLERANCE
                  and before["percent"] is not None and abs(after["percent"] - before["percent"]) > TOLERANCE
                  and still_open == expect_open)
            entry = {"name": name, "node": "sink", "before_percent": before["percent"], "after_percent": after["percent"],
                     "target": target, "popup_open_after": still_open, "osd_shown": seen,
                     "start_x": start[0], "waypoints": waypoints, "pass": ok}
            checks.append(entry)
            return entry

        inside_25 = (thumb_x(popup, 25), cy)
        sink_check("devices-drag-inside", [inside_25], 25, True)

        # Release beyond the LEFT popup edge (the popup sits at the right screen edge); value clamps to 0.
        outside_x = popup["x"] - 60
        doc["outside_release_x"] = outside_x
        if outside_x < 0:
            raise osd.Blocked(f"popup left edge {popup['x']} leaves no room for an outside release")
        middle_60 = (thumb_x(popup, 60), cy)
        outside_release = (outside_x, cy)
        sink_check("devices-drag-outside", [middle_60, outside_release], 0, True)

        mute_before = sink_volume(pw_env)
        xt.click(popup["x"] + SLOT1_MUTE_CENTRE[0], popup["y"] + SLOT1_MUTE_CENTRE[1])
        mute_after = settle(lambda: sink_volume(pw_env), None, 2)
        seen = osd_seen(canary, env)
        mute_ok = mute_before["mute"] is False and mute_after["mute"] is True
        checks.append({"name": "devices-mute", "node": "sink", "mute_before": mute_before["mute"],
                       "mute_after": mute_after["mute"], "osd_shown": seen, "pass": mute_ok})
        shot(env, shots, "g1-mute-after.png")

        xt.click(popup["x"] + APPS_TAB[0], popup["y"] + APPS_TAB[1])
        time.sleep(1.0)
        apps_shot = shot(env, shots, "g1-apps-tab.png")
        popup = viewable(canary, env, POPUP_TITLE) or popup
        slot = ocr_row(env, apps_shot, popup, "qmmp", run, "g1-apps") if apps_shot else None
        if slot is None:
            checks.append({"name": "apps-drag-inside", "node": "stream:QMMP", "pass": False,
                           "reason": "QMMP row not found by OCR of the Applications tab"})
        else:
            (run / "pw-dump-before-apps.json").write_text(json.dumps(pw_dump(pw_env), indent=1), encoding="utf-8")
            stream_before = stream_volume(pw_env, "QMMP")
            if stream_before is None or stream_before["percent"] is None:
                checks.append({"name": "apps-drag-inside", "node": "stream:QMMP", "pass": False,
                               "reason": "QMMP stream node not visible to pw-dump"})
            else:
                row_cy = popup["y"] + LIST_TOP + GROUP_H * (slot - 1) + SLOT1_LANE_CY
                start = (thumb_x(popup, stream_before["percent"]), row_cy)
                xt.drag(start, [(thumb_x(popup, 25), row_cy)])
                stream_after = settle(lambda: stream_volume(pw_env, "QMMP"), 25)
                shot(env, shots, "g1-apps-after.png")
                ok = (stream_after["percent"] is not None and abs(stream_after["percent"] - 25) <= TOLERANCE
                      and abs(stream_after["percent"] - stream_before["percent"]) > TOLERANCE)
                checks.append({"name": "apps-drag-inside", "node": "stream:QMMP", "row_slot": slot,
                               "before_percent": stream_before["percent"], "after_percent": stream_after["percent"],
                               "target": 25, "pass": ok})
        doc["all_pass"] = bool(checks) and all(c.get("pass") for c in checks)
        verdict, code = ("PASS", 0) if doc["all_pass"] else ("FAIL", 1)
    except osd.Blocked as error:
        doc["blocked"] = str(error)
    finally:
        teardown = procs.teardown()
        if runtime is not None:
            shutil.rmtree(runtime, ignore_errors=True)
        doc["teardown"] = teardown
        doc["finished_utc"] = utc_now()
        if not teardown["ok"]:
            verdict, code = "FAIL", 1
        doc["verdict"], doc["exit_code"] = verdict, code
        write_json(run / "result.json", doc)
    return code


def parse_args(argv: list[str]) -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--run-dir", required=True)
    p.add_argument("--shot-dir", required=True)
    p.add_argument("--display", default=DEFAULT_DISPLAY)
    p.add_argument("--bin-dir", default=DEFAULT_BIN)
    return p.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    raw = sys.argv[1:] if argv is None else list(argv)
    args = parse_args(raw)
    run = Path(args.run_dir).expanduser().absolute()
    if os.environ.get(INNER_ENV) == "1":
        return inner(args, run)
    run.mkdir(parents=True, exist_ok=True)
    if shutil.which("dbus-run-session") is None:
        print("error: dbus-run-session not found", file=sys.stderr)
        return 3
    for tool in ("Xvfb", "pipewire", "pipewire-pulse", "pw-cat", "pw-metadata", "pw-cli", "pw-dump", "gdbus", "convert", "import", "tesseract"):
        if shutil.which(tool) is None:
            print(f"error: missing tool {tool}", file=sys.stderr)
            return 3
    child = subprocess.Popen(["dbus-run-session", "--", sys.executable, "-I", str(Path(__file__).resolve()), *raw],
                             env=dict(os.environ, **{INNER_ENV: "1"}))
    code = child.wait()
    print(run / "result.json")
    return code


if __name__ == "__main__":
    raise SystemExit(main())

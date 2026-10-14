#!/usr/bin/env python3
"""G8 probe: does the volume OSD appear for a MUTE change made from the audio popup?

Reuses the private stack of tools/flamewm_osd_audio_acceptance.py (Xvfb, private D-Bus,
private PipeWire + pipewire-pulse, null sink) and the XTEST helpers of
tools/flamewm_g1_slider_drag_acceptance.py. Nothing outside the private stack is addressed.

Cases (REPS each), every measured action is polled for WINDOW_S at POLL_S intervals with the
canary `list`, watching the OSD window (430x74, title "FlameWM volume", map viewable):
  B  popup closed, mute toggled at the sink with pw-cli (private sink only)
  A  popup open, click the slot-1 mute button (state alternates each rep)
  D  popup open, click mute then click again (unmute); both clicks measured
  C  popup open, drag the slot-1 slider between 25% and 75%
Per measurement: sink mute/percent before and after, first/last poll the OSD was viewable
(ms from action start), OCR of the card at first sight, and the WM GetSnapshot sink mute.

Statuses: PASS (probe completed), ENVIRONMENT_BLOCKED, FAIL (teardown). Exit: 0, 1, 3.
The result is observational; it does not assert product behaviour.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))
import flamewm_osd_audio_acceptance as osd  # noqa: E402
import flamewm_g1_slider_drag_acceptance as g1  # noqa: E402

INNER_ENV = "FLAMEWM_G8_INNER"
DEFAULT_DISPLAY = ":114"
REPS = 5
WINDOW_S = 4.0
POLL_S = 0.05
OSD_SIZE = (430, 74)


def utc_now() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def set_sink(pw_env: dict[str, str], sink_id: int, mute: bool | None = None, percent: int | None = None) -> None:
    parts = []
    if percent is not None:
        channel = (percent / 100) ** 3
        parts.append(f"channelVolumes: [ {channel:.6f}, {channel:.6f} ]")
    if mute is not None:
        parts.append(f"mute: {'true' if mute else 'false'}")
    subprocess.run(["pw-cli", "s", str(sink_id), "Props", "{ " + ", ".join(parts) + " }"],
                   env=pw_env, capture_output=True, timeout=20, check=False)


def snapshot_sink_mute(env: dict[str, str]) -> str | None:
    text = osd.audio_snapshot(env)
    if not text:
        return None
    idx = text.find(osd.SINK_NAME)
    if idx < 0:
        return "sink-not-in-snapshot"
    match = re.search(r"'mute[d]?': <(true|false)>", text[idx:idx + 1500])
    return match.group(1) if match else "no-mute-field"


def ocr_card(png: str, geom: str, run: Path, name: str) -> str:
    crop = run / f"{name}-osd.png"
    big = run / f"{name}-osd-ocr.png"
    subprocess.run(["convert", png, "-crop", geom, "+repage", str(crop)], check=False)
    subprocess.run(["convert", str(crop), "-colorspace", "Gray", "-resize", "300%", str(big)], check=False)
    out = subprocess.run(["tesseract", str(big), "stdout", "--psm", "7"], capture_output=True, text=True, check=False)
    return out.stdout.strip()


def measure(label: str, action, canary: Path, env: dict[str, str], pw_env: dict[str, str], sink_id: int,
            popup_open, shots: Path, run: Path) -> dict:
    """Run `action` on a worker thread and poll the OSD window for WINDOW_S after it starts."""
    before = g1.sink_volume(pw_env)
    t0 = time.monotonic()
    worker = threading.Thread(target=action, daemon=True)
    worker.start()
    first = last = exists = None
    first_png = first_geom = None
    end = t0 + WINDOW_S
    while time.monotonic() < end:
        now_ms = round((time.monotonic() - t0) * 1000)
        wins = [w for w in g1.list_windows(canary, env) if w["name"] == g1.OSD_TITLE]
        if wins and exists is None:
            exists = now_ms
        vis = [w for w in wins if w["map"] == "viewable" and (w["w"], w["h"]) == OSD_SIZE]
        if vis:
            if first is None:
                first = now_ms
                first_geom = f"{vis[0]['w']}x{vis[0]['h']}+{vis[0]['x']}+{vis[0]['y']}"
                first_png = g1.shot(env, shots, f"g8-{label}-first.png")
            last = now_ms
        time.sleep(POLL_S)
    worker.join(timeout=5)
    after = g1.sink_volume(pw_env)
    ocr = ocr_card(first_png, first_geom, run, label) if first_png and first_geom else None
    return {
        "label": label,
        "popup_open": popup_open(),
        "mute_before": before["mute"], "mute_after": after["mute"],
        "percent_before": before["percent"], "percent_after": after["percent"],
        "osd_window_first_ms": exists,
        "osd_viewable_first_ms": first, "osd_viewable_last_ms": last,
        "osd_ocr": ocr,
        "snapshot_sink_mute": snapshot_sink_mute(env),
    }


def inner(args: argparse.Namespace, run: Path) -> int:
    shots = Path(args.shot_dir)
    shots.mkdir(parents=True, exist_ok=True)
    logs = run / "logs"
    logs.mkdir(parents=True, exist_ok=True)
    for sub in ("home", "xdg-config/pipewire/pipewire.conf.d", "xdg-data", "xdg-cache"):
        (run / sub).mkdir(parents=True, exist_ok=True)
    (run / "xdg-config/pipewire/pipewire.conf.d/50-b15-null-sink.conf").write_text(osd.PIPEWIRE_CONF_DROPIN, encoding="utf-8")
    measurements: list[dict] = []
    doc: dict = {"started_utc": utc_now(), "run_dir": str(run), "display": args.display, "measurements": measurements}
    procs = osd.Procs()
    runtime: Path | None = None
    verdict, code = "ENVIRONMENT_BLOCKED", 3
    try:
        if args.display not in osd.CANDIDATE_DISPLAYS:
            raise osd.Blocked(f"{args.display} outside the private display range")
        if osd.pick_display(args.display) is None:
            raise osd.Blocked(f"{args.display} is in use")
        runtime = Path(tempfile.mkdtemp(prefix="fwmg8-", dir="/tmp"))
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
        if not osd.wait_until(lambda: g1.viewable(canary, env, g1.PANEL_TITLE) is not None, 40):
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

        xt = g1.Xtest(args.display)
        sink_id = osd.sink_node_id(pw_env)
        if sink_id is None:
            raise osd.Blocked("private sink node not visible to pw-dump")
        set_sink(pw_env, sink_id, mute=False, percent=60)
        g1.settle(lambda: g1.sink_volume(pw_env), 60, 4)
        time.sleep(3.0)
        panel = g1.viewable(canary, env, g1.PANEL_TITLE)
        popup_now = lambda: g1.viewable(canary, env, g1.POPUP_TITLE) is not None  # noqa: E731

        # B: popup closed, sink-level mute toggles.
        doc["popup_open_before_B"] = popup_now()
        for rep in range(1, REPS + 1):
            cur = g1.sink_volume(pw_env)["mute"]
            m = measure(f"B{rep}", lambda v=not cur: set_sink(pw_env, sink_id, mute=v), canary, env, pw_env,
                        sink_id, popup_now, shots, run)
            m.update(case="B", rep=rep, action=f"pw-cli mute={not cur}")
            measurements.append(m)
            time.sleep(0.3)

        # Open the popup from the taskbar; reset to unmuted 60% (not measured).
        xt.click(panel["x"] + panel["w"] - 120, panel["y"] + panel["h"] // 2)
        popup = g1.wait_for(lambda: g1.viewable(canary, env, g1.POPUP_TITLE), 5)
        if popup is None:
            raise osd.Blocked("audio popup did not open from the taskbar button")
        time.sleep(0.6)
        popup = g1.viewable(canary, env, g1.POPUP_TITLE) or popup
        cy = popup["y"] + g1.LIST_TOP + g1.SLOT1_LANE_CY
        mute_xy = (popup["x"] + g1.SLOT1_MUTE_CENTRE[0], popup["y"] + g1.SLOT1_MUTE_CENTRE[1])
        doc["popup_open"] = popup

        # A: popup open, click mute; state alternates each rep.
        set_sink(pw_env, sink_id, mute=False, percent=60)
        time.sleep(3.0)
        for rep in range(1, REPS + 1):
            m = measure(f"A{rep}", lambda: xt.click(*mute_xy), canary, env, pw_env, sink_id, popup_now, shots, run)
            m.update(case="A", rep=rep, action="click slot-1 mute")
            measurements.append(m)
            time.sleep(0.3)

        # D: popup open, click mute (mute) then click again (unmute); both clicks measured.
        set_sink(pw_env, sink_id, mute=False, percent=60)
        time.sleep(3.0)
        for rep in range(1, REPS + 1):
            for half in ("mute", "unmute"):
                m = measure(f"D{rep}{half[0]}", lambda: xt.click(*mute_xy), canary, env, pw_env, sink_id,
                            popup_now, shots, run)
                m.update(case="D", rep=rep, action=f"click slot-1 mute ({half})")
                measurements.append(m)
                time.sleep(0.3)

        # C: popup open, drag the slot-1 slider between 25% and 75%.
        set_sink(pw_env, sink_id, mute=False, percent=60)
        time.sleep(3.0)
        for rep in range(1, REPS + 1):
            current = g1.sink_volume(pw_env)["percent"]
            target = 25 if rep % 2 else 75
            start = (g1.thumb_x(popup, current), cy)
            end = (g1.thumb_x(popup, target), cy)
            m = measure(f"C{rep}", lambda: xt.drag(start, [end]), canary, env, pw_env, sink_id, popup_now, shots, run)
            m.update(case="C", rep=rep, action=f"drag slider {current:.0f}% -> {target}%")
            measurements.append(m)
            time.sleep(0.3)

        log = logs / "flamewm-shell.log"
        text = log.read_text(encoding="utf-8", errors="replace") if log.exists() else ""
        doc["shell_log_lines"] = len(text.splitlines())
        doc["shell_log_osd_lines"] = [line for line in text.splitlines() if "OSD" in line or "osd" in line][-10:]
        doc["shell_log_audio_lines"] = len([line for line in text.splitlines() if "audio" in line.lower()])
        doc["blocked"] = None
        verdict, code = "PASS", 0
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
        (run / "result.json").write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")
    return code


def parse_args(argv: list[str]) -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--run-dir", required=True)
    p.add_argument("--shot-dir", required=True)
    p.add_argument("--display", default=DEFAULT_DISPLAY)
    p.add_argument("--bin-dir", default=g1.DEFAULT_BIN)
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

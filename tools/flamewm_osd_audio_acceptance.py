#!/usr/bin/env python3
"""Isolated native proof for the volume OSD and the audio popup Applications tab (B15).

Runs the real flamewm WM and flamewm-shell on a private Xvfb display, a private D-Bus
session, and a private PipeWire + pipewire-pulse stack. The stack holds one null sink named
like the host sink (`alsa_output.pci-0000_00_1f.3.analog-stereo`, "Built-in Audio Analog
Stereo") and two playback streams (QMMP, Firefox). The host PipeWire and the user's real
sink are never addressed: every client gets an explicit XDG_RUNTIME_DIR, PIPEWIRE_RUNTIME_DIR
and PULSE_SERVER inside the private tree. Volume and mute changes are made by the canary
scenarios on that private sink only.

Scenarios (canary `j07`, explicit only, never part of `all`):
  explicit-osd          OSD card per level: 430x74, centred, track fill ~ level/150, muted look,
                        hides within 1.5 s, two changes in 300 ms give one card, no focus change,
                        a real click through the card reaches the window beneath it.
  explicit-audio-apps   audio popup Applications tab: rows QMMP and Firefox (OCR), no raw
                        alsa_output text (OCR).

Statuses: PASS, FAIL, NOT_RUN, ENVIRONMENT_BLOCKED. Exit: 0 no FAIL, 1 any FAIL, 2 usage,
3 environment blocked or scenario not run.

The runtime directory must be a short path under /tmp: AF_UNIX socket paths are limited
to 108 bytes, so the PipeWire sockets cannot live under a long scratchpad path. Everything
else (config, logs, screenshots, result.json) lives under --run-dir.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import wave
from dataclasses import dataclass, field
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
ROOT = TOOLS.parent
SINK_NAME = "alsa_output.pci-0000_00_1f.3.analog-stereo"
SINK_DESCRIPTION = "Built-in Audio Analog Stereo"
STREAMS = (("QMMP", "qmmp"), ("Firefox", "firefox"))
SCENARIOS = ("explicit-osd", "explicit-audio-apps")
X_SOCKET_DIR = Path("/tmp/.X11-unix")
# Allowed private displays for this job; excludes :93-:98, :105, :106, :111, :119, :201, :202.
CANDIDATE_DISPLAYS = (":99", ":100", ":101", ":103", ":104", ":107", ":108", ":109", ":110",
                      ":112", ":113", ":114", ":115", ":117", ":118")
STATUS_PASS = "PASS"
STATUS_FAIL = "FAIL"
STATUS_NOT_RUN = "NOT_RUN"
STATUS_ENV_BLOCKED = "ENVIRONMENT_BLOCKED"
PIPEWIRE_CONF_DROPIN = f"""context.objects = [
    {{ factory = metadata
        args = {{ metadata.name = default }}
    }}
    {{ factory = adapter
        args = {{
            factory.name     = support.null-audio-sink
            node.name        = "{SINK_NAME}"
            node.description = "{SINK_DESCRIPTION}"
            media.class      = "Audio/Sink"
            audio.position   = [ FL FR ]
            monitor.channel-volumes = true
            object.linger    = true
        }}
    }}
]
"""
INNER_ENV = "FLAMEWM_B15_INNER"


def utc_now() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def wait_until(predicate, timeout: float, interval: float = 0.2) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(interval)
    return False


@dataclass
class Procs:
    """Owns every child this run starts. Teardown signals only these PIDs."""

    owned: list[tuple[str, subprocess.Popen]] = field(default_factory=list)

    def spawn(self, name: str, argv: list[str], env: dict[str, str], log: Path) -> subprocess.Popen:
        with log.open("ab") as handle:
            proc = subprocess.Popen(argv, env=env, stdout=handle, stderr=subprocess.STDOUT, cwd=str(log.parent))
        self.owned.append((name, proc))
        return proc

    def teardown(self) -> dict:
        for _, proc in reversed(self.owned):
            if proc.poll() is None:
                proc.send_signal(signal.SIGTERM)
        for _, proc in reversed(self.owned):
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=5)
        remaining = [proc.pid for _, proc in self.owned if proc.poll() is None]
        return {"owned": [{"name": n, "pid": p.pid, "exit": p.poll()} for n, p in self.owned],
                "remaining": remaining, "ok": not remaining}


def pick_display(explicit: str | None) -> str | None:
    candidates = [explicit] if explicit else list(CANDIDATE_DISPLAYS)
    for display in candidates:
        number = display[1:]
        if display not in CANDIDATE_DISPLAYS:
            raise SystemExit(f"error: display {display} is outside the B15 private range")
        if (X_SOCKET_DIR / f"X{number}").exists() or Path(f"/tmp/.X{number}-lock").exists():
            if explicit:
                raise SystemExit(f"error: display {display} is already in use; refusing")
            continue
        return display
    return None


def app_env(display: str, runtime: Path, run: Path) -> dict[str, str]:
    """Environment for FlameWM clients: every path points inside the private trees."""
    bus = os.environ.get("DBUS_SESSION_BUS_ADDRESS", "")
    return {
        "PATH": "/usr/bin:/bin",
        "LANG": "C.UTF-8",
        "HOME": str(run / "home"),
        "XDG_CONFIG_HOME": str(run / "xdg-config"),
        "XDG_DATA_HOME": str(run / "xdg-data"),
        "XDG_CACHE_HOME": str(run / "xdg-cache"),
        "XDG_RUNTIME_DIR": str(runtime),
        "XDG_SESSION_TYPE": "x11",
        "PIPEWIRE_RUNTIME_DIR": str(runtime),
        "PULSE_SERVER": f"unix:{runtime}/pulse/native",
        "DISPLAY": display,
        "DBUS_SESSION_BUS_ADDRESS": bus,
    }


def write_silence(path: Path, seconds: int = 120) -> None:
    with wave.open(str(path), "wb") as handle:
        handle.setnchannels(2)
        handle.setsampwidth(2)
        handle.setframerate(48000)
        handle.writeframes(b"\x00\x00\x00\x00" * 48000 * seconds)


def canary_lists(canary: Path, env: dict[str, str], name: str) -> bool:
    out = subprocess.run([str(canary), "list"], env=env, capture_output=True, text=True, timeout=20, check=False)
    return any(name in line and "map=viewable" in line for line in out.stdout.splitlines())


def audio_snapshot(env: dict[str, str]) -> str | None:
    out = subprocess.run(["gdbus", "call", "--session", "-d", "com.arkflame.FlameWM1", "-o",
                          "/com/arkflame/FlameWM1", "-m", "com.arkflame.FlameWM1.System.GetSnapshot"],
                         env=env, capture_output=True, text=True, timeout=20, check=False)
    if out.returncode != 0:
        return None
    text = out.stdout
    start = text.find("'audioAvailability'")
    streams = text.find("'audioStreams'")
    if start < 0 or streams < 0:
        return None
    end = text.find("]>", streams)
    return text[start:end + 2] if end > 0 else None


def snapshot_ready(text: str | None) -> bool:
    return bool(text) and f"'name': <'{SINK_NAME}'>" in text and "'default': <true>" in text and all(
        f"'appName': <'{app}'>" in text for app, _ in STREAMS)


def sink_node_id(pw_env: dict[str, str]) -> int | None:
    out = subprocess.run(["pw-dump"], env=pw_env, capture_output=True, text=True, timeout=20, check=False)
    try:
        objects = json.loads(out.stdout)
    except json.JSONDecodeError:
        return None
    for obj in objects:
        props = obj.get("info", {}).get("props", {}) if isinstance(obj, dict) else {}
        if props.get("node.name") == SINK_NAME:
            return int(obj["id"])
    return None


def set_sink_percent(pw_env: dict[str, str], sink_id: int, percent: int) -> None:
    channel = (percent / 100) ** 3  # PipeWire channel = (pulse percent / 100) cubed
    subprocess.run(["pw-cli", "s", str(sink_id), "Props",
                    f"{{ channelVolumes: [ {channel:.6f}, {channel:.6f} ] }}"],
                   env=pw_env, capture_output=True, timeout=20, check=False)


def prime_osd(canary: Path, env: dict[str, str], pw_env: dict[str, str], sink_id: int) -> bool:
    """Two visible changes before the scenarios; the OSD must show and hide on its own."""
    for percent in (50, 100):
        set_sink_percent(pw_env, sink_id, percent)
        if not wait_until(lambda: canary_lists(canary, env, "FlameWM volume"), 4, 0.05):
            return False
        if not wait_until(lambda: not canary_lists(canary, env, "FlameWM volume"), 4, 0.05):
            return False
    return True


def parse_scenario(stdout: str, scenario: str, exit_code: int) -> dict:
    match = re.search(r"^CANARY_J07 (\S+) pass=(true|false) (.*)$", stdout, re.M)
    if not match:
        return {"status": STATUS_NOT_RUN, "exit": exit_code, "reason": "no verdict line"}
    return {"status": STATUS_PASS if match.group(2) == "true" else STATUS_FAIL, "exit": exit_code,
            "name": match.group(1), "detail": match.group(3)}


def run_scenario(canary: Path, env: dict[str, str], scenario: str, shots: Path, logs: Path) -> dict:
    argv = [str(canary), "j07", "--scenario", scenario, "--shot-dir", str(shots)]
    try:
        proc = subprocess.run(argv, env=env, capture_output=True, text=True, timeout=420, check=False)
    except subprocess.TimeoutExpired:
        return {"status": STATUS_NOT_RUN, "reason": "scenario timeout"}
    (logs / f"{scenario}.txt").write_text(proc.stdout + proc.stderr, encoding="utf-8")
    return parse_scenario(proc.stdout, scenario, proc.returncode)


def inner(args: argparse.Namespace, run: Path) -> int:
    results: dict[str, dict] = {}
    doc: dict = {"started_utc": utc_now(), "run_dir": str(run), "scenarios": results}
    procs = Procs()
    runtime: Path | None = None
    display = pick_display(args.display)
    logs, shots = run / "logs", Path(args.shot_dir)
    for sub in ("home", "xdg-config/pipewire/pipewire.conf.d", "xdg-data", "xdg-cache"):
        (run / sub).mkdir(parents=True, exist_ok=True)
    logs.mkdir(parents=True, exist_ok=True)
    shots.mkdir(parents=True, exist_ok=True)
    (run / "xdg-config/pipewire/pipewire.conf.d/50-b15-null-sink.conf").write_text(PIPEWIRE_CONF_DROPIN, encoding="utf-8")
    try:
        if display is None:
            raise Blocked("no free private display in the B15 range")
        runtime = Path(tempfile.mkdtemp(prefix="fwmb15-", dir="/tmp"))
        os.chmod(runtime, 0o700)
        env = app_env(display, runtime, run)
        base_env = {"PATH": env["PATH"], "HOME": env["HOME"]}
        doc["display"], doc["runtime_dir"] = display, str(runtime)
        bin_dir = Path(args.bin_dir)
        wm, shell_bin, canary = bin_dir / "flamewm", bin_dir / "flamewm-shell", bin_dir / "flamewm-x11-canary"
        missing = [str(p) for p in (wm, shell_bin, canary) if not os.access(p, os.X_OK)]
        if missing:
            raise Blocked(f"binaries not executable: {missing}")

        procs.spawn("xvfb", ["Xvfb", display, "-screen", "0", "1280x800x24", "-ac", "-noreset", "-nolisten", "tcp"],
                    base_env, logs / "xvfb.log")
        if not wait_until(lambda: (X_SOCKET_DIR / f"X{display[1:]}").exists(), 20):
            raise Blocked(f"Xvfb did not create socket for {display}")

        pw_env = {"PATH": env["PATH"], "XDG_RUNTIME_DIR": str(runtime), "PIPEWIRE_RUNTIME_DIR": str(runtime),
                  "XDG_CONFIG_HOME": env["XDG_CONFIG_HOME"], "HOME": env["HOME"]}
        procs.spawn("pipewire", ["/usr/bin/pipewire"], pw_env, logs / "pipewire.log")
        if not wait_until(lambda: (runtime / "pipewire-0").exists(), 20):
            raise Blocked("private pipewire socket did not appear")
        procs.spawn("pipewire-pulse", ["/usr/bin/pipewire-pulse"], pw_env, logs / "pipewire-pulse.log")
        if not wait_until(lambda: (runtime / "pulse" / "native").exists(), 20):
            raise Blocked("private pipewire-pulse socket did not appear")
        meta = subprocess.run(["pw-metadata", "-n", "default", "0", "default.audio.sink",
                               json.dumps({"name": SINK_NAME}), "Spa:String:JSON"],
                              env=pw_env, capture_output=True, text=True, timeout=20, check=False)
        (logs / "pw-metadata.txt").write_text(meta.stdout + meta.stderr, encoding="utf-8")

        procs.spawn("flamewm", [str(wm)], env, logs / "flamewm.log")
        time.sleep(3)
        procs.spawn("flamewm-shell", [str(shell_bin)], env, logs / "flamewm-shell.log")
        if not wait_until(lambda: canary_lists(canary, env, "FlameWM panel"), 40):
            raise Blocked("flamewm shell did not show the taskbar panel")

        streams = write_streams(run)
        for app, icon in STREAMS:
            props = json.dumps({"application.name": app, "application.icon_name": icon, "media.name": "Playback"})
            procs.spawn(f"stream-{app.lower()}", ["pw-cat", "--playback", "-P", props, str(streams)], pw_env,
                        logs / f"stream-{app.lower()}.log")
        snap = {"text": None}

        def ready() -> bool:
            snap["text"] = audio_snapshot(env)
            return snapshot_ready(snap["text"])

        ready_ok = wait_until(ready, 30, 0.5)
        (run / "audio-snapshot.txt").write_text(snap["text"] or "", encoding="utf-8")
        if not ready_ok:
            raise Blocked("WM snapshot lacks the private sink as default or the two QMMP/Firefox streams")
        doc["snapshot_audio_excerpt"] = str(run / "audio-snapshot.txt")

        sink_id = sink_node_id(pw_env)
        if sink_id is None:
            raise Blocked("private sink node not visible to pw-dump")
        doc["prime_osd"] = prime_osd(canary, env, pw_env, sink_id)
        if not doc["prime_osd"]:
            raise Blocked("OSD did not show and hide for the priming changes")
        for scenario in SCENARIOS:
            results[scenario] = run_scenario(canary, env, scenario, shots, logs)
    except Blocked as error:
        for scenario in SCENARIOS:
            results.setdefault(scenario, {"status": STATUS_ENV_BLOCKED, "reason": str(error)})
    finally:
        teardown = procs.teardown()
        if runtime is not None:
            shutil.rmtree(runtime, ignore_errors=True)
        doc["teardown"] = teardown
        doc["finished_utc"] = utc_now()
        statuses = [entry["status"] for entry in results.values()]
        if any(s == STATUS_FAIL for s in statuses):
            verdict, code = STATUS_FAIL, 1
        elif statuses and all(s == STATUS_PASS for s in statuses):
            verdict, code = STATUS_PASS, 0
        else:
            verdict, code = STATUS_ENV_BLOCKED, 3
        if not teardown["ok"]:
            verdict, code = STATUS_FAIL, 1
        doc["verdict"], doc["exit_code"] = verdict, code
        (run / "result.json").write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")
    return doc["exit_code"]


class Blocked(Exception):
    """Environment prevented the run from starting; scenarios are reported ENVIRONMENT_BLOCKED."""


def write_streams(run: Path) -> Path:
    path = run / "silence.wav"
    if not path.exists():
        write_silence(path)
    return path


def parse_args(argv: list[str]) -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--run-dir", required=True, help="new or empty directory for logs and result.json")
    p.add_argument("--shot-dir", required=True, help="directory for screenshots (scratchpad)")
    p.add_argument("--display", default=None, help="private display in the B15 range; default: first free")
    p.add_argument("--bin-dir", default=str(ROOT / ".target/release"),
                   help="directory holding flamewm, flamewm-shell and flamewm-x11-canary")
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
    for tool in ("Xvfb", "pipewire", "pipewire-pulse", "pw-cat", "pw-metadata", "pw-cli", "gdbus", "convert", "tesseract"):
        if shutil.which(tool) is None and not Path(f"/usr/bin/{tool}").exists():
            print(f"error: missing tool {tool}", file=sys.stderr)
            return 3
    child = subprocess.Popen(["dbus-run-session", "--", sys.executable, "-I", str(Path(__file__).resolve()), *raw],
                             env=dict(os.environ, **{INNER_ENV: "1"}))
    code = child.wait()
    print(run / "result.json")
    return code


if __name__ == "__main__":
    raise SystemExit(main())

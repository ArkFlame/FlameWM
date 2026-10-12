#!/usr/bin/env python3
"""Isolated native acceptance for the idle-to-lock trigger (IL01..IL03).

Runs in a private Xephyr (default :112), a private D-Bus session and private XDG
directories under --run-dir (must not exist or be empty). Reuses the lock runner's
LockSession (xwininfo tree, XTEST input, WM-owned lock pids) and its settings seeding.

  IL01 lockTimeoutMinutes=1, one XTEST motion as the last input, then no input:
       a covering flamewm-lock window appears 55..90 s after that input
  IL02 lockTimeoutMinutes=1, XTEST motion every 20 s for 90 s: no lock appears
  IL03 lockTimeoutMinutes=0, no input for 75 s: no lock appears

Capability probe: xdpyinfo must list MIT-SCREEN-SAVER on the owned display. If the WM
logs "flamewm: idle lock disabled" the idle counter is unusable there: that scenario
is ENVIRONMENT_BLOCKED with the logged line, never PASS.

Exit status: 0 no FAIL, 1 any FAIL, 2 usage error, 3 environment blocked.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import secrets
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))

import flamewm_settings_acceptance as base  # noqa: E402
import flamewm_appearance_acceptance as appear  # noqa: E402
import flamewm_lock_acceptance as lk  # noqa: E402

ROOT = base.ROOT
SCHEMA = "flamewm-idle-lock-acceptance/1"
IDS = ("IL01", "IL02", "IL03")
TITLES = {
    "IL01": "idle timeout locks the screen 55..90 s after the last input",
    "IL02": "input every 20 s keeps the screen unlocked for 90 s",
    "IL03": "lockTimeoutMinutes=0 never locks from idle",
}
PASS, FAIL = appear.PASS, appear.FAIL
NOT_RUN, ENV_BLOCKED = appear.NOT_RUN, appear.ENV_BLOCKED
STATUSES = (PASS, FAIL, NOT_RUN, ENV_BLOCKED)
IDLE_PROBE_EXTENSION = "MIT-SCREEN-SAVER"
WM_DISABLED_MARK = "flamewm: idle lock disabled"
LOCK_EARLIEST_S, LOCK_LATEST_S = 55.0, 90.0
NEGATIVE_SECONDS = 90.0
MOTION_EVERY_S = 20.0
ZERO_SECONDS = 75.0
POLL_S = 1.0
DEFAULT_DISPLAY = ":112"
WM_BIN_DEFAULT = ROOT / ".target/release/flamewm"
SHELL_BIN_DEFAULT = ROOT / ".target/release/flamewm-shell"
LOCK_BIN_DEFAULT = ROOT / ".target/release/flamewm-lock"


# ---------------------------------------------------------------- pure verdicts

def capability_reason(xdpyinfo_text: str, display: str) -> str | None:
    """None when the owned display lists MIT-SCREEN-SAVER; otherwise the blocking reason."""
    # xdpyinfo prints "    MIT-SCREEN-SAVER  (opcode: 144, base event: 92)" per extension.
    if re.search(rf"^\s*{re.escape(IDLE_PROBE_EXTENSION)}(\s|$)", xdpyinfo_text, re.MULTILINE):
        return None
    return f"{IDLE_PROBE_EXTENSION} not listed by xdpyinfo -queryExtensions on {display}"


def wm_disabled_line(wm_log: str) -> str | None:
    """The WM's own report that the idle trigger could not query the server, or None."""
    for line in wm_log.splitlines():
        if WM_DISABLED_MARK in line:
            return line.strip()
    return None


def classify_lock_appearance(delta_s: float | None, disabled_line: str | None) -> tuple[str, str]:
    """delta_s: seconds from the last input to the first covering lock, None when none appeared."""
    if delta_s is None:
        if disabled_line is not None:
            return ENV_BLOCKED, f"idle counter unusable on this server: {disabled_line}"
        return FAIL, f"no covering lock within {LOCK_LATEST_S:g}s of the last input"
    if delta_s < LOCK_EARLIEST_S:
        return FAIL, f"lock appeared {delta_s:.1f}s after the last input (before {LOCK_EARLIEST_S:g}s)"
    if delta_s > LOCK_LATEST_S:
        return FAIL, f"lock appeared {delta_s:.1f}s after the last input (after {LOCK_LATEST_S:g}s)"
    return PASS, f"lock appeared {delta_s:.1f}s after the last input"


def classify_no_lock(lock_seen_at: list[float], disabled_line: str | None, label: str) -> tuple[str, str]:
    """A negative case only passes when the idle trigger was live; a disabled trigger proves nothing."""
    if lock_seen_at:
        return FAIL, f"{label}: covering lock appeared at {lock_seen_at[0]:.1f}s"
    if disabled_line is not None:
        return ENV_BLOCKED, f"{label}: idle counter unusable on this server: {disabled_line}"
    return PASS, f"{label}: no covering lock observed"


# ---------------------------------------------------------------- results

def entry(sid: str, status: str, reason: str = "", evidence: list | None = None, commands: list | None = None) -> dict:
    if status not in STATUSES:
        raise ValueError(f"unknown status {status!r}")
    return {"id": sid, "title": TITLES[sid], "status": status, "reason": reason,
            "evidence": evidence or [], "command": " && ".join(commands or []) or "none"}


def compose_document(results: dict[str, dict]) -> dict:
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


# ---------------------------------------------------------------- live session

class IdleSession:
    def __init__(self, ctx: base.Ctx, args: argparse.Namespace) -> None:
        self.ctx, self.args = ctx, args
        self.lock = lk.LockSession(ctx, args)
        self.shell: base.Owned | None = None

    def start(self, minutes: int) -> bool:
        """Seed lockTimeoutMinutes, restart WM and shell; reap WM-started locks from the previous scenario first."""
        pids = self.lock.wm_lock_pids()
        self.ctx.stop(self.shell)
        self.shell = None
        for pid in pids:
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        lk.seed_settings(self.ctx.run, {"lockTimeoutMinutes": str(minutes)})
        if not base.restart_owned_wm(self.ctx, self.args, "flamewm-wm"):
            return False
        self.shell = self.ctx.spawn("flamewm-shell", [self.args.shell_bin],
                                    dict(self.ctx.app_env, XDG_SESSION_TYPE="x11"))
        time.sleep(self.args.settle_seconds)
        return self.shell.proc.poll() is None and not self.ctx.wm_dead()

    def covering(self, label: str) -> dict | None:
        win, _ = self.lock.window(label)
        return win

    def motion(self, index: int) -> None:
        # Alternate between two points so every event is a real pointer move.
        self.lock.input(move=f"{200 if index % 2 == 0 else 600},400")

    def disabled_line(self) -> str | None:
        log = self.ctx.run / "logs" / "flamewm-wm.log"
        return wm_disabled_line(log.read_text(encoding="utf-8", errors="replace")) if log.exists() else None


def watch(s: IdleSession, seconds: float, on_tick=None) -> list[float]:
    """Poll the tree once per POLL_S for `seconds`; returns elapsed times where a covering lock was seen."""
    started = time.monotonic()
    seen: list[float] = []
    while True:
        elapsed = time.monotonic() - started
        if elapsed >= seconds:
            break
        if on_tick is not None:
            on_tick(elapsed)
        if s.covering("watch") is not None:
            seen.append(elapsed)
            break
        time.sleep(POLL_S)
    return seen


def il01(s: IdleSession) -> dict:
    if not s.start(1):
        return entry("IL01", FAIL, "flamewm WM or shell did not become ready")
    s.motion(0)  # the last input; the idle clock starts here
    seen = watch(s, LOCK_LATEST_S + POLL_S)
    status, reason = classify_lock_appearance(seen[0] if seen else None, s.disabled_line())
    return entry("IL01", status, reason, [f"lock_seen_at={seen}", f"poll_s={POLL_S:g}"],
                 ["flamewm_xtest.py --move", "xwininfo -root -tree"])


def il02(s: IdleSession) -> dict:
    if not s.start(1):
        return entry("IL02", FAIL, "flamewm WM or shell did not become ready")
    moves: list[float] = []
    last_motion = [-MOTION_EVERY_S]

    def tick(elapsed: float) -> None:
        if elapsed - last_motion[0] >= MOTION_EVERY_S:
            last_motion[0] = elapsed
            s.motion(len(moves))
            moves.append(elapsed)

    seen = watch(s, NEGATIVE_SECONDS, on_tick=tick)
    status, reason = classify_no_lock(seen, s.disabled_line(), "motion-every-20s")
    if status == PASS and s.ctx.wm_dead():
        status, reason = FAIL, "WM exited during the input run"
    return entry("IL02", status, reason, [f"motions={len(moves)}", f"lock_seen_at={seen}"],
                 ["flamewm_xtest.py --move", "xwininfo -root -tree"])


def il03(s: IdleSession) -> dict:
    if not s.start(0):
        return entry("IL03", FAIL, "flamewm WM or shell did not become ready")
    seen = watch(s, ZERO_SECONDS)
    status, reason = classify_no_lock(seen, None, "no-input-zero")
    return entry("IL03", status, reason, [f"lock_seen_at={seen}"], ["xwininfo -root -tree"])


SCENARIOS = (il01, il02, il03)


# ---------------------------------------------------------------- driver

def parse_args(argv: list[str]) -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--run-dir", required=True)
    p.add_argument("--geometry", default="1280x800")
    p.add_argument("--display", default=DEFAULT_DISPLAY)
    p.add_argument("--lock-bin", default=str(LOCK_BIN_DEFAULT))
    p.add_argument("--wm-bin", default=str(WM_BIN_DEFAULT))
    p.add_argument("--shell-bin", default=str(SHELL_BIN_DEFAULT))
    p.add_argument("--settle-seconds", type=float, default=3.0)
    args = p.parse_args(argv)
    if not re.fullmatch(r"\d+x\d+", args.geometry):
        p.error("--geometry must be WxH")
    if not re.fullmatch(r":\d+", args.display):
        p.error("--display must be :N")
    args.screen_w, args.screen_h = (int(v) for v in args.geometry.split("x"))
    return args


def write_doc(run: Path, doc: dict) -> None:
    (run / "result.json").write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")


def block(results: dict[str, dict], reason: str) -> None:
    for sid in IDS:
        results[sid] = entry(sid, ENV_BLOCKED, reason)


def run_inner(args: argparse.Namespace, run: Path) -> int:
    ctx = base.Ctx(run)
    results: dict[str, dict] = {}
    started = appear.utc_now()
    for sub in ("home", "xdg-config", "xdg-data", "xdg-cache", "xdg-runtime", "shots", "logs"):
        (run / sub).mkdir(parents=True, exist_ok=True)
    try:
        display = base.pick_display(args.display)
        if display is None:
            block(results, f"display {args.display} is in use")
        else:
            ctx.display = display
            ctx.build_app_env()
            ctx.token = secrets.token_hex(32)
            ctx.marker.write_text(f"{ctx.display}\n{ctx.token}\n", encoding="ascii")
            os.chmod(ctx.marker, 0o600)
            xephyr = ctx.spawn("xephyr", ["Xephyr", ctx.display, "-screen", args.geometry, "-ac", "-noreset", "-br"],
                               ctx.host_env)
            if not base.wait_for_display(ctx, xephyr):
                block(results, f"Xephyr did not become ready on {ctx.display}")
            else:
                probe = subprocess.run(["xdpyinfo", "-display", ctx.display, "-queryExtensions"],
                                       env=ctx.host_env, capture_output=True, text=True, timeout=30, check=False)
                (run / "logs" / "xdpyinfo-queryExtensions.txt").write_text(probe.stdout + probe.stderr,
                                                                           encoding="utf-8")
                reason = capability_reason(probe.stdout, ctx.display) if probe.returncode == 0 else \
                    f"xdpyinfo failed (exit {probe.returncode}) on {ctx.display}"
                if reason is not None:
                    block(results, reason)
                else:
                    session = IdleSession(ctx, args)
                    for scenario in SCENARIOS:
                        sid = scenario.__name__.upper()
                        try:
                            results[sid] = scenario(session)
                        except Exception as error:  # a crashed scenario is a FAIL, never a PASS
                            results[sid] = entry(sid, FAIL, f"scenario raised {type(error).__name__}: {error}")
    finally:
        teardown = ctx.teardown()
        doc = compose_document(results)
        doc.update({"run_dir": str(run), "display": ctx.display, "geometry": args.geometry,
                    "started_utc": started, "finished_utc": appear.utc_now(), "teardown": teardown})
        if not teardown["ok"]:
            doc["verdict"], doc["exit_code"] = "FAIL", 1
        write_doc(run, doc)
    return doc["exit_code"]


def main(argv: list[str] | None = None) -> int:
    raw = sys.argv[1:] if argv is None else list(argv)
    args = parse_args(raw)
    run = Path(args.run_dir).expanduser().absolute()
    inner = os.environ.get("FLAMEWM_IL_INNER") == "1"
    if inner:
        appear.validate_inner_run_dir(run)
    else:
        appear.validate_run_dir(run)
    missing = [name for name in ("Xephyr", "xwininfo", "xdpyinfo", "gdbus", "pgrep") if shutil.which(name) is None]
    lock_ok = os.access(args.lock_bin, os.X_OK) and Path(args.lock_bin).is_file()
    if missing or not os.environ.get("DISPLAY") or not lock_ok:
        run.mkdir(parents=True, exist_ok=True)
        reason = "; ".join(filter(None, [
            f"missing tools: {missing}" if missing else "",
            "" if os.environ.get("DISPLAY") else "host DISPLAY unset",
            "" if lock_ok else f"lock binary missing or not executable: {args.lock_bin}",
        ]))
        results: dict[str, dict] = {}
        block(results, reason)
        write_doc(run, compose_document(results) | {"run_dir": str(run)})
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
    (run / base.RUN_OWNER_MARKER).write_text(f"owner-pid={os.getpid()}\ncreated={appear.utc_now()}\n", encoding="ascii")
    if shutil.which("dbus-run-session") is None:
        print("error: dbus-run-session not found on PATH", file=sys.stderr)
        return 3
    child = subprocess.Popen(["dbus-run-session", "--", sys.executable, "-I", str(Path(__file__).resolve()), *raw],
                             env=dict(os.environ, FLAMEWM_IL_INNER="1"))
    code = child.wait()
    print(run / "result.json")
    return code


if __name__ == "__main__":
    raise SystemExit(main())

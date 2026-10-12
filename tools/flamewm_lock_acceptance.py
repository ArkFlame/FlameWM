#!/usr/bin/env python3
"""Isolated native acceptance for the flamewm-lock screen (LK01..LK06).

Runs in a private Xephyr (display :90..:99), a private D-Bus session and private
XDG directories under --run-dir (must not exist or be empty). Reuses the appearance
runner's machinery (tree parsing, OCR, pixel helpers) and the settings runner's Ctx,
XTEST input and capture. Statuses: PASS, FAIL, NOT_RUN, ENVIRONMENT_BLOCKED.

  LK01 lock starts and covers the screen (xwininfo tree, mapped)
  LK02 grab effective: Super and Ctrl+Alt+Right via XTEST leave the WM tree unchanged
       and the lock window topmost
  LK03 wrong password + Return keeps it locked and OCR shows a failure word
  LK04 SIGTERM does not unlock (process alive, window still covers the screen)
  LK05 Breeze look: dominant colour of the lock screenshot ~ #232629
  LK06 success path: always NOT_RUN ('needs real credentials'); never faked

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

ROOT = base.ROOT
SCHEMA = "flamewm-lock-acceptance/1"
IDS = tuple(f"LK{n:02d}" for n in range(1, 7))
TITLES = {
    "LK01": "lock starts and covers the screen",
    "LK02": "grab effective: WM shortcuts do not act, lock stays topmost",
    "LK03": "wrong password keeps the lock and shows a failure message",
    "LK04": "SIGTERM does not unlock",
    "LK05": "Breeze look: background ~#232629",
    "LK06": "success path with real credentials",
}
PASS, FAIL = appear.PASS, appear.FAIL
NOT_RUN, ENV_BLOCKED = appear.NOT_RUN, appear.ENV_BLOCKED
STATUSES = (PASS, FAIL, NOT_RUN, ENV_BLOCKED)
LOCK_BIN_DEFAULT = ROOT / ".target/release/flamewm-lock"
WM_BIN_DEFAULT = ROOT / ".target/release/flamewm"
BREEZE_BACKGROUND = (0x23, 0x26, 0x29)
BACKGROUND_TOLERANCE = 4
FAILURE_WORDS = {"incorrect", "failed", "wrong", "invalid"}
WRONG_PASSWORD_KEYS = ("x", "y", "z")


# ---------------------------------------------------------------- pure metrics

def covers_screen(node: dict, screen_w: int, screen_h: int) -> bool:
    """A window covers the screen when it sits at (0,0) with at least the screen size."""
    return node["x"] == 0 and node["y"] == 0 and node["w"] >= screen_w and node["h"] >= screen_h


def lock_window(nodes: list[dict], screen_w: int, screen_h: int) -> dict | None:
    """Largest root-level window covering the screen, or None."""
    candidates = [n for n in nodes if n["parent"] is None and covers_screen(n, screen_w, screen_h)]
    return max(candidates, key=lambda n: n["w"] * n["h"]) if candidates else None


def is_topmost(nodes: list[dict], window_id: str) -> bool:
    """`xwininfo -tree` lists the root's children topmost first (verified natively by raising a window)."""
    roots = [n["id"] for n in nodes if n["parent"] is None]
    return bool(roots) and roots[0] == window_id


def tree_signature(nodes: list[dict], exclude: set[str]) -> tuple:
    """Stable snapshot of every window except the lock's own (ids excluded)."""
    return tuple(sorted((n["id"], n["name"], n["x"], n["y"], n["w"], n["h"], str(n["parent"]))
                        for n in nodes if n["id"] not in exclude))


def failure_text_seen(words: list[tuple[str, int, int]]) -> bool:
    return any(word in FAILURE_WORDS for word, _, _ in words)


def background_ok(pixels: list[tuple[int, int, int]]) -> bool:
    return appear.close_enough(appear.dominant_color(pixels), BREEZE_BACKGROUND, BACKGROUND_TOLERANCE)


# ---------------------------------------------------------------- results

def entry(sid: str, status: str, reason: str = "", evidence: list | None = None, commands: list | None = None) -> dict:
    if status not in STATUSES:
        raise ValueError(f"unknown status {status!r}")
    return {"id": sid, "title": TITLES[sid], "status": status, "reason": reason,
            "evidence": evidence or [], "command": " && ".join(commands or []) or "none"}


def compose_document(results: dict[str, dict]) -> dict:
    """Every LK ID appears once; PASS needs a PASS entry; missing IDs are NOT_RUN."""
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

class LockSession:
    def __init__(self, ctx: base.Ctx, args: argparse.Namespace) -> None:
        self.ctx, self.args = ctx, args
        self.lock: base.Owned | None = None

    def ensure_lock(self) -> bool:
        if self.lock is None or self.lock.proc.poll() is not None:
            self.lock = self.ctx.spawn("flamewm-lock", [self.args.lock_bin], self.ctx.app_env)
            time.sleep(self.args.settle_seconds)
        return self.lock.proc.poll() is None

    def tree(self, label: str) -> list[dict]:
        result = subprocess.run(["xwininfo", "-display", self.ctx.display, "-root", "-tree"],
                                env=self.ctx.host_env, capture_output=True, text=True, timeout=30, check=False)
        (self.ctx.run / "logs" / f"tree-{label}.txt").write_text(result.stdout + result.stderr, encoding="utf-8")
        return appear.parse_tree(result.stdout) if result.returncode == 0 else []

    def window(self, label: str) -> tuple[dict | None, list[dict]]:
        nodes = self.tree(label)
        return lock_window(nodes, self.args.screen_w, self.args.screen_h), nodes

    def shot(self, name: str) -> tuple[Path | None, list[tuple[int, int, int]] | None]:
        path = self.ctx.run / "shots" / f"{name}.png"
        ok, _ = base.capture(self.ctx, path)
        if not ok or base.Image is None:
            return None, None
        with base.Image.open(path) as image:
            return path, list(image.convert("RGB").getdata())

    def input(self, **kwargs) -> tuple[int, str]:
        kwargs.setdefault("click", None)
        kwargs.setdefault("key", None)
        return base.send_input(self.ctx, **kwargs)


def lk01(s: LockSession) -> dict:
    if not s.ensure_lock():
        return entry("LK01", FAIL, "flamewm-lock exited before the lock surface appeared")
    win, _ = s.window("LK01")
    if win is None:
        return entry("LK01", ENV_BLOCKED, "flamewm-lock exposes no native full-screen X surface")
    return entry("LK01", PASS, "", [f"window={win['id']} {win['w']}x{win['h']}+{win['x']}+{win['y']}"],
                 ["xwininfo -root -tree"])


def lk02(s: LockSession) -> dict:
    if not s.ensure_lock():
        return entry("LK02", FAIL, "flamewm-lock not running")
    win, before = s.window("LK02-before")
    if win is None:
        return entry("LK02", NOT_RUN, "no lock surface (LK01 did not observe one)")
    exclude = {win["id"]}
    snapshot_before = tree_signature(before, exclude)
    sent = []
    for label, status in (("Super_L", s.input(key="Super_L")[0]),
                          ("Ctrl+Alt+Right", s.input(chord="Ctrl+Alt+Right")[0])):
        sent.append(f"{label}={status}")
    time.sleep(s.args.settle_seconds)
    win_after, after = s.window("LK02-after")
    unchanged = tree_signature(after, exclude) == snapshot_before
    topmost = win_after is not None and is_topmost(after, win_after["id"])
    ok = unchanged and topmost and s.lock.proc.poll() is None
    reason = "" if ok else (f"tree changed={not unchanged} lock_topmost={topmost} "
                            f"alive={s.lock.proc.poll() is None}")
    return entry("LK02", PASS if ok else FAIL, reason, sent + [f"unchanged={unchanged}", f"topmost={topmost}"],
                 ["xwininfo -root -tree", "flamewm_xtest.py --key/--chord"])


def lk03(s: LockSession) -> dict:
    if not s.ensure_lock():
        return entry("LK03", FAIL, "flamewm-lock not running")
    for key in WRONG_PASSWORD_KEYS:
        s.input(key=key)
    s.input(key="Return")
    time.sleep(s.args.settle_seconds)
    win, nodes = s.window("LK03")
    path, _ = s.shot("LK03-failure")
    if path is None:
        return entry("LK03", ENV_BLOCKED, "capture failed")
    if shutil.which("tesseract") is None:
        return entry("LK03", ENV_BLOCKED, "tesseract unavailable for failure-text OCR")
    words = appear.ocr_word_centres(path, FAILURE_WORDS)
    still_locked = win is not None and is_topmost(nodes, win["id"]) and s.lock.proc.poll() is None
    seen = failure_text_seen(words)
    ok = still_locked and seen
    reason = "" if ok else f"still_locked={still_locked} failure_text_seen={seen}"
    return entry("LK03", PASS if ok else FAIL, reason, [str(path), f"ocr_hits={words}"],
                 ["xdotool-style XTEST key x,y,z,Return", "tesseract"])


def lk04(s: LockSession) -> dict:
    if not s.ensure_lock():
        return entry("LK04", FAIL, "flamewm-lock not running")
    os.kill(s.lock.pid, signal.SIGTERM)
    time.sleep(s.args.settle_seconds)
    alive = s.lock.proc.poll() is None
    win, _ = s.window("LK04")
    ok = alive and win is not None
    reason = "" if ok else f"alive={alive} covering_window={win is not None}"
    return entry("LK04", PASS if ok else FAIL, reason, [f"alive={alive}"], ["kill -TERM <lock pid>"])


def lk05(s: LockSession) -> dict:
    if not s.ensure_lock():
        return entry("LK05", FAIL, "flamewm-lock not running")
    path, px = s.shot("LK05-look")
    if px is None:
        return entry("LK05", ENV_BLOCKED, "capture failed")
    dom = appear.dominant_color(px)
    ok = background_ok(px)
    return entry("LK05", PASS if ok else FAIL,
                 "" if ok else f"dominant {dom} != {BREEZE_BACKGROUND} (tol {BACKGROUND_TOLERANCE})",
                 [str(path), f"dominant={dom}"], ["import -window root"])


def lk06(_: LockSession) -> dict:
    return entry("LK06", NOT_RUN, "needs real credentials")


SCENARIOS = (lk01, lk02, lk03, lk04, lk05, lk06)


# ---------------------------------------------------------------- driver

def parse_args(argv: list[str]) -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--run-dir", required=True)
    p.add_argument("--geometry", default="1280x800")
    p.add_argument("--display", default=None)
    p.add_argument("--lock-bin", default=str(LOCK_BIN_DEFAULT))
    p.add_argument("--wm-bin", default=str(WM_BIN_DEFAULT))
    p.add_argument("--settle-seconds", type=float, default=3.0)
    args = p.parse_args(argv)
    if not re.fullmatch(r"\d+x\d+", args.geometry):
        p.error("--geometry must be WxH")
    args.screen_w, args.screen_h = (int(v) for v in args.geometry.split("x"))
    return args


def write_doc(run: Path, doc: dict) -> None:
    (run / "result.json").write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")


def run_inner(args: argparse.Namespace, run: Path) -> int:
    ctx = base.Ctx(run)
    results: dict[str, dict] = {}
    started = appear.utc_now()
    for sub in ("home", "xdg-config", "xdg-data", "xdg-cache", "xdg-runtime", "shots", "logs"):
        (run / sub).mkdir(parents=True, exist_ok=True)
    try:
        display = base.pick_display(args.display)
        if display is None:
            block(results, "no free display in :90..:99")
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
            elif not base.restart_owned_wm(ctx, args, "flamewm-wm"):
                block(results, "flamewm WM did not become ready (needed for LK02 grab check)")
            else:
                session = LockSession(ctx, args)
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
                    "lock_bin": str(Path(args.lock_bin).resolve()), "started_utc": started,
                    "finished_utc": appear.utc_now(), "teardown": teardown})
        if not teardown["ok"]:
            doc["verdict"], doc["exit_code"] = "FAIL", 1
        write_doc(run, doc)
    return doc["exit_code"]


def block(results: dict[str, dict], reason: str) -> None:
    for sid in IDS:
        results[sid] = entry(sid, ENV_BLOCKED, reason)


def main(argv: list[str] | None = None) -> int:
    raw = sys.argv[1:] if argv is None else list(argv)
    args = parse_args(raw)
    run = Path(args.run_dir).expanduser().absolute()
    inner = os.environ.get("FLAMEWM_LK_INNER") == "1"
    if inner:
        appear.validate_inner_run_dir(run)
    else:
        appear.validate_run_dir(run)
    missing = [name for name in ("Xephyr", "xwininfo", "import", "gdbus") if shutil.which(name) is None]
    lock_ok = os.access(args.lock_bin, os.X_OK) and Path(args.lock_bin).is_file()
    if missing or not os.environ.get("DISPLAY") or base.Image is None or not lock_ok:
        run.mkdir(parents=True, exist_ok=True)
        reason = "; ".join(filter(None, [
            f"missing tools: {missing}" if missing else "",
            "" if os.environ.get("DISPLAY") else "host DISPLAY unset",
            "" if base.Image is not None else "PIL unavailable",
            "" if lock_ok else f"lock binary missing or not executable: {args.lock_bin}",
        ]))
        results: dict[str, dict] = {}
        block(results, reason)
        write_doc(run, compose_document(results) | {"run_dir": str(run)})
        print(run / "result.json")
        return 3
    if not os.access(args.wm_bin, os.X_OK):
        print(f"error: binary not executable: {args.wm_bin}", file=sys.stderr)
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
                             env=dict(os.environ, FLAMEWM_LK_INNER="1"))
    code = child.wait()
    print(run / "result.json")
    return code


if __name__ == "__main__":
    raise SystemExit(main())

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
  LK07 lockBackgroundColor=#336699: screen outside the card ~#336699, card still readable
  LK08 lockClockFormat=12h, lockShowDate=false, lockShowUsername=false: AM/PM seen by OCR,
       date words and the username label absent
  LK09 lockBackgroundImage=<generated PNG>: its quadrant colours show at the screen corners

LK07..LK09 each restart the WM with their own seeded settings (no state shared between them).

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
from datetime import datetime
from pathlib import Path

TOOLS =Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))

import flamewm_settings_acceptance as base  # noqa: E402
import flamewm_appearance_acceptance as appear  # noqa: E402

ROOT = base.ROOT
SCHEMA = "flamewm-lock-acceptance/1"
IDS = tuple(f"LK{n:02d}" for n in range(1, 10))
TITLES = {
    "LK01": "lock starts and covers the screen",
    "LK02": "grab effective: WM shortcuts do not act, lock stays topmost",
    "LK03": "wrong password keeps the lock and shows a failure message",
    "LK04": "SIGTERM does not unlock",
    "LK05": "Breeze look: background ~#232629",
    "LK06": "success path with real credentials",
    "LK07": "customised background colour fills the screen outside the card",
    "LK08": "clock 12h, date and username hidden",
    "LK09": "customised background image is cover-scaled behind the card",
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
TEXT_BRIGHT = (0xEF, 0xF0, 0xF1)   # Breeze foreground: clock and username
TEXT_TOLERANCE = 24
CLOCK_MIN_ROWS = 50                # the 80px clock is taller than any other lock text
CARD_HALF_W, CARD_HALF_H = 240, 228  # lock-panel is 480px wide (fits a 12h clock); 456px tall with date and username shown
CUSTOM_COLOUR = (0x33, 0x66, 0x99)
CUSTOM_COLOUR_TOLERANCE = 3
CARD_COLOUR = (0x2A, 0x2E, 0x32)   # lock-panel background
PATTERN_QUADRANTS = {               # generated 320x200 PNG; 4x cover-scale at 1280x800 keeps it exact
    "top-left": ((0, 0, 160, 100), (0xC0, 0x39, 0x2B)),
    "top-right": ((160, 0, 320, 100), (0x27, 0xAE, 0x60)),
    "bottom-left": ((0, 100, 160, 200), (0x29, 0x80, 0xB9)),
    "bottom-right": ((160, 100, 320, 200), (0xF1, 0xC4, 0x0F)),
}
PATTERN_TOLERANCE = 6


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


def colour_dominates(pixels: list[tuple[int, int, int]], rgb: tuple[int, int, int], tolerance: int) -> bool:
    """At least 95% of the pixels sit within tolerance of rgb, and the dominant colour does too."""
    return (appear.fraction_matching(pixels, rgb, tolerance) >= 0.95
            and appear.close_enough(appear.dominant_color(pixels), rgb, tolerance))


def row_runs(rows: list[int]) -> list[tuple[int, int]]:
    """Inclusive (first, last) runs of consecutive row indices."""
    runs: list[tuple[int, int]] = []
    for y in sorted(set(rows)):
        if runs and y == runs[-1][1] + 1:
            runs[-1] = (runs[-1][0], y)
        else:
            runs.append((y, y))
    return runs


def rows_with(pixels: list[tuple[int, int, int]], width: int, rgb: tuple[int, int, int], tolerance: int) -> list[int]:
    """Indices of the rows of a `width`-pixel-wide crop that contain a pixel near rgb."""
    return [y for y in range(len(pixels) // width)
            if any(appear.close_enough(p, rgb, tolerance) for p in pixels[y * width:(y + 1) * width])]


def clock_and_username(runs: list[tuple[int, int]]) -> tuple[bool, bool]:
    """(clock present, username label present): the 96px clock is a tall run, the 20px username a short one."""
    heights = [last - first + 1 for first, last in runs]
    return any(h >= CLOCK_MIN_ROWS for h in heights), any(h < CLOCK_MIN_ROWS for h in heights)


def date_words(now: datetime) -> set[str]:
    """Lower-cased OCR tokens of today's date line (weekday with and without comma, month, year)."""
    weekday = now.strftime("%A").lower()
    return {weekday, weekday + ",", now.strftime("%B").lower(), now.strftime("%Y")}


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

    def crop(self, pixels: list[tuple[int, int, int]], rect: tuple[int, int, int, int]) -> list[tuple[int, int, int]]:
        """Crop (x, y, w, h) from a flat list captured at the screen size."""
        x0, y0, w, h = rect
        width = self.args.screen_w
        return [pixels[y * width + x] for y in range(y0, y0 + h) for x in range(x0, x0 + w)]

    def restart(self, values: dict[str, str]) -> bool:
        """Seed the settings, restart the WM so it serves them, then start a fresh lock instance."""
        seed_settings(self.ctx.run, values)
        self.ctx.stop(self.lock)
        self.lock = None
        if not base.restart_owned_wm(self.ctx, self.args, "flamewm-wm"):
            return False
        return self.ensure_lock()


def seed_settings(run: Path, values: dict[str, str]) -> None:
    # Same locations the appearance runner seeds: FLAMEWM_CONFIG_DIR (HOME/.config) and XDG_CONFIG_HOME.
    for root in (run / "home" / ".config", run / "xdg-config"):
        path = root / "flamewm" / "settings.toml"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(appear.settings_text(values), encoding="ascii")


def card_column(s: LockSession, pixels: list[tuple[int, int, int]]) -> list[tuple[int, int, int]]:
    """Full-height strip through the card centre (clock, date and username all lie inside it)."""
    return s.crop(pixels, (s.args.screen_w // 2 - CARD_HALF_W, 0, 2 * CARD_HALF_W, s.args.screen_h))


def column_text(s: LockSession, pixels: list[tuple[int, int, int]]) -> tuple[bool, bool]:
    column = card_column(s, pixels)
    return clock_and_username(row_runs(rows_with(column, 2 * CARD_HALF_W, TEXT_BRIGHT, TEXT_TOLERANCE)))


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


def lk07(s: LockSession) -> dict:
    if not s.restart({"lockBackgroundColor": "#336699"}):
        return entry("LK07", FAIL, "WM or lock did not start with the seeded colour")
    path, px = s.shot("LK07-colour")
    if px is None:
        return entry("LK07", ENV_BLOCKED, "capture failed")
    outside = s.crop(px, (0, 0, s.args.screen_w // 2 - CARD_HALF_W, s.args.screen_h))
    card = s.crop(px, (s.args.screen_w // 2 - CARD_HALF_W, s.args.screen_h // 2 - CARD_HALF_H,
                       2 * CARD_HALF_W, 2 * CARD_HALF_H))
    colour_ok = colour_dominates(outside, CUSTOM_COLOUR, CUSTOM_COLOUR_TOLERANCE)
    card_ok = appear.close_enough(appear.dominant_color(card), CARD_COLOUR, CUSTOM_COLOUR_TOLERANCE)
    clock_ok, _ = column_text(s, px)
    ok = colour_ok and card_ok and clock_ok
    reason = "" if ok else (f"outside_colour_ok={colour_ok} outside_dominant={appear.dominant_color(outside)} "
                            f"card_ok={card_ok} clock_readable={clock_ok}")
    return entry("LK07", PASS if ok else FAIL, reason,
                 [str(path), f"outside_dominant={appear.dominant_color(outside)}",
                  f"card_dominant={appear.dominant_color(card)}"],
                 ["xwininfo -root -tree", "import -window root"])


def lk08(s: LockSession) -> dict:
    if shutil.which("tesseract") is None:
        return entry("LK08", ENV_BLOCKED, "tesseract unavailable for date and AM/PM OCR")
    if not s.restart({}):
        return entry("LK08", FAIL, "WM or lock did not start with default settings")
    base_path, base_px = s.shot("LK08-default")
    if base_px is None:
        return entry("LK08", ENV_BLOCKED, "capture failed")
    base_clock, base_user = column_text(s, base_px)
    base_date = appear.ocr_word_centres(base_path, date_words(datetime.now()))
    if not (base_clock and base_user and base_date):
        return entry("LK08", ENV_BLOCKED,
                     f"default probe inconclusive: clock={base_clock} username={base_user} date_ocr={base_date}")
    if not s.restart({"lockClockFormat": "12h", "lockShowDate": "false", "lockShowUsername": "false"}):
        return entry("LK08", FAIL, "WM or lock did not start with the seeded clock, date and username settings")
    path, px = s.shot("LK08-12h-no-date-no-user")
    if px is None:
        return entry("LK08", ENV_BLOCKED, "capture failed")
    clock_ok, user_seen = column_text(s, px)
    ampm = appear.ocr_word_centres(path, {"am", "pm"})
    date_seen = appear.ocr_word_centres(path, date_words(datetime.now()))
    checks = {"clock_present": clock_ok, "ampm_ocr": bool(ampm), "date_absent": not date_seen,
              "username_absent": not user_seen}
    ok = all(checks.values())
    reason = "" if ok else "failed: " + ", ".join(name for name, passed in checks.items() if not passed)
    return entry("LK08", PASS if ok else FAIL, reason,
                 [str(path), f"ampm_ocr={ampm}", f"date_ocr_hits={date_seen}", f"username_seen={user_seen}",
                  f"default_date_ocr={base_date}"],
                 ["import -window root", "tesseract"])


def lk09(s: LockSession) -> dict:
    if base.Image is None:
        return entry("LK09", ENV_BLOCKED, "PIL unavailable to generate the background image")
    assets = s.ctx.run / "assets"
    assets.mkdir(parents=True, exist_ok=True)
    image_path = assets / "lock-pattern.png"
    canvas = base.Image.new("RGB", (320, 200))
    for box, rgb in PATTERN_QUADRANTS.values():
        canvas.paste(rgb, box)
    canvas.save(image_path, format="PNG")
    if not s.restart({"lockBackgroundImage": str(image_path)}):
        return entry("LK09", FAIL, "WM or lock did not start with the seeded background image")
    path, px = s.shot("LK09-image")
    if px is None:
        return entry("LK09", ENV_BLOCKED, "capture failed")
    w, h = s.args.screen_w, s.args.screen_h
    corners = {"top-left": (20, 20, 160, 120), "top-right": (w - 180, 20, 160, 120),
               "bottom-left": (20, h - 140, 160, 120), "bottom-right": (w - 180, h - 140, 160, 120)}
    seen, failed = [], []
    for name, rect in corners.items():
        expected = PATTERN_QUADRANTS[name][1]
        dom = appear.dominant_color(s.crop(px, rect))
        seen.append(f"{name}={dom}")
        if not appear.close_enough(dom, expected, PATTERN_TOLERANCE):
            failed.append(f"{name} {dom} != {expected}")
    ok = not failed
    return entry("LK09", PASS if ok else FAIL, "" if ok else "; ".join(failed),
                 [str(path), str(image_path)] + seen, ["import -window root"])


SCENARIOS = (lk01, lk02, lk03, lk04, lk05, lk06, lk07, lk08, lk09)


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

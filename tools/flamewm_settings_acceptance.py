#!/usr/bin/env python3
"""Isolated native acceptance runner for FlameWM System Settings.

Usage:
  tools/flamewm_settings_acceptance.py --run-dir DIR [--geometry WxH]
      [--display :N] [--wm-bin PATH] [--settings-bin PATH]
      [--settle-seconds S] [--hotkey-click X,Y] [--hotkey-key KEYSYM]
      [--hotkey-row-rect X,Y,W,H] [--hotkey-error-rect X,Y,W,H]
      [--ss12-click X,Y] [--ss12-reset-click X,Y] [--ss12-chord MOD+...+KEY]
      [--ss14-accent-pixel X,Y]
      [--ss05-click X,Y]
      [--ss10-scale-click X,Y] [--ss11-family-click X,Y] [--ss11-size-click X,Y]

For SS02, each launched settings page receives `--window-size WxH` and its
client geometry is measured independently from the Xephyr screen geometry.

What it does:
  * creates a fresh private run directory DIR (must not exist or be empty;
    must not live under .debug, .performance, .target or .agents/runtime);
  * re-execs itself once under `dbus-run-session` (private session bus);
  * gives the child apps a private HOME, XDG_CONFIG_HOME, XDG_DATA_HOME,
    XDG_CACHE_HOME and XDG_RUNTIME_DIR under DIR;
  * starts an owned Xephyr on a free display in :90-:99 (:97-:99 are preferred
    because tools/flamewm_xtest.py only accepts :97..:119) at --geometry;
  * starts the WM binary, then `flamewm-settings --page <page>` per page,
    capturing `import -display DISP -window root` into DIR/shots/;
  * sends XTEST input only through `tools/flamewm_xtest.py` (CLI, not copied);
  * writes DIR/result.json: canonical SS00..SS19 plus LEGACY_HARNESS_* sub-scenarios;
  * terminates only the PIDs it started, then verifies none remain.

Exit status: 0 when no scenario FAILs (0 never means READY; read `verdict`), 1 when any
scenario FAILs, 2 for usage errors, 3 when the run could not start (environment blocked).

Window sizes: SS02 launches every Settings page with the diagnostic
`--window-size WxH` option and records the actual client geometry from
`xwininfo`. A missing size option, wrong geometry, missing window or blank
capture fails SS02; screen size alone never counts as a window size.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import secrets
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
import tomllib
from dataclasses import dataclass
from pathlib import Path

from flamewm_harness_common import display_in_use, stop_process, write_json

try:  # Pillow is preferred for NATIVE_HOTKEY_INPUT crops and pixel diffs; ImageMagick is the fallback.
    from PIL import Image, ImageChops
except ImportError:
    Image = None
    ImageChops = None

HAVE_PIL = Image is not None

ROOT = Path(__file__).resolve().parents[1]


def workspace_version() -> str:
    with (ROOT / "Cargo.toml").open("rb") as fh:
        return tomllib.load(fh)["workspace"]["package"]["version"]
SCHEMA_VERSION = 2
SCHEMA = "flamewm-settings-acceptance/2"
PAGES = ("appearance", "desktop", "taskbar", "displays", "fonts", "hotkeys", "about")
SIZES = ((760, 500), (820, 540), (960, 640))
MIN_SCREEN = (max(w for w, _ in SIZES), max(h for _, h in SIZES))  # largest matrix size
SAFE_DISPLAYS = tuple(f":{n}" for n in (*range(90, 100), 102))
PREFERRED_DISPLAYS = tuple(f":{n}" for n in (97, 98, 99, 90, 91, 92, 93, 94, 95, 96))
XTEST_DISPLAY_RANGE = range(97, 120)
X11_SOCKET_DIR = Path("/tmp/.X11-unix")
FORBIDDEN_RUN_PREFIXES = (".debug", ".performance", ".target", ".agents/runtime")
RUN_OWNER_MARKER = ".run-owner"
GEOMETRY_RE = re.compile(r"^(\d+)x(\d+)$")
RECT_RE = re.compile(r"\d+,\d+,\d+,\d+")
HOTKEY_ROW_DEFAULT_SIZE = (460, 48)
HOTKEY_ERROR_DEFAULT_HEIGHT = 80
# A crop counts as changed when at least this fraction of its pixels differ.
HOTKEY_MIN_CHANGED_FRACTION = 0.002
WINDOW_RE = re.compile(r'0x[0-9a-fA-F]+ "([^"]*)": \([^)]*\)\s+(\d+)x(\d+)\+(-?\d+)\+(-?\d+)')
# `xwininfo -root -children` reports the WM's reparented frame. `-tree`
# includes its named client descendant, whose dimensions are the requested
# Settings client dimensions.
TREE_WINDOW_RE = re.compile(
    r'^\s*(0x[0-9a-fA-F]+)\s+(?:"([^"]*)"|\(has no name\)):\s*'
    r'.*?\b(\d+)x(\d+)\+(-?\d+)\+(-?\d+)'
    r'(?:\s+\+-?\d+\+-?\d+)?\s*$', re.MULTILINE
)
DESKTOP_OPACITY_MAX = 60
# SS10 target: the 125 percent Displays scale button (ui/settings.html display-scale-125).
SS10_TARGET_SCALE = 125
# SS05 wallpaper row (ui/settings.html: Choose, Clear, Default). Choose centre at 960x640 is screen x 637..725.
SS05_CHOOSE_CLIENT_DEFAULT = "681,195"
# At 820x540 the scale row is below the fold, so SS10 uses a client that shows it. When the output lists
# modes, SS10 wheel-scrolls the page with the pointer at the client centre before the scrolled click.
# Rightmost client columns hold the scrollbar track; keep it out of OCR crops.
SCROLLBAR_COLUMN_PX = 8
SS10_CLIENT = (960, 640)
SS10_WHEEL_POINTER = (SS10_CLIENT[0] // 2, SS10_CLIENT[1] // 2)
# SS12 fixture constants measured natively on a private Xephyr (Settings --page hotkeys, default 820x540
# client at screen origin (0,31)): Toggle Start menu capture field centre and its Reset button centre.
# They are only used after the live client geometry equals SS12_CLIENT; any other size is refused.
SS12_CLIENT = (820, 540)
SS12_CAPTURE_CLIENT = (593, 149)
SS12_RESET_CLIENT = (733, 149)
START_TOGGLE_MATCH = "type='signal',interface='com.arkflame.FlameWM1',member='StartMenuToggleRequested'"
# Platform default binding for ToggleStartMenu (crates/flamewm-platform/src/shortcuts.rs).
SHORTCUT_DEFAULT_BINDING = "Super_L"

STATUS_PASS = "PASS"
STATUS_FAIL = "FAIL"
STATUS_NOT_RUN = "NOT_RUN"
STATUS_ENV_BLOCKED = "ENVIRONMENT_BLOCKED"
STATUSES = (STATUS_PASS, STATUS_FAIL, STATUS_NOT_RUN, STATUS_ENV_BLOCKED)
LEGACY_PREFIX = "LEGACY_HARNESS_"

# Canonical wave-1 acceptance matrix. The labels are the contract; do not relabel.
CANONICAL_MATRIX: tuple[tuple[str, str], ...] = (
    ("SS00", "source/git preservation"),
    ("SS01", "seven pages open"),
    ("SS02", "seven pages x 760x500/820x540/960x640"),
    ("SS03", "accent"),
    ("SS04", "icon theme"),
    ("SS05", "wallpaper"),
    ("SS06", "sticky notes/opacities"),
    ("SS07", "taskbar"),
    ("SS08", "displays 1/2/4/5+ outputs and modes"),
    ("SS09", "15s rollback"),
    ("SS10", "scale"),
    ("SS11", "fonts"),
    ("SS12", "hotkeys"),
    ("SS13", "about"),
    ("SS14", "external signal + auto-revert"),
    ("SS15", "traversal/disabled states"),
    ("SS16", "cargo/CSS/architecture gates"),
    ("SS17", "private XTEST+screenshot matrix"),
    ("SS18", "installed-prefix release"),
    ("SS19", "stability/perf/PID cleanup"),
)
CANONICAL_IDS = tuple(sid for sid, _ in CANONICAL_MATRIX)

# Legacy sub-scenarios of this runner: (id, description, canonical ID they partially cover or None).
# Their reports carry the LEGACY_HARNESS_ prefix and never count as a canonical PASS on their own.
LEGACY_SUBSCENARIOS: tuple[tuple[str, str, str | None], ...] = (
    ("NATIVE_ONE_OUTPUT", "Displays page with exactly one output (Xephyr RandR)", "SS08"),
    ("NATIVE_HOTKEY_INPUT", "Hotkey capture via XTEST keys", "SS12"),
    ("NATIVE_PID_CLEANUP", "Runner teardown: every PID it started is gone", "SS19"),
    ("NATIVE_PERSISTENCE_RESTART", "Persistence and restart under private XDG directories", None),
)
LEGACY_IDS = tuple(lid for lid, _, _ in LEGACY_SUBSCENARIOS)
# Sub-scenarios this runner executes (SS00 and NATIVE_PID_CLEANUP are always set by inner()).
RUNNER_IDS = ("SS01", "SS02", "SS03", "SS05", "SS10", "SS11", "SS12", "SS13", "SS15", "SS19",
              "NATIVE_ONE_OUTPUT", "NATIVE_HOTKEY_INPUT", "NATIVE_PERSISTENCE_RESTART")

TITLES: dict[str, str] = dict(CANONICAL_MATRIX)
TITLES.update({lid: f"{LEGACY_PREFIX}{lid}: {desc}" for lid, desc, _ in LEGACY_SUBSCENARIOS})

# Why a canonical scenario is not runnable on this host or by this runner (appended to NOT_RUN reasons).
CANONICAL_NOTES: dict[str, str] = {
    "SS08": "the 2/4/5+ output and mode fixtures need a virtual RandR provider this host does not supply",
    "SS09": "run_ss09 was not reached",
    "SS14": "the native Settings accent case does not cover panels, shortcuts, display rollback or signal loss",
    "SS18": "the installed-prefix release gate is outside the isolated runner",
}

def utc_now() -> str:
    return time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())


def shell(argv: list[str]) -> str:
    return shlex.join(str(part) for part in argv)


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--run-dir", required=True, help="new private run directory (must not exist or be empty)")
    parser.add_argument("--geometry", default="1280x800", help="Xephyr screen WxH (default 1280x800, minimum 960x640)")
    parser.add_argument("--display", default=None, help="explicit display in :90..:99 or :102; refused when already in use")
    parser.add_argument("--wm-bin", default=str(ROOT / ".target/debug/flamewm"))
    parser.add_argument("--settings-bin", default=None, help="default: flamewm-settings next to --wm-bin")
    parser.add_argument("--shell-bin", default=None, help="default: flamewm-shell next to --wm-bin")
    parser.add_argument("--desktop-bin", default=None, help="default: flamewm-desktop next to --wm-bin")
    parser.add_argument("--settle-seconds", type=float, default=3.0)
    parser.add_argument("--hotkey-click", metavar="X,Y", default=None, help="XTEST click before the NATIVE_HOTKEY_INPUT key press")
    parser.add_argument("--hotkey-key", metavar="KEYSYM", default=None, help="XTEST key for NATIVE_HOTKEY_INPUT (needs --hotkey-click too)")
    parser.add_argument("--hotkey-row-rect", metavar="X,Y,W,H", default=None,
                        help="NATIVE_HOTKEY_INPUT crop of the hotkey row (default: 460x48 band centred on --hotkey-click)")
    parser.add_argument("--hotkey-error-rect", metavar="X,Y,W,H", default=None,
                        help="NATIVE_HOTKEY_INPUT crop of the error bar (default: full-width bottom 80px band of the screen)")
    parser.add_argument("--ss12-click", metavar="X,Y", default=None,
                        help="SS12 screen click on the ToggleStartMenu capture row (default: measured 820x540 fixture constant)")
    parser.add_argument("--ss12-reset-click", metavar="X,Y", default=None,
                        help="SS12 screen click on the ToggleStartMenu Reset button (default: measured 820x540 fixture constant)")
    parser.add_argument("--ss12-chord", metavar="MOD+...+KEY", default="Super+F8",
                        help="SS12 binding to capture and replay after lease end (default Super+F8)")
    parser.add_argument("--ss18-installed-result", metavar="PATH", default=None,
                        help="installed-result.json from tools/flamewm_settings_installed_acceptance.py to map into SS18")
    parser.add_argument("--ss18-prefix", metavar="DIR", default=None,
                        help="staged package prefix; the runner invokes scripts/settings-installed-acceptance on it and "
                             "ingests installed-result.json (ignored when --ss18-installed-result is given)")
    parser.add_argument("--canary-bin", default=str(ROOT / ".target/debug/flamewm-x11-canary"),
                        help="flamewm-x11-canary binary used by SS17 j07 rows")
    parser.add_argument("--ss14-accent-pixel", metavar="X,Y", default="100,92",
                        help="SS14 client-relative point inside the selected nav-row background (default: 100,92)")
    parser.add_argument("--ss05-click", metavar="X,Y", default=SS05_CHOOSE_CLIENT_DEFAULT,
                        help="SS05 client-relative wallpaper Choose center at 960x640, left of Clear and Default "
                             f"(default: {SS05_CHOOSE_CLIENT_DEFAULT})")
    parser.add_argument("--ss10-scale-click", metavar="X,Y", default="751,553",
                        help="SS10 client-relative click on the 125 scale button of the Displays page (960x640 client, scale row visible); default measured")
    parser.add_argument("--ss10-wheel-steps", metavar="N", type=int, default=None,
                        help="SS10 wheel-down steps over the Displays page; required when the output lists modes")
    parser.add_argument("--ss10-scrolled-click", metavar="X,Y", default=None,
                        help="SS10 client-relative click on the 125 scale button after the wheel scroll (960x640 client); "
                             "required when the output lists modes")
    parser.add_argument("--ss11-family-click", metavar="X,Y", default="697,150",
                        help="SS11 client-relative click on the Font family control of the Fonts page (820x540 client); default measured")
    parser.add_argument("--ss11-size-click", metavar="X,Y", default="760,254",
                        help="SS11 client-relative click on the Text size + control of the Fonts page (820x540 client); default measured")
    args = parser.parse_args(argv)
    # Children start with cwd set to the run directory, so relative binary paths must be resolved first.
    args.wm_bin = str(Path(args.wm_bin).resolve())
    for name in ("settings", "shell", "desktop"):
        value = getattr(args, f"{name}_bin") or str(Path(args.wm_bin).parent / f"flamewm-{name}")
        setattr(args, f"{name}_bin", str(Path(value).resolve()))
    args.canary_bin = str(Path(args.canary_bin).resolve())
    return args


def parse_rect(text: str) -> tuple[int, int, int, int] | None:
    if not RECT_RE.fullmatch(text):
        return None
    x, y, width, height = (int(part) for part in text.split(","))
    if width <= 0 or height <= 0:
        return None
    return x, y, width, height


def validate(args: argparse.Namespace) -> Path:
    match = GEOMETRY_RE.fullmatch(args.geometry)
    if not match:
        raise SystemExit("error: --geometry must be WxH")
    width, height = int(match.group(1)), int(match.group(2))
    if width < MIN_SCREEN[0] or height < MIN_SCREEN[1]:
        raise SystemExit("error: --geometry must be at least 960x640 so the size matrix fits")
    if args.display is not None and args.display not in SAFE_DISPLAYS:
        raise SystemExit("error: --display must be one of :90..:99 or :102")
    if args.hotkey_click is not None and not re.fullmatch(r"\d+,\d+", args.hotkey_click):
        raise SystemExit("error: --hotkey-click must be X,Y")
    if args.hotkey_click is not None:
        click_x, click_y = (int(part) for part in args.hotkey_click.split(","))
        if click_x >= width or click_y >= height:
            raise SystemExit("error: --hotkey-click must lie inside --geometry")
    if args.ss12_click is not None:
        if not re.fullmatch(r"\d+,\d+", args.ss12_click):
            raise SystemExit("error: --ss12-click must be X,Y")
        click_x, click_y = (int(part) for part in args.ss12_click.split(","))
        if click_x >= width or click_y >= height:
            raise SystemExit("error: --ss12-click must lie inside --geometry")
    if args.ss12_reset_click is not None:
        if not re.fullmatch(r"\d+,\d+", args.ss12_reset_click):
            raise SystemExit("error: --ss12-reset-click must be X,Y")
        click_x, click_y = (int(part) for part in args.ss12_reset_click.split(","))
        if click_x >= width or click_y >= height:
            raise SystemExit("error: --ss12-reset-click must lie inside --geometry")
    if args.ss18_prefix is not None and not Path(args.ss18_prefix).is_dir():
        raise SystemExit("error: --ss18-prefix must be an existing directory")
    if not re.fullmatch(r"\d+,\d+", args.ss05_click):
        raise SystemExit("error: --ss05-click must be X,Y")
    ss05_x, ss05_y = (int(part) for part in args.ss05_click.split(","))
    if ss05_x >= 960 or ss05_y >= 640:
        raise SystemExit("error: --ss05-click must lie inside the 960x640 Settings client")
    for flag, value, (limit_x, limit_y) in (
            ("--ss10-scale-click", args.ss10_scale_click, SS10_CLIENT),
            ("--ss10-scrolled-click", args.ss10_scrolled_click, SS10_CLIENT),
            ("--ss11-family-click", args.ss11_family_click, (820, 540)),
            ("--ss11-size-click", args.ss11_size_click, (820, 540))):
        if value is None:
            continue
        if not re.fullmatch(r"\d+,\d+", value):
            raise SystemExit(f"error: {flag} must be X,Y")
        point_x, point_y = (int(part) for part in value.split(","))
        if point_x >= limit_x or point_y >= limit_y:
            raise SystemExit(f"error: {flag} must lie inside the {limit_x}x{limit_y} Settings client")
    if args.ss10_wheel_steps is not None:
        import flamewm_xtest
        if not 1 <= args.ss10_wheel_steps <= flamewm_xtest.MAX_WHEEL_STEPS:
            raise SystemExit(f"error: --ss10-wheel-steps must be 1..{flamewm_xtest.MAX_WHEEL_STEPS}")
    if args.ss14_accent_pixel is not None:
        if not re.fullmatch(r"\d+,\d+", args.ss14_accent_pixel):
            raise SystemExit("error: --ss14-accent-pixel must be X,Y")
        pixel_x, pixel_y = (int(part) for part in args.ss14_accent_pixel.split(","))
        if pixel_x >= 820 or pixel_y >= 540:
            raise SystemExit("error: --ss14-accent-pixel must lie inside the Settings client")
    try:
        import flamewm_xtest
        flamewm_xtest.parse_chord(args.ss12_chord)
    except ValueError as error:
        raise SystemExit(f"error: --ss12-chord: {error}") from error
    for flag, value in (("--hotkey-row-rect", args.hotkey_row_rect), ("--hotkey-error-rect", args.hotkey_error_rect)):
        if value is None:
            continue
        rect = parse_rect(value)
        if rect is None:
            raise SystemExit(f"error: {flag} must be X,Y,W,H with W and H greater than 0")
        if rect[0] + rect[2] > width or rect[1] + rect[3] > height:
            raise SystemExit(f"error: {flag} must lie inside --geometry {args.geometry}")
    run = Path(args.run_dir).expanduser().resolve()
    for prefix in FORBIDDEN_RUN_PREFIXES:
        forbidden = (ROOT / prefix).resolve()
        if run == forbidden or forbidden in run.parents:
            raise SystemExit(f"error: run dir may not live under {prefix}")
    owned_by_outer = (os.environ.get("FLAMEWM_SS_INNER") == "1"
                      and (run / RUN_OWNER_MARKER).is_file())
    if run.exists() and not owned_by_outer and (not run.is_dir() or any(run.iterdir())):
        raise SystemExit("error: --run-dir must not exist or must be an empty directory")
    if not (ROOT / "Cargo.toml").is_file():
        raise SystemExit("error: repository root does not contain Cargo.toml")
    return run


def prepare_run(run: Path) -> None:
    run.mkdir(parents=True, exist_ok=True)
    os.chmod(run, 0o700)
    for name in ("home", "xdg-config", "xdg-data", "xdg-cache", "xdg-runtime", "shots", "logs"):
        (run / name).mkdir(parents=True, exist_ok=True)
    os.chmod(run / "xdg-runtime", 0o700)
    (run / RUN_OWNER_MARKER).write_text(f"owner-pid={os.getpid()}\ncreated={utc_now()}\n", encoding="ascii")


def git_snapshot() -> str:
    try:
        result = subprocess.run(["git", "-C", str(ROOT), "status", "--porcelain"],
                                capture_output=True, text=True, timeout=120, check=False)
    except (OSError, subprocess.SubprocessError) as error:
        return f"unavailable:{error}"
    return hashlib.sha256(result.stdout.encode("utf-8", "replace")).hexdigest()


@dataclass
class Owned:
    name: str
    proc: subprocess.Popen

    @property
    def pid(self) -> int:
        return self.proc.pid


class Ctx:
    """Owns every child process this run starts; nothing is killed by name."""

    def __init__(self, run: Path) -> None:
        self.run = run
        self.owned: list[Owned] = []
        self.display: str | None = None
        self.host_env = dict(os.environ)
        self.app_env: dict[str, str] = {}
        self.token: str | None = None
        self.marker: Path = run / "xephyr-owner"
        self.wm: Owned | None = None

    def wm_dead(self) -> bool:
        return self.wm is None or self.wm.proc.poll() is not None

    def build_app_env(self) -> None:
        self.app_env = {
            "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
            "LANG": "C.UTF-8",
            "HOME": str(self.run / "home"),
            "XDG_CONFIG_HOME": str(self.run / "xdg-config"),
            "XDG_DATA_HOME": str(self.run / "xdg-data"),
            "XDG_CACHE_HOME": str(self.run / "xdg-cache"),
            "XDG_RUNTIME_DIR": str(self.run / "xdg-runtime"),
            "DBUS_SESSION_BUS_ADDRESS": os.environ.get("DBUS_SESSION_BUS_ADDRESS", ""),
        }
        if self.display:
            self.app_env["DISPLAY"] = self.display

    def spawn(self, name: str, argv: list[str], env: dict[str, str]) -> Owned:
        log = open(self.run / "logs" / f"{name}.log", "ab")
        try:
            proc = subprocess.Popen(argv, env=env, stdout=log, stderr=subprocess.STDOUT, cwd=str(self.run))
        finally:
            log.close()
        owned = Owned(name, proc)
        self.owned.append(owned)
        return owned

    @staticmethod
    def stop(owned: Owned | None) -> None:
        if owned is None:
            return
        stop_process(owned.proc)

    def teardown(self) -> dict:
        for owned in reversed(self.owned):
            self.stop(owned)
        remaining = [o.pid for o in self.owned if o.proc.poll() is None]
        return {
            "owned_pids": [{"name": o.name, "pid": o.pid, "exit_code": o.proc.poll()} for o in self.owned],
            "remaining": remaining,
            "ok": not remaining,
        }


def pick_display(explicit: str | None) -> str | None:
    candidates = [explicit] if explicit else list(PREFERRED_DISPLAYS)
    for display in candidates:
        number = display[1:]
        if display_in_use(number):
            if explicit:
                raise SystemExit(f"error: display {display} is already in use; refusing")
            continue
        return display
    return None


def wait_for_display(ctx: Ctx, proc: Owned, timeout: float = 20.0) -> bool:
    deadline = time.monotonic() + timeout
    socket = X11_SOCKET_DIR / f"X{ctx.display[1:]}"
    while time.monotonic() < deadline:
        if proc.proc.poll() is not None:
            return False
        if socket.exists():
            probe = subprocess.run(["xdpyinfo", "-display", ctx.display], env=ctx.host_env,
                                   capture_output=True, timeout=10, check=False)
            if probe.returncode == 0:
                return True
        time.sleep(0.25)
    return False


def capture(ctx: Ctx, out: Path) -> tuple[bool, str]:
    argv = ["import", "-display", ctx.display, "-window", "root", str(out)]
    result = subprocess.run(argv, env=ctx.host_env, capture_output=True, timeout=60, check=False)
    ok = result.returncode == 0 and out.is_file() and out.stat().st_size > 0
    return ok, shell(argv)


def nonblank(path: Path) -> bool | None:
    result = subprocess.run(["identify", "-format", "%k", str(path)], env=dict(os.environ),
                            capture_output=True, text=True, timeout=60, check=False)
    if result.returncode != 0:
        return None
    try:
        return int(result.stdout.strip()) > 1
    except ValueError:
        return None


def parse_window_tree(text: str) -> dict | None:
    """Select the named Settings client from an xwininfo recursive tree.

    xwininfo prints x/y relative to the parent, which is the WM frame. The returned x/y are
    screen coordinates (sum of the relative positions up the chain), so a frame the WM moved
    (for example on re-adoption) is reflected instead of the client's constant frame offset.
    """
    best = None
    stack: list[tuple[int, int, int]] = []  # (indent, screen_x, screen_y) of the ancestors
    for line in text.splitlines():
        match = TREE_WINDOW_RE.match(line)
        if not match:
            continue
        indent = len(line) - len(line.lstrip())
        while stack and stack[-1][0] >= indent:
            stack.pop()
        _, name, width, height, rel_x, rel_y = match.groups()
        base_x, base_y = (stack[-1][1], stack[-1][2]) if stack else (0, 0)
        screen_x, screen_y = base_x + int(rel_x), base_y + int(rel_y)
        stack.append((indent, screen_x, screen_y))
        if not name or not re.search(r"(?i)settings", name):
            continue
        candidate = {"name": name, "width": int(width), "height": int(height), "x": screen_x, "y": screen_y}
        if best is None or candidate["width"] * candidate["height"] > best["width"] * best["height"]:
            best = candidate
    return best


def window_geometry(ctx: Ctx, text_out: Path) -> dict | None:
    result = subprocess.run(["xwininfo", "-display", ctx.display, "-root", "-tree"],
                            env=ctx.host_env, capture_output=True, text=True, timeout=30, check=False)
    text_out.write_text(result.stdout + result.stderr, encoding="utf-8")
    if result.returncode != 0:
        return None
    return parse_window_tree(result.stdout)


def page_shot(ctx: Ctx, args: argparse.Namespace, page: str, label: str,
              window_size: tuple[int, int] | None = None) -> dict:
    launch = [args.settings_bin, "--page", page]
    if window_size is not None:
        launch += ["--window-size", f"{window_size[0]}x{window_size[1]}"]
    settings = ctx.spawn(f"settings-{page}-{label}", launch, ctx.app_env)
    time.sleep(args.settle_seconds)
    alive = settings.proc.poll() is None
    out = ctx.run / "shots" / f"{label}-{page}.png"
    captured, capture_cmd = capture(ctx, out)
    geometry = window_geometry(ctx, out.with_suffix(".xwininfo.txt")) if captured else None
    blank_check = nonblank(out) if captured else False
    ctx.stop(settings)
    return {
        "page": page,
        "label": label,
        "alive": alive,
        "captured": captured,
        "nonblank": blank_check,
        "shot": str(out),
        "window": geometry,
        "commands": [shell(launch), capture_cmd],
    }


def exact_window_size(shot: dict, expected: tuple[int, int]) -> bool:
    """Require a live capture and measured client geometry equal to the request."""
    window = shot.get("window")
    return bool(
        shot.get("alive") and shot.get("captured") and shot.get("nonblank") is True
        and window is not None
        and (window.get("width"), window.get("height")) == expected
    )


def next_desktop_opacity(current: int) -> int:
    """Choose a distinct valid SS06 fixture value for the bounded 0..60 fields."""
    if not 0 <= current <= DESKTOP_OPACITY_MAX:
        raise ValueError(f"desktop opacity must be 0..{DESKTOP_OPACITY_MAX}")
    return 21 if current != 21 else 20


def assess_ss12(observations: dict[str, bool | None]) -> tuple[str, str]:
    """Grade the complete SS12 contract; unmeasured subcontracts cannot pass."""
    required = ("lease_begin_end_same_owner", "binding_stored", "action_effect",
                "grab_restored_start_behavior", "owner_loss_restored", "clear_reset_persisted")
    failed = [key for key in required if observations.get(key) is False]
    if failed:
        return STATUS_FAIL, "measured hotkey contract failure: " + ", ".join(failed)
    missing = [key for key in required if observations.get(key) is not True]
    if missing:
        return STATUS_NOT_RUN, "partial capture evidence; unverified contract cases: " + ", ".join(missing)
    return STATUS_PASS, "capture lease, stored binding, action, grab restoration, owner loss and persistence verified"


def assess_ss03(observations: dict[str, bool | None]) -> tuple[str, str]:
    required = ("blue_snapshot", "teal_snapshot", "selected_visuals", "reset_persisted_after_restart")
    failed = [key for key in required if observations.get(key) is False]
    if failed:
        return STATUS_FAIL, "measured accent contract failure: " + ", ".join(failed)
    missing = [key for key in required if observations.get(key) is not True]
    if missing:
        return STATUS_NOT_RUN, "partial accent evidence; unverified: " + ", ".join(missing)
    return STATUS_PASS, "palette choices, visible selection and persisted value verified"


def ss05_wallpaper_matches_fixture(value: str | None, fixture: Path, run: Path) -> bool:
    """The product copies a chosen wallpaper into its managed store ($XDG_DATA_HOME/flamewm/wallpapers,
    media_store.rs, commit a63a6908), so the authoritative value is the store copy, not the picked path.
    Accept the picked path itself or a file inside that store with byte-identical contents."""
    if not value:
        return False
    store = (run / "xdg-data" / "flamewm" / "wallpapers").resolve()
    try:
        candidate = Path(value).resolve()
        if candidate == fixture.resolve():
            return True
        return candidate.is_relative_to(store) and candidate.read_bytes() == fixture.read_bytes()
    except OSError:
        return False


def assess_ss05(observations: dict[str, bool | None]) -> tuple[str, str]:
    required = ("success_applied_without_input", "cancel_preserved_without_error",
                "error_preserved_and_visible")
    failed = [key for key in required if observations.get(key) is False]
    if failed:
        return STATUS_FAIL, "measured wallpaper chooser failure: " + ", ".join(failed)
    missing = [key for key in required if observations.get(key) is not True]
    if missing:
        return STATUS_NOT_RUN, "unverified wallpaper chooser cases: " + ", ".join(missing)
    return STATUS_PASS, "success, cancel and error effects verified without follow-up input"


def assess_ss15(observations: dict[str, bool | None]) -> tuple[str, str]:
    required = ("single_output_boundary_observed", "disabled_output_prev_inert",
                "disabled_output_next_inert", "disabled_mode_ends_inert",
                "settings_survived_keyboard_escape")
    failed = [key for key in required if observations.get(key) is False]
    if failed:
        return STATUS_FAIL, "measured traversal/disabled-state failure: " + ", ".join(failed)
    missing = [key for key in required if observations.get(key) is not True]
    if missing:
        return STATUS_NOT_RUN, "partial traversal evidence; unverified: " + ", ".join(missing)
    return STATUS_PASS, "disabled pager endpoints were inert and keyboard Escape left Settings responsive"


def ss15_escape_state_ok(process_alive: bool, geometry: dict | None, input_exits: tuple[int, int, int],
                         initial: str | None, cleared: str | None, restored: str | None) -> bool:
    return bool(process_alive and geometry is not None
                and (geometry.get("width"), geometry.get("height")) == (820, 540)
                and input_exits == (0, 0, 0) and initial is not None and cleared == ""
                and restored == initial)


def assess_ss19(observations: dict[str, bool | None]) -> tuple[str, str]:
    required = ("repeated_pages_opened_and_closed", "idle_cpu_bounded", "fd_growth_bounded",
                "memory_growth_bounded", "external_update_responsive", "owned_processes_clean")
    failed = [key for key in required if observations.get(key) is False]
    if failed:
        return STATUS_FAIL, "measured stability failure: " + ", ".join(failed)
    missing = [key for key in required if observations.get(key) is not True]
    if missing:
        return STATUS_NOT_RUN, "partial stability evidence; unverified: " + ", ".join(missing)
    return STATUS_PASS, "bounded idle use, repeated page lifecycle, external update and owned-process cleanup verified"


def settings_relative_point(ctx: Ctx, geometry: dict, point: tuple[int, int]) -> str:
    """Translate a point in the 820x540 Settings client into owned X screen coordinates."""
    return f"{geometry['x'] + point[0]},{geometry['y'] + point[1]}"


def ss05_choose_screen_point(geometry: dict, point: tuple[int, int]) -> tuple[int, int]:
    """Translate measured client-relative Choose center to the X screen."""
    return geometry["x"] + point[0], geometry["y"] + point[1]


def ss14_accent_screen_point(geometry: dict, point: tuple[int, int]) -> tuple[int, int]:
    """Sample nav-selection interior, away from its static 3px accent stripe."""
    return geometry["x"] + point[0], geometry["y"] + point[1]


def expected_selected_row_pixel(accent: str, base: tuple[int, int, int] = (27, 30, 32), alpha: int = 56):
    """CSS nav-selection is the typed accent at alpha=56 over the #1b1e20 sidebar."""
    rgb = accent_rgb(accent)
    if rgb is None:
        return None
    return tuple((src * alpha + dst * (255 - alpha) + 127) // 255 for src, dst in zip(rgb, base))


def wait_for_settings_window(ctx: Ctx, evidence_path: Path, timeout: float = 5.0) -> dict | None:
    """Wait for a mapped, measurable Settings client before taking baseline screenshots."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        geometry = window_geometry(ctx, evidence_path)
        if geometry is not None and geometry["width"] > 0 and geometry["height"] > 0:
            return geometry
        time.sleep(0.05)
    return None


def ss03_selected_row_matches(pixel: tuple[int, int, int] | None, accent: str) -> bool:
    expected = expected_selected_row_pixel(accent)
    return bool(pixel is not None and expected is not None
                and max(abs(actual - target) for actual, target in zip(pixel, expected)) <= 8)


def ss03_nav_click_diagnostic(ctx: Ctx, geometry: dict, run: Path) -> dict:
    """Click Desktop then Appearance and compare title-region captures to diagnose hit delivery."""
    title_rect = (geometry["x"] + 230, geometry["y"] + 35, 420, 70)
    observations = {}
    for page, point in (("appearance_before", (70, 92)), ("desktop", (70, 132)),
                        ("appearance_after", (70, 92))):
        if page != "appearance_before":
            send_input(ctx, settings_relative_point(ctx, geometry, point), None)
            time.sleep(0.25)
        shot = run / "shots" / f"ss03-nav-diagnostic-{page}.png"
        captured, _ = capture(ctx, shot)
        observations[page] = image_region_digest(shot, title_rect) if captured else None
    return {"captures": observations,
            "navigation_changed_and_returned": bool(observations["appearance_before"] is not None
                and observations["desktop"] is not None
                and observations["appearance_after"] is not None
                and observations["appearance_before"] != observations["desktop"]
                and observations["appearance_before"] == observations["appearance_after"])}


def capture_accent_visual(ctx: Ctx, run: Path, name: str, geometry: dict,
                          swatch_point: tuple[int, int], expected: str) -> dict:
    shot = run / "shots" / f"ss03-{name}.png"
    captured, command = capture(ctx, shot)
    expected_rgb = accent_rgb(expected)
    # Runtime positions nav-selection at x=8 and nav_top(Appearance)=72, with a
    # 192x38 background; x=170,y=92 samples its interior. x=100 sat on the "Appearance"
    # label glyphs (ui/settings.css nav label), so the sample read text antialiasing.
    accent_pixel = image_pixel(shot, geometry["x"] + 170, geometry["y"] + 92) if captured else None
    # HTML/CSS defines a 30x30 button with a centered 9x9 white selected marker.
    # Count marker-colored pixels in that source-defined 9x9 region, not one assumed center pixel.
    marker = image_region_color_count(shot, (geometry["x"] + swatch_point[0] - 4,
        geometry["y"] + swatch_point[1] - 4, 9, 9), (241, 242, 243)) if captured else None
    return {
        "shot": str(shot), "command": command, "captured": captured,
        "accent_pixel": accent_pixel, "expected_rgb": expected_rgb,
        "selected_row_pixel": accent_pixel, "expected_selected_row_pixel": expected_selected_row_pixel(expected),
        "selected_marker_white_pixels_9x9": marker,
        "ok": bool(expected_rgb and ss03_selected_row_matches(accent_pixel, expected)
                   and marker is not None and marker >= 16),
    }


def inspect_accent_after_click(ctx: Ctx, run: Path, name: str, geometry: dict,
                               swatch_point: tuple[int, int], expected: str,
                               click_exit: int) -> dict:
    """Move the pointer before sampling pixels and assemble evidence in dependency order."""
    move_exit, move_command = send_input(ctx, None, None, move="900,600")
    visual = capture_accent_visual(ctx, run, name, geometry, swatch_point, expected)
    return {
        "move_exit": move_exit,
        "move_command": move_command,
        "visual": visual,
        "passed": click_exit == 0 and move_exit == 0 and visual["ok"],
    }


def dbus_get_snapshot(ctx: Ctx, interface: str) -> tuple[str | None, str]:
    argv = ["gdbus", "call", "--session", "--dest", "com.arkflame.FlameWM1",
            "--object-path", "/com/arkflame/FlameWM1", "--method",
            f"com.arkflame.FlameWM1.{interface}.GetSnapshot"]
    try:
        result = subprocess.run(argv, env=ctx.app_env, capture_output=True, text=True,
                                timeout=10, check=False)
    except (OSError, subprocess.SubprocessError) as error:
        return None, str(error)
    if result.returncode:
        return None, result.stderr.strip() or f"gdbus exited {result.returncode}"
    return result.stdout, shell(argv)


def dbus_displays_snapshot(ctx: Ctx) -> tuple[str | None, str]:
    return dbus_get_snapshot(ctx, "Displays")


def parse_proc_metrics(pid: int) -> dict | None:
    """Read process-local counters without invoking ps or changing the process."""
    proc = Path("/proc") / str(pid)
    try:
        stat = (proc / "stat").read_text(encoding="ascii")
        fields = stat[stat.rfind(")") + 2:].split()
        # The remainder starts at field 3 (state); utime/stime are fields 14/15.
        cpu_ticks = int(fields[11]) + int(fields[12])
        status = (proc / "status").read_text(encoding="ascii")
        rss = re.search(r"^VmRSS:\s+(\d+)\s+kB$", status, re.MULTILINE)
        return {"cpu_ticks": cpu_ticks, "rss_kb": int(rss.group(1)) if rss else None,
                "fd_count": len(list((proc / "fd").iterdir()))}
    except (OSError, ValueError, IndexError):
        return None


def image_region_digest(path: Path, rect: tuple[int, int, int, int]) -> str | None:
    """Hash a measured status-label crop so pager progress is not inferred from a click exit."""
    if Image is None:
        return None
    try:
        with Image.open(path) as shot:
            x, y, width, height = rect
            if x < 0 or y < 0 or x + width > shot.width or y + height > shot.height:
                return None
            return hashlib.sha256(shot.convert("RGB").crop((x, y, x + width, y + height)).tobytes()).hexdigest()
    except OSError:
        return None


def image_region_color_count(path: Path, rect: tuple[int, int, int, int],
                             color: tuple[int, int, int]) -> int | None:
    if Image is None:
        return None
    try:
        with Image.open(path) as shot:
            x, y, width, height = rect
            if x < 0 or y < 0 or x + width > shot.width or y + height > shot.height:
                return None
            pixels = shot.convert("RGB").crop((x, y, x + width, y + height)).getdata()
            return sum(1 for pixel in pixels if max(
                abs(actual - expected) for actual, expected in zip(pixel, color)) <= 12)
    except OSError:
        return None


def capture_lease_calls(log: str) -> list[tuple[str, str | None]]:
    """Return observed Begin/End method calls and unique bus senders from dbus-monitor."""
    calls = []
    for line in log.splitlines():
        if "member=BeginCapture" in line or "member=EndCapture" in line:
            member = "BeginCapture" if "member=BeginCapture" in line else "EndCapture"
            sender = re.search(r"sender=([^ ]+)", line)
            calls.append((member, sender.group(1) if sender else None))
    return calls


def assess_ss14(observations: dict[str, bool | None]) -> tuple[str, str]:
    """Only pass SS14 when all independent signal paths and rollback are exercised."""
    required = ("settings_accent_source_revision", "settings_signals_observed",
                "settings_visible_without_input", "panels_external_change",
                "shortcuts_external_change", "display_rollback_signal", "signal_loss_recovery")
    failed = [key for key in required if observations.get(key) is False]
    if failed:
        return STATUS_FAIL, "measured external-update failure: " + ", ".join(failed)
    missing = [key for key in required if observations.get(key) is not True]
    if missing:
        return STATUS_NOT_RUN, "partial external-update evidence; unverified paths: " + ", ".join(missing)
    return STATUS_PASS, "settings, panels, shortcuts, display rollback and signal-loss paths verified"


def assess_ss04(observations: dict[str, bool | None]) -> tuple[str, str]:
    required = ("supported_theme_snapshots", "unknown_theme_snapshot", "real_icon_fallback_consumer",
                "theme_persistence_after_restart")
    failed = [key for key in required if observations.get(key) is False]
    if failed:
        return STATUS_FAIL, "measured icon-theme failure: " + ", ".join(failed)
    missing = [key for key in required if observations.get(key) is not True]
    if missing:
        return STATUS_NOT_RUN, "partial icon-theme evidence; unverified: " + ", ".join(missing)
    return STATUS_PASS, "theme cycling, fallback consumer and restart persistence verified"


def assess_ss06(observations: dict[str, bool | None]) -> tuple[str, str]:
    required = ("desktop_values_snapshot", "settings_persisted_after_wm_restart", "desktop_visual_consumer",
                "snap_preview_consumer")
    failed = [key for key in required if observations.get(key) is False]
    if failed:
        return STATUS_FAIL, "measured Desktop settings failure: " + ", ".join(failed)
    missing = [key for key in required if observations.get(key) is not True]
    if missing:
        return STATUS_NOT_RUN, "partial Desktop settings evidence; unverified: " + ", ".join(missing)
    return STATUS_PASS, "Desktop settings, persistence and visual consumer verified"


def assess_ss07(observations: dict[str, bool | None]) -> tuple[str, str]:
    required = ("settings_snapshot_values", "panel_edge_and_size_snapshot", "taskbar_visual_consumer",
                "settings_persistence_after_restart")
    failed = [key for key in required if observations.get(key) is False]
    if failed:
        return STATUS_FAIL, "measured Taskbar settings failure: " + ", ".join(failed)
    missing = [key for key in required if observations.get(key) is not True]
    if missing:
        return STATUS_NOT_RUN, "partial Taskbar evidence; unverified: " + ", ".join(missing)
    return STATUS_PASS, "Taskbar values, panel geometry, visual consumer and persistence verified"


def assess_ss10(observations: dict[str, bool | None]) -> tuple[str, str]:
    required = ("scale_changed_snapshot", "mode_list_unchanged", "scale_persisted_after_restart")
    failed = [key for key in required if observations.get(key) is False]
    if failed:
        return STATUS_FAIL, "measured display scale failure: " + ", ".join(failed)
    missing = [key for key in required if observations.get(key) is not True]
    if missing:
        return STATUS_NOT_RUN, "partial display scale evidence; unverified: " + ", ".join(missing)
    return STATUS_PASS, "Displays click set the output scale to 125 with the mode list unchanged and the value persisted after an owned WM restart"


def assess_ss11(observations: dict[str, bool | None]) -> tuple[str, str]:
    required = ("family_changed_snapshot", "size_changed_snapshot", "fonts_persisted_after_restart")
    failed = [key for key in required if observations.get(key) is False]
    if failed:
        return STATUS_FAIL, "measured font settings failure: " + ", ".join(failed)
    missing = [key for key in required if observations.get(key) is not True]
    if missing:
        return STATUS_NOT_RUN, "partial font evidence; unverified: " + ", ".join(missing)
    return STATUS_PASS, "Fonts family and size clicks changed the authoritative snapshot and both persisted after an owned WM restart"


def assess_ss13(observations: dict[str, bool | None]) -> tuple[str, str]:
    required = ("about_render_760x500", "about_render_820x540", "about_render_960x640",
                "version_matches_flamewm_version", "version_live_ocr_matches")
    failed = [key for key in required if observations.get(key) is False]
    if failed:
        return STATUS_FAIL, "measured About page failure: " + ", ".join(failed)
    missing = [key for key in required if observations.get(key) is not True]
    if missing:
        return STATUS_NOT_RUN, "partial About page evidence; unverified: " + ", ".join(missing)
    return STATUS_PASS, "About rendered non-blank at 760x500, 820x540 and 960x640, the live about-version text was OCR-read as 'Version FLAMEWM_VERSION' at all three sizes (negative controls rejected) and the source test about_version_text_is_exact_flamewm_version passed"


def gvariant_field_string(output: str, key: str) -> str | None:
    """Read one string field from gdbus's printed a{sv} snapshot without eval."""
    match = re.search(r"['\"]" + re.escape(key) + r"['\"]\s*:\s*<\s*['\"]([^'\"]*)['\"]\s*>", output)
    return match.group(1) if match else None


def gvariant_field_u64(output: str, key: str) -> int | None:
    match = re.search(r"['\"]" + re.escape(key) + r"['\"]\s*:\s*<\s*(?:uint64\s+)?(\d+)\s*>", output)
    return int(match.group(1)) if match else None


def gvariant_field_uint(output: str, key: str) -> int | None:
    match = re.search(r"['\"]" + re.escape(key) + r"['\"]\s*:\s*<\s*(?:u?int\d+\s+)?(\d+)\s*>", output)
    return int(match.group(1)) if match else None


def gvariant_field_int(output: str, key: str) -> int | None:
    match = re.search(r"['\"]" + re.escape(key) + r"['\"]\s*:\s*<\s*(?:u?int\d+\s+)?(-?\d+)\s*>", output)
    return int(match.group(1)) if match else None


def gvariant_field_bool(output: str, key: str) -> bool | None:
    match = re.search(r"['\"]" + re.escape(key) + r"['\"]\s*:\s*<\s*(true|false)\s*>", output)
    return match.group(1) == "true" if match else None


def settings_signal_observations(log: str) -> dict[str, bool]:
    return {
        "apply_call": "member=Apply" in log,
        "settings_changed": "member=SettingsChanged" in log,
        "snapshot_changed": "member=SettingsSnapshotChanged" in log,
    }


def settings_bus_events(log: str) -> list[dict[str, str | None]]:
    events = []
    for line in log.splitlines():
        for member in ("Apply", "SettingsChanged", "SettingsSnapshotChanged"):
            if f"member={member}" not in line:
                continue
            sender = re.search(r"sender=([^ ]+)", line)
            events.append({"member": member, "sender": sender.group(1) if sender else None,
                           "kind": "signal" if member != "Apply" else "method_call"})
            break
    return events


def accent_rgb(value: str) -> tuple[int, int, int] | None:
    if not re.fullmatch(r"#[0-9a-fA-F]{6}", value):
        return None
    return tuple(int(value[index:index + 2], 16) for index in (1, 3, 5))


def parse_pixel_rgb(output: str) -> tuple[int, int, int] | None:
    values = re.findall(r"\d+", output)
    if len(values) < 3:
        return None
    rgb = tuple(int(value) for value in values[:3])
    return rgb if all(0 <= value <= 255 for value in rgb) else None


def image_pixel(path: Path, x: int, y: int) -> tuple[int, int, int] | None:
    if Image is not None:
        try:
            with Image.open(path) as shot:
                if not (0 <= x < shot.width and 0 <= y < shot.height):
                    return None
                return tuple(shot.convert("RGB").getpixel((x, y)))
        except OSError:
            return None
    if shutil.which("identify") is None:
        return None
    result = subprocess.run(["identify", "-format", f"%[pixel:p{{{x},{y}}}]", str(path)],
                            capture_output=True, text=True, timeout=20, check=False)
    if result.returncode:
        return None
    return parse_pixel_rgb(result.stdout)


def stop_dbus_monitor(ctx: Ctx, monitor: Owned | None) -> None:
    """Ask the owned monitor to flush its observed calls before ordinary teardown."""
    if monitor is None or monitor.proc.poll() is not None:
        return
    monitor.proc.send_signal(2)  # SIGINT lets dbus-monitor finish its output stream.
    try:
        monitor.proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        ctx.stop(monitor)


def send_input(ctx: Ctx, click: str | None, key: str | None, chord: str | None = None,
               move: str | None = None, wheel: str | None = None, drag: str | None = None,
               drag_keep: bool = False, release: bool = False) -> tuple[int, str]:
    """Delegate XTEST input to tools/flamewm_xtest.py; token is redacted in the recorded command."""
    script = ROOT / "tools" / "flamewm_xtest.py"
    base = [sys.executable, str(script), "--display", ctx.display,
            "--owner-token", ctx.token, "--owner-marker", str(ctx.marker)]
    recorded = [part if part != ctx.token else "<owner-token>" for part in base]
    if click:
        base += ["--click", click]
        recorded += ["--click", click]
    if key:
        base += ["--key", key]
        recorded += ["--key", key]
    if chord:
        base += ["--chord", chord]
        recorded += ["--chord", chord]
    if move:
        base += ["--move", move]
        recorded += ["--move", move]
    if wheel:
        base += ["--wheel", wheel]
        recorded += ["--wheel", wheel]
    if drag:
        base += ["--drag", drag]
        recorded += ["--drag", drag]
        if drag_keep:
            base.append("--drag-keep")
            recorded.append("--drag-keep")
    if release:
        base.append("--release")
        recorded.append("--release")
    result = subprocess.run(base, env=ctx.host_env, capture_output=True, text=True, timeout=60, check=False)
    return result.returncode, shell(recorded)


def xtest_control(ctx: Ctx, flag: str) -> tuple[subprocess.CompletedProcess, str]:
    """Run a read-only or modifier-release flamewm_xtest.py action on the owned display; token is redacted."""
    script = ROOT / "tools" / "flamewm_xtest.py"
    argv = [sys.executable, str(script), "--display", ctx.display,
            "--owner-token", ctx.token, "--owner-marker", str(ctx.marker), flag]
    recorded = [part if part != ctx.token else "<owner-token>" for part in argv]
    result = subprocess.run(argv, env=ctx.host_env, capture_output=True, text=True, timeout=60, check=False)
    return result, shell(recorded)


def query_modifiers(ctx: Ctx) -> tuple[str | None, list[str] | None, str]:
    """Return (keymap hex snapshot, held modifier keysyms, command); snapshot and held are None when unreadable."""
    result, command = xtest_control(ctx, "--query-modifiers")
    match = re.search(r"KEYMAP_SNAPSHOT: ([0-9a-f]{64}) HELD_MODIFIERS: (\S+)", result.stdout)
    if result.returncode or not match:
        return None, None, f"{command}  # exit {result.returncode}: {result.stderr.strip()[:300]}"
    held = [] if match.group(2) == "none" else match.group(2).split(",")
    return match.group(1), held, command


def keyboard_preflight(ctx: Ctx, details: dict) -> str | None:
    """Ensure no modifier is logically stuck down before a keyboard-driven scenario types anything.

    Returns an environment NOT_RUN reason when the keymap is unreadable or a modifier stays down after one
    release attempt; otherwise None. Held modifiers that were released are recorded (never hidden) as
    released_leaked_modifiers while the verdict logic stays unchanged.
    """
    snapshot, held, command = query_modifiers(ctx)
    details["commands"].append(command)
    details["keymap_snapshot_before"] = snapshot
    if held is None:
        return f"environment: keymap unreadable on {ctx.display}"
    details["held_modifiers_before"] = held
    if not held:
        return None
    _, release_command = xtest_control(ctx, "--release-modifiers")
    details["commands"].append(release_command)
    _, after, requery_command = query_modifiers(ctx)
    details["commands"].append(requery_command)
    details["held_modifiers_after_release"] = after
    if after is None:
        return f"environment: keymap unreadable after modifier release on {ctx.display}"
    if after:
        return f"environment: stuck modifier {', '.join(after)} on {ctx.display}"
    details["released_leaked_modifiers"] = held
    return None


def dbus_shortcuts_snapshot(ctx: Ctx) -> tuple[str | None, str]:
    return dbus_get_snapshot(ctx, "Shortcuts")


def dbus_settings_snapshot(ctx: Ctx) -> tuple[str | None, str]:
    return dbus_get_snapshot(ctx, "Settings")


def dbus_apply_accent(ctx: Ctx, revision: int, accent: str) -> tuple[str | None, str, int | None]:
    return dbus_apply_settings(ctx, revision, {"accent": "'" + accent + "'"})


def dbus_apply_settings(ctx: Ctx, revision: int, changes: dict[str, str]) -> tuple[str | None, str, int | None]:
    change = "{" + ", ".join("'" + key + "': <" + value + ">" for key, value in changes.items()) + "}"
    argv = ["gdbus", "call", "--session", "--dest", "com.arkflame.FlameWM1",
            "--object-path", "/com/arkflame/FlameWM1", "--method",
            "com.arkflame.FlameWM1.Settings.Apply", f"uint64 {revision}", change]
    try:
        result = subprocess.run(argv, env=ctx.app_env, capture_output=True, text=True,
                                timeout=10, check=False)
    except (OSError, subprocess.SubprocessError) as error:
        return str(error), shell(argv), None
    return (result.stdout if result.returncode == 0 else result.stderr.strip(), shell(argv), result.returncode)


def dbus_panels_snapshot(ctx: Ctx) -> tuple[str | None, str]:
    return dbus_get_snapshot(ctx, "Panels")


def top_level_blocks(body: str) -> list[str]:
    """Split a gdbus dictionary array body into its top-level {...} blocks, skipping quoted text."""
    blocks = []
    depth = 0
    start = None
    quoted = False
    escaped = False
    for index, char in enumerate(body):
        if quoted:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == "'":
                quoted = False
            continue
        if char == "'":
            quoted = True
        elif char == "{":
            if depth == 0:
                start = index
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0 and start is not None:
                blocks.append(body[start:index + 1])
                start = None
    return blocks


def parse_panels_snapshot(output: str) -> dict | None:
    revision = gvariant_field_u64(output, "revision")
    match = re.search(r"['\"]panels['\"]\s*:\s*<\[(.*?)\]>", output)
    if revision is None or match is None:
        return None
    panels = []
    for block in top_level_blocks(match.group(1)):
        output_key = gvariant_field_string(block, "output")
        edge = gvariant_field_string(block, "edge")
        size = gvariant_field_uint(block, "size")
        if output_key is None or edge is None or size is None:
            return None
        panels.append({"output": output_key, "edge": edge, "size": size,
                       "visible": gvariant_field_bool(block, "visible"),
                       "x": gvariant_field_uint(block, "x"),
                       "y": gvariant_field_uint(block, "y"),
                       "width": gvariant_field_uint(block, "width"),
                       "height": gvariant_field_uint(block, "height")})
    return {"revision": revision, "panels": panels}


def display_output_scales(snapshot: str) -> dict[str, int] | None:
    """Map each authoritative Displays output id to its scale percent; None when any output is malformed."""
    outputs = re.search(r"['\"]outputs['\"]\s*:\s*<\[(.*)\]>\s*[,}]", snapshot, re.DOTALL)
    if outputs is None:
        return None
    scales: dict[str, int] = {}
    for block in top_level_blocks(outputs.group(1)):
        output_id = gvariant_field_string(block, "id")
        scale = gvariant_field_uint(block, "scale")
        if output_id is None or scale is None:
            return None
        scales[output_id] = scale
    return scales or None


def display_output_mode_count(snapshot: str, output_id: str) -> int | None:
    """Number of selectable modes the snapshot lists for one output; None when unparsable."""
    outputs = re.search(r"['\"]outputs['\"]\s*:\s*<\[(.*)\]>\s*[,}]", snapshot, re.DOTALL)
    if outputs is None:
        return None
    for block in top_level_blocks(outputs.group(1)):
        if gvariant_field_string(block, "id") != output_id:
            continue
        modes = re.search(r"['\"]modes['\"]\s*:\s*<\[(.*)\]>", block, re.DOTALL)
        return None if modes is None else len(top_level_blocks(modes.group(1)))
    return None


def ss10_scale_applied(exit_code: int, scales: dict[str, int] | None, output_id: str) -> bool:
    """A click counts only when XTEST succeeded and the authoritative output reports the target scale."""
    return exit_code == 0 and scales is not None and scales.get(output_id) == SS10_TARGET_SCALE


def ss11_family_applied(exit_code: int, before: str, after: str | None) -> bool:
    return exit_code == 0 and after is not None and after != before


def ss11_size_applied(exit_code: int, before_size: int, after_size: int | None,
                      family_before: str | None, family_after: str | None) -> bool:
    """The + control steps the offset by exactly one and leaves the family untouched."""
    return (exit_code == 0 and after_size == before_size + 1
            and family_before is not None and family_after == family_before)


def ss11_fonts_persisted(persisted_text: str, family: str | None, size: int | None) -> bool:
    return (family is not None and size is not None
            and gvariant_field_string(persisted_text, "fontFamily") == family
            and gvariant_field_int(persisted_text, "fontSizeOffset") == size)


def ss12_reset_restored(exit_code: int, binding: str | None) -> bool:
    return exit_code == 0 and binding == SHORTCUT_DEFAULT_BINDING


ABOUT_VERSION_TEST = "about_version_text_is_exact_flamewm_version"
ABOUT_VERSION_PASSED = "test result: ok. 1 passed"


def ss13_version_test_observed(exit_code: int, output: str) -> bool:
    """Exact pass of the single About-version Rust test; any other cargo outcome is a measured False."""
    return exit_code == 0 and ABOUT_VERSION_PASSED in output


def run_about_version_test() -> tuple[bool | None, dict]:
    """Run the Rust test that pins About's version text to FLAMEWM_VERSION; None when cargo is not on PATH."""
    argv = ["cargo", "test", "-p", "flamewm-settings", ABOUT_VERSION_TEST]
    record: dict = {"command": shell(argv)}
    if shutil.which("cargo") is None:
        record["reason"] = "cargo not on PATH; About version test not run"
        return None, record
    env = {**os.environ, "CARGO_BUILD_JOBS": "2"}
    try:
        result = subprocess.run(argv, cwd=ROOT, env=env, capture_output=True, text=True, timeout=1800, check=False)
    except subprocess.TimeoutExpired:
        record["reason"] = "cargo test timed out"
        return None, record
    output = result.stdout + result.stderr
    record["exit_code"] = result.returncode
    record["result_line"] = ABOUT_VERSION_PASSED if ABOUT_VERSION_PASSED in output else None
    return ss13_version_test_observed(result.returncode, output), record


def native_window_titles(ctx: Ctx) -> tuple[set[str] | None, str]:
    argv = ["xwininfo", "-display", ctx.display, "-root", "-children"]
    result = subprocess.run(argv, env=ctx.host_env, capture_output=True, text=True,
                            timeout=20, check=False)
    if result.returncode:
        return None, result.stderr.strip() or f"xwininfo exited {result.returncode}"
    return {name for name, *_ in WINDOW_RE.findall(result.stdout)}, shell(argv)


def active_window_title(ctx: Ctx) -> tuple[str | None, str]:
    root = subprocess.run(["xprop", "-display", ctx.display, "-root", "_NET_ACTIVE_WINDOW"],
                          env=ctx.host_env, capture_output=True, text=True, timeout=15, check=False)
    match = re.search(r"_NET_ACTIVE_WINDOW\(WINDOW\): window id # (0x[0-9a-fA-F]+)", root.stdout)
    if root.returncode or not match:
        return None, "xprop could not identify _NET_ACTIVE_WINDOW"
    info = subprocess.run(["xwininfo", "-display", ctx.display, "-id", match.group(1)],
                          env=ctx.host_env, capture_output=True, text=True, timeout=15, check=False)
    # xwininfo prints the name on its first line: `xwininfo: Window id: 0x... "Title"`.
    name = re.search(r'Window id: 0x[0-9a-fA-F]+ "([^"]*)"', info.stdout)
    return (name.group(1) if info.returncode == 0 and name else None), "xprop _NET_ACTIVE_WINDOW + xwininfo -id"


def start_toggle_count(ctx: Ctx, name: str, action, settle: float) -> tuple[int | None, object]:
    """Run action() under an owned dbus-monitor and count StartMenuToggleRequested signals.

    The Xephyr session has no shell process, so the Start popup has no X window; the WM's
    StartMenuToggleRequested signal is the observable. None means the monitor was unavailable.
    """
    log = ctx.run / "logs" / f"{name}.log"
    monitor = ctx.spawn(name, ["dbus-monitor", "--session", START_TOGGLE_MATCH], ctx.app_env)
    time.sleep(1.0)
    if monitor.proc.poll() is not None:
        return None, action()
    result = action()
    time.sleep(settle)
    stop_dbus_monitor(ctx, monitor)
    if not log.exists():
        return None, result
    return log.read_text(encoding="utf-8", errors="replace").count("member=StartMenuToggleRequested"), result


# SS16 static gates (scripts/check-equivalent, no cargo): each must exit 0 on the same source.
STATIC_GATES: tuple[tuple[str, tuple[str, ...]], ...] = (
    ("architecture_guard", ("tools/architecture_guard.py",)),
    ("verify_rwr_promotion", ("tools/verify_rwr_promotion.py",)),
    ("j18_frame_guard", ("tools/test_j18_frame_guard.py",)),
)


def assess_ss16(exit_codes: dict[str, int | None]) -> tuple[str, str]:
    """PASS only when every static gate exited 0; any nonzero exit FAILs, a missing exit is NOT_RUN."""
    failed = [f"{name}={code}" for name, code in exit_codes.items() if code not in (0, None)]
    if failed:
        return STATUS_FAIL, "static gate failed: " + ", ".join(failed)
    unrun = [name for name, code in exit_codes.items() if code is None]
    if unrun or not exit_codes:
        return STATUS_NOT_RUN, "static gate did not run: " + ", ".join(unrun)
    return STATUS_PASS, "static gates exited 0 on one source: " + ", ".join(exit_codes) \
        + " (cargo check/test/build are not part of this runner)"


def run_ss16() -> dict:
    snapshot = git_snapshot()
    codes: dict[str, int | None] = {}
    commands = []
    for name, argv in STATIC_GATES:
        command = [sys.executable, *argv]
        commands.append(shell(["python3", *argv]))
        try:
            codes[name] = subprocess.run(command, cwd=ROOT, capture_output=True, text=True,
                                         timeout=900, check=False).returncode
        except (OSError, subprocess.SubprocessError):
            codes[name] = None
    status, reason = assess_ss16(codes)
    if status == STATUS_PASS and git_snapshot() != snapshot:
        status, reason = STATUS_NOT_RUN, "source changed while the static gates ran"
    entry = result_entry("SS16", status, reason, commands=commands)
    entry["details"] = {"exit_codes": codes}
    return entry


def map_ss18(path: str | None) -> dict:
    """Map an installed-result.json into SS18; PASS only when installed behavioral scenarios were executed."""
    if not path:
        return result_entry("SS18", STATUS_NOT_RUN,
                            "no --ss18-installed-result; run scripts/settings-installed-acceptance on a staged prefix")
    try:
        document = json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        return result_entry("SS18", STATUS_NOT_RUN, f"installed result unreadable: {error}")
    if document.get("schema") != "flamewm-settings-installed-acceptance-v1":
        return result_entry("SS18", STATUS_NOT_RUN, "installed result has an unknown schema")
    behavioral = document.get("behavioral_scenarios")
    if not isinstance(behavioral, dict) or not behavioral:
        return result_entry("SS18", STATUS_NOT_RUN, "installed result carries no executed behavioral scenarios (launch smoke only)")
    if document.get("ss18_status") == STATUS_FAIL or STATUS_FAIL in behavioral.values():
        status, reason = STATUS_FAIL, "installed gate failed: " + document.get("ss18_reason", "")
    elif document.get("ss18_status") != STATUS_PASS or any(value != STATUS_PASS for value in behavioral.values()):
        status, reason = STATUS_NOT_RUN, "installed behavioral proof incomplete: " + document.get("ss18_reason", "")
    else:
        status, reason = STATUS_PASS, "installed staged binaries passed behavioral scenarios " + ", ".join(behavioral)
    entry = result_entry("SS18", status, reason, evidence=[str(path)], commands=[shell(document.get("command", []))])
    entry["details"] = {"behavioral_scenarios": behavioral, "ss18_status": document.get("ss18_status"), "prefix": document.get("prefix")}
    return entry


def run_ss03(ctx: Ctx, args: argparse.Namespace) -> dict:
    """Exercise palette clicks, reset, selected visuals and private-state restart."""
    # Palette order is red, blue, purple, teal, then the colour-picker wheel (ui/settings.html);
    # the wheel moved to the fifth slot, so the old 685/762 points hit purple and the wheel.
    # Accent row order (ui/settings.html, Theme page): red 610, blue 648, purple 686, teal 724, then the wheel.
    # The Accent preset Reset was removed (settings.accent.reset is rejected), so the last palette choice
    # is the value that must persist across a WM restart.
    choices = (("blue", "#3DAEE9", (648, 151)), ("teal", "#1ABC9C", (724, 151)))
    observations: dict[str, bool | None] = {
        "blue_snapshot": None, "teal_snapshot": None,
        "selected_visuals": None, "reset_persisted_after_restart": None,
    }
    details: dict = {"observations": observations, "actions": [], "commands": [],
                     "private_xdg": {name: str(ctx.run / name) for name in
                                     ("xdg-config", "xdg-data", "xdg-cache")}}
    def finish() -> dict:
        status, reason = assess_ss03(observations)
        entry = result_entry("SS03", status, reason, details.get("evidence", []), details["commands"])
        entry["details"] = details
        return entry

    if shutil.which("gdbus") is None or Image is None and shutil.which("identify") is None:
        details["reason"] = "authoritative D-Bus snapshot or screenshot pixel reader unavailable"
        return finish()
    settings = ctx.spawn("settings-ss03-appearance", [args.settings_bin, "--page", "appearance"], ctx.app_env)
    details["commands"].append(shell([args.settings_bin, "--page", "appearance"]))
    time.sleep(args.settle_seconds)
    if settings.proc.poll() is not None:
        observations["blue_snapshot"] = False
        details["reason"] = "Settings exited before accent interaction"
        return finish()
    geometry_path = ctx.run / "logs" / "ss03-window-tree.txt"
    geometry = wait_for_settings_window(ctx, geometry_path)
    details["window"] = geometry
    if geometry is None or (geometry["width"], geometry["height"]) != (820, 540):
        details["reason"] = "could not measure the expected 820x540 Settings client"
        ctx.stop(settings)
        return finish()
    before_text, before_command = dbus_settings_snapshot(ctx)
    details["commands"].append(before_command)
    before_revision = gvariant_field_u64(before_text or "", "revision")
    before_accent = gvariant_field_string(before_text or "", "accent")
    details["before_snapshot"] = {"revision": before_revision, "accent": before_accent}
    if before_revision is None or before_accent is None:
        details["reason"] = "initial authoritative Settings snapshot is missing accent or revision"
        ctx.stop(settings)
        return finish()

    visuals_ok = True
    previous_revision = before_revision
    for name, accent, point in choices:
        click = settings_relative_point(ctx, geometry, point)
        code, command = send_input(ctx, click, None)
        details["commands"].append(command)
        details["actions"].append({"swatch": name, "coordinate": click, "exit": code})
        time.sleep(min(max(args.settle_seconds, 0.25), 1.0))
        snapshot, query = dbus_settings_snapshot(ctx)
        details["commands"].append(query)
        revision = gvariant_field_u64(snapshot or "", "revision")
        value = gvariant_field_string(snapshot or "", "accent")
        expected = accent
        observed = (revision is not None and revision > previous_revision
                    and value is not None and value.lower() == expected.lower())
        observations[f"{name}_snapshot"] = (code == 0 and observed) if snapshot is not None else None
        inspected = inspect_accent_after_click(ctx, ctx.run, name, geometry, point, expected, code)
        move_code = inspected["move_exit"]
        visual = inspected["visual"]
        details["commands"].append(inspected["move_command"])
        details.setdefault("visuals", {})[name] = visual
        details.setdefault("evidence", []).append(visual["shot"])
        visuals_ok = visuals_ok and inspected["passed"]
        details.setdefault("snapshots", []).append({"name": name, "revision": revision, "accent": value,
                                                     "expected": expected, "match": observed})
        if revision is not None:
            previous_revision = revision
        details["actions"][-1]["pointer_move_exit"] = move_code
    if observations["blue_snapshot"] is not True or observations["teal_snapshot"] is not True:
        details["navigation_click_diagnostic"] = ss03_nav_click_diagnostic(ctx, geometry, ctx.run)
        details.setdefault("evidence", []).extend(str(ctx.run / "shots" / f"ss03-nav-diagnostic-{page}.png")
            for page in ("appearance_before", "desktop", "appearance_after"))
    ctx.stop(settings)
    observations["selected_visuals"] = visuals_ok
    final_name, final_accent, final_point = choices[-1]
    teal_persisted = observations["teal_snapshot"] is True

    if teal_persisted and restart_owned_wm(ctx, args, "wm-ss03-restart"):
        persisted_text, persisted_query = dbus_settings_snapshot(ctx)
        details["commands"].extend(["restart owned WM", persisted_query])
        persisted_value = gvariant_field_string(persisted_text or "", "accent")
        details["persisted_after_restart"] = {"accent": persisted_value, "snapshot_read": persisted_text is not None}
        observations["reset_persisted_after_restart"] = (
            persisted_value is not None and persisted_value.lower() == final_accent.lower()
        ) if persisted_text is not None else None
        post = ctx.spawn("settings-ss03-after-restart", [args.settings_bin, "--page", "appearance"], ctx.app_env)
        time.sleep(min(max(args.settle_seconds, 0.25), 1.0))
        post_geometry = window_geometry(ctx, ctx.run / "logs" / "ss03-post-restart-tree.txt")
        if post.proc.poll() is None and post_geometry:
            post_visual = capture_accent_visual(ctx, ctx.run, "after-restart", post_geometry,
                                                final_point, final_accent)
            details["visuals"]["after_restart"] = post_visual
            details["evidence"].append(post_visual["shot"])
            if observations["reset_persisted_after_restart"] is True:
                observations["reset_persisted_after_restart"] = post_visual["ok"]
        ctx.stop(post)
    elif teal_persisted:
        observations["reset_persisted_after_restart"] = None
        details["reason"] = "owned WM could not be restarted to verify private-state persistence"
    else:
        observations["reset_persisted_after_restart"] = None
    return finish()


def run_ss12(ctx: Ctx, args: argparse.Namespace) -> dict:
    """Drive the real Settings capture row; verify binding, Start replay, owner loss, Escape clear and Reset.

    Start behaviour is the WM's StartMenuToggleRequested signal (the session has no shell process, so
    no Start window exists). Clear and Reset are each checked in the authoritative shortcut snapshot
    and again after an owned WM restart; after Clear the old chord must not fire Start and after
    Reset the platform default must. Click points default to measured fixture constants that are
    used only when the live Settings client is exactly SS12_CLIENT.
    """
    observations: dict[str, bool | None] = {
        "lease_begin_end_same_owner": None, "binding_stored": None,
        "action_effect": None, "grab_restored_start_behavior": None,
        "owner_loss_restored": None, "clear_reset_persisted": None,
    }
    details: dict = {"action": "ToggleStartMenu", "requested_chord": args.ss12_chord,
                     "start_observable": "StartMenuToggleRequested signal (no Start window exists)",
                     "observations": observations, "commands": []}
    settle = min(max(args.settle_seconds, 0.25), 1.0)

    def finish() -> dict:
        status, reason = assess_ss12(observations)
        entry = result_entry("SS12", status, reason, commands=details["commands"])
        entry["details"] = details
        return entry

    def not_run(reason: str) -> dict:
        details["reason"] = reason
        return result_entry("SS12", STATUS_NOT_RUN, reason, commands=details["commands"])

    if ctx.display not in (f":{n}" for n in XTEST_DISPLAY_RANGE):
        return not_run("SS12 XTEST requires an owned display in :97..:119")
    if shutil.which("gdbus") is None or shutil.which("dbus-monitor") is None:
        return not_run("SS12 lease and authoritative snapshot observation tools unavailable")
    preflight = keyboard_preflight(ctx, details)
    if preflight:
        details["reason"] = preflight
        entry = result_entry("SS12", STATUS_NOT_RUN, preflight, commands=details["commands"])
        entry["details"] = details
        return entry

    before_output, before_command = dbus_shortcuts_snapshot(ctx)
    details["commands"].append(before_command)
    before_binding = gvariant_field_string(before_output or "", "ToggleStartMenu")
    details["before_binding"] = before_binding
    if before_output is None or before_binding is None:
        return not_run("cannot read authoritative ToggleStartMenu binding before capture")

    monitor_path = ctx.run / "logs" / "ss12-dbus-monitor.log"
    monitor_argv = ["dbus-monitor", "--session",
                    "type='method_call',interface='com.arkflame.FlameWM1.Shortcuts',member='BeginCapture'",
                    "type='method_call',interface='com.arkflame.FlameWM1.Shortcuts',member='EndCapture'"]
    monitor = ctx.spawn("ss12-dbus-monitor", monitor_argv, ctx.app_env)
    settings = ctx.spawn("settings-ss12-hotkeys", [args.settings_bin, "--page", "hotkeys"], ctx.app_env)
    details["commands"].append(shell([args.settings_bin, "--page", "hotkeys"]))
    time.sleep(args.settle_seconds)
    if settings.proc.poll() is not None:
        stop_dbus_monitor(ctx, monitor)
        details["reason"] = "Settings exited before SS12 capture interaction"
        return result_entry("SS12", STATUS_FAIL, details["reason"], commands=details["commands"])

    geometry = wait_for_settings_window(ctx, ctx.run / "logs" / "ss12-window-tree.txt")
    details["window"] = geometry
    capture_click, reset_click = args.ss12_click, args.ss12_reset_click
    if capture_click is None or reset_click is None:
        if geometry is None or (geometry["width"], geometry["height"]) != SS12_CLIENT:
            ctx.stop(settings); stop_dbus_monitor(ctx, monitor)
            return not_run(f"Settings client is not the measured {SS12_CLIENT[0]}x{SS12_CLIENT[1]} fixture "
                           f"({geometry and (geometry['width'], geometry['height'])}); default clicks refused")
        if capture_click is None:
            capture_click = settings_relative_point(ctx, geometry, SS12_CAPTURE_CLIENT)
        if reset_click is None:
            reset_click = settings_relative_point(ctx, geometry, SS12_RESET_CLIENT)
    details["capture_click"], details["reset_click"] = capture_click, reset_click

    code, click_cmd = send_input(ctx, capture_click, None)
    details["commands"].append(click_cmd)
    if code:
        ctx.stop(settings); stop_dbus_monitor(ctx, monitor)
        details["reason"] = f"capture-row click input failed with exit {code}"
        return result_entry("SS12", STATUS_FAIL, details["reason"], commands=details["commands"])
    time.sleep(args.settle_seconds)
    focused_title, focus_command = active_window_title(ctx)
    details["commands"].append(focus_command)
    details["focused_window"] = focused_title
    if focused_title is None or "settings" not in focused_title.lower():
        ctx.stop(settings); stop_dbus_monitor(ctx, monitor)
        return not_run("Settings focus was not confirmed after capture-row click")
    # The chord is pressed while the capture lease is held: Start must not fire.
    during_count, (code, chord_cmd) = start_toggle_count(
        ctx, "ss12-start-during-capture", lambda: send_input(ctx, None, None, args.ss12_chord), args.settle_seconds)
    details["commands"].append(chord_cmd)
    details["start_signals_during_capture"] = during_count
    if code:
        ctx.stop(settings); stop_dbus_monitor(ctx, monitor)
        details["reason"] = f"capture chord input failed with exit {code}"
        return result_entry("SS12", STATUS_FAIL, details["reason"], commands=details["commands"])
    after_output, after_command = dbus_shortcuts_snapshot(ctx)
    details["commands"].append(after_command)
    stored = gvariant_field_string(after_output or "", "ToggleStartMenu")
    details["stored_binding"] = stored
    observations["binding_stored"] = stored == args.ss12_chord if after_output is not None else None
    stop_dbus_monitor(ctx, monitor)
    monitor_text = monitor_path.read_text(encoding="utf-8", errors="replace") if monitor_path.exists() else ""
    calls = capture_lease_calls(monitor_text)
    details["lease_calls"] = calls
    begin = next((sender for name, sender in calls if name == "BeginCapture"), None)
    end = next((sender for name, sender in calls if name == "EndCapture"), None)
    observations["lease_begin_end_same_owner"] = bool(begin and end and begin == end)
    observations["action_effect"] = None if during_count is None else during_count == 0

    # After the lease ended the committed chord must drive Start.
    replay_count, (code, replay_cmd) = start_toggle_count(
        ctx, "ss12-start-after-commit", lambda: send_input(ctx, None, None, args.ss12_chord), args.settle_seconds)
    details["commands"].append(replay_cmd)
    details["start_signals_after_commit"] = replay_count
    observations["grab_restored_start_behavior"] = None if replay_count is None else bool(code == 0 and replay_count >= 1)

    # Owner loss: Settings exits mid-capture; the persisted chord must still drive Start.
    owner_monitor_path = ctx.run / "logs" / "ss12-owner-loss-monitor.log"
    owner_monitor = ctx.spawn("ss12-owner-loss-monitor", monitor_argv, ctx.app_env)
    owner_settings = ctx.spawn("settings-ss12-owner-loss", [args.settings_bin, "--page", "hotkeys"], ctx.app_env)
    time.sleep(args.settle_seconds)
    owner_click_code, owner_click_cmd = send_input(ctx, capture_click, None)
    details["commands"].extend([shell([args.settings_bin, "--page", "hotkeys"]), owner_click_cmd])
    time.sleep(args.settle_seconds)
    ctx.stop(owner_settings)
    time.sleep(args.settle_seconds)
    stop_dbus_monitor(ctx, owner_monitor)
    owner_log = owner_monitor_path.read_text(encoding="utf-8", errors="replace") if owner_monitor_path.exists() else ""
    owner_calls = capture_lease_calls(owner_log)
    details["owner_loss_begin_calls"] = owner_calls
    owner_begin = next((sender for name, sender in owner_calls if name == "BeginCapture"), None)
    owner_end = next((sender for name, sender in owner_calls if name == "EndCapture"), None)
    owner_count, (owner_replay_code, owner_replay_cmd) = start_toggle_count(
        ctx, "ss12-start-after-owner-loss", lambda: send_input(ctx, None, None, args.ss12_chord), args.settle_seconds)
    details["commands"].append(owner_replay_cmd)
    details["start_signals_after_owner_loss"] = owner_count
    details["owner_loss_case"] = "Settings exited during active capture; subsequent stored Start chord observed"
    observations["owner_loss_restored"] = None if owner_count is None else bool(
        owner_click_code == 0 and owner_begin and not owner_end and owner_replay_code == 0 and owner_count >= 1)

    # Escape clear through the real control, then persistence across an owned WM restart.
    clear_settings = ctx.spawn("settings-ss12-clear", [args.settings_bin, "--page", "hotkeys"], ctx.app_env)
    details["commands"].append(shell([args.settings_bin, "--page", "hotkeys"]))
    time.sleep(settle)
    clear_click, clear_command = send_input(ctx, capture_click, None)
    clear_escape, escape_command = send_input(ctx, None, "Escape")
    details["commands"].extend([clear_command, escape_command])
    time.sleep(settle)
    clear_text, clear_query = dbus_shortcuts_snapshot(ctx)
    details["commands"].append(clear_query)
    cleared = gvariant_field_string(clear_text or "", "ToggleStartMenu")
    details["clear_binding"] = cleared
    ctx.stop(clear_settings)
    clear_restart_ok = clear_text is not None and clear_click == 0 and clear_escape == 0 \
        and cleared == "" and restart_owned_wm(ctx, args, "wm-ss12-clear-restart")
    if clear_restart_ok:
        after_clear_text, clear_restart_query = dbus_shortcuts_snapshot(ctx)
        details["commands"].extend(["restart owned WM after Escape clear", clear_restart_query])
        after_clear = gvariant_field_string(after_clear_text or "", "ToggleStartMenu")
        details["binding_after_clear_restart"] = after_clear
        # The cleared binding must leave Start with no chord: the old chord fires nothing.
        cleared_count, (_, cleared_cmd) = start_toggle_count(
            ctx, "ss12-start-after-clear", lambda: send_input(ctx, None, None, args.ss12_chord), args.settle_seconds)
        details["commands"].append(cleared_cmd)
        details["start_signals_after_clear"] = cleared_count
        clear_restart_ok = after_clear_text is not None and after_clear == "" and cleared_count == 0
    details["clear_persisted"] = clear_restart_ok

    reset_settings = ctx.spawn("settings-ss12-reset", [args.settings_bin, "--page", "hotkeys"], ctx.app_env)
    details["commands"].append(shell([args.settings_bin, "--page", "hotkeys"]))
    time.sleep(settle)
    reset_code, reset_command = send_input(ctx, reset_click, None)
    details["commands"].append(reset_command)
    time.sleep(settle)
    reset_text, reset_query = dbus_shortcuts_snapshot(ctx)
    details["commands"].append(reset_query)
    reset_binding = gvariant_field_string(reset_text or "", "ToggleStartMenu")
    details["reset_binding"] = reset_binding
    reset_ok = ss12_reset_restored(reset_code, reset_binding) and restart_owned_wm(
        ctx, args, "wm-ss12-reset-restart")
    if reset_ok:
        after_reset_text, reset_restart_query = dbus_shortcuts_snapshot(ctx)
        details["commands"].extend(["restart owned WM after reset", reset_restart_query])
        after_reset = gvariant_field_string(after_reset_text or "", "ToggleStartMenu")
        details["binding_after_reset_restart"] = after_reset
        # After Reset the platform default (Super_L) must drive Start again.
        reset_count, (_, reset_start_cmd) = start_toggle_count(
            ctx, "ss12-start-after-reset", lambda: send_input(ctx, None, SHORTCUT_DEFAULT_BINDING), args.settle_seconds)
        details["commands"].append(reset_start_cmd)
        details["start_signals_after_reset"] = reset_count
        reset_ok = after_reset_text is not None and after_reset == SHORTCUT_DEFAULT_BINDING \
            and bool(reset_count and reset_count >= 1)
    ctx.stop(reset_settings)
    observations["clear_reset_persisted"] = bool(clear_restart_ok and reset_ok)
    details["clear_reset_persistence_case"] = {
        "escape_clear_and_restart": clear_restart_ok,
        "reset_to_default_start_and_restart": reset_ok,
        "initial_binding": before_binding,
    }
    ctx.stop(settings)
    return finish()


def display_mode_count(snapshot: str) -> int | None:
    """Count mode dictionaries in the authoritative Displays snapshot's outputs array."""
    outputs = re.search(r"['\"]outputs['\"]\s*:\s*<\[(.*)\]>\s*[,}]", snapshot, re.DOTALL)
    if outputs is None:
        return None
    body = outputs.group(1)
    first_start = body.find("{")
    if first_start < 0:
        return 0
    depth = 0
    quoted = False
    escaped = False
    first_end = None
    for index in range(first_start, len(body)):
        char = body[index]
        if quoted:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == "'":
                quoted = False
            continue
        if char == "'":
            quoted = True
        elif char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                first_end = index + 1
                break
    if first_end is None:
        return None
    modes = re.search(r"['\"]modes['\"]\s*:\s*<\[(.*?)\]>", body[first_start:first_end], re.DOTALL)
    if modes is None:
        return None
    mode_body = modes.group(1)
    count = depth = 0
    quoted = escaped = False
    for char in mode_body:
        if quoted:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == "'":
                quoted = False
            continue
        if char == "'":
            quoted = True
        elif char == "{":
            if depth == 0:
                count += 1
            depth += 1
        elif char == "}":
            depth -= 1
            if depth < 0:
                return None
    return count if depth == 0 and not quoted else None


SS15_RENDER_SETTLE = 0.4


def run_ss15(ctx: Ctx, args: argparse.Namespace) -> dict:
    """Exercise disabled pager endpoints and the actual hotkey Escape keyboard path."""
    observations: dict[str, bool | None] = {
        "single_output_boundary_observed": None, "disabled_output_prev_inert": None,
        "disabled_output_next_inert": None, "disabled_mode_ends_inert": None,
        "settings_survived_keyboard_escape": None,
    }
    details: dict = {"observations": observations, "commands": [], "actions": []}
    def finish() -> dict:
        status, reason = assess_ss15(observations)
        entry = result_entry("SS15", status, reason, details.get("evidence", []), details["commands"])
        entry["details"] = details
        return entry

    if ctx.display not in (f":{n}" for n in XTEST_DISPLAY_RANGE):
        details["reason"] = "SS15 XTEST requires an owned display in :97..:119"
        return finish()
    preflight = keyboard_preflight(ctx, details)
    if preflight:
        details["reason"] = preflight
        entry = result_entry("SS15", STATUS_NOT_RUN, preflight, commands=details["commands"])
        entry["details"] = details
        return entry
    displays = ctx.spawn("settings-ss15-displays", [args.settings_bin, "--page", "displays"], ctx.app_env)
    details["commands"].append(shell([args.settings_bin, "--page", "displays"]))
    time.sleep(min(max(args.settle_seconds, 0.25), 1.0))
    geometry = window_geometry(ctx, ctx.run / "logs" / "ss15-displays-window-tree.txt")
    before_text, before_query = dbus_displays_snapshot(ctx)
    details["commands"].append(before_query)
    output_count = gvariant_field_uint(before_text or "", "outputCount")
    generation = gvariant_field_u64(before_text or "", "generation")
    mode_count = display_mode_count(before_text or "") if before_text is not None else None
    details["display_snapshot"] = {"output_count": output_count, "generation": generation,
                                    "mode_count": mode_count, "raw": before_text}
    if displays.proc.poll() is not None or geometry is None or before_text is None or output_count is None:
        observations["single_output_boundary_observed"] = False if displays.proc.poll() is not None else None
        ctx.stop(displays)
        details["reason"] = "Displays window geometry or authoritative display snapshot unavailable"
        return finish()
    if output_count != 1:
        details["reason"] = f"native fixture has {output_count} outputs; disabled one-output pager boundary not exercised"
        ctx.stop(displays)
        return finish()
    observations["single_output_boundary_observed"] = True
    output_label_rect = (geometry["x"] + 465, geometry["y"] + 96, 140, 30)
    initial_output_shot = ctx.run / "shots" / "ss15-output-label-before.png"
    capture(ctx, initial_output_shot)
    output_label_before = image_region_digest(initial_output_shot, output_label_rect)
    for name, point in (("output_prev", (253, 110)), ("output_next", (774, 110))):
        coordinate = settings_relative_point(ctx, geometry, point)
        code, command = send_input(ctx, coordinate, None)
        details["commands"].append(command)
        after_text, query = dbus_displays_snapshot(ctx)
        details["commands"].append(query)
        label_shot = ctx.run / "shots" / f"ss15-{name}-label.png"
        capture(ctx, label_shot)
        after_label = image_region_digest(label_shot, output_label_rect)
        inert = (code == 0 and after_text == before_text and output_label_before is not None
                 and after_label == output_label_before)
        observations[f"disabled_{name}_inert"] = inert if after_text is not None and after_label is not None else None
        details.setdefault("evidence", []).extend([str(initial_output_shot), str(label_shot)])
        details["actions"].append({"action": name, "coordinate": coordinate,
                                   "exit": code, "snapshot_unchanged": after_text == before_text,
                                   "page_label_unchanged": after_label == output_label_before})
    if mode_count is None:
        observations["disabled_mode_ends_inert"] = None
        details["mode_note"] = "could not parse authoritative mode list"
    else:
        mode_label_rect = (geometry["x"] + 465, geometry["y"] + 482, 140, 30)
        mode_pages = (mode_count + 7) // 8
        label_path = ctx.run / "shots" / "ss15-mode-label-before.png"
        capture(ctx, label_path)
        label_hash = image_region_digest(label_path, mode_label_rect)
        mode_endpoint_ok = label_hash is not None
        # The first-page previous control is disabled for both empty and paged mode lists.
        prev_coordinate = settings_relative_point(ctx, geometry, (268, 497))
        code, command = send_input(ctx, prev_coordinate, None)
        details["commands"].append(command)
        time.sleep(SS15_RENDER_SETTLE)  # the page repaints asynchronously after the click
        after_text, query = dbus_displays_snapshot(ctx)
        details["commands"].append(query)
        prev_path = ctx.run / "shots" / "ss15-mode-prev-label.png"
        capture(ctx, prev_path)
        prev_hash = image_region_digest(prev_path, mode_label_rect)
        mode_endpoint_ok = mode_endpoint_ok and code == 0 and after_text == before_text and prev_hash == label_hash
        page = 0
        for next_page in range(1, mode_pages):
            coordinate = settings_relative_point(ctx, geometry, (760, 497))
            code, command = send_input(ctx, coordinate, None)
            details["commands"].append(command)
            time.sleep(SS15_RENDER_SETTLE)
            after_text, query = dbus_displays_snapshot(ctx)
            details["commands"].append(query)
            next_path = ctx.run / "shots" / f"ss15-mode-label-page-{next_page}.png"
            capture(ctx, next_path)
            next_hash = image_region_digest(next_path, mode_label_rect)
            progressed = code == 0 and after_text == before_text and next_hash is not None and next_hash != label_hash
            details["actions"].append({"action": "mode_next", "page": next_page,
                                       "exit": code, "label_changed": next_hash != label_hash,
                                       "snapshot_unchanged": after_text == before_text})
            mode_endpoint_ok = mode_endpoint_ok and progressed
            label_hash = next_hash
            page = next_page
        next_coordinate = settings_relative_point(ctx, geometry, (760, 497))
        code, command = send_input(ctx, next_coordinate, None)
        details["commands"].append(command)
        time.sleep(SS15_RENDER_SETTLE)
        after_text, query = dbus_displays_snapshot(ctx)
        details["commands"].append(query)
        end_path = ctx.run / "shots" / "ss15-mode-label-end.png"
        capture(ctx, end_path)
        end_hash = image_region_digest(end_path, mode_label_rect)
        at_end = code == 0 and after_text == before_text and end_hash is not None and end_hash == label_hash
        observations["disabled_mode_ends_inert"] = (mode_endpoint_ok and at_end) if end_hash is not None else None
        details.setdefault("evidence", []).extend([str(label_path), str(prev_path), str(end_path)])
        details["mode_note"] = {"count": mode_count, "pages": mode_pages, "last_page": page,
                                 "previous_disabled_at_first": prev_hash == image_region_digest(label_path, mode_label_rect),
                                 "next_disabled_at_end": at_end}
    ctx.stop(displays)

    # Escape is part of the Settings keyboard contract: arm the real row, send Escape,
    # observe the empty authoritative binding, then use Reset to restore its initial value.
    hotkeys = ctx.spawn("settings-ss15-hotkeys", [args.settings_bin, "--page", "hotkeys"], ctx.app_env)
    details["commands"].append(shell([args.settings_bin, "--page", "hotkeys"]))
    time.sleep(min(max(args.settle_seconds, 0.25), 1.0))
    key_point = settings_relative_point(ctx, geometry, (592, 149))
    initial_text, initial_query = dbus_shortcuts_snapshot(ctx)
    details["commands"].append(initial_query)
    initial_binding = gvariant_field_string(initial_text or "", "ToggleStartMenu")
    key_code, key_command = send_input(ctx, key_point, None)
    escape_code, escape_command = send_input(ctx, None, "Escape")
    details["commands"].extend([key_command, escape_command])
    time.sleep(min(max(args.settle_seconds, 0.25), 1.0))
    clear_text, clear_query = dbus_shortcuts_snapshot(ctx)
    details["commands"].append(clear_query)
    cleared_binding = gvariant_field_string(clear_text or "", "ToggleStartMenu")
    reset_point = args.ss12_reset_click or settings_relative_point(ctx, geometry, (732, 149))
    reset_code, reset_command = send_input(ctx, reset_point, None)
    details["commands"].append(reset_command)
    time.sleep(min(max(args.settle_seconds, 0.25), 1.0))
    reset_text, reset_query = dbus_shortcuts_snapshot(ctx)
    details["commands"].append(reset_query)
    reset_binding = gvariant_field_string(reset_text or "", "ToggleStartMenu")
    escape_geometry = window_geometry(ctx, ctx.run / "logs" / "ss15-after-escape-window-tree.txt")
    survives = ss15_escape_state_ok(hotkeys.proc.poll() is None, escape_geometry,
                                    (key_code, escape_code, reset_code), initial_binding,
                                    cleared_binding, reset_binding)
    observations["settings_survived_keyboard_escape"] = survives if clear_text is not None and reset_text is not None else None
    details["keyboard_escape"] = {"initial": initial_binding, "after_escape": cleared_binding,
                                   "after_reset": reset_binding, "named_client_geometry": escape_geometry,
                                   "settings_process_alive": hotkeys.proc.poll() is None}
    ctx.stop(hotkeys)
    return finish()


def run_ss19(ctx: Ctx, args: argparse.Namespace, external_update: bool | None) -> dict:
    """Bounded idle and repeated page lifecycle probe in the private session."""
    observations: dict[str, bool | None] = {
        "repeated_pages_opened_and_closed": None, "idle_cpu_bounded": None,
        "fd_growth_bounded": None, "memory_growth_bounded": None,
        "external_update_responsive": external_update, "owned_processes_clean": None,
    }
    details: dict = {"observations": observations, "commands": [], "iterations": [],
                     "thresholds": {"idle_cpu_seconds_max": 0.2, "fd_growth_max": 2,
                                    "rss_growth_kb_max": 16384, "page_cycles": 14}}
    def finish() -> dict:
        status, reason = assess_ss19(observations)
        entry = result_entry("SS19", status, reason, details.get("evidence", []), details["commands"])
        entry["details"] = details
        return entry

    if ctx.wm is None or ctx.wm.proc.poll() is not None:
        observations["owned_processes_clean"] = False
        details["reason"] = "owned WM is not alive for stability probe"
        return finish()
    baseline = parse_proc_metrics(ctx.wm.pid)
    details["wm_metrics_before_idle"] = baseline
    if baseline is None:
        details["reason"] = "could not read owned WM process counters from /proc"
        return finish()
    idle_started = time.monotonic()
    time.sleep(4.0)
    idle_elapsed = time.monotonic() - idle_started
    idle = parse_proc_metrics(ctx.wm.pid)
    details["wm_metrics_after_idle"] = idle
    if idle is not None:
        hz = os.sysconf("SC_CLK_TCK")
        cpu_seconds = (idle["cpu_ticks"] - baseline["cpu_ticks"]) / hz
        details["idle_cpu_seconds"] = cpu_seconds
        observations["idle_cpu_bounded"] = 0 <= cpu_seconds <= 0.2 and idle_elapsed >= 3.5
    pages = ("appearance", "desktop", "taskbar", "displays", "fonts", "hotkeys", "about")
    settle = min(max(args.settle_seconds, 0.2), 0.45)
    launched = []
    for cycle in range(2):
        for page in pages:
            name = f"settings-ss19-{cycle}-{page}"
            child = ctx.spawn(name, [args.settings_bin, "--page", page], ctx.app_env)
            time.sleep(settle)
            alive = child.proc.poll() is None
            geometry = window_geometry(ctx, ctx.run / "logs" / f"ss19-{cycle}-{page}-tree.txt")
            ready = bool(alive and geometry and (geometry["width"], geometry["height"]) == (820, 540))
            ctx.stop(child)
            closed = child.proc.poll() is not None
            launched.append({"page": page, "cycle": cycle + 1, "pid": child.pid,
                             "alive_after_open": alive, "client_geometry_ready": ready,
                             "window": geometry, "closed": closed,
                             "exit_code": child.proc.poll()})
    details["iterations"] = launched
    observations["repeated_pages_opened_and_closed"] = len(launched) == 14 and all(
        row["alive_after_open"] and row["client_geometry_ready"] and row["closed"] for row in launched)
    after = parse_proc_metrics(ctx.wm.pid)
    details["wm_metrics_after_cycles"] = after
    if after is not None:
        observations["fd_growth_bounded"] = after["fd_count"] - baseline["fd_count"] <= 2
        observations["memory_growth_bounded"] = (
            baseline["rss_kb"] is not None and after["rss_kb"] is not None
            and after["rss_kb"] - baseline["rss_kb"] <= 16384)
    observations["owned_processes_clean"] = all(
        child.proc.poll() is not None for child in ctx.owned
        if child.name.startswith("settings-ss19-")) and ctx.wm.proc.poll() is None
    details["external_update_source"] = "SS14 no-input accent projection" if external_update is True else None
    return finish()


def gdbus_call(ctx: Ctx, interface_method: str, args_list: list[str], details: dict) -> tuple[int, str]:
    argv = ["gdbus", "call", "--session", "--dest", "com.arkflame.FlameWM1", "--object-path", "/com/arkflame/FlameWM1",
            "--method", "com.arkflame.FlameWM1." + interface_method, *args_list]
    result = subprocess.run(argv, env=ctx.app_env, capture_output=True, text=True, timeout=10, check=False)
    details["commands"].append(shell(argv))
    return result.returncode, (result.stdout if result.returncode == 0 else result.stderr).strip()


def settings_client_digest(ctx: Ctx, name: str, evidence: Path) -> tuple[str | None, dict | None]:
    """Capture the root, locate the Settings client by xwininfo and hash exactly that client rectangle."""
    geometry = wait_for_settings_window(ctx, evidence, timeout=3.0)
    path = ctx.run / "shots" / f"{name}.png"
    ok, _ = capture(ctx, path)
    if geometry is None or not ok:
        return None, geometry
    return image_region_digest(path, (geometry["x"], geometry["y"], geometry["width"], geometry["height"])), geometry


def ss14_ui_refresh(ctx: Ctx, args: argparse.Namespace, details: dict, key: str, page: str,
                    external, unrelated) -> bool | None:
    """Open Settings on `page`; an external D-Bus mutation must repaint the client with no input.

    Controls: a second capture with no change and an unrelated external mutation must both leave
    the client pixels identical before the real external change is allowed to differ."""
    record: dict = {"page": page}
    details[key] = record
    window = ctx.spawn(f"settings-ss14-{key}", [args.settings_bin, "--page", page], ctx.app_env)
    time.sleep(args.settle_seconds)
    evidence = ctx.run / "logs" / f"ss14-{key}-window-tree.txt"
    try:
        if window.proc.poll() is not None:
            record["reason"] = "Settings exited before measurement"
            return None
        first, geometry = settings_client_digest(ctx, f"ss14-{key}-0", evidence)
        second, _ = settings_client_digest(ctx, f"ss14-{key}-0b", evidence)
        record["geometry"] = geometry
        if first is None or second is None:
            record["reason"] = "Settings client rectangle or capture unavailable"
            return None
        record["unchanged_capture_equal"] = first == second
        ok, detail = unrelated()
        record["unrelated"] = detail
        time.sleep(args.settle_seconds)
        after_unrelated, _ = settings_client_digest(ctx, f"ss14-{key}-unrelated", evidence)
        record["unrelated_leaves_client_unchanged"] = bool(ok and after_unrelated == first)
        ok, detail = external()
        record["external"] = detail
        time.sleep(args.settle_seconds)
        changed, geometry_after = settings_client_digest(ctx, f"ss14-{key}-external", evidence)
        record["geometry_after"] = geometry_after
        record["client_changed_without_input"] = bool(ok and changed is not None and changed != first)
        record["settings_alive"] = window.proc.poll() is None
        if not (record["unchanged_capture_equal"] and record["unrelated_leaves_client_unchanged"] and ok):
            record["reason"] = "negative control failed or external call rejected; measurement not attributable"
            return None
        return bool(record["client_changed_without_input"] and record["settings_alive"])
    finally:
        ctx.stop(window)


def ss14_external_paths(ctx: Ctx, args: argparse.Namespace, details: dict, observations: dict) -> None:
    def desktop_opacity_unrelated() -> tuple[bool, dict]:
        text, _ = dbus_settings_snapshot(ctx)
        revision = gvariant_field_u64(text or "", "revision")
        current = gvariant_field_uint(text or "", "DesktopSelectionFillOpacity")
        if revision is None or current is None:
            return False, {"reason": "settings snapshot unavailable"}
        code, reply = gdbus_call(ctx, "Settings.Apply", [f"uint64 {revision}", "{'DesktopSelectionFillOpacity': <uint32 "
                                                         + str(21 if current != 21 else 20) + ">}"], details)
        return code == 0, {"exit": code, "reply": reply}

    # Panels: the Taskbar page shows the Edge selector for the output.
    panels_text, _ = dbus_panels_snapshot(ctx)
    panels = parse_panels_snapshot(panels_text or "")
    if panels is None or not panels["panels"]:
        details["panels_reason"] = "Panels snapshot unavailable or no panel output"
    else:
        output = panels["panels"][0]["output"]
        original_edge = panels["panels"][0]["edge"]

        def panels_external() -> tuple[bool, dict]:
            current_text, _ = dbus_panels_snapshot(ctx)
            current = parse_panels_snapshot(current_text or "")
            if current is None:
                return False, {"reason": "Panels snapshot unavailable"}
            edge = "top" if current["panels"][0]["edge"] != "top" else "bottom"
            code, reply = gdbus_call(ctx, "Panels.SetEdge", ["'" + output + "'", "'" + edge + "'", f"uint64 {current['revision']}"], details)
            return code == 0, {"exit": code, "reply": reply, "edge": edge}

        observations["panels_external_change"] = ss14_ui_refresh(
            ctx, args, details, "panels", "taskbar", panels_external, desktop_opacity_unrelated)
        restored_text, _ = dbus_panels_snapshot(ctx)
        restored = parse_panels_snapshot(restored_text or "")
        if restored and restored["panels"] and restored["panels"][0]["edge"] != original_edge:
            gdbus_call(ctx, "Panels.SetEdge", ["'" + output + "'", "'" + original_edge + "'", f"uint64 {restored['revision']}"], details)

    # Shortcuts: a row shown on the Hotkeys page (Desktop down) receives a new binding from a second client.
    def shortcuts_external() -> tuple[bool, dict]:
        text, _ = dbus_shortcuts_snapshot(ctx)
        revision = gvariant_field_u64(text or "", "revision")
        current = gvariant_field_string(text or "", "WorkspaceDown")
        if revision is None or current is None:
            return False, {"reason": "shortcuts snapshot unavailable"}
        binding = "Ctrl+Alt+J" if current != "Ctrl+Alt+J" else "Ctrl+Alt+K"
        code, reply = gdbus_call(ctx, "Shortcuts.SetBinding", ["'WorkspaceDown'", "'" + binding + "'", f"uint64 {revision}"], details)
        return code == 0, {"exit": code, "reply": reply, "binding": binding}

    shortcuts_before, _ = dbus_shortcuts_snapshot(ctx)
    observations["shortcuts_external_change"] = ss14_ui_refresh(
        ctx, args, details, "shortcuts", "hotkeys", shortcuts_external, desktop_opacity_unrelated)
    text, _ = dbus_shortcuts_snapshot(ctx)
    revision = gvariant_field_u64(text or "", "revision")
    if revision is not None and gvariant_field_string(text or "", "WorkspaceDown") != \
            gvariant_field_string(shortcuts_before or "", "WorkspaceDown"):
        gdbus_call(ctx, "Shortcuts.ResetBinding", ["'WorkspaceDown'", f"uint64 {revision}"], details)


def ss14_stable_pixel(ctx: Ctx, evidence: Path, name: str, point: tuple[int, int]) -> tuple[tuple | None, dict | None]:
    """Capture and sample the point at the live Settings geometry; the geometry must be unchanged across the capture."""
    for _ in range(3):
        before = wait_for_settings_window(ctx, evidence)
        if before is None:
            return None, None
        shot = ctx.run / "shots" / f"{name}.png"
        if not capture(ctx, shot)[0]:
            return None, before
        after = window_geometry(ctx, evidence)
        if after == before:
            x, y = ss14_accent_screen_point(before, point)
            return image_pixel(shot, x, y), before
        time.sleep(0.3)
    return None, None


def ss14_signal_loss(ctx: Ctx, args: argparse.Namespace, details: dict, observations: dict) -> None:
    """Restart the owned WM (bus-name owner loss) under an open Settings client; it must keep following changes."""
    record: dict = {}
    details["signal_loss"] = record
    snapshot, _ = dbus_settings_snapshot(ctx)
    accent = gvariant_field_string(snapshot or "", "accent")
    point = (170, int(args.ss14_accent_pixel.split(",")[1]))  # selected-row interior right of the label text
    if accent is None or Image is None:
        record["reason"] = "settings snapshot or Pillow unavailable"
        return
    window = ctx.spawn("settings-ss14-signal-loss", [args.settings_bin, "--page", "appearance"], ctx.app_env)
    time.sleep(args.settle_seconds)
    evidence = ctx.run / "logs" / "ss14-signal-loss-window-tree.txt"
    try:
        geometry = wait_for_settings_window(ctx, evidence)
        if geometry is None:
            record["reason"] = "Settings client was not mapped"
            return
        pre_pixel, pre_geometry = ss14_stable_pixel(ctx, evidence, "ss14-signal-loss-pre", point)
        record["pre_geometry"] = pre_geometry
        restarted = restart_owned_wm(ctx, args, "flamewm-ss14-signal-loss-restart")
        record["wm_restarted"] = restarted
        time.sleep(args.settle_seconds)
        # The WM re-adopts and re-places the client on restart: sample at the live post-restart geometry.
        mid_pixel, mid_geometry = ss14_stable_pixel(ctx, evidence, "ss14-signal-loss-restarted", point)
        record["restarted_geometry"] = mid_geometry
        record["settings_alive_after_restart"] = window.proc.poll() is None
        text, _ = dbus_settings_snapshot(ctx)
        revision = gvariant_field_u64(text or "", "revision")
        current = gvariant_field_string(text or "", "accent") or accent
        target = next(value for value in ("#2878D7", "#23A982", "#EF4048") if value.lower() != current.lower())
        if not restarted or revision is None or pre_pixel is None or mid_pixel is None:
            record["reason"] = "restart, snapshot or pixel sample unavailable"
            return
        code, reply = gdbus_call(ctx, "Settings.Apply", [f"uint64 {revision}", "{'accent': <'" + target + "'>}"], details)
        record["apply"] = {"exit": code, "reply": reply, "target": target}
        time.sleep(args.settle_seconds)
        post_pixel, post_geometry = ss14_stable_pixel(ctx, evidence, "ss14-signal-loss-post", point)
        record["post_geometry"] = post_geometry
        expected = expected_selected_row_pixel(target)
        record.update({"pre_pixel": pre_pixel, "restarted_pixel": mid_pixel, "post_pixel": post_pixel,
                       "expected_pixel": expected})
        if code != 0:
            return
        observations["signal_loss_recovery"] = bool(
            record["settings_alive_after_restart"] and mid_pixel == pre_pixel
            and post_pixel is not None and post_pixel != mid_pixel and rgb_close(post_pixel, expected, 8))
    finally:
        ctx.stop(window)



def run_ss14(ctx: Ctx, args: argparse.Namespace) -> dict:
    """Mutate one Settings value through a second D-Bus client; require idle visible propagation."""
    observations: dict[str, bool | None] = {
        "settings_accent_source_revision": None,
        "settings_signals_observed": None,
        "settings_visible_without_input": None,
        "panels_external_change": None,
        "shortcuts_external_change": None,
        "display_rollback_signal": None,
        "signal_loss_recovery": None,
    }
    details: dict = {"observations": observations,
                     "paths_exercised": ["Settings.Apply(accent) from gdbus second client"],
                     "paths_not_exercised": ["panels", "shortcuts", "display timeout rollback", "signal connection loss"],
                     "input_events_sent": [], "commands": []}

    def finish() -> dict:
        status, reason = assess_ss14(observations)
        entry = result_entry("SS14", status, reason, commands=details["commands"])
        entry["details"] = details
        return entry

    if any(shutil.which(tool) is None for tool in ("gdbus", "dbus-monitor", "xwininfo", "import")):
        details["reason"] = "required owned-session D-Bus or screenshot observer tool is unavailable"
        return finish()
    if Image is None and shutil.which("identify") is None:
        details["reason"] = "no PNG pixel reader is available"
        return finish()

    initial_text, initial_cmd = dbus_settings_snapshot(ctx)
    details["commands"].append(initial_cmd)
    initial_revision = gvariant_field_u64(initial_text or "", "revision")
    initial_accent = gvariant_field_string(initial_text or "", "accent")
    details["before_snapshot"] = {"revision": initial_revision, "accent": initial_accent}
    if initial_text is None or initial_revision is None or initial_accent is None:
        details["reason"] = "authoritative initial settings snapshot is unavailable or malformed"
        return finish()

    choices = ("#2878D7", "#23A982")
    target = next(value for value in choices if value.lower() != initial_accent.lower())
    target_rgb = accent_rgb(target)
    details["target_accent"] = target
    settings = ctx.spawn("settings-ss14-appearance", [args.settings_bin, "--page", "appearance"], ctx.app_env)
    details["commands"].append(shell([args.settings_bin, "--page", "appearance"]))
    time.sleep(args.settle_seconds)
    if settings.proc.poll() is not None:
        ctx.stop(settings)
        details["reason"] = "Settings exited before external mutation"
        observations["settings_accent_source_revision"] = False
        return finish()

    geometry = wait_for_settings_window(ctx, ctx.run / "logs" / "ss14-settings-window-tree.txt")
    details["window_geometry"] = geometry
    if geometry is None:
        ctx.stop(settings)
        details["reason"] = "named Settings client was not mapped before the baseline screenshot"
        return finish()
    point = tuple(int(part) for part in args.ss14_accent_pixel.split(","))
    pixel_x, pixel_y = ss14_accent_screen_point(geometry, point)
    before_path = ctx.run / "shots" / "ss14-before.png"
    before_ok, before_capture_cmd = capture(ctx, before_path)
    details["commands"].append(before_capture_cmd)
    before_pixel = image_pixel(before_path, pixel_x, pixel_y) if before_ok else None
    details["before_pixel"] = before_pixel
    if before_pixel is None:
        ctx.stop(settings)
        details["reason"] = "could not sample the configured Settings accent pixel before mutation"
        return finish()

    monitor_path = ctx.run / "logs" / "ss14-settings-monitor.log"
    monitor_argv = ["dbus-monitor", "--session",
                    "type='method_call',interface='com.arkflame.FlameWM1.Settings',member='Apply'",
                    "type='signal',interface='com.arkflame.FlameWM1.Settings',member='SettingsChanged'",
                    "type='signal',interface='com.arkflame.FlameWM1.Settings',member='SettingsSnapshotChanged'"]
    monitor = ctx.spawn("ss14-settings-monitor", monitor_argv, ctx.app_env)
    details["commands"].append(shell(monitor_argv))
    time.sleep(0.2)
    apply_text, apply_cmd, apply_exit = dbus_apply_accent(ctx, initial_revision, target)
    details["commands"].append(apply_cmd)
    details["mutation_exit"] = apply_exit
    details["mutation_reply"] = apply_text
    if apply_exit is None or apply_exit != 0:
        ctx.stop(settings)
        stop_dbus_monitor(ctx, monitor)
        details["reason"] = "second-client Settings.Apply failed: " + str(apply_text)
        observations["settings_accent_source_revision"] = False
        return finish()

    time.sleep(args.settle_seconds)
    # No XTEST or mouse/keyboard input occurs between Apply and these observations.
    after_text, after_cmd = dbus_settings_snapshot(ctx)
    details["commands"].append(after_cmd)
    after_revision = gvariant_field_u64(after_text or "", "revision")
    after_accent = gvariant_field_string(after_text or "", "accent")
    details["after_snapshot"] = {"revision": after_revision, "accent": after_accent}
    if after_text is not None and after_revision is not None and after_accent is not None:
        observations["settings_accent_source_revision"] = (
            after_revision > initial_revision and after_accent.lower() == target.lower()
        )
    else:
        observations["settings_accent_source_revision"] = None

    stop_dbus_monitor(ctx, monitor)
    signal_log = monitor_path.read_text(encoding="utf-8", errors="replace") if monitor_path.exists() else ""
    observed_signals = settings_signal_observations(signal_log)
    details["dbus_observed"] = observed_signals
    details["dbus_events"] = settings_bus_events(signal_log)
    if observations["settings_accent_source_revision"] is True:
        observations["settings_signals_observed"] = all(observed_signals.values())

    after_path = ctx.run / "shots" / "ss14-after-no-input.png"
    after_ok, after_capture_cmd = capture(ctx, after_path)
    details["commands"].append(after_capture_cmd)
    after_pixel = image_pixel(after_path, pixel_x, pixel_y) if after_ok else None
    details["after_pixel"] = after_pixel
    if after_pixel is not None and target_rgb is not None:
        expected_row = expected_selected_row_pixel(target)
        within_target = (expected_row is not None and max(
            abs(actual - expected) for actual, expected in zip(after_pixel, expected_row)) <= 8)
        observations["settings_visible_without_input"] = before_pixel != after_pixel and within_target
    else:
        observations["settings_visible_without_input"] = None
    details["idle_observation"] = "no mouse/key input after the external Apply call"
    details["evidence"] = [str(before_path), str(after_path), str(monitor_path)]
    ctx.stop(settings)
    details["paths_exercised"] += ["Panels.SetEdge refresh of open Taskbar page", "Shortcuts.SetBinding refresh of open Hotkeys page",
                                   "owned WM restart under an open Settings client"]
    details["paths_not_exercised"] = []
    ss14_external_paths(ctx, args, details, observations)
    ss14_signal_loss(ctx, args, details, observations)
    snapshot, _ = dbus_displays_snapshot(ctx)
    modes = display_mode_count(snapshot or "")
    details["display_rollback"] = {
        "mode_count": modes,
        "reason": "Displays snapshot exposes no alternative modes under Xephyr (modes=[]); "
                  "BeginModeChange has no mode id and no 15 s transaction can be started" if not modes else
                  "runner does not drive a timed revert transaction"}
    details["paths_not_exercised"].append("display timeout rollback: " + details["display_rollback"]["reason"])
    return finish()


def apply_and_read_settings(ctx: Ctx, revision: int, changes: dict[str, str], details: dict) -> dict | None:
    reply, command, exit_code = dbus_apply_settings(ctx, revision, changes)
    details["commands"].append(command)
    details.setdefault("apply_replies", []).append({"exit": exit_code, "reply": reply})
    if exit_code != 0:
        return None
    text, command = dbus_settings_snapshot(ctx)
    details["commands"].append(command)
    return {"text": text, "revision": gvariant_field_u64(text or "", "revision")}


def restart_owned_wm(ctx: Ctx, args: argparse.Namespace, name: str) -> bool:
    ctx.stop(ctx.wm)
    ctx.wm = ctx.spawn(name, [args.wm_bin], ctx.app_env)
    deadline = time.monotonic() + max(10.0, args.settle_seconds * 4)
    while time.monotonic() < deadline:
        if ctx.wm.proc.poll() is not None:
            return False
        snapshot, _ = dbus_settings_snapshot(ctx)
        if snapshot is not None:
            return True
        time.sleep(0.25)
    return False


GTK_ICON_CONSUMER = """
import json, sys, time, gi
gi.require_version("Gtk", "3.0")
from gi.repository import Gtk, GLib
settings = Gtk.Settings.get_default()
def sample():
    info = Gtk.IconTheme.get_default().lookup_icon("folder", 48, 0)
    rec = {"t": time.time(), "name": settings.get_property("gtk-icon-theme-name"),
           "folder": info.get_filename() if info else None}
    with open(sys.argv[1], "a") as out:
        out.write(json.dumps(rec) + "\\n")
    return True
GLib.timeout_add(250, sample)
Gtk.main()
"""


def start_session_clients(ctx: Ctx, args: argparse.Namespace, which: tuple[str, ...], tag: str) -> dict[str, Owned]:
    """Spawn flamewm-desktop / flamewm-shell as owned children with the product-session environment."""
    env = dict(ctx.app_env, XDG_SESSION_TYPE="x11")
    started: dict[str, Owned] = {}
    for name in which:
        binary = getattr(args, f"{name}_bin")
        if os.access(binary, os.X_OK):
            started[name] = ctx.spawn(f"{name}-{tag}", [binary], env)
    time.sleep(args.settle_seconds)
    return started


def stop_session_clients(ctx: Ctx, clients: dict[str, Owned]) -> None:
    for owned in clients.values():
        ctx.stop(owned)


def clients_alive(clients: dict[str, Owned]) -> bool:
    return all(owned.proc.poll() is None for owned in clients.values())


def rgb_close(actual: tuple[int, int, int] | None, expected: tuple[int, int, int] | None, tolerance: int) -> bool:
    return (actual is not None and expected is not None
            and max(abs(a - e) for a, e in zip(actual, expected)) <= tolerance)


# Black rows tolerated before a floating Taskbar (default taskbarStyle floating, inset 8px).
TASKBAR_EDGE_MARGIN = 16


def taskbar_measure(ctx: Ctx, name: str) -> dict | None:
    """Real capture of the Shell taskbar: contiguous non-black extent at screen mid-column."""
    if Image is None:
        return None
    path = ctx.run / "shots" / f"{name}.png"
    ok, _ = capture(ctx, path)
    if not ok:
        return None
    try:
        with Image.open(path) as shot:
            image = shot.convert("RGB")
    except OSError:
        return None
    width, height = image.size
    column = width // 2

    def extent(rows) -> tuple[int, int | None]:
        """Contiguous lit run from the first lit row within TASKBAR_EDGE_MARGIN of the screen edge.
        The default floating Taskbar is inset from the edge, so black rows before the bar are skipped."""
        rows = list(rows)
        start = next((index for index, row in enumerate(rows[:TASKBAR_EDGE_MARGIN + 1])
                      if image.getpixel((column, row)) != (0, 0, 0)), None)
        if start is None:
            return 0, None
        count = 0
        for row in rows[start:]:
            if image.getpixel((column, row)) == (0, 0, 0):
                break
            count += 1
        return count, rows[start]

    (top, top_row), (bottom, bottom_row) = extent(range(height)), extent(range(height - 1, -1, -1))
    if top == 0 and bottom == 0:
        return {"shot": str(path), "edge": None, "extent": 0, "inside": None, "band_digest": None}
    edge, size = ("bottom", bottom) if bottom >= top else ("top", top)
    band_y = bottom_row - size + 1 if edge == "bottom" else top_row
    inside_y = band_y + size // 2
    return {"shot": str(path), "edge": edge, "extent": size, "inside": image.getpixel((column, inside_y)),
            "band_digest": image_region_digest(path, (100, band_y, max(1, width - 400), size))}


def taskbar_measure_settled(ctx: Ctx, name: str, edge: str, height: int, color: str, opacity: int,
                            timeout: float) -> dict | None:
    """Re-capture until the Taskbar matches the expected edge/extent/colour or the timeout expires.
    The Shell animates a Panels edge move, so the first frame after SetEdge can be mid-transition
    (measured: top bar at ~88% opacity 1.5 s after the move, 67% once settled). The threshold is unchanged."""
    deadline = time.monotonic() + timeout
    attempt = 0
    while True:
        attempt += 1
        measured = taskbar_measure(ctx, f"{name}-{attempt}")
        if taskbar_matches(measured, edge, height, color, opacity) or time.monotonic() >= deadline:
            if measured is not None:
                measured["attempts"] = attempt
            return measured
        time.sleep(0.5)


def taskbar_matches(measure: dict | None, edge: str, height: int, color: str, opacity: int) -> bool:
    rgb = accent_rgb(color)
    if measure is None or rgb is None or measure["edge"] != edge or measure["extent"] != height:
        return False
    expected = tuple(round(channel * opacity / 100) for channel in rgb)
    return rgb_close(measure["inside"], expected, 16)


def gtk_consumer_latest(path: Path) -> dict | None:
    try:
        lines = [line for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]
        return json.loads(lines[-1]) if lines else None
    except (OSError, ValueError):
        return None


def gtk_consumer_wait(path: Path, expected: str, timeout: float = 4.0) -> dict | None:
    """Wait for the real GTK client to report the expected icon theme; return the last sample either way."""
    deadline = time.monotonic() + timeout
    latest = gtk_consumer_latest(path)
    while time.monotonic() < deadline:
        latest = gtk_consumer_latest(path)
        if latest is not None and latest.get("name") == expected:
            return latest
        time.sleep(0.25)
    return latest



def ss04_gtk_consumer(ctx: Ctx, args: argparse.Namespace, details: dict, observations: dict) -> None:
    """Real XSETTINGS consumer: a GTK3 client under the owned display reads gtk-icon-theme-name."""
    probe = subprocess.run([sys.executable, "-c", "import gi;gi.require_version('Gtk','3.0');from gi.repository import Gtk"],
                           capture_output=True, timeout=30, check=False)
    if probe.returncode != 0:
        details["consumer_reason"] = "python3 GTK3 bindings unavailable; no real icon-theme consumer"
        return
    out = ctx.run / "logs" / "ss04-gtk-consumer.jsonl"
    consumer = ctx.spawn("ss04-gtk-consumer", [sys.executable, "-c", GTK_ICON_CONSUMER, str(out)], ctx.app_env)
    time.sleep(2.0)
    base = gtk_consumer_latest(out)
    time.sleep(1.0)
    idle = gtk_consumer_latest(out)
    live = (consumer.proc.poll() is None and base is not None and idle is not None and idle["t"] > base["t"])
    details["consumer"] = {"baseline": base, "idle": idle, "live": live}
    if not live:
        details["consumer_reason"] = "GTK consumer did not produce live samples"
        return
    snapshot, _ = dbus_settings_snapshot(ctx)
    revision = gvariant_field_u64(snapshot or "", "revision")
    if revision is None:
        return
    # Negative control: unrelated setting must not move the consumer's icon theme.
    opacity = gvariant_field_uint(snapshot or "", "surfaceOpacity")
    applied = apply_and_read_settings(ctx, revision, {"surfaceOpacity": f"uint32 {67 if opacity != 67 else 68}"}, details)
    revision = (applied or {}).get("revision") or revision
    time.sleep(1.5)
    unrelated = gtk_consumer_latest(out)
    details["consumer"]["after_unrelated_change"] = unrelated
    unrelated_unchanged = unrelated is not None and unrelated["name"] == idle["name"]
    target = "breeze-dark" if idle["name"] != "breeze-dark" else "HighContrast"
    applied = apply_and_read_settings(ctx, revision, {"iconTheme": "'" + target + "'"}, details)
    revision = (applied or {}).get("revision") or revision
    seen = gtk_consumer_wait(out, target)
    details["consumer"]["after_supported_theme"] = {"requested": target, "observed": seen}
    unknown = "FlameWM-Unknown-Theme-Fixture"
    applied = apply_and_read_settings(ctx, revision, {"iconTheme": "'" + unknown + "'"}, details)
    revision = (applied or {}).get("revision") or revision
    fallback = gtk_consumer_wait(out, unknown)
    details["consumer"]["after_unknown_theme"] = {"requested": unknown, "observed": fallback}
    log_text = (ctx.run / "logs" / "ss04-gtk-consumer.log").read_text(encoding="utf-8", errors="replace") \
        if (ctx.run / "logs" / "ss04-gtk-consumer.log").exists() else ""
    details["consumer"]["invalid_xsettings_warnings"] = log_text.count("Invalid XSETTINGS property")
    details["consumer"]["invalid_xsettings_sample"] = next(
        (line for line in log_text.splitlines() if "Invalid XSETTINGS" in line), None)
    if not unrelated_unchanged:
        observations["real_icon_fallback_consumer"] = False
        return
    observations["real_icon_fallback_consumer"] = bool(
        seen and seen.get("name") == target and fallback and fallback.get("name") == unknown
        and fallback.get("folder"))
    # Restart persistence: a fresh GTK client after a WM restart must read the last applied theme.
    ctx.stop(consumer)
    restarted = restart_owned_wm(ctx, args, "flamewm-ss04-persistence-restart")
    details["consumer"]["wm_restarted"] = restarted
    if not restarted:
        return
    out2 = ctx.run / "logs" / "ss04-gtk-consumer-restart.jsonl"
    second = ctx.spawn("ss04-gtk-consumer-restart", [sys.executable, "-c", GTK_ICON_CONSUMER, str(out2)], ctx.app_env)
    persisted = gtk_consumer_wait(out2, unknown, timeout=6.0)
    details["consumer"]["after_restart"] = persisted
    observations["theme_persistence_after_restart"] = bool(persisted and persisted.get("name") == unknown)
    ctx.stop(second)



def run_ss04(ctx: Ctx, args: argparse.Namespace) -> dict:
    observations: dict[str, bool | None] = {
        "supported_theme_snapshots": None, "unknown_theme_snapshot": None,
        "real_icon_fallback_consumer": None, "theme_persistence_after_restart": None,
    }
    details: dict = {"observations": observations,
                     "paths_exercised": ["Settings.Apply(iconTheme) supported cycle and unknown value snapshot",
                                         "real GTK3 client XSETTINGS gtk-icon-theme-name + icon lookup fallback",
                                         "WM restart persistence read by a fresh GTK client"],
                     "paths_not_exercised": ["Shell icon-cache invalidation (flamewm-shell does not read the iconTheme setting; XSETTINGS Net/IconThemeName is the only consumer path)"],
                     "commands": [], "theme_observations": []}
    initial, command = dbus_settings_snapshot(ctx)
    details["commands"].append(command)
    revision = gvariant_field_u64(initial or "", "revision")
    original = gvariant_field_string(initial or "", "iconTheme")
    if initial is None or revision is None or original is None:
        details["reason"] = "authoritative iconTheme snapshot is unavailable or malformed"
        status, reason = assess_ss04(observations)
        return {**result_entry("SS04", status, reason), "details": details}
    themes = ("*:-HighContrast", "breeze-dark", "HighContrast")
    start = themes.index(original) if original in themes else -1
    order = [themes[(start + 1 + offset) % len(themes)] for offset in range(len(themes))]
    current_revision = revision
    cycle_ok = True
    for theme in order:
        applied = apply_and_read_settings(ctx, current_revision, {"iconTheme": "'" + theme + "'"}, details)
        observed = gvariant_field_string((applied or {}).get("text") or "", "iconTheme")
        observed_revision = (applied or {}).get("revision")
        details["theme_observations"].append({"requested": theme, "observed": observed,
                                             "revision": observed_revision})
        if observed != theme or not isinstance(observed_revision, int) or observed_revision <= current_revision:
            cycle_ok = False
            break
        current_revision = observed_revision
    observations["supported_theme_snapshots"] = cycle_ok
    unknown = "FlameWM-Unknown-Theme-Fixture"
    applied = apply_and_read_settings(ctx, current_revision, {"iconTheme": "'" + unknown + "'"}, details)
    unknown_value = gvariant_field_string((applied or {}).get("text") or "", "iconTheme")
    unknown_revision = (applied or {}).get("revision")
    details["theme_observations"].append({"requested": unknown, "observed": unknown_value,
                                         "revision": unknown_revision})
    observations["unknown_theme_snapshot"] = (
        unknown_value == unknown and isinstance(unknown_revision, int) and unknown_revision > current_revision
    ) if applied else False
    ss04_gtk_consumer(ctx, args, details, observations)
    refreshed, command = dbus_settings_snapshot(ctx)
    details["commands"].append(command)
    latest_revision = gvariant_field_u64(refreshed or "", "revision")
    if latest_revision is None:
        latest_revision = unknown_revision if isinstance(unknown_revision, int) else current_revision
    if original != gvariant_field_string(refreshed or "", "iconTheme"):
        restored = apply_and_read_settings(ctx, latest_revision, {"iconTheme": "'" + original + "'"}, details)
        details["restored_theme"] = gvariant_field_string((restored or {}).get("text") or "", "iconTheme")
    status, reason = assess_ss04(observations)
    entry = result_entry("SS04", status, reason, commands=details["commands"])
    entry["details"] = details
    return entry


def run_ss05(ctx: Ctx, args: argparse.Namespace) -> dict:
    """Click the real Wallpaper Choose action against a private XDG portal fixture."""
    observations: dict[str, bool | None] = {
        "success_applied_without_input": None,
        "cancel_preserved_without_error": None,
        "error_preserved_and_visible": None,
    }
    details: dict = {
        "observations": observations,
        "paths_exercised": ["native Choose click", "FileChooser.OpenFile", "Request.Response success/cancel/error",
                            "Settings snapshot with no input after portal completion"],
        "commands": [],
        "cases": {},
        "input_after_choose": False,
    }

    def finish(status: str | None = None, reason: str | None = None) -> dict:
        if status is None:
            status, reason = assess_ss05(observations)
        entry = result_entry("SS05", status, reason or "", details.get("evidence", []), details["commands"])
        entry["details"] = details
        return entry

    helper = ROOT / "tools" / "flamewm_settings_fake_portal.py"
    probe = subprocess.run([sys.executable, str(helper), "--probe"], env=ctx.app_env,
                           capture_output=True, text=True, timeout=10, check=False)
    details["commands"].append(shell([sys.executable, str(helper), "--probe"]))
    details["portal_capability_probe"] = {"exit": probe.returncode, "stdout": probe.stdout.strip(),
                                           "stderr": probe.stderr.strip()}
    if probe.returncode != 0:
        return finish(STATUS_ENV_BLOCKED,
                      "private fake portal capability probe failed: " + (probe.stderr.strip() or probe.stdout.strip()))

    source_image = ROOT / "assets" / "web" / "flamewm.png"
    fixture = ctx.run / "fixtures" / "wallpaper.png"
    try:
        if not source_image.is_file():
            return finish(STATUS_ENV_BLOCKED, f"PNG fixture source is unavailable: {source_image}")
        fixture.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source_image, fixture)
    except OSError as error:
        return finish(STATUS_ENV_BLOCKED, f"cannot create owned wallpaper image fixture: {error}")
    details["fixture"] = str(fixture)

    click_relative = tuple(int(part) for part in args.ss05_click.split(","))
    settings_size = (960, 640)
    timeout = max(6.0, args.settle_seconds * 3.0)
    previous_value: str | None = None
    previous_revision: int | None = None

    for mode in ("success", "cancel", "error"):
        case: dict = {"mode": mode, "commands": []}
        details["cases"][mode] = case
        report = ctx.run / "logs" / f"ss05-portal-{mode}.json"
        portal_argv = [sys.executable, str(helper), "--mode", mode, "--report", str(report)]
        if mode == "success":
            portal_argv.extend(["--image", str(fixture)])
        portal = ctx.spawn(f"ss05-portal-{mode}", portal_argv, ctx.app_env)
        case["portal_pid"] = portal.pid
        case["commands"].append(shell(portal_argv))
        details["commands"].append(shell(portal_argv))
        settings = None
        try:
            ready_deadline = time.monotonic() + 5.0
            portal_state = None
            while time.monotonic() < ready_deadline:
                try:
                    portal_state = json.loads(report.read_text(encoding="utf-8"))
                except (OSError, ValueError):
                    portal_state = None
                if portal_state and portal_state.get("ready"):
                    break
                if portal.proc.poll() is not None:
                    break
                time.sleep(0.05)
            if not portal_state or not portal_state.get("ready"):
                case["portal_error"] = (portal_state or {}).get("error") if portal_state else None
                observations[{"success": "success_applied_without_input",
                              "cancel": "cancel_preserved_without_error",
                              "error": "error_preserved_and_visible"}[mode]] = False
                case["failure"] = "owned fake portal did not become ready"
                continue

            settings_argv = [args.settings_bin, "--page", "desktop", "--window-size", "960x640"]
            settings = ctx.spawn(f"settings-ss05-{mode}", settings_argv, ctx.app_env)
            case["settings_pid"] = settings.pid
            case["commands"].append(shell(settings_argv))
            details["commands"].append(shell(settings_argv))
            time.sleep(args.settle_seconds)
            geometry = wait_for_settings_window(ctx, ctx.run / "logs" / f"ss05-{mode}-window.txt")
            case["window"] = geometry
            before_text, before_command = dbus_settings_snapshot(ctx)
            case["commands"].append(before_command)
            details["commands"].append(before_command)
            before_value = gvariant_field_string(before_text or "", "wallpaper")
            before_revision = gvariant_field_u64(before_text or "", "revision")
            case["before"] = {"wallpaper": before_value, "revision": before_revision}
            if (settings.proc.poll() is not None or geometry is None
                    or (geometry.get("width"), geometry.get("height")) != settings_size
                    or before_text is None or before_value is None or before_revision is None):
                case["failure"] = "Settings client geometry or authoritative wallpaper snapshot unavailable"
                observations[{"success": "success_applied_without_input",
                              "cancel": "cancel_preserved_without_error",
                              "error": "error_preserved_and_visible"}[mode]] = False
                continue
            if previous_value is not None and before_value != previous_value:
                case["failure"] = f"wallpaper changed between scenarios: expected {previous_value!r}, saw {before_value!r}"
                observations[{"success": "success_applied_without_input",
                              "cancel": "cancel_preserved_without_error",
                              "error": "error_preserved_and_visible"}[mode]] = False
                continue
            if previous_revision is not None and before_revision != previous_revision:
                case["failure"] = f"revision changed before scenario: expected {previous_revision}, saw {before_revision}"
                observations[{"success": "success_applied_without_input",
                              "cancel": "cancel_preserved_without_error",
                              "error": "error_preserved_and_visible"}[mode]] = False
                continue

            before_error = ctx.run / "shots" / f"ss05-{mode}-error-before.png"
            captured_before, capture_cmd = capture(ctx, before_error)
            case["commands"].append(capture_cmd)
            details["commands"].append(capture_cmd)
            case["before_error_capture"] = str(before_error)
            click_xy = ss05_choose_screen_point(geometry, click_relative)
            click = ",".join(map(str, click_xy))
            case["choose_click_screen_xy"] = click_xy
            click_code, click_command = send_input(ctx, click, None)
            case["click"] = click
            case["click_exit"] = click_code
            case["commands"].append(click_command)
            details["commands"].append(click_command)
            if click_code != 0:
                case["failure"] = f"owned XTEST click failed with exit {click_code}"
                observations[{"success": "success_applied_without_input",
                              "cancel": "cancel_preserved_without_error",
                              "error": "error_preserved_and_visible"}[mode]] = False
                continue

            invocation_deadline = time.monotonic() + timeout
            invocation = None
            while time.monotonic() < invocation_deadline:
                try:
                    current_report = json.loads(report.read_text(encoding="utf-8"))
                    calls = current_report.get("invocations", [])
                    if calls:
                        invocation = calls[-1]
                        break
                except (OSError, ValueError):
                    pass
                if settings.proc.poll() is not None:
                    break
                time.sleep(0.05)
            case["portal_invocation"] = invocation
            if not invocation or invocation.get("mode") != mode or invocation.get("title") != "Choose Wallpaper":
                case["failure"] = "Choose click did not produce the expected private FileChooser.OpenFile call"
                observations[{"success": "success_applied_without_input",
                              "cancel": "cancel_preserved_without_error",
                              "error": "error_preserved_and_visible"}[mode]] = False
                continue

            observed_text = None
            observed_value = None
            observed_revision = None
            if mode == "success":
                apply_deadline = time.monotonic() + timeout
                while time.monotonic() < apply_deadline:
                    observed_text, query = dbus_settings_snapshot(ctx)
                    observed_value = gvariant_field_string(observed_text or "", "wallpaper")
                    observed_revision = gvariant_field_u64(observed_text or "", "revision")
                    if ss05_wallpaper_matches_fixture(observed_value, fixture, ctx.run) and observed_revision is not None and observed_revision > before_revision:
                        break
                    if settings.proc.poll() is not None:
                        break
                    time.sleep(0.1)
                case["commands"].append(query)
                details["commands"].append(query)
                case["after"] = {"wallpaper": observed_value, "revision": observed_revision}
                case["idle_observation"] = "Settings snapshot reached fixture value with no input after the Choose click"
                after_shot = ctx.run / "shots" / "ss05-success-after.png"
                captured_after, after_capture_cmd = capture(ctx, after_shot)
                case["commands"].append(after_capture_cmd)
                details["commands"].append(after_capture_cmd)
                case["after_capture"] = str(after_shot)
                applied = (settings.proc.poll() is None and ss05_wallpaper_matches_fixture(observed_value, fixture, ctx.run)
                           and observed_revision is not None and observed_revision > before_revision
                           and captured_before and captured_after)
                observations["success_applied_without_input"] = applied
                if applied:
                    previous_value, previous_revision = observed_value, observed_revision
            elif mode == "cancel":
                time.sleep(0.6)  # allow the no-input completion wake to be consumed
                observed_text, query = dbus_settings_snapshot(ctx)
                observed_value = gvariant_field_string(observed_text or "", "wallpaper")
                observed_revision = gvariant_field_u64(observed_text or "", "revision")
                case["commands"].append(query)
                details["commands"].append(query)
                case["after"] = {"wallpaper": observed_value, "revision": observed_revision}
                after_error = ctx.run / "shots" / "ss05-cancel-error-after.png"
                captured_after, after_capture_cmd = capture(ctx, after_error)
                case["commands"].append(after_capture_cmd)
                details["commands"].append(after_capture_cmd)
                case["after_error_capture"] = str(after_error)
                error_unchanged = False
                if captured_before and captured_after:
                    error_diff = pixel_change(str(before_error), str(after_error),
                                              (geometry["x"] + 208, geometry["y"] + geometry["height"] - 28,
                                               geometry["width"] - 208, 28))
                    case["error_region_diff"] = error_diff
                    error_unchanged = "fraction" in error_diff and error_diff["fraction"] < 0.002
                observations["cancel_preserved_without_error"] = (
                    settings.proc.poll() is None and observed_value == before_value
                    and observed_revision == before_revision and error_unchanged)
                case["idle_observation"] = "no input after cancel response; authoritative value/revision and blank error bar checked"
            else:
                error_deadline = time.monotonic() + timeout
                error_diff = None
                after_error = ctx.run / "shots" / "ss05-error-after.png"
                while time.monotonic() < error_deadline:
                    captured_after, after_capture_cmd = capture(ctx, after_error)
                    if captured_before and captured_after:
                        error_diff = pixel_change(str(before_error), str(after_error),
                                                  (geometry["x"] + 208, geometry["y"] + geometry["height"] - 28,
                                                   geometry["width"] - 208, 28))
                        if "fraction" in error_diff and error_diff["fraction"] >= 0.002:
                            break
                    if settings.proc.poll() is not None:
                        break
                    time.sleep(0.1)
                observed_text, query = dbus_settings_snapshot(ctx)
                observed_value = gvariant_field_string(observed_text or "", "wallpaper")
                observed_revision = gvariant_field_u64(observed_text or "", "revision")
                case["commands"].extend([after_capture_cmd, query])
                details["commands"].extend([after_capture_cmd, query])
                case["after"] = {"wallpaper": observed_value, "revision": observed_revision}
                case["error_region_diff"] = error_diff
                case["after_error_capture"] = str(after_error)
                case["idle_observation"] = "visible error bar and unchanged authoritative value/revision checked without follow-up input"
                tesseract = shutil.which("tesseract")
                expected_error = "portal FileChooser returned response code 2"
                expected_error_seen = None
                if tesseract and captured_after:
                    error_crop = ctx.run / "shots" / "ss05-error-message.png"
                    crop_ok, crop_command = crop(
                        after_error,
                        (geometry["x"] + 208, geometry["y"] + geometry["height"] - 28,
                         geometry["width"] - 208 - SCROLLBAR_COLUMN_PX, 28),
                        error_crop,
                    )
                    case["commands"].append(crop_command)
                    details["commands"].append(crop_command)
                    if crop_ok:
                        ocr_argv = [tesseract, str(error_crop), "stdout", "--psm", "7"]
                        ocr = subprocess.run(ocr_argv, capture_output=True, text=True,
                                             timeout=20, check=False)
                        case["commands"].append(shell(ocr_argv))
                        details["commands"].append(shell(ocr_argv))
                        case["error_text"] = ocr.stdout.strip()
                        normalize_ocr = lambda value: re.sub(r"[^a-z0-9]+", " ", value.lower()).strip()
                        expected_tokens = normalize_ocr(expected_error)
                        observed_tokens = normalize_ocr(ocr.stdout)
                        # tesseract confuses o/a in small text; tolerate only that one glyph.
                        tolerant = re.escape(expected_tokens).replace("code", "c[oa]de")
                        expected_error_seen = (ocr.returncode == 0 and re.search(tolerant, observed_tokens) is not None)
                    case["error_message_crop"] = str(error_crop)
                else:
                    case["ocr_capability"] = {"tesseract": bool(tesseract), "captured_after": captured_after}
                observations["error_preserved_and_visible"] = (
                    settings.proc.poll() is None and observed_value == before_value
                    and observed_revision == before_revision and error_diff is not None
                    and "fraction" in error_diff and error_diff["fraction"] >= 0.002
                    and expected_error_seen is True)
                if expected_error_seen is None:
                    observations["error_preserved_and_visible"] = None
                if case.get("error_message_crop"):
                    details["evidence"].append(case["error_message_crop"])
                previous_value, previous_revision = before_value, before_revision
        except (OSError, subprocess.SubprocessError, json.JSONDecodeError) as error:
            case["failure"] = f"scenario execution error: {error}"
            observations[{"success": "success_applied_without_input",
                          "cancel": "cancel_preserved_without_error",
                          "error": "error_preserved_and_visible"}[mode]] = False
        finally:
            ctx.stop(settings)
            ctx.stop(portal)
        details.setdefault("evidence", []).append(str(report))
        if case.get("before_error_capture"):
            details["evidence"].append(case["before_error_capture"])
        for image_key in ("after_capture", "after_error_capture"):
            if case.get(image_key):
                details["evidence"].append(case[image_key])

    return finish()


SS06_NOTE_RECT = (600, 300, 240, 160)
SS06_NOTE_COLOR = (255, 235, 59)
SS06_ICON_CLICK = "37,30"
SS06_ICON_RECT = (0, 0, 80, 100)


def ss06_seed_fixtures(ctx: Ctx) -> None:
    """Private-HOME input fixtures: one Desktop file and one sticky note on output 'default', workspace 0."""
    home = ctx.run / "home"
    (home / "Desktop").mkdir(parents=True, exist_ok=True)
    (home / "Desktop" / "ss06-fixture.txt").write_text("fixture\n", encoding="utf-8")
    state = home / ".local" / "state"
    state.mkdir(parents=True, exist_ok=True)
    x, y, width, height = SS06_NOTE_RECT
    hexed = lambda text: text.encode("utf-8").hex()
    fields = [hexed("ss06-note"), "0", hexed("default"), str(x), str(y), str(width), str(height),
              hexed("ss06"), *(str(v) for v in SS06_NOTE_COLOR), "0", "0", "0", "16", "0"]
    (state / "sticky-notes.state").write_text("v2\t1\n" + "\t".join(fields) + "\n", encoding="utf-8")


def ss06_capture(ctx: Ctx, name: str) -> Path | None:
    path = ctx.run / "shots" / f"{name}.png"
    ok, _ = capture(ctx, path)
    return path if ok else None


def ss06_note_visible(path: Path | None) -> bool | None:
    if path is None:
        return None
    pixel = image_pixel(path, SS06_NOTE_RECT[0] + SS06_NOTE_RECT[2] // 2, SS06_NOTE_RECT[1] + SS06_NOTE_RECT[3] - 20)
    if pixel is None:
        return None
    return rgb_close(pixel, SS06_NOTE_COLOR, 8)


def ss06_selection_red(base: Path | None, selected: Path | None) -> int | None:
    """Median red channel of the pixels the selection fill changed inside the Desktop icon rect."""
    if base is None or selected is None or Image is None:
        return None
    x, y, width, height = SS06_ICON_RECT
    try:
        with Image.open(base) as first, Image.open(selected) as second:
            rect = (x, y, x + width, y + height)
            before = list(zip(*[iter(first.convert("RGB").crop(rect).tobytes())] * 3))
            after = list(zip(*[iter(second.convert("RGB").crop(rect).tobytes())] * 3))
    except OSError:
        return None
    reds = sorted(a[0] for b, a in zip(before, after) if b == (0, 0, 0) and a != (0, 0, 0))
    return reds[len(reds) // 2] if len(reds) >= 200 else None


def ss06_select_and_capture(ctx: Ctx, name: str, settle: float, base: Path | None) -> tuple[Path | None, int | None]:
    send_input(ctx, SS06_ICON_CLICK, None)
    send_input(ctx, None, None, move="700,600")
    time.sleep(settle)
    path = ss06_capture(ctx, name)
    return path, ss06_selection_red(base, path)



SNAP_PREVIEW_TITLE = "FlameWM snap preview"
SS06_SNAP_MIN_CHANGED_FRACTION = 0.05
SS06_SNAP_NOISE_MAX_FRACTION = 0.001


def ss06_snap_preview_observed(preview_before: bool | None, preview_during: bool | None,
                               changed_fraction: float | None, noise_fraction: float | None,
                               committed_narrower: bool | None, preview_after: bool | None) -> bool | None:
    """Real-difference rule: no preview in the control, a preview surface at the edge whose rect differs
    from the control capture (and the control is stable), then gone after release with the window snapped."""
    values = (preview_before, preview_during, changed_fraction, noise_fraction, committed_narrower, preview_after)
    if any(value is None for value in values):
        return None
    return bool(not preview_before and preview_during
                and changed_fraction >= SS06_SNAP_MIN_CHANGED_FRACTION
                and noise_fraction <= SS06_SNAP_NOISE_MAX_FRACTION
                and committed_narrower and not preview_after)


def ss06_preview_rect(ctx: Ctx) -> dict | None:
    """Geometry of the mapped (IsViewable) snap-preview surface; the WM keeps it created but unmapped when idle."""
    tree = subprocess.run(["xwininfo", "-display", ctx.display, "-root", "-children"], env=ctx.host_env,
                          capture_output=True, text=True, timeout=20, check=False)
    for window_id in re.findall(r'(0x[0-9a-fA-F]+) "' + re.escape(SNAP_PREVIEW_TITLE) + '"', tree.stdout):
        info = subprocess.run(["xwininfo", "-display", ctx.display, "-id", window_id], env=ctx.host_env,
                              capture_output=True, text=True, timeout=20, check=False).stdout
        if "Map State: IsViewable" not in info:
            continue
        box = {key: re.search(rf"{label}:\s+(-?\d+)", info) for key, label in
               (("x", "Absolute upper-left X"), ("y", "Absolute upper-left Y"), ("width", "Width"), ("height", "Height"))}
        if all(box.values()):
            return {key: int(match.group(1)) for key, match in box.items()}
    return None


def ss06_snap_preview(ctx: Ctx, args: argparse.Namespace, details: dict, observations: dict) -> None:
    """Drag a real Settings window title to the left edge with flamewm-desktop running and compare captures.

    The control is the same drag held at a mid-screen point (identical window position, no snap candidate);
    a second control capture measures capture noise.
    """
    record: dict = {}
    details["snap_preview"] = record
    if Image is None or ctx.display not in (f":{n}" for n in XTEST_DISPLAY_RANGE):
        record["reason"] = "Pillow or an XTEST-range owned display is unavailable"
        return
    settle = min(args.settle_seconds, 1.5)
    settings = ctx.spawn("settings-ss06-snap", [args.settings_bin, "--page", "appearance"], ctx.app_env)
    held = False
    try:
        time.sleep(args.settle_seconds)
        geometry = wait_for_settings_window(ctx, ctx.run / "logs" / "ss06-snap-window-tree.txt")
        if geometry is None:
            record["reason"] = "Settings client was not mapped"
            return
        record["window_before"] = geometry
        title = (geometry["x"] + geometry["width"] // 2, geometry["y"] - 16)
        mid, edge = (640, 400), (2, 400)
        nodrag = ss06_capture(ctx, "ss06-snap-nodrag")
        code, command = send_input(ctx, None, None, drag=f"{title[0]},{title[1]},{mid[0]},{mid[1]}:15:300",
                                   drag_keep=True)
        held = True
        details["commands"].append(command)
        time.sleep(settle)
        control = ss06_capture(ctx, "ss06-snap-control")
        control2 = ss06_capture(ctx, "ss06-snap-control2")
        preview_before = ss06_preview_rect(ctx) is not None
        code2, command = send_input(ctx, None, None, drag=f"{mid[0]},{mid[1]},{edge[0]},{edge[1]}:15:300",
                                    drag_keep=True)
        details["commands"].append(command)
        time.sleep(settle)
        preview_rect = ss06_preview_rect(ctx)
        shot = ss06_capture(ctx, "ss06-snap-preview")
        fraction = noise = None
        if None not in (nodrag, control, control2, shot) and preview_rect is not None:
            rect = (preview_rect["x"], preview_rect["y"], preview_rect["width"], preview_rect["height"])
            fraction = pixel_change(str(control), str(shot), rect).get("fraction")
            noise = pixel_change(str(control), str(control2), rect).get("fraction")
        code3, command = send_input(ctx, None, None, release=True)
        held = False
        details["commands"].append(command)
        time.sleep(settle)
        after = wait_for_settings_window(ctx, ctx.run / "logs" / "ss06-snap-window-after.txt")
        preview_after = ss06_preview_rect(ctx) is not None
        narrower = None if after is None else after["width"] < geometry["width"]
        record.update({"drag_exit": [code, code2, code3], "preview_rect": preview_rect, "window_after": after,
                       "preview_before": preview_before, "preview_after": preview_after,
                       "changed_fraction": fraction, "noise_fraction": noise,
                       "control": str(control), "preview": str(shot), "no_drag": str(nodrag)})
        if (code, code2, code3) == (0, 0, 0):
            observations["snap_preview_consumer"] = ss06_snap_preview_observed(
                preview_before, preview_rect is not None, fraction, noise, narrower, preview_after)
    finally:
        if held:
            send_input(ctx, None, None, release=True)
        ctx.stop(settings)


def run_ss06_desktop(ctx: Ctx, args: argparse.Namespace) -> dict:
    observations: dict[str, bool | None] = {
        "desktop_values_snapshot": None, "settings_persisted_after_wm_restart": None,
        "desktop_visual_consumer": None, "snap_preview_consumer": None,
    }
    details: dict = {"observations": observations,
                     "paths_exercised": ["Settings.Apply(stickyNotesEnabled, DesktopSelectionFillOpacity, WindowSnapPreviewFillOpacity)",
                                         "flamewm-desktop sticky-note visibility and selection fill pixels (real captures)",
                                         "XTEST drag of a real Settings title to the left snap edge: preview surface and pixels vs control"],
                     "paths_not_exercised": ["WindowSnapPreviewFillOpacity effect on preview pixels: the WM preview is "
                                             "border-only and ignores opacity by design (snap_preview.rs set_live_opacity)"],
                     "commands": []}
    clients: dict[str, Owned] = {}
    initial, command = dbus_settings_snapshot(ctx)
    details["commands"].append(command)
    revision = gvariant_field_u64(initial or "", "revision")
    sticky = gvariant_field_bool(initial or "", "stickyNotesEnabled")
    selection = gvariant_field_uint(initial or "", "DesktopSelectionFillOpacity")
    snap = gvariant_field_uint(initial or "", "WindowSnapPreviewFillOpacity")
    details["before"] = {"revision": revision, "stickyNotesEnabled": sticky,
                         "selectionOpacity": selection, "snapOpacity": snap}
    if initial is None or revision is None or sticky is None or selection is None or snap is None:
        status, reason = assess_ss06(observations)
        entry = result_entry("SS06", status, "authoritative Desktop settings snapshot is missing or malformed; " + reason)
        entry["details"] = details
        return entry
    next_selection = next_desktop_opacity(selection)
    next_snap = next_desktop_opacity(snap)
    visual: dict = {}
    base_path: Path | None = None
    details["visual"] = visual
    settle = min(args.settle_seconds, 1.5)
    if Image is not None and os.access(args.desktop_bin, os.X_OK) and sticky:
        ss06_seed_fixtures(ctx)
        clients = start_session_clients(ctx, args, ("desktop",), "ss06")
        base_path = ss06_capture(ctx, "ss06-base")
        visual["alive"] = clients_alive(clients)
        visual["note_visible_base"] = ss06_note_visible(base_path)
        sel0_path, red0 = ss06_select_and_capture(ctx, "ss06-selected", settle, base_path)
        again = ss06_capture(ctx, "ss06-selected-unchanged")
        visual["selection_red_before"] = red0
        visual["selection_red_unchanged_capture"] = ss06_selection_red(base_path, again)
        visual["expected_red_before"] = round(239 * selection / 100)
        unrelated = apply_and_read_settings(ctx, revision, {
            "surfaceOpacity": "uint32 " + str(67 if gvariant_field_uint(initial, "surfaceOpacity") != 67 else 68)}, details)
        time.sleep(settle)
        after_unrelated = ss06_capture(ctx, "ss06-after-unrelated")
        visual["unrelated_applied"] = unrelated is not None
        visual["selection_red_after_unrelated"] = ss06_selection_red(base_path, after_unrelated)
        visual["note_visible_after_unrelated"] = ss06_note_visible(after_unrelated)
        if unrelated and isinstance(unrelated.get("revision"), int):
            revision = unrelated["revision"]
        visual["base_path"] = str(base_path)
    elif not sticky:
        visual["reason"] = "stickyNotesEnabled starts false; no visible-note control possible"
    else:
        visual["reason"] = "Pillow or flamewm-desktop binary unavailable"
    changes = {"stickyNotesEnabled": "false" if sticky else "true",
               "DesktopSelectionFillOpacity": f"uint32 {next_selection}",
               "WindowSnapPreviewFillOpacity": f"uint32 {next_snap}"}
    applied = apply_and_read_settings(ctx, revision, changes, details)
    text = (applied or {}).get("text") or ""
    new_revision = (applied or {}).get("revision")
    updated = {"stickyNotesEnabled": gvariant_field_bool(text, "stickyNotesEnabled"),
               "selectionOpacity": gvariant_field_uint(text, "DesktopSelectionFillOpacity"),
               "snapOpacity": gvariant_field_uint(text, "WindowSnapPreviewFillOpacity")}
    details["after_apply"] = {"revision": new_revision, **updated}
    expected = {"stickyNotesEnabled": not sticky, "selectionOpacity": next_selection, "snapOpacity": next_snap}
    values_ok = updated == expected and isinstance(new_revision, int) and new_revision > revision
    observations["desktop_values_snapshot"] = values_ok if applied else False
    if values_ok and visual.get("selection_red_before") is not None:
        time.sleep(settle)
        shot = ss06_capture(ctx, "ss06-after-apply")
        red_apply = ss06_selection_red(base_path, shot)
        if red_apply is None:  # settings refresh may drop the selection; select again, then measure
            shot, red_apply = ss06_select_and_capture(ctx, "ss06-after-apply-reselect", settle, base_path)
        visual["note_visible_after_apply"] = ss06_note_visible(shot)
        visual["selection_red_after_apply"] = red_apply
        visual["expected_red_after_apply"] = round(239 * next_selection / 100)
        # Distinguishable fill: 21% vs 20% differs by ~2 red levels, so also drive a 50% fill.
        high = apply_and_read_settings(ctx, new_revision, {"DesktopSelectionFillOpacity": "uint32 50"}, details)
        time.sleep(settle)
        high_shot = ss06_capture(ctx, "ss06-selection-50")
        red_high = ss06_selection_red(base_path, high_shot)
        if red_high is None:
            high_shot, red_high = ss06_select_and_capture(ctx, "ss06-selection-50-reselect", settle, base_path)
        visual["selection_red_at_50"] = red_high
        visual["expected_red_at_50"] = round(239 * 50 / 100)
        restore_revision = (high or {}).get("revision")
        restored = apply_and_read_settings(
            ctx, restore_revision if isinstance(restore_revision, int) else new_revision,
            {"DesktopSelectionFillOpacity": f"uint32 {next_selection}"}, details)
        visual["selection_restored"] = restored is not None
        controls = (
            visual.get("alive") is True and clients_alive(clients)
            and visual.get("note_visible_base") is True
            and visual.get("selection_red_unchanged_capture") == visual["selection_red_before"]
            and visual.get("note_visible_after_unrelated") is True
            and visual.get("selection_red_after_unrelated") is not None
            and abs(visual["selection_red_after_unrelated"] - visual["selection_red_before"]) <= 2
            and abs(visual["selection_red_before"] - round(239 * selection / 100)) <= 12)
        visual["controls_ok"] = controls
        if controls and red_apply is not None and red_high is not None and high is not None:
            observations["desktop_visual_consumer"] = bool(
                visual["note_visible_after_apply"] is False
                and abs(red_apply - visual["expected_red_after_apply"]) <= 12
                and abs(red_high - visual["expected_red_at_50"]) <= 12
                and red_high - visual["selection_red_before"] >= 40)
    if values_ok and clients and clients_alive(clients):
        ss06_snap_preview(ctx, args, details, observations)
    stop_session_clients(ctx, clients)
    if values_ok:
        restarted = restart_owned_wm(ctx, args, "flamewm-ss06-persistence-restart")
        details["wm_restarted"] = restarted
        persisted_text, persisted_command = dbus_settings_snapshot(ctx) if restarted else (None, "restart failed")
        details["commands"].append(persisted_command)
        persisted = {
            "stickyNotesEnabled": gvariant_field_bool(persisted_text or "", "stickyNotesEnabled"),
            "selectionOpacity": gvariant_field_uint(persisted_text or "", "DesktopSelectionFillOpacity"),
            "snapOpacity": gvariant_field_uint(persisted_text or "", "WindowSnapPreviewFillOpacity"),
        }
        details["after_restart"] = persisted
        observations["settings_persisted_after_wm_restart"] = persisted == expected if restarted else False
    status, reason = assess_ss06(observations)
    entry = result_entry("SS06", status, reason, commands=details["commands"])
    entry["details"] = details
    return entry


def run_ss07(ctx: Ctx, args: argparse.Namespace) -> dict:
    observations: dict[str, bool | None] = {
        "settings_snapshot_values": None, "panel_edge_and_size_snapshot": None,
        "taskbar_visual_consumer": None, "settings_persistence_after_restart": None,
    }
    details: dict = {"observations": observations,
                     "paths_exercised": ["Settings.Apply(taskbarHeight/surfaceOpacity/surfaceColor)", "Panels.SetEdge/GetSnapshot"],
                     "paths_not_exercised": [],
                     "commands": []}
    clients: dict[str, Owned] = {}
    visual: dict = {}
    details["visual"] = visual
    settings_before, command = dbus_settings_snapshot(ctx)
    details["commands"].append(command)
    panels_before_text, command = dbus_panels_snapshot(ctx)
    details["commands"].append(command)
    settings_revision = gvariant_field_u64(settings_before or "", "revision")
    panels_before = parse_panels_snapshot(panels_before_text or "")
    details["panels_before"] = panels_before
    if settings_before is None or settings_revision is None or panels_before is None:
        status, reason = assess_ss07(observations)
        entry = result_entry("SS07", status, "settings or Panels snapshot unavailable/malformed; " + reason)
        entry["details"] = details
        return entry
    if not panels_before["panels"]:
        details["reason"] = "private WM reports no panel outputs; host outputs are never substituted"
        status, reason = assess_ss07(observations)
        entry = result_entry("SS07", status, details["reason"] + "; " + reason)
        entry["details"] = details
        return entry
    panel = panels_before["panels"][0]
    output_key = panel["output"]
    current_height = gvariant_field_uint(settings_before, "taskbarHeight") or panel["size"]
    current_opacity = gvariant_field_uint(settings_before, "surfaceOpacity")
    next_height = 53 if current_height != 53 else 54
    next_opacity = 67 if current_opacity != 67 else 68
    color = "#284F77" if gvariant_field_string(settings_before, "surfaceColor") != "#284F77" else "#4A3828"
    shell_ok = Image is not None and os.access(args.shell_bin, os.X_OK)
    if shell_ok:
        clients = start_session_clients(ctx, args, ("shell",), "ss07")
        first = taskbar_measure(ctx, "ss07-before-a")
        second = taskbar_measure(ctx, "ss07-before-b")
        visual["before"] = first
        visual["unchanged_capture_equal"] = bool(
            first and second and first["band_digest"] and first["band_digest"] == second["band_digest"]
            and first["extent"] == second["extent"])
        unrelated = apply_and_read_settings(ctx, settings_revision, {
            "DesktopSelectionFillOpacity": "uint32 " + str(
                21 if gvariant_field_uint(settings_before, "DesktopSelectionFillOpacity") != 21 else 20)}, details)
        time.sleep(min(args.settle_seconds, 1.5))
        after_unrelated = taskbar_measure(ctx, "ss07-after-unrelated")
        visual["after_unrelated"] = after_unrelated
        visual["unrelated_leaves_taskbar_unchanged"] = bool(
            unrelated and first and after_unrelated and first["band_digest"]
            and first["band_digest"] == after_unrelated["band_digest"]
            and first["extent"] == after_unrelated["extent"])
        if unrelated and isinstance(unrelated.get("revision"), int):
            settings_revision = unrelated["revision"]
    else:
        visual["reason"] = "Pillow or flamewm-shell binary unavailable"
    changes = {"taskbarHeight": f"uint32 {next_height}",
               "surfaceOpacity": f"uint32 {next_opacity}",
               "surfaceColor": "'" + color + "'"}
    applied = apply_and_read_settings(ctx, settings_revision, changes, details)
    text = (applied or {}).get("text") or ""
    new_settings_revision = (applied or {}).get("revision")
    settings_ok = (
        gvariant_field_uint(text, "taskbarHeight") == next_height
        and gvariant_field_uint(text, "surfaceOpacity") == next_opacity
        and gvariant_field_string(text, "surfaceColor") == color
        and isinstance(new_settings_revision, int) and new_settings_revision > settings_revision
    ) if applied else False
    observations["settings_snapshot_values"] = settings_ok
    details["settings_after"] = {"revision": new_settings_revision, "taskbarHeight": gvariant_field_uint(text, "taskbarHeight"),
                                 "surfaceOpacity": gvariant_field_uint(text, "surfaceOpacity"),
                                 "surfaceColor": gvariant_field_string(text, "surfaceColor")}
    if shell_ok and settings_ok:
        applied_measure = taskbar_measure_settled(ctx, "ss07-after-apply", (visual.get("before") or {}).get("edge") or "bottom",
                                                  next_height, color, next_opacity, max(10.0, args.settle_seconds * 4))
        visual["after_apply"] = applied_measure
        visual["expected"] = {"height": next_height, "color": color, "opacity": next_opacity}
        visual["after_apply_matches"] = taskbar_matches(
            applied_measure, (visual.get("before") or {}).get("edge") or "bottom", next_height, color, next_opacity)
    panels_current_text, command = dbus_panels_snapshot(ctx)
    details["commands"].append(command)
    panels_current = parse_panels_snapshot(panels_current_text or "")
    if panels_current is None:
        observations["panel_edge_and_size_snapshot"] = None
        details["reason"] = "post-settings Panels snapshot is unavailable or malformed"
    else:
        current_panel = next((item for item in panels_current["panels"] if item["output"] == output_key), None)
        if current_panel is None:
            observations["panel_edge_and_size_snapshot"] = False
        else:
            next_edge = "top" if current_panel["edge"] != "top" else "bottom"
            panel_set_argv = ["gdbus", "call", "--session", "--dest", "com.arkflame.FlameWM1",
                             "--object-path", "/com/arkflame/FlameWM1", "--method",
                             "com.arkflame.FlameWM1.Panels.SetEdge", "'" + output_key + "'",
                             "'" + next_edge + "'", f"uint64 {panels_current['revision']}"]
            result = subprocess.run(panel_set_argv, env=ctx.app_env, capture_output=True, text=True,
                                    timeout=10, check=False)
            details["commands"].append(shell(panel_set_argv))
            details["panel_set_edge_reply"] = result.stdout if result.returncode == 0 else result.stderr.strip()
            panels_after_text, command = dbus_panels_snapshot(ctx)
            details["commands"].append(command)
            panels_after = parse_panels_snapshot(panels_after_text or "")
            details["panels_after"] = panels_after
            updated_panel = next((item for item in (panels_after or {}).get("panels", [])
                                  if item["output"] == output_key), None)
            observations["panel_edge_and_size_snapshot"] = bool(
                result.returncode == 0 and updated_panel is not None
                and updated_panel["edge"] == next_edge and updated_panel["size"] == next_height
                and (panels_after or {}).get("revision", 0) > panels_before["revision"]
            )
            details["selected_output"] = output_key
            details["expected_panel_edge"] = next_edge
            if shell_ok and observations["panel_edge_and_size_snapshot"] is True and settings_ok:
                time.sleep(min(args.settle_seconds, 1.5))
                moved = taskbar_measure_settled(ctx, "ss07-after-edge", next_edge, next_height, color, next_opacity,
                                                max(4.0, args.settle_seconds * 2.0))
                visual["after_edge"] = moved
                visual["after_edge_matches"] = taskbar_matches(moved, next_edge, next_height, color, next_opacity)
                controls = bool(visual.get("unchanged_capture_equal") and visual.get("unrelated_leaves_taskbar_unchanged")
                                and clients_alive(clients) and (visual.get("before") or {}).get("extent"))
                visual["controls_ok"] = controls
                if controls:
                    observations["taskbar_visual_consumer"] = bool(
                        visual.get("after_apply_matches") and visual["after_edge_matches"]
                        and (visual["before"]["inside"] != (visual.get("after_apply") or {}).get("inside")
                             or visual["before"]["extent"] != (visual.get("after_apply") or {}).get("extent")))
                restarted = restart_owned_wm(ctx, args, "flamewm-ss07-persistence-restart")
                visual["wm_restarted"] = restarted
                if restarted:
                    time.sleep(args.settle_seconds)
                    # Evidence only: the Shell that survived the WM restart vs the WM's Panels snapshot.
                    visual["surviving_shell_after_restart"] = taskbar_measure(ctx, "ss07-surviving-shell")
                    stop_session_clients(ctx, clients)
                    clients = start_session_clients(ctx, args, ("shell",), "ss07-restart")
                    persisted_text, command = dbus_settings_snapshot(ctx)
                    details["commands"].append(command)
                    persisted_panels_text, command = dbus_panels_snapshot(ctx)
                    details["commands"].append(command)
                    persisted_panels = parse_panels_snapshot(persisted_panels_text or "")
                    persisted_panel = next((item for item in (persisted_panels or {}).get("panels", [])
                                            if item["output"] == output_key), None)
                    values_persisted = (
                        gvariant_field_uint(persisted_text or "", "taskbarHeight") == next_height
                        and gvariant_field_uint(persisted_text or "", "surfaceOpacity") == next_opacity
                        and gvariant_field_string(persisted_text or "", "surfaceColor") == color)
                    restarted_measure = taskbar_measure(ctx, "ss07-after-restart")
                    visual["after_restart"] = restarted_measure
                    visual["panel_after_restart"] = persisted_panel
                    visual["panel_edge_persisted"] = bool(persisted_panel and persisted_panel["edge"] == next_edge)
                    observations["settings_persistence_after_restart"] = bool(
                        values_persisted and persisted_panel is not None and clients_alive(clients)
                        and taskbar_matches(restarted_measure, persisted_panel["edge"], next_height, color, next_opacity))
    stop_session_clients(ctx, clients)
    status, reason = assess_ss07(observations)
    entry = result_entry("SS07", status, reason, commands=details["commands"])
    entry["details"] = details
    return entry


def run_ss10(ctx: Ctx, args: argparse.Namespace) -> dict:
    """Click the 125 Displays scale button; PASS needs the snapshot change and the value after a WM restart."""
    observations: dict[str, bool | None] = {"scale_changed_snapshot": None, "mode_list_unchanged": None,
                                           "scale_persisted_after_restart": None}
    details: dict = {"observations": observations, "commands": [], "target_scale": SS10_TARGET_SCALE,
                     "paths_exercised": ["Displays page wheel scroll (only when modes are listed)",
                                         "Displays page scale button click", "Displays.GetSnapshot output scale",
                                         "owned WM restart"]}

    def finish() -> dict:
        status, reason = assess_ss10(observations)
        entry = result_entry("SS10", status, reason, commands=details["commands"])
        entry["details"] = details
        return entry

    if not args.ss10_scale_click:
        return result_entry("SS10", STATUS_NOT_RUN,
                            "SS10 needs --ss10-scale-click X,Y on the 125 scale button of the Displays page (960x640 client)")
    if ctx.display not in (f":{n}" for n in XTEST_DISPLAY_RANGE):
        return result_entry("SS10", STATUS_NOT_RUN, "SS10 XTEST requires an owned display in :97..:119")
    if shutil.which("gdbus") is None:
        return result_entry("SS10", STATUS_NOT_RUN, "SS10 needs gdbus for the authoritative Displays snapshot")

    settings = ctx.spawn("settings-ss10-displays", [args.settings_bin, "--page", "displays", "--window-size", f"{SS10_CLIENT[0]}x{SS10_CLIENT[1]}"], ctx.app_env)
    details["commands"].append(shell([args.settings_bin, "--page", "displays", "--window-size", f"{SS10_CLIENT[0]}x{SS10_CLIENT[1]}"]))
    time.sleep(args.settle_seconds)
    geometry = wait_for_settings_window(ctx, ctx.run / "logs" / "ss10-window-tree.txt")
    details["window"] = geometry
    before_text, before_command = dbus_displays_snapshot(ctx)
    details["commands"].append(before_command)
    before_scales = display_output_scales(before_text or "")
    if (settings.proc.poll() is not None or geometry is None
            or (geometry["width"], geometry["height"]) != SS10_CLIENT or before_scales is None):
        ctx.stop(settings)
        details["reason"] = "Displays client 960x640 or authoritative output scale snapshot unavailable"
        return finish()
    output_id = next(iter(before_scales))
    before_scale = before_scales[output_id]
    details["before"] = {"output": output_id, "scale": before_scale}
    if before_scale == SS10_TARGET_SCALE:
        ctx.stop(settings)
        details["reason"] = "initial output scale is already 125; no change to observe"
        return finish()

    # Each listed mode pushes the scale row below the fold, so an unscrolled click would hit a mode button.
    # With modes listed, the page is wheel-scrolled first and the caller supplies the scrolled click point.
    mode_count = display_output_mode_count(before_text or "", output_id)
    details["before"]["mode_count"] = mode_count
    if mode_count is None or (mode_count and not (args.ss10_wheel_steps and args.ss10_scrolled_click)):
        ctx.stop(settings)
        if mode_count is None:
            details["reason"] = "output mode list is unparsable; not clicked"
        else:
            details["reason"] = (f"output lists {mode_count} modes; the scale row is below the fold at 960x640 and "
                                 "needs --ss10-wheel-steps and --ss10-scrolled-click after scrolling; not clicked")
        return finish()

    if mode_count:
        pointer = settings_relative_point(ctx, geometry, SS10_WHEEL_POINTER)
        scroll_code, scroll_command = send_input(ctx, None, None, move=pointer, wheel=f"down:{args.ss10_wheel_steps}")
        details["commands"].append(scroll_command)
        details["scroll"] = {"direction": "down", "steps": args.ss10_wheel_steps, "pointer": pointer,
                             "exit_code": scroll_code}
        if scroll_code != 0:
            ctx.stop(settings)
            details["reason"] = f"wheel scroll input failed with exit {scroll_code}; not clicked"
            return finish()
        time.sleep(min(max(args.settle_seconds, 0.25), 1.0))
        point = tuple(int(part) for part in args.ss10_scrolled_click.split(","))
    else:
        point = tuple(int(part) for part in args.ss10_scale_click.split(","))
    click = settings_relative_point(ctx, geometry, point)
    code, click_command = send_input(ctx, click, None)
    details["commands"].append(click_command)
    time.sleep(min(max(args.settle_seconds, 0.25), 1.0))
    after_text, after_command = dbus_displays_snapshot(ctx)
    details["commands"].append(after_command)
    after_scales = display_output_scales(after_text or "")
    after_mode_count = display_output_mode_count(after_text or "", output_id)
    details["after"] = {"output": output_id, "scale": (after_scales or {}).get(output_id), "mode_count": after_mode_count,
                        "click": click}
    if after_text is not None:
        observations["scale_changed_snapshot"] = ss10_scale_applied(code, after_scales, output_id)
        observations["mode_list_unchanged"] = after_mode_count == mode_count
    ctx.stop(settings)

    if observations["scale_changed_snapshot"] is True and restart_owned_wm(ctx, args, "wm-ss10-restart"):
        persisted_text, persisted_command = dbus_displays_snapshot(ctx)
        details["commands"].extend(["restart owned WM", persisted_command])
        if persisted_text is not None:
            persisted_scales = display_output_scales(persisted_text) or {}
            observations["scale_persisted_after_restart"] = persisted_scales.get(output_id) == SS10_TARGET_SCALE
    elif observations["scale_changed_snapshot"] is True:
        details["reason"] = "owned WM could not be restarted to verify persistence"
    return finish()


def run_ss11(ctx: Ctx, args: argparse.Namespace) -> dict:
    """Click the Fonts family and size controls; PASS needs both snapshot changes and persistence after a restart."""
    observations: dict[str, bool | None] = {"family_changed_snapshot": None, "size_changed_snapshot": None,
                                           "fonts_persisted_after_restart": None}
    details: dict = {"observations": observations, "commands": [],
                     "paths_exercised": ["Fonts page family cycle click", "Fonts page text size + click",
                                         "Settings.GetSnapshot fontFamily/fontSizeOffset", "owned WM restart"]}

    def finish() -> dict:
        status, reason = assess_ss11(observations)
        entry = result_entry("SS11", status, reason, commands=details["commands"])
        entry["details"] = details
        return entry

    if not (args.ss11_family_click and args.ss11_size_click):
        return result_entry("SS11", STATUS_NOT_RUN,
                            "SS11 needs --ss11-family-click X,Y and --ss11-size-click X,Y on the Fonts page (820x540 client)")
    if ctx.display not in (f":{n}" for n in XTEST_DISPLAY_RANGE):
        return result_entry("SS11", STATUS_NOT_RUN, "SS11 XTEST requires an owned display in :97..:119")
    if shutil.which("gdbus") is None:
        return result_entry("SS11", STATUS_NOT_RUN, "SS11 needs gdbus for the authoritative Settings snapshot")

    settings = ctx.spawn("settings-ss11-fonts", [args.settings_bin, "--page", "fonts", "--window-size", "820x540"], ctx.app_env)
    details["commands"].append(shell([args.settings_bin, "--page", "fonts", "--window-size", "820x540"]))
    time.sleep(args.settle_seconds)
    geometry = wait_for_settings_window(ctx, ctx.run / "logs" / "ss11-window-tree.txt")
    details["window"] = geometry
    before_text, before_command = dbus_settings_snapshot(ctx)
    details["commands"].append(before_command)
    before_family = gvariant_field_string(before_text or "", "fontFamily")
    before_size = gvariant_field_int(before_text or "", "fontSizeOffset")
    details["before"] = {"fontFamily": before_family, "fontSizeOffset": before_size}
    if (settings.proc.poll() is not None or geometry is None
            or (geometry["width"], geometry["height"]) != (820, 540)
            or before_family is None or before_size is None):
        ctx.stop(settings)
        details["reason"] = "Fonts client 820x540 or authoritative fonts snapshot unavailable"
        return finish()
    if before_size >= 4:
        ctx.stop(settings)
        details["reason"] = "text size offset is already at its maximum (4); no size step to observe"
        return finish()

    family_point = tuple(int(part) for part in args.ss11_family_click.split(","))
    family_code, family_command = send_input(ctx, settings_relative_point(ctx, geometry, family_point), None)
    details["commands"].append(family_command)
    time.sleep(min(max(args.settle_seconds, 0.25), 1.0))
    mid_text, mid_command = dbus_settings_snapshot(ctx)
    details["commands"].append(mid_command)
    mid_family = gvariant_field_string(mid_text or "", "fontFamily")
    if mid_text is not None:
        observations["family_changed_snapshot"] = ss11_family_applied(family_code, before_family, mid_family)

    size_point = tuple(int(part) for part in args.ss11_size_click.split(","))
    size_code, size_command = send_input(ctx, settings_relative_point(ctx, geometry, size_point), None)
    details["commands"].append(size_command)
    time.sleep(min(max(args.settle_seconds, 0.25), 1.0))
    after_text, after_command = dbus_settings_snapshot(ctx)
    details["commands"].append(after_command)
    after_family = gvariant_field_string(after_text or "", "fontFamily")
    after_size = gvariant_field_int(after_text or "", "fontSizeOffset")
    details["after"] = {"fontFamily": after_family, "fontSizeOffset": after_size}
    if after_text is not None:
        observations["size_changed_snapshot"] = ss11_size_applied(
            size_code, before_size, after_size, mid_family, after_family)
    ctx.stop(settings)

    if (observations["family_changed_snapshot"] is True and observations["size_changed_snapshot"] is True
            and restart_owned_wm(ctx, args, "wm-ss11-restart")):
        persisted_text, persisted_command = dbus_settings_snapshot(ctx)
        details["commands"].extend(["restart owned WM", persisted_command])
        if persisted_text is not None:
            observations["fonts_persisted_after_restart"] = ss11_fonts_persisted(
                persisted_text, after_family, after_size)
    return finish()


ABOUT_VERSION_CROP_W, ABOUT_VERSION_CROP_H, ABOUT_VERSION_CROP_Y = 120, 22, 213
ABOUT_TAGLINE_CROP = (-180, 180, 360, 28)  # relative to content centre / client y; control region


def about_content_centre(width: int) -> int:
    """Client-relative x centre of the About content column, measured natively (104 + W/2)."""
    return 104 + width // 2


def ss13_version_ocr_matches(text: str, version: str) -> bool:
    """Token 'version' (tolerating o/a/0 confusion only) followed by the exact dotted number."""
    normalized = re.sub(r"[^a-z0-9.]+", " ", text.lower()).strip()
    pattern = r"(?<![a-z0-9])versi[oa0]n\s+" + re.escape(version) + r"(?![0-9.])"
    return re.search(pattern, normalized) is not None


def ocr_text(tesseract: str, image: Path) -> tuple[str | None, list[str]]:
    """Upscale 4x (small grey UI text) and OCR one line; None when a tool fails."""
    big = image.with_name(image.stem + "-x4.png")
    resize = ["convert", str(image), "-resize", "400%", str(big)]
    ocr = [tesseract, str(big), "stdout", "--psm", "7"]
    commands = [shell(resize), shell(ocr)]
    if subprocess.run(resize, capture_output=True, timeout=60, check=False).returncode != 0:
        return None, commands
    result = subprocess.run(ocr, capture_output=True, text=True, timeout=30, check=False)
    return (result.stdout.strip() if result.returncode == 0 else None), commands


def ss13_live_version(shot: dict, size: tuple[int, int], tesseract: str, version: str,
                      out_dir: Path) -> dict:
    """OCR the about-version region and a tagline control region from the real native screenshot."""
    width, height = size
    window = shot.get("window") or {}
    record: dict = {"size": f"{width}x{height}", "observed": None}
    if (window.get("width"), window.get("height")) != size or not shot.get("captured"):
        record["refused"] = "measured client geometry differs from the calibrated size"
        return record
    x0, y0 = window["x"], window["y"]
    centre = about_content_centre(width)
    regions = {
        "version": (x0 + centre - ABOUT_VERSION_CROP_W // 2, y0 + ABOUT_VERSION_CROP_Y,
                    ABOUT_VERSION_CROP_W, ABOUT_VERSION_CROP_H),
        "tagline_control": (x0 + centre + ABOUT_TAGLINE_CROP[0], y0 + ABOUT_TAGLINE_CROP[1],
                            ABOUT_TAGLINE_CROP[2], ABOUT_TAGLINE_CROP[3]),
    }
    texts: dict[str, str | None] = {}
    record["commands"] = []
    for name, rect in regions.items():
        crop_path = out_dir / f"ss13-{width}x{height}-{name}.png"
        ok, command = crop(Path(shot["shot"]), rect, crop_path)
        record["commands"].append(command)
        if not ok:
            return record
        texts[name], commands = ocr_text(tesseract, crop_path)
        record["commands"].extend(commands)
        record[f"{name}_crop"] = str(crop_path)
    record["version_text"], record["control_text"] = texts["version"], texts["tagline_control"]
    if texts["version"] is None or texts["tagline_control"] is None:
        return record
    record["observed"] = (ss13_version_ocr_matches(texts["version"], version)
                          and not ss13_version_ocr_matches(texts["tagline_control"], version)
                          and not ss13_version_ocr_matches(texts["version"], "0.0.8"))
    return record


def run_ss13(ctx: Ctx, args: argparse.Namespace) -> dict:
    """Open About at each matrix size; PASS needs live nonblank renders and the Rust exact-version test."""
    observations: dict[str, bool | None] = {f"about_render_{width}x{height}": None for width, height in SIZES}
    observations["version_matches_flamewm_version"] = None
    observations["version_live_ocr_matches"] = None
    details: dict = {"observations": observations, "commands": [], "renders": [], "live_ocr": [],
                     "paths_exercised": ["flamewm-settings --page about at 760x500, 820x540, 960x640",
                                         "xwininfo client geometry",
                                         "tesseract OCR of the about-version region of the native screenshot",
                                         f"cargo test -p flamewm-settings {ABOUT_VERSION_TEST}"]}
    shots = []
    for width, height in SIZES:
        shot = page_shot(ctx, args, "about", f"ss13-{width}x{height}", (width, height))
        shots.append(shot)
        details["commands"].extend(shot["commands"])
        details["renders"].append({"size": f"{width}x{height}", "alive": shot["alive"], "captured": shot["captured"],
                                   "nonblank": shot["nonblank"], "window": shot["window"]})
        observations[f"about_render_{width}x{height}"] = exact_window_size(shot, (width, height))

    tesseract = shutil.which("tesseract")
    if tesseract is None:
        details["live_ocr_reason"] = "tesseract not on PATH; live About version read not performed"
    else:
        version = workspace_version()
        reads = [ss13_live_version(shot, size, tesseract, version, ctx.run / "shots")
                 for shot, size in zip(shots, SIZES)]
        details["live_ocr"] = reads
        for read in reads:
            details["commands"].extend(read.get("commands", []))
        if any(read["observed"] is False for read in reads):
            observations["version_live_ocr_matches"] = False
        elif all(read["observed"] is True for read in reads):
            observations["version_live_ocr_matches"] = True

    version_matches, version_record = run_about_version_test()
    observations["version_matches_flamewm_version"] = version_matches
    details["version_test"] = version_record
    status, reason = assess_ss13(observations)
    crops = [read["version_crop"] for read in details["live_ocr"] if "version_crop" in read]
    entry = result_entry("SS13", status, reason, [shot["shot"] for shot in shots] + crops, details["commands"])
    entry["details"] = details
    return entry


def result_entry(sid: str, status: str, reason: str = "", evidence: list | None = None, commands: list | None = None) -> dict:
    if status not in STATUSES:
        raise ValueError(f"unknown status {status!r}")
    return {
        "id": sid,
        "report_label": sid if sid in CANONICAL_IDS else LEGACY_PREFIX + sid,
        "title": TITLES[sid],
        "status": status,
        "reason": reason,
        "evidence": evidence or [],
        "command": " && ".join(commands or []) or "none",
    }


def blocked(sid: str, reason: str) -> dict:
    return result_entry(sid, STATUS_ENV_BLOCKED, reason)


def block_runner_ids(results: dict[str, dict], reason: str, commands: list | None = None) -> None:
    for sid in RUNNER_IDS:
        results[sid] = result_entry(sid, STATUS_ENV_BLOCKED, reason, commands=commands)


def compose_canonical(results: dict[str, dict]) -> list[dict]:
    """One entry per canonical ID. A canonical ID is PASS only when results holds a PASS for it directly.

    Legacy sub-scenarios mapped to a canonical ID can only FAIL it or leave it NOT_RUN (partial coverage).
    """
    composed = []
    for sid, label in CANONICAL_MATRIX:
        if sid in results:
            composed.append(results[sid])
            continue
        mapped = [results[lid] for lid, _, canon in LEGACY_SUBSCENARIOS if canon == sid and lid in results]
        note = CANONICAL_NOTES.get(sid, "")
        evidence = [path for entry in mapped for path in entry["evidence"]]
        commands = [entry["command"] for entry in mapped if entry["command"] != "none"]
        if any(entry["status"] == STATUS_FAIL for entry in mapped):
            failed = ", ".join(entry["report_label"] for entry in mapped if entry["status"] == STATUS_FAIL)
            composed.append(result_entry(sid, STATUS_FAIL, f"legacy sub-scenario failed: {failed}", evidence, commands))
            continue
        measured = "; ".join(f"{entry['report_label']}={entry['status']}" for entry in mapped)
        reason = f"partial coverage ({measured}); " if mapped else ""
        reason += f"no runner implementation for '{label}' in this harness"
        reason += f"; {note}" if note else ""
        composed.append(result_entry(sid, STATUS_NOT_RUN, reason, evidence, commands))
    return composed


def compose_document(results: dict[str, dict]) -> dict:
    """Pure: canonical matrix, legacy reports, status summary, verdict and exit code.

    The verdict is never READY. ALL_SCENARIOS_PASS means every canonical ID passed, and readiness
    is still decided by the release gates. Exit code 0 is returned for no FAIL, so 0 is not readiness.
    """
    scenarios = compose_canonical(results)
    legacy = [results[lid] for lid in LEGACY_IDS if lid in results]
    summary = {status: sum(1 for entry in scenarios if entry["status"] == status) for status in STATUSES}
    any_fail = any(entry["status"] == STATUS_FAIL for entry in scenarios + legacy)
    if any_fail:
        verdict, exit_code = "FAIL", 1
    elif summary[STATUS_PASS] == len(CANONICAL_MATRIX):
        verdict, exit_code = "ALL_SCENARIOS_PASS", 0
    else:
        verdict, exit_code = "INCOMPLETE", 0
    return {
        "schema": SCHEMA,
        "schema_version": SCHEMA_VERSION,
        "scenarios": scenarios,
        "legacy_subscenarios": legacy,
        "summary": summary,
        "verdict": verdict,
        "exit_code": exit_code,
        "readiness": "not decided by this runner; see verdict and the release gates (SS16-SS18)",
    }

def host_blockers(args: argparse.Namespace) -> list[str]:
    reasons = []
    if not os.environ.get("DISPLAY"):
        reasons.append("host DISPLAY is unset; Xephyr needs the host X server")
    for tool in ("Xephyr", "xdpyinfo", "xwininfo", "import", "identify"):
        if shutil.which(tool) is None:
            reasons.append(f"{tool} not found on PATH")
    if shutil.which("dbus-run-session") is None:
        reasons.append("dbus-run-session not found on PATH")
    for label, path in (("WM", args.wm_bin), ("settings", args.settings_bin)):
        if not os.access(path, os.X_OK):
            reasons.append(f"{label} binary missing or not executable: {path}")
    if not (ROOT / "tools" / "flamewm_xtest.py").is_file():
        reasons.append("tools/flamewm_xtest.py missing; XTEST input unavailable")
    return reasons


def crop(src: Path, rect: tuple[int, int, int, int], out: Path) -> tuple[bool, str]:
    x, y, width, height = rect
    if HAVE_PIL:
        try:
            with Image.open(src) as image:
                image.convert("RGB").crop((x, y, x + width, y + height)).save(str(out))
        except OSError as error:
            return False, f"PIL crop failed: {error}"
        return out.is_file(), f"PIL crop {x},{y},{width},{height} of {src.name} -> {out.name}"
    argv = ["convert", str(src), "-crop", f"{width}x{height}+{x}+{y}", "+repage", str(out)]
    result = subprocess.run(argv, capture_output=True, text=True, timeout=60, check=False)
    return result.returncode == 0 and out.is_file(), shell(argv)


def pixel_change(before: str, after: str, rect: tuple[int, int, int, int]) -> dict:
    """Fraction of changed pixels in rect (or the supplied crop when rect starts at 0,0)."""
    x, y, width, height = rect
    if x < 0 or y < 0 or width <= 0 or height <= 0:
        return {"metric": "pixel difference", "error": "invalid crop rectangle"}
    if HAVE_PIL:
        metric = "PIL ImageChops.difference: changed pixels / (W*H)"
        try:
            with Image.open(before) as first, Image.open(after) as second:
                left, right = first.convert("RGB"), second.convert("RGB")
        except OSError as error:
            return {"metric": metric, "error": f"cannot open crops: {error}"}
        if left.size != right.size:
            return {"metric": metric, "error": "image sizes differ"}
        if x + width > left.width or y + height > left.height:
            return {"metric": metric, "error": "crop rectangle is outside the images"}
        left, right = (image.crop((x, y, x + width, y + height)) for image in (left, right))
        total = left.width * left.height
        mask = ImageChops.difference(left, right).convert("L").point(lambda value: 255 if value else 0)
        changed = mask.histogram()[255]
        command = f"PIL ImageChops.difference({Path(before).name}, {Path(after).name})"
    else:
        metric = "ImageMagick compare -metric AE: changed pixels / (W*H)"
        with tempfile.TemporaryDirectory(prefix="flamewm-pixel-diff-") as temp_dir:
            left_path, right_path = Path(temp_dir) / "before.png", Path(temp_dir) / "after.png"
            for source, output in ((before, left_path), (after, right_path)):
                ok, detail = crop(Path(source), rect, output)
                if not ok:
                    return {"metric": metric, "error": f"cannot crop image: {detail}"}
            argv = ["compare", "-metric", "AE", "-fuzz", "0%", str(left_path), str(right_path), "null:"]
            result = subprocess.run(argv, capture_output=True, text=True, timeout=60, check=False)
            command = shell(argv)
            if result.returncode not in (0, 1):
                return {"metric": metric, "command": command,
                        "error": f"compare exited {result.returncode}: {result.stderr.strip()[:200]}"}
            try:
                changed = int(result.stderr.strip().split()[0])
            except (ValueError, IndexError):
                return {"metric": metric, "command": command, "error": "unparseable compare output"}
            total = rect[2] * rect[3]
    return {"metric": metric, "command": command, "changed_pixels": changed,
            "total_pixels": total, "fraction": changed / total}


def hotkey_stage(ctx: Ctx, stage: str, row: tuple, err: tuple) -> tuple[dict | None, str]:
    """Full-screen capture plus the row and error-bar crops for one NATIVE_HOTKEY_INPUT stage."""
    full = ctx.run / "shots" / f"hotkeys-{stage}.png"
    captured, capture_cmd = capture(ctx, full)
    if not captured:
        return None, f"screen capture failed at stage '{stage}' ({capture_cmd})"
    out = {"full": str(full), "commands": [capture_cmd]}
    for part, rect in (("row", row), ("error", err)):
        crop_path = ctx.run / "shots" / f"hotkeys-{stage}-{part}.png"
        ok, crop_cmd = crop(full, rect, crop_path)
        if not ok:
            return None, f"crop of {part} failed at stage '{stage}' ({crop_cmd})"
        out[part] = str(crop_path)
        out["commands"].append(crop_cmd)
    return out, ""


def run_ss06(ctx: Ctx, args: argparse.Namespace) -> dict:
    """NATIVE_HOTKEY_INPUT PASS requires a measured effect: row enters capture, capture ends, no error bar."""
    if not (args.hotkey_click and args.hotkey_key):
        return result_entry("NATIVE_HOTKEY_INPUT", "NOT_RUN",
                            "NATIVE_HOTKEY_INPUT needs both --hotkey-click X,Y and --hotkey-key KEYSYM; without them no effect is measured")
    if ctx.display not in (f":{n}" for n in XTEST_DISPLAY_RANGE):
        return blocked("NATIVE_HOTKEY_INPUT", f"{ctx.display} is outside flamewm_xtest.py's :97..:119 range")
    if not HAVE_PIL and (shutil.which("convert") is None or shutil.which("compare") is None):
        return blocked("NATIVE_HOTKEY_INPUT", "neither Pillow nor ImageMagick convert/compare is available for crops and pixel diffs")

    screen_w, screen_h = (int(part) for part in GEOMETRY_RE.fullmatch(args.geometry).groups())
    click_x, click_y = (int(part) for part in args.hotkey_click.split(","))
    if args.hotkey_row_rect:
        row, row_source = parse_rect(args.hotkey_row_rect), "--hotkey-row-rect"
    else:
        width, height = HOTKEY_ROW_DEFAULT_SIZE
        row = (min(max(click_x - 420, 0), screen_w - width), min(max(click_y - height // 2, 0), screen_h - height),
               width, height)
        row_source = "default: 460x48 band placed relative to --hotkey-click (layout-dependent)"
    if args.hotkey_error_rect:
        err, err_source = parse_rect(args.hotkey_error_rect), "--hotkey-error-rect"
    else:
        err = (0, screen_h - HOTKEY_ERROR_DEFAULT_HEIGHT, screen_w, HOTKEY_ERROR_DEFAULT_HEIGHT)
        err_source = "default: full-width bottom 80px band of the screen"

    details: dict = {"row_rect": list(row), "row_rect_source": row_source,
                     "error_rect": list(err), "error_rect_source": err_source,
                     "metric": "changed pixels / (W*H) per crop; PIL" if HAVE_PIL else "changed pixels / (W*H) per crop; ImageMagick AE",
                     "min_changed_fraction": HOTKEY_MIN_CHANGED_FRACTION,
                     "stages": {}, "diffs": {}}
    commands = [shell([args.settings_bin, "--page", "hotkeys"])]

    def finish(status: str, reason: str) -> dict:
        evidence = [path for stage in details["stages"].values() for path in (stage["full"], stage["row"], stage["error"])]
        entry = result_entry("NATIVE_HOTKEY_INPUT", status, reason, evidence, commands)
        entry["details"] = details
        return entry

    min_fraction = HOTKEY_MIN_CHANGED_FRACTION
    settings = ctx.spawn("settings-hotkeys-input", [args.settings_bin, "--page", "hotkeys"], ctx.app_env)
    try:
        time.sleep(args.settle_seconds)
        before, problem = hotkey_stage(ctx, "before", row, err)
        if before is None:
            return finish("ENVIRONMENT_BLOCKED", "cannot measure before input: " + problem)
        details["stages"]["before"] = before
        commands.extend(before["commands"])

        code, click_cmd = send_input(ctx, args.hotkey_click, None)
        commands.append(click_cmd)
        if code != 0:
            detail = " (INPUT_BLOCKED)" if code == 3 else ""
            return finish("ENVIRONMENT_BLOCKED", f"flamewm_xtest.py exited {code} on the click step{detail}")
        time.sleep(args.settle_seconds)
        capture_stage, problem = hotkey_stage(ctx, "capture", row, err)
        if capture_stage is None:
            return finish("ENVIRONMENT_BLOCKED", "cannot measure capture state: " + problem)
        details["stages"]["capture"] = capture_stage
        commands.extend(capture_stage["commands"])
        entered = pixel_change(before["row"], capture_stage["row"], (0, 0, row[2], row[3]))
        details["diffs"]["row_before_vs_capture"] = entered
        if "error" in entered:
            return finish("ENVIRONMENT_BLOCKED", "cannot compare row crops: " + entered["error"])
        if entered["fraction"] < min_fraction:
            return finish("FAIL", f"click on {args.hotkey_click} did not enter capture mode: row crop changed by "
                                  f"{entered['fraction']:.4%} (threshold {min_fraction:.2%})")

        code, key_cmd = send_input(ctx, None, args.hotkey_key)
        commands.append(key_cmd)
        if code != 0:
            detail = " (INPUT_BLOCKED)" if code == 3 else ""
            return finish("ENVIRONMENT_BLOCKED", f"flamewm_xtest.py exited {code} on the key step{detail}")
        time.sleep(args.settle_seconds)
        after, problem = hotkey_stage(ctx, "after", row, err)
        if after is None:
            return finish("ENVIRONMENT_BLOCKED", "cannot measure after input: " + problem)
        details["stages"]["after"] = after
        commands.extend(after["commands"])
        ended = pixel_change(capture_stage["row"], after["row"], (0, 0, row[2], row[3]))
        error_bar = pixel_change(before["error"], after["error"], (0, 0, err[2], err[3]))
        details["diffs"]["row_capture_vs_after"] = ended
        details["diffs"]["error_before_vs_after"] = error_bar
        for diff in (ended, error_bar):
            if "error" in diff:
                return finish("ENVIRONMENT_BLOCKED", "cannot compare crops: " + diff["error"])
        alive = settings.proc.poll() is None
    finally:
        ctx.stop(settings)

    reasons = []
    if not alive:
        reasons.append("settings page did not survive the input")
    if error_bar["fraction"] >= min_fraction:
        reasons.append(f"error bar appeared: bottom error region changed by {error_bar['fraction']:.4%} "
                       f"(error crop {details['stages']['after']['error']})")
    if ended["fraction"] < min_fraction:
        reasons.append(f"capture state still shown after key {args.hotkey_key}: row crop changed by "
                       f"{ended['fraction']:.4%} from the capture state (threshold {min_fraction:.2%})")
    if reasons:
        return finish("FAIL", "; ".join(reasons))
    return finish("PASS", f"row entered capture ({entered['fraction']:.4%} changed), capture ended after "
                          f"{args.hotkey_key} ({ended['fraction']:.4%} changed), no error bar "
                          f"({error_bar['fraction']:.4%} changed); threshold {min_fraction:.2%}")


# ---- SS09: 15s display-mode rollback -------------------------------------------------------------
SS09_SOURCE_TESTS: dict[str, tuple[str, ...]] = {
    "source_no_revert_at_14999ms": (
        "services::display::tests::fake_clock_15000ms_auto_revert_reverts_once_on_pending_output_and_changes_snapshot",
        "host::tests::unconfirmed_display_mode_reverts_only_after_deadline_tick"),
    "source_single_revert_at_15000ms": (
        "services::display::tests::fake_clock_15000ms_auto_revert_reverts_once_on_pending_output_and_changes_snapshot",
        "host::tests::unconfirmed_display_mode_reverts_only_after_deadline_tick"),
    "source_keep_prevents_revert": ("services::display::tests::keep_clears_pending_and_never_auto_reverts",),
}
SS09_REVERT_WINDOW = (14.5, 17.5)


def parse_cargo_test_lines(output: str) -> dict[str, str]:
    """Map `test NAME ... ok|FAILED|ignored` lines of cargo test output to their result word."""
    return {name: word for name, word in re.findall(r"^test (\S+) \.\.\. (\w+)", output, re.MULTILINE)}


def ss09_source_observations(exit_code: int | None, results: dict[str, str]) -> dict[str, bool | None]:
    """True only when cargo exited 0 and every required named test reported ok; a missing name is False."""
    if exit_code is None:
        return {key: None for key in SS09_SOURCE_TESTS}
    return {key: exit_code == 0 and all(results.get(name) == "ok" for name in names)
            for key, names in SS09_SOURCE_TESTS.items()}


def ss09_native_observed(applied: bool, pending_seen: bool, revert_elapsed: float | None,
                         reverted_mode_matches: bool) -> bool:
    """The deadline revert is real only when the mode change applied, stayed pending, and returned to the
    original mode with pending cleared inside the measured window (an early revert falls outside it)."""
    return bool(applied and pending_seen and reverted_mode_matches
                and revert_elapsed is not None and SS09_REVERT_WINDOW[0] <= revert_elapsed <= SS09_REVERT_WINDOW[1])


def assess_ss09(observations: dict[str, bool | None]) -> tuple[str, str]:
    required = tuple(SS09_SOURCE_TESTS)
    failed = [key for key in required if observations.get(key) is False]
    native = observations.get("native_timeout_revert")
    keep = observations.get("native_keep_control")
    if native is False:
        failed.append("native_timeout_revert")
    if keep is False:
        failed.append("native_keep_control")
    if failed:
        return STATUS_FAIL, "measured display rollback failure: " + ", ".join(failed)
    missing = [key for key in required if observations.get(key) is not True]
    if missing:
        return STATUS_NOT_RUN, "display rollback source tests did not run: " + ", ".join(missing)
    native_text = ("; native owned-session proof: BeginModeChange on a real RandR mode reverted at the 15 s deadline"
                   " with Keep control holding the mode" if native is True and keep is True
                   else "; native owned-session proof not completed")
    return STATUS_PASS, ("SOURCE-TEST evidence (fake-clock cargo tests in flamewm-platform, not a live-session "
                         "observation): 14999 ms no revert, 15000 ms single revert, Keep prevents revert" + native_text)


def xrandr_current_size(ctx: Ctx) -> tuple[int, int] | None:
    result = subprocess.run(["xrandr", "--query"], env=dict(ctx.host_env, DISPLAY=ctx.display),
                            capture_output=True, text=True, timeout=15, check=False)
    match = re.search(r"^\s+(\d+)x(\d+)\s.*\*", result.stdout, re.MULTILINE)
    return (int(match.group(1)), int(match.group(2))) if match else None


def display_modes(snapshot: str) -> list[tuple[int, int, int]]:
    """(id, width, height) of every mode of the first output in a Displays snapshot."""
    match = re.search(r"['\"]modes['\"]\s*:\s*<\[(.*?)\]>", snapshot, re.DOTALL)
    modes = []
    for block in top_level_blocks(match.group(1)) if match else []:
        mode_id, width, height = (gvariant_field_u64(block, "id"), gvariant_field_int(block, "width"),
                                  gvariant_field_int(block, "height"))
        if None not in (mode_id, width, height):
            modes.append((mode_id, width, height))
    return modes


def ss09_native(ctx: Ctx, args: argparse.Namespace, details: dict) -> dict[str, bool | None]:
    """Real owned-session proof: BeginModeChange over D-Bus, observe the 15 s revert and a Keep control."""
    out: dict[str, bool | None] = {"native_timeout_revert": None, "native_keep_control": None,
                                   "native_rollback_signal": None}
    record: dict = {}
    details["native"] = record
    if shutil.which("gdbus") is None or shutil.which("xrandr") is None:
        record["reason"] = "gdbus or xrandr unavailable"
        return out
    text, command = dbus_displays_snapshot(ctx)
    details["commands"].append(command)
    scales = display_output_scales(text or "")
    generation, current = gvariant_field_u64(text or "", "generation"), gvariant_field_u64(text or "", "mode")
    modes = display_modes(text or "")
    if scales is None or len(scales) != 1 or generation is None or current is None:
        record["reason"] = "single-output authoritative Displays snapshot unavailable"
        return out
    output_id = next(iter(scales))
    start_size = xrandr_current_size(ctx)
    alternatives = sorted((m for m in modes if m[0] != current and (m[1], m[2]) != start_size),
                          key=lambda m: m[1] * m[2])
    if start_size is None or not alternatives:
        record["reason"] = f"RandR exposes no alternative mode (modes={len(modes)}); nothing to change"
        return out
    mode_id, width, height = alternatives[-1] if len(alternatives) < 4 else alternatives[len(alternatives) // 2]
    record.update({"output": output_id, "from_mode": current, "to_mode": mode_id, "to_size": [width, height],
                   "from_size": list(start_size)})

    def begin(target: int) -> tuple[int | None, float]:
        snapshot, _ = dbus_displays_snapshot(ctx)
        gen = gvariant_field_u64(snapshot or "", "generation")
        started = time.monotonic()
        code, reply = gdbus_call(ctx, "Displays.BeginModeChange",
                                 [output_id, f"uint64 {target}", f"uint64 {gen}"], details)
        match = re.search(r"uint64 (\d+)", reply)
        return (int(match.group(1)) if code == 0 and match else None), started

    def pending() -> bool | None:
        snapshot, _ = dbus_displays_snapshot(ctx)
        return gvariant_field_bool(snapshot or "", "hasPending") if snapshot else None

    monitor = ctx.spawn("ss09-displays-monitor", ["dbus-monitor", "--session",
                                                  "type='signal',interface='com.arkflame.FlameWM1.Displays'"],
                        ctx.app_env)
    time.sleep(0.5)
    transaction, started = begin(mode_id)
    record["transaction"] = transaction
    if transaction is None:
        stop_dbus_monitor(ctx, monitor)
        record["reason"] = "BeginModeChange was refused"
        return out
    applied = pending_seen = False
    revert_elapsed = None
    samples = []
    while time.monotonic() - started < 25.0:
        size, flag = xrandr_current_size(ctx), pending()
        elapsed = round(time.monotonic() - started, 2)
        samples.append((elapsed, size, flag))
        if size == (width, height):
            applied = True
        if flag:
            pending_seen = True
        if applied and size == start_size and not flag:
            revert_elapsed = elapsed
            break
        time.sleep(0.25)
    final_size = xrandr_current_size(ctx)
    stop_dbus_monitor(ctx, monitor)
    log = ctx.run / "logs" / "ss09-displays-monitor.log"
    signals = log.read_text(encoding="utf-8", errors="replace") if log.exists() else ""
    record["revert_signals"] = {name: signals.count(f"member={name}")
                                for name in ("DisplayTransactionChanged", "DisplaysSnapshotChanged")}
    out["native_rollback_signal"] = (None if not log.exists() else
                                     bool(applied and revert_elapsed is not None
                                          and any(record["revert_signals"].values())))
    record.update({"applied": applied, "pending_seen": pending_seen, "revert_elapsed_s": revert_elapsed,
                   "samples_first_last": [samples[0], samples[-1]] if samples else [],
                   "last_sample_at_or_before_14s": next((sample for sample in reversed(samples) if sample[0] <= 14.0), None)})
    out["native_timeout_revert"] = ss09_native_observed(applied, pending_seen, revert_elapsed,
                                                       final_size == start_size)

    # Keep control: confirm the same change; the mode must stay past the 15 s deadline.
    if out["native_timeout_revert"] is True:
        transaction, started = begin(mode_id)
        code, reply = (gdbus_call(ctx, "Displays.Keep", [f"uint64 {transaction}"], details)
                       if transaction is not None else (1, "begin refused"))
        time.sleep(16.5)
        kept_size, flag = xrandr_current_size(ctx), pending()
        record["keep"] = {"exit": code, "size_after_deadline": kept_size, "pending": flag}
        out["native_keep_control"] = bool(code == 0 and kept_size == (width, height) and flag is False)
        restore, _ = begin(current)
        if restore is not None:
            gdbus_call(ctx, "Displays.Keep", [f"uint64 {restore}"], details)
        time.sleep(1.0)
        record["restored_size"] = xrandr_current_size(ctx)
    return out


def run_ss09(ctx: Ctx, args: argparse.Namespace) -> dict:
    observations: dict[str, bool | None] = {key: None for key in SS09_SOURCE_TESTS}
    observations.update({"native_timeout_revert": None, "native_keep_control": None,
                         "native_rollback_signal": None})
    details: dict = {"observations": observations, "commands": [],
                     "paths_exercised": ["cargo test -p flamewm-platform display (fake clock)",
                                         "Displays.BeginModeChange/Keep over D-Bus with real RandR modes"]}
    argv = ["cargo", "test", "-p", "flamewm-platform", "display"]
    details["commands"].append(shell(argv))
    exit_code: int | None = None
    results: dict[str, str] = {}
    if shutil.which("cargo") is None:
        details["source_reason"] = "cargo not on PATH"
    else:
        try:
            done = subprocess.run(argv, cwd=ROOT, env={**os.environ, "CARGO_BUILD_JOBS": "2"},
                                  capture_output=True, text=True, timeout=1800, check=False)
            exit_code, results = done.returncode, parse_cargo_test_lines(done.stdout + done.stderr)
        except subprocess.TimeoutExpired:
            details["source_reason"] = "cargo test timed out"
    details["source_exit_code"] = exit_code
    details["source_tests"] = {name: results.get(name) for names in SS09_SOURCE_TESTS.values() for name in names}
    observations.update(ss09_source_observations(exit_code, results))
    if ctx.display is not None and not ctx.wm_dead():
        observations.update(ss09_native(ctx, args, details))
    status, reason = assess_ss09(observations)
    entry = result_entry("SS09", status, reason, commands=details["commands"])
    entry["details"] = details
    return entry


# ---- SS08: displays 1/2/4/5+ outputs --------------------------------------------------------------
def count_connected(xrandr_text: str) -> int:
    return sum(1 for line in xrandr_text.splitlines() if re.search(r"\sconnected\b", line)
               and "disconnected" not in line)


def assess_ss08(one_output_status: str, outputs_by_provider: dict[str, int | None], dummy_driver: bool,
                modes_listed: int | None) -> tuple[str, str]:
    """One output never counts as 1/2/4/5+: PASS needs a provider with >=5 connected outputs (no runner yet)."""
    summary = ", ".join(f"{name}={count if count is not None else 'unprobed'}" for name, count in outputs_by_provider.items())
    if one_output_status == STATUS_FAIL:
        return STATUS_FAIL, "the single-output Displays page failed"
    base = (f"1-output Displays case {one_output_status} with {modes_listed} listed modes; connected RandR outputs "
            f"measured per provider: {summary}")
    if any(count is not None and count >= 2 for count in outputs_by_provider.values()) or dummy_driver:
        return STATUS_NOT_RUN, base + "; a multi-output provider exists but this runner has no 2/4/5+ output fixture driver"
    return STATUS_ENV_BLOCKED, (base + "; no private X server on this host exposes >=2 RandR outputs (Xephyr -output only "
                                "restricts fullscreen to a host output, xrandr --setmonitor adds monitors not outputs, "
                                "no Xdummy/Xvnc/xf86-video-dummy); 2/4/5+ output cases cannot be produced")


def probe_provider_outputs(ctx: Ctx, argv_for: dict[str, list[str]]) -> dict[str, int | None]:
    """Start each private X provider on a free :9x display and count connected RandR outputs; None when unusable."""
    counts: dict[str, int | None] = {}
    for name, argv in argv_for.items():
        if shutil.which(argv[0]) is None:
            counts[name] = None
            continue
        display = pick_display(None)
        if display is None:
            counts[name] = None
            continue
        owned = ctx.spawn(f"probe-{name}", [argv[0], display, *argv[1:]], ctx.host_env)
        try:
            socket = X11_SOCKET_DIR / f"X{display[1:]}"
            deadline = time.monotonic() + 10.0
            while time.monotonic() < deadline and not socket.exists() and owned.proc.poll() is None:
                time.sleep(0.2)
            time.sleep(0.5)
            result = subprocess.run(["xrandr", "--query"], env=dict(ctx.host_env, DISPLAY=display),
                                    capture_output=True, text=True, timeout=15, check=False)
            counts[name] = count_connected(result.stdout) if result.returncode == 0 else None
        finally:
            ctx.stop(owned)
    return counts


def run_ss08(ctx: Ctx, args: argparse.Namespace, one_output: dict | None) -> dict:
    ours = subprocess.run(["xrandr", "--query"], env=dict(ctx.host_env, DISPLAY=ctx.display),
                          capture_output=True, text=True, timeout=30, check=False)
    counts: dict[str, int | None] = {"Xephyr(owned)": count_connected(ours.stdout) if ours.returncode == 0 else None}
    counts.update(probe_provider_outputs(ctx, {"Xvfb": ["Xvfb", "-screen", "0", "1280x800x24"]}))
    counts.update({name: None for name in ("Xvnc", "Xdummy")})  # not installed on this host: unprobed
    dummy = any(Path(root, "xorg/modules/drivers/dummy_drv.so").is_file() for root in ("/usr/lib", "/usr/lib64", "/usr/lib/x86_64-linux-gnu"))
    snapshot, _ = dbus_displays_snapshot(ctx)
    modes = len(display_modes(snapshot or "")) if snapshot else None
    status, reason = assess_ss08((one_output or {}).get("status", STATUS_NOT_RUN), counts, dummy, modes)
    entry = result_entry("SS08", status, reason, evidence=(one_output or {}).get("evidence", []),
                         commands=["xrandr --query (owned Xephyr)", "Xvfb probe on a free :9x display"])
    entry["details"] = {"connected_outputs": counts, "xf86_video_dummy_present": dummy, "listed_modes": modes,
                        "source_fixtures": "flamewm-platform display tests cover 2- and 5-output fixtures "
                                           "(two_output_fixture_*, five_output_*); not native evidence"}
    return entry


# ---- SS17: private XTEST + screenshot matrix ------------------------------------------------------
SS17_J07_ROWS: tuple[tuple[str, str, str], ...] = (
    ("j07_popup", "popup", "t03-popup-lifecycle"),
    ("j07_chrome", "chrome", "t09-t10-chrome-client"),
    ("j07_max_snap", "max-snap", "t11-max-snap-fullscreen"),
    ("j07_move", "move", "t06-real-move"),
    ("j07_resize", "resize", "t07-real-resize-8"),
)
SS17_REQUIRED = ("x11_acceptance_all",) + tuple(row for row, _, _ in SS17_J07_ROWS) + (
    "close_hover_pixel_difference", "scale_scroll_ss10")
SS17_HOVER_MIN_FRACTION = 0.01


def canary_check_passed(output: str, check: str) -> bool:
    """The canary prints a JSON document; the named check must be present and pass (never inferred from exit)."""
    for line in reversed(output.strip().splitlines()):
        try:
            document = json.loads(line)
        except ValueError:
            continue
        return bool(document.get("pass") is True and any(
            item.get("name") == check and item.get("pass") is True for item in document.get("checks", [])))
    return False


def ss17_hover_observed(away_a_vs_b: float | None, hover_vs_away: float | None) -> bool | None:
    """Close hover is real only when the button rect changes under the pointer and two away captures agree."""
    if away_a_vs_b is None or hover_vs_away is None:
        return None
    return away_a_vs_b == 0.0 and hover_vs_away >= SS17_HOVER_MIN_FRACTION


def ss17_scale_scroll_observed(ss10: dict | None) -> bool | None:
    if not ss10:
        return None
    details = ss10.get("details", {})
    scroll = details.get("scroll")
    if not scroll:
        return None
    return bool(ss10.get("status") == STATUS_PASS and scroll.get("exit_code") == 0)


def assess_ss17(observations: dict[str, bool | None]) -> tuple[str, str]:
    failed = [key for key in SS17_REQUIRED if observations.get(key) is False]
    if failed:
        return STATUS_FAIL, "measured private-matrix failure: " + ", ".join(failed)
    missing = [key for key in SS17_REQUIRED if observations.get(key) is not True]
    if missing:
        return STATUS_NOT_RUN, "required matrix rows not executed: " + ", ".join(missing)
    return STATUS_PASS, "whole required matrix executed: " + ", ".join(SS17_REQUIRED)


def run_ss17(ctx: Ctx, args: argparse.Namespace, ss10: dict | None) -> dict:
    observations: dict[str, bool | None] = {key: None for key in SS17_REQUIRED}
    details: dict = {"observations": observations, "commands": [], "rows": {}}
    observations["scale_scroll_ss10"] = ss17_scale_scroll_observed(ss10)

    # Row 1: existing four-scenario Xephyr canary harness in its own isolated Xephyr + private D-Bus.
    # A package without the canary (installed prefix) cannot run it: that is NOT_RUN, never a product FAIL.
    if not os.access(args.canary_bin, os.X_OK):
        details["rows"]["x11_acceptance_all"] = {"skipped": f"canary binary not executable: {args.canary_bin}"}
    else:
        harness = ROOT / "tools" / "flamewm_x11_acceptance.py"
        argv = [sys.executable, str(harness), "--scenario", "all", "--timeout", "240",
                "--evidence-dir", str(ctx.run / "x11-acceptance")]
        details["commands"].append(shell(argv))
        env = dict(ctx.host_env, FLAMEWM_XEPHYR_BUILD="0", FLAMEWM_XEPHYR_WM_BIN=args.wm_bin,
                   FLAMEWM_XEPHYR_DESKTOP_BIN=args.desktop_bin, FLAMEWM_XEPHYR_SHELL_BIN=args.shell_bin,
                   FLAMEWM_XEPHYR_CANARY_BIN=args.canary_bin)
        env.pop("FLAMEWM_SS_INNER", None)
        child = subprocess.Popen(argv, cwd=ROOT, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                 text=True, start_new_session=True)
        try:
            output, _ = child.communicate(timeout=420)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, 15)
            output, _ = child.communicate()
            output += "\nTIMEOUT"
        observations["x11_acceptance_all"] = child.returncode == 0 and "FLAMEWM_XEPHYR_CANARY_PASS" in output
        details["rows"]["x11_acceptance_all"] = {"exit": child.returncode, "tail": output[-400:]}

    # Rows 2..: j07 canaries against the owned session with desktop, shell and the xterm fixture.
    if (ctx.display in (f":{n}" for n in XTEST_DISPLAY_RANGE) and os.access(args.canary_bin, os.X_OK)
            and shutil.which("xterm") and not ctx.wm_dead()):
        clients = start_session_clients(ctx, args, ("desktop", "shell"), "ss17")
        xterm = ctx.spawn("xterm-ss17", ["xterm", "-T", "FlameWM-T06-Canary", "-n", "FlameWM-T06-Canary",
                                         "-geometry", "80x24+90+90"], ctx.app_env)
        time.sleep(min(args.settle_seconds, 2.0))
        try:
            for row, scenario, check in SS17_J07_ROWS:
                cmd = [args.canary_bin, "j07", "--scenario", scenario, "--name", "FlameWM-T06-Canary",
                       "--timeout-secs", "5", "--json"]
                details["commands"].append(shell(cmd))
                try:
                    done = subprocess.run(cmd, env=dict(ctx.app_env, DISPLAY=ctx.display), capture_output=True,
                                          text=True, timeout=90, check=False)
                    observations[row] = canary_check_passed(done.stdout, check)
                    details["rows"][row] = {"exit": done.returncode, "tail": done.stdout[-300:]}
                except subprocess.TimeoutExpired:
                    observations[row] = False
                    details["rows"][row] = {"reason": "canary timed out"}
        finally:
            ctx.stop(xterm)
            stop_session_clients(ctx, clients)
    else:
        details["j07_reason"] = "owned XTEST display, canary binary, xterm or live WM unavailable"

    # Close-hover pixel difference on the Settings title bar close button (real XTEST pointer motion).
    if Image is not None and ctx.display in (f":{n}" for n in XTEST_DISPLAY_RANGE) and not ctx.wm_dead():
        settings = ctx.spawn("settings-ss17-hover", [args.settings_bin, "--page", "appearance"], ctx.app_env)
        try:
            time.sleep(args.settle_seconds)
            geometry = wait_for_settings_window(ctx, ctx.run / "logs" / "ss17-window-tree.txt")
            if geometry is not None:
                rect = (geometry["x"] + geometry["width"] - 38, geometry["y"] - 25, 38, 25)
                center = f"{rect[0] + 19},{rect[1] + 12}"
                away = f"{geometry['x'] + 300},{geometry['y'] - 12}"  # inside the title bar, clear of buttons
                shots = {}
                for name, point in (("away-a0", away), ("away-a", away), ("hover", center),
                                    ("left-frame", "1000,700")):
                    send_input(ctx, None, None, move=point)
                    time.sleep(0.6)
                    shots[name] = ss06_capture(ctx, f"ss17-close-{name}")
                if None not in shots.values():
                    fractions = {name: pixel_change(str(shots["away-a"]), str(shots[name]), rect).get("fraction")
                                 for name in ("away-a0", "hover", "left-frame")}
                    observations["close_hover_pixel_difference"] = ss17_hover_observed(
                        fractions["away-a0"], fractions["hover"])
                    details["rows"]["close_hover_pixel_difference"] = {
                        "rect": list(rect), "noise": fractions["away-a0"], "hover": fractions["hover"],
                        "after_leaving_frame": fractions["left-frame"],
                        "hover_cleared_on_leave": fractions["left-frame"] == 0.0}
        finally:
            send_input(ctx, None, None, move="640,600")
            ctx.stop(settings)
    status, reason = assess_ss17(observations)
    if details["rows"].get("close_hover_pixel_difference", {}).get("hover_cleared_on_leave") is False:
        reason += " (not gated: Close hover highlight stayed painted after the pointer left the frame)"
    entry = result_entry("SS17", status, reason, commands=details["commands"])
    entry["details"] = details
    return entry


def ss18_result(args: argparse.Namespace, run: Path) -> dict:
    """SS18 from an explicit installed-result.json, else by invoking the installed gate on --ss18-prefix."""
    if args.ss18_installed_result or not args.ss18_prefix:
        return map_ss18(args.ss18_installed_result)
    target = run / "ss18-installed"
    argv = [str(ROOT / "scripts" / "settings-installed-acceptance"), "--prefix", str(args.ss18_prefix),
            "--run-dir", str(target)]
    env = {key: value for key, value in os.environ.items() if key != "FLAMEWM_SS_INNER"}
    try:
        subprocess.run(argv, env=env, capture_output=True, text=True, timeout=2400, check=False)
    except (OSError, subprocess.SubprocessError) as error:
        return result_entry("SS18", STATUS_NOT_RUN, f"installed acceptance did not run: {error}", commands=[shell(argv)])
    entry = map_ss18(str(target / "installed-result.json"))
    entry["command"] = shell(argv)
    return entry


def run_scenarios(ctx: Ctx, args: argparse.Namespace, results: dict[str, dict]) -> None:
    ctx.display = pick_display(args.display)
    if ctx.display is None:
        block_runner_ids(results, "no free display in :90..:99")
        return
    ctx.build_app_env()
    ctx.token = secrets.token_hex(32)
    ctx.marker.write_text(f"{ctx.display}\n{ctx.token}\n", encoding="ascii")
    os.chmod(ctx.marker, 0o600)

    xephyr_argv = ["Xephyr", ctx.display, "-screen", args.geometry, "-ac", "-noreset", "-br"]
    xephyr = ctx.spawn("xephyr", xephyr_argv, ctx.host_env)
    if not wait_for_display(ctx, xephyr):
        block_runner_ids(results, f"Xephyr did not become ready on {ctx.display}", commands=[shell(xephyr_argv)])
        return

    wm_argv = [args.wm_bin]
    ctx.wm = ctx.spawn("flamewm", wm_argv, ctx.app_env)
    time.sleep(args.settle_seconds)
    wm_alive = ctx.wm.proc.poll() is None
    base_label = f"base-{args.geometry}"

    # SS01: all seven pages at the base screen size.
    base_shots = [page_shot(ctx, args, page, base_label) for page in PAGES] if wm_alive else []
    if not wm_alive:
        results["SS01"] = result_entry("SS01", STATUS_FAIL, "flamewm WM exited before the first settings page")
    else:
        ok = all(s["alive"] and s["captured"] and s["nonblank"] for s in base_shots)
        results["SS01"] = result_entry(
            "SS01", STATUS_PASS if ok else STATUS_FAIL,
            "" if ok else "a page exited, failed capture or captured a blank screen",
            [s["shot"] for s in base_shots],
            [cmd for s in base_shots for cmd in s["commands"]],
        )

    # SS02: launch the actual Settings client at each requested size and measure it.
    if not wm_alive:
        results["SS02"] = result_entry("SS02", STATUS_FAIL, "flamewm WM exited; no matrix captures")
    else:
        matrix = {}
        for width, height in SIZES:
            label = f"{width}x{height}"
            matrix[label] = [page_shot(ctx, args, page, label, (width, height)) for page in PAGES]
        all_shots = [s for shots in matrix.values() for s in shots]
        crashed = [s["label"] + ":" + s["page"] for s in all_shots if not (s["alive"] and s["captured"])]
        mismatched = []
        for width, height in SIZES:
            for shot in matrix[f"{width}x{height}"]:
                if not exact_window_size(shot, (width, height)):
                    window = shot["window"]
                    mismatched.append(f"{shot['label']}:{shot['page']}={'unknown' if window is None else str(window['width']) + 'x' + str(window['height'])}")
        if crashed:
            results["SS02"] = result_entry("SS02", STATUS_FAIL, "pages failed to stay alive or capture: " + ", ".join(crashed),
                                           [s["shot"] for s in all_shots], [c for s in all_shots for c in s["commands"]])
        elif mismatched:
            # A missing resize feature is a product/harness gap, not hardware: FAIL, never ENVIRONMENT_BLOCKED.
            results["SS02"] = result_entry(
                "SS02", STATUS_FAIL,
                "requested client geometry, live window, or nonblank capture did not match; "
                "Xephyr screen geometry is not a substitute: " + ", ".join(mismatched[:12])
                + (" ..." if len(mismatched) > 12 else ""),
                [s["shot"] for s in all_shots], [c for s in all_shots for c in s["commands"]])
        else:
            results["SS02"] = result_entry("SS02", STATUS_PASS, "", [s["shot"] for s in all_shots],
                                           [c for s in all_shots for c in s["commands"]])

    results["SS03"] = run_ss03(ctx, args)

    # NATIVE_ONE_OUTPUT (legacy, was labelled SS03): Displays page with one connected output.
    # It covers only the single-output case of canonical SS08, so it never makes SS08 PASS.
    displays_base = next((s for s in base_shots if s["page"] == "displays"), None)
    if displays_base is None:
        results["NATIVE_ONE_OUTPUT"] = result_entry("NATIVE_ONE_OUTPUT", STATUS_FAIL, "no Displays capture was taken")
    else:
        xrandr = subprocess.run(["xrandr", "--query"], env=dict(ctx.host_env, DISPLAY=ctx.display),
                                capture_output=True, text=True, timeout=30, check=False)
        connected = sum(1 for line in xrandr.stdout.splitlines() if " connected" in line)
        commands = displays_base["commands"] + ["xrandr --query"]
        if xrandr.returncode != 0:
            results["NATIVE_ONE_OUTPUT"] = blocked("NATIVE_ONE_OUTPUT", "xrandr unavailable; cannot count outputs")
        elif connected == 1 and displays_base["alive"] and displays_base["nonblank"]:
            results["NATIVE_ONE_OUTPUT"] = result_entry("NATIVE_ONE_OUTPUT", STATUS_PASS, "", [displays_base["shot"]], commands)
        else:
            results["NATIVE_ONE_OUTPUT"] = result_entry("NATIVE_ONE_OUTPUT", STATUS_FAIL,
                                                        f"expected 1 connected output, observed {connected}; or displays page not alive/nonblank",
                                                        [displays_base["shot"]], commands)

    # NATIVE_HOTKEY_INPUT (legacy, was labelled SS06): PASS only on a measured effect (see run_ss06).
    results["NATIVE_HOTKEY_INPUT"] = run_ss06(ctx, args)
    results["SS04"] = run_ss04(ctx, args)
    results["SS05"] = run_ss05(ctx, args)
    results["SS06"] = run_ss06_desktop(ctx, args)
    results["SS07"] = run_ss07(ctx, args)
    results["SS10"] = run_ss10(ctx, args)
    results["SS11"] = run_ss11(ctx, args)
    # SS12 is stricter than pixel-based capture smoke: verify lease method ownership,
    # authoritative shortcut state, normal Start replay, owner-loss restoration, and
    # Escape clear / Reset persistence.
    results["SS12"] = run_ss12(ctx, args)
    results["SS13"] = run_ss13(ctx, args)
    results["SS15"] = run_ss15(ctx, args)
    # SS14 currently exercises only Settings.accent: full status remains NOT_RUN until
    # independent panels, shortcuts, display rollback and signal-loss scenarios exist.
    results["SS14"] = run_ss14(ctx, args)
    ss14_observations = results["SS14"].get("details", {}).get("observations", {})
    external_update = ss14_observations.get("settings_visible_without_input")
    results["SS19"] = run_ss19(ctx, args, external_update)
    results["SS17"] = run_ss17(ctx, args, results.get("SS10"))
    results["SS08"] = run_ss08(ctx, args, results.get("NATIVE_ONE_OUTPUT"))
    # SS09 changes the owned screen size (and restores it): keep it after every pixel scenario.
    results["SS09"] = run_ss09(ctx, args)
    # SS14 display_rollback_signal: the unattended revert must have emitted a Displays signal (no client input).
    rollback_signal = results["SS09"]["details"]["observations"].get("native_rollback_signal")
    ss14 = results["SS14"]
    ss14_observations = ss14.get("details", {}).get("observations")
    if ss14_observations is not None and rollback_signal is not None:
        ss14_observations["display_rollback_signal"] = rollback_signal
        ss14["status"], ss14["reason"] = assess_ss14(ss14_observations)

    # NATIVE_PERSISTENCE_RESTART (legacy, was labelled SS08): persistence and restart under private XDG.
    persisted = sorted(
        str(p) for root in (ctx.run / "xdg-config", ctx.run / "xdg-data") for p in root.rglob("*") if p.is_file()
    )
    if not persisted:
        results["NATIVE_PERSISTENCE_RESTART"] = blocked(
            "NATIVE_PERSISTENCE_RESTART",
            "no persisted state was written under the private XDG directories in this run; "
            "change a setting (UI or --hotkey-click/--hotkey-key) and rerun")
    else:
        ctx.stop(ctx.wm)
        ctx.wm = ctx.spawn("flamewm-restart", wm_argv, ctx.app_env)
        time.sleep(args.settle_seconds)
        restart = page_shot(ctx, args, "displays", "restart")
        still = all(Path(p).is_file() for p in persisted)
        ok = still and restart["alive"] and restart["nonblank"]
        results["NATIVE_PERSISTENCE_RESTART"] = result_entry(
            "NATIVE_PERSISTENCE_RESTART", STATUS_PASS if ok else STATUS_FAIL,
            "" if ok else "persisted files missing or restarted settings page not alive/nonblank",
            [restart["shot"]] + persisted[:20], restart["commands"])

    for sid in RUNNER_IDS:
        if sid not in results:
            results[sid] = blocked(sid, "not reached in this run")

def inner(args: argparse.Namespace, run: Path) -> int:
    ctx = Ctx(run)
    results: dict[str, dict] = {}
    started = utc_now()
    git_before = git_snapshot()
    teardown: dict = {"owned_pids": [], "remaining": [], "ok": True}
    try:
        results["SS16"] = run_ss16()
        results["SS18"] = ss18_result(args, run)
        reasons = host_blockers(args)
        if reasons:
            block_runner_ids(results, "; ".join(reasons))
        else:
            run_scenarios(ctx, args, results)
    finally:
        teardown = ctx.teardown()
        git_after = git_snapshot()
        results["SS00"] = result_entry(
            "SS00", STATUS_PASS if git_before == git_after else STATUS_FAIL,
            "" if git_before == git_after else "git status changed during the run",
            commands=["git status --porcelain (before and after)"])
        results["NATIVE_PID_CLEANUP"] = result_entry(
            "NATIVE_PID_CLEANUP", STATUS_PASS if teardown["ok"] else STATUS_FAIL,
            "" if teardown["ok"] else f"PIDs still running: {teardown['remaining']}",
            commands=["Popen.terminate/kill on owned PIDs only", "Popen.poll verification"])
        document = compose_document(results)
        document.update({
            "run_dir": str(run),
            "display": ctx.display,
            "geometry": args.geometry,
            "wm_bin": str(Path(args.wm_bin).resolve()),
            "settings_bin": str(Path(args.settings_bin).resolve()),
            "started_utc": started,
            "finished_utc": utc_now(),
            "teardown": teardown,
        })
        write_json(run / "result.json", document)
    return document["exit_code"]

def main(argv: list[str] | None = None) -> int:
    raw = sys.argv[1:] if argv is None else list(argv)
    args = parse_args(raw)
    run = validate(args)

    if os.environ.get("FLAMEWM_SS_INNER") == "1":
        if not (run / RUN_OWNER_MARKER).is_file():
            print("error: inner invocation without run owner marker", file=sys.stderr)
            return 2
        return inner(args, run)

    prepare_run(run)
    if shutil.which("dbus-run-session") is None:
        results: dict[str, dict] = {}
        block_runner_ids(results, "dbus-run-session not found on PATH")
        results["SS00"] = blocked("SS00", "dbus-run-session not found on PATH")
        results["NATIVE_PID_CLEANUP"] = blocked("NATIVE_PID_CLEANUP", "dbus-run-session not found on PATH")
        document = {"run_dir": str(run), **compose_document(results)}
        write_json(run / "result.json", document)
        print(run / "result.json")
        return 3

    env = dict(os.environ, FLAMEWM_SS_INNER="1")
    command = ["dbus-run-session", "--", sys.executable, str(Path(__file__).resolve()), *raw]
    child = subprocess.Popen(command, env=env)
    code = child.wait()
    result_path = run / "result.json"
    if result_path.is_file():
        document = json.loads(result_path.read_text(encoding="utf-8"))
    else:
        document = {"schema": SCHEMA, "run_dir": str(run), "scenarios": []}
    document["outer"] = {
        "dbus_run_session_pid": child.pid,
        "exit_code": code,
        "remaining": [] if child.poll() is not None else [child.pid],
        "command": shell(["dbus-run-session", "--", "python3", str(Path(__file__).resolve()), "<args>"]),
    }
    result_path.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
    print(result_path)
    return code


if __name__ == "__main__":
    raise SystemExit(main())

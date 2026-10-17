#!/usr/bin/env python3
"""Native desktop rename, live theme propagation and taskbar tray visibility on a private Xephyr (:105).

Usage:
  tools/flamewm_desktop_theme_acceptance.py --run-dir DIR [--display :105] [--geometry 1280x800]
      [--wm-bin PATH --shell-bin PATH --settings-bin PATH --desktop-bin PATH --client-bin xclock]
      [--with-audio] [--only rename,live,taskbar] [--point X,Y] [--override NAME=X,Y ...]

Sections (each records PASS/FAIL/NOT_RUN in DIR/result.json and screenshots in DIR/shots):
  rename   right-click blank desktop -> Create New Folder; Rename the just-created entry, type the new name
           and confirm with Return; create another, Rename it and cancel with Escape. Each entry is located
           by OCR of its own label (or --override entry-ok=X,Y / entry-cancel=X,Y). Verified on disk.
  live     Desktop restarted with XCURSOR_PATH=/usr/share/icons; Settings.Apply over com.arkflame.FlameWM1
           (the same control-plane write the Settings app uses) for an accent and icon theme that differ from
           the active ones, plus cursor themes; Settings closed before the after-shot. Accent and icon region
           pixel changes and cursor digests are compared.
  taskbar  each tray toggle (taskbarShowWeather/Volume/Network/Battery/Clock/Workspaces) off then on;
           slot pixels compared against the baseline strip.

Input is XTEST only through tools/flamewm_xtest.py. Only :105 by default (never :97).
Exit status: 0 all PASS, 1 any FAIL, 2 usage error, 3 environment blocked or incomplete.
"""

from __future__ import annotations

import argparse
import ctypes
import ctypes.util
import json
import os
import re
import secrets
import shutil
import subprocess
import sys
import time
from pathlib import Path

from flamewm_harness_common import write_json

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))

import flamewm_context_menu_weather_acceptance as cmw  # noqa: E402
import flamewm_settings_acceptance as base  # noqa: E402
import flamewm_tray_popups_acceptance as tray  # noqa: E402

SCHEMA = "flamewm-desktop-theme-acceptance/1"
PASS, FAIL, NOT_RUN, NOT_PRESENT = "PASS", "FAIL", "NOT_RUN", "NOT_PRESENT"
BUS_NAME, OBJECT_PATH, IFACE = "com.arkflame.FlameWM1", "/com/arkflame/FlameWM1", "com.arkflame.FlameWM1.Settings"
DEFAULT_DISPLAY = ":105"
# Taskbar tray slots as laid out on the default 1280x800 taskbar (see tray items in P-*-final shots).
TOGGLES = {"weather": "taskbarShowWeather", "volume": "taskbarShowVolume", "network": "taskbarShowNetwork",
           "battery": "taskbarShowBattery", "clock": "taskbarShowClock", "workspaces": "taskbarShowWorkspaces"}
NEW_NAME = "flamewm-renamed"
CANCEL_NAME = "should-not-apply"
DESKTOP_POINT = (900, 300)
ACCENTS = ("#e53935", "#1e88e5")
ICON_THEMES = ("Adwaita", "breeze")
DESKTOP_XCURSOR_PATH = "/usr/share/icons"
# Packaged Breeze glyphs (folder, document-edit) win over freedesktop themes, so they cannot show an icon-theme
# change. This launcher's Icon= exists only in Adwaita: before the change it paints the packaged default glyph.
LIVE_LAUNCHER_NAME = "acceptance-theme.desktop"
LIVE_LAUNCHER = ("[Desktop Entry]\nType=Application\nName=Acceptance Theme\nExec=true\n"
                 "Icon=printer-network\n")


# ---------------------------------------------------------------- pure helpers (unit-tested)

def new_entries(before: list[str], after: list[str]) -> list[str]:
    return sorted(set(after) - set(before))


def pick_other(current: str | None, candidates: tuple[str, ...]) -> str:
    """First candidate that differs from the currently active value, so the apply is a real change."""
    return next(value for value in candidates if value != current)


def blue_icon_center(before: list[tuple[int, int, int]], after: list[tuple[int, int, int]], width: int,
                     min_pixels: int = 200) -> tuple[int, int] | None:
    """Centre of the blue folder glyph that appeared between two equal-size captures (white labels excluded)."""
    if len(before) != len(after):
        raise ValueError("regions differ in size")
    points = [(i % width, i // width) for i, (p, q) in enumerate(zip(before, after))
              if max(abs(p[k] - q[k]) for k in range(3)) > 24 and q[2] > 200 and q[0] < 120 and q[2] - q[0] > 120]
    if len(points) < min_pixels:
        return None
    return (round(sum(x for x, _ in points) / len(points)), round(sum(y for _, y in points) / len(points)))


def changed_pixels(a: list[tuple[int, int, int]], b: list[tuple[int, int, int]], tolerance: int = 24) -> int:
    """Count of pixel pairs whose channels differ by more than TOLERANCE (equal-length regions only)."""
    if len(a) != len(b):
        raise ValueError("regions differ in size")
    return sum(1 for p, q in zip(a, b) if max(abs(p[i] - q[i]) for i in range(3)) > tolerance)


def item_verdict(removed_delta: int, restored_delta: int, threshold: int) -> str:
    """PASS when hiding the item changes the strip and showing it again restores the baseline strip."""
    if removed_delta <= threshold:
        return "NOT_PRESENT"  # toggling changed nothing: the item is not rendered in the baseline
    return PASS if restored_delta <= threshold else FAIL


def cursor_signature(pixels: list[int]) -> int:
    """Order-sensitive FNV-1a digest of the ARGB cursor pixels (identical shape gives identical digest)."""
    value = 0xcbf29ce484222325
    for pixel in pixels:
        value ^= pixel & 0xFFFFFFFF
        value = (value * 0x100000001b3) & 0xFFFFFFFFFFFFFFFF
    return value


def cursor_pixels_from_xfixes(display: str) -> list[int] | None:
    """Current cursor ARGB pixels via XFixesGetCursorImage (ctypes); None when libXfixes/X11 is unavailable."""
    xlib_path, xfixes_path = ctypes.util.find_library("X11"), ctypes.util.find_library("Xfixes")
    if not xlib_path or not xfixes_path:
        return None

    class XFixesCursorImage(ctypes.Structure):
        _fields_ = [("x", ctypes.c_short), ("y", ctypes.c_short), ("width", ctypes.c_ushort),
                    ("height", ctypes.c_ushort), ("xhot", ctypes.c_ushort), ("yhot", ctypes.c_ushort),
                    ("cursor_serial", ctypes.c_ulong), ("pixels", ctypes.POINTER(ctypes.c_ulong)),
                    ("atom", ctypes.c_ulong), ("name", ctypes.c_char_p)]

    xlib, xfixes = ctypes.CDLL(xlib_path), ctypes.CDLL(xfixes_path)
    xlib.XOpenDisplay.restype = ctypes.c_void_p
    xfixes.XFixesGetCursorImage.restype = ctypes.POINTER(XFixesCursorImage)
    dpy = xlib.XOpenDisplay(display.encode())
    if not dpy:
        return None
    image = xfixes.XFixesGetCursorImage(ctypes.c_void_p(dpy))
    if not image:
        xlib.XCloseDisplay(ctypes.c_void_p(dpy))
        return None
    count = image.contents.width * image.contents.height
    pixels = [int(image.contents.pixels[i]) & 0xFFFFFFFF for i in range(count)]
    xlib.XCloseDisplay(ctypes.c_void_p(dpy))
    return pixels


def parse_override(items: list[str]) -> dict[str, tuple[int, int]]:
    out: dict[str, tuple[int, int]] = {}
    for item in items:
        name, _, xy = item.partition("=")
        out[name] = cmw.xy_arg(xy)
    return out


# ---------------------------------------------------------------- session helpers

def capture(display: str, out: Path) -> None:
    subprocess.run(["import", "-display", display, "-window", "root", str(out)], check=True,
                   capture_output=True, timeout=30)


def crop_pixels(png: Path, box: tuple[int, int, int, int]) -> list[tuple[int, int, int]]:
    with base.Image.open(png) as image:
        return list(image.convert("RGB").crop(box).getdata())


def gdbus(ctx: base.Ctx, *argv: str) -> str:
    result = subprocess.run(["gdbus", "call", "--session", "--dest", BUS_NAME, "--object-path", OBJECT_PATH,
                             "--method", *argv], env=ctx.app_env, capture_output=True, text=True, timeout=20,
                            check=False)
    if result.returncode:
        raise RuntimeError(f"gdbus {argv[-1]} failed: {result.stderr.strip()[:200]}")
    return result.stdout


def settings_revision(ctx: base.Ctx) -> int:
    value = base.gvariant_field_u64(gdbus(ctx, f"{IFACE}.GetSnapshot"), "revision")
    if value is None:
        raise RuntimeError("settings snapshot has no revision")
    return value


def apply(ctx: base.Ctx, changes: dict[str, str]) -> str:
    """Settings.Apply with the snapshot revision; the value text is GVariant (<'str'> or <true>)."""
    revision = settings_revision(ctx)
    argv = ["gdbus", "call", "--session", "--dest", BUS_NAME, "--object-path", OBJECT_PATH, "--method",
            f"{IFACE}.Apply", f"uint64 {revision}", "{" + ", ".join(
                f"'{k}': {v}" for k, v in changes.items()) + "}"]
    result = subprocess.run(argv, env=ctx.app_env, capture_output=True, text=True, timeout=20, check=False)
    if result.returncode:
        raise RuntimeError(f"Settings.Apply failed: {result.stderr.strip()[:200]}")
    time.sleep(1.5)
    return result.stdout.strip()


def text_value(value: str) -> str:
    return f"<'{value}'>"


def bool_value(value: bool) -> str:
    return f"<{'true' if value else 'false'}>"


def ocr_word(session: cmw.Session, shot: str, words: set[str], region=None, name=None, overrides=None):
    """Center of the first OCR word in WORDS (lower-case) on a fresh capture; an override wins."""
    if overrides and name in overrides:
        return overrides[name]
    path, _ = session.shot(f"locate-{shot}")
    if path is None:
        return None
    found = cmw.ocr_locate(path, lambda text: text in words, region)
    return None if not found else (found[0][1], found[0][2])


def wait_mapped(session: cmw.Session, seconds: float = 1.0) -> None:
    time.sleep(seconds)


# ---------------------------------------------------------------- sections

class EditorNotDrawn(RuntimeError):
    """The rename editor did not paint after its Rename row was clicked (a product result, not a harness gap)."""


def editor_drawn(png: Path, icon: tuple[int, int]) -> bool:
    """True when the editor's #202326 background shows in the 220x48 box under the entry (desktop-rename)."""
    x0, y0 = max(0, icon[0] - 40), icon[1] + 40
    pixels = crop_pixels(png, (x0, y0, x0 + 230, y0 + 60))
    dark = sum(1 for r, g, b in pixels if abs(r - 0x20) < 6 and abs(g - 0x23) < 6 and abs(b - 0x26) < 6)
    return dark > 300


def section_rename(session: cmw.Session, shots: Path, desktop: Path, overrides: dict, results: dict) -> None:
    ctx = session.ctx
    point = overrides.get("desktop", DESKTOP_POINT)
    checks: dict[str, dict] = {}

    def create_folder(tag: str) -> tuple[str, tuple[int, int]]:
        before = sorted(os.listdir(desktop))
        empty = shots / f"rename-{tag}-desktop-empty.png"
        capture(ctx.display, empty)
        session.rclick(point)
        wait_mapped(session)
        capture(ctx.display, shots / f"rename-{tag}-desktop-menu.png")
        at = ocr_word(session, "menu-create", {"create"}, (point[0], point[1], 320, 260), "menu-create", overrides)
        if at is None:
            raise RuntimeError("Create New Folder row not found (pass --override menu-create=X,Y)")
        session.click(at)
        wait_mapped(session, 1.5)
        after = sorted(os.listdir(desktop))
        created = new_entries(before, after)
        if len(created) != 1:
            raise RuntimeError(f"expected one new Desktop entry, got {created}")
        filled = shots / f"rename-{tag}-desktop-created.png"
        capture(ctx.display, filled)
        # Target the entry itself: the blue glyph that appeared in the left desktop column.
        column = (0, 0, 300, 700)
        icon = blue_icon_center(crop_pixels(empty, column), crop_pixels(filled, column), column[2])
        if icon is None:
            raise RuntimeError(f"new folder {created[0]!r} glyph not located on the desktop")
        return created[0], icon

    def rename_entry(old: str, new_text: str, action: str, located: tuple[int, int]) -> dict:
        icon = overrides.get(f"entry-{action}", located)
        session.rclick(icon)
        wait_mapped(session)
        capture(ctx.display, shots / f"rename-{action}-entry-menu.png")
        at = ocr_word(session, f"menu-rename-{old}", {"rename"}, (icon[0], icon[1], 320, 200), "menu-rename", overrides)
        if at is None:
            raise RuntimeError("Rename row not found")
        session.click(at)
        capture(ctx.display, shots / f"rename-{action}-after-row.png")
        wait_mapped(session)
        capture(ctx.display, shots / f"rename-{action}-editor.png")
        if not editor_drawn(shots / f"rename-{action}-editor.png", icon):
            raise EditorNotDrawn(f"rename editor not drawn after Rename click (see rename-{action}-after-row.png)")
        for _ in range(len(old) + 8):
            session.xtest("--key", "BackSpace")
        for ch in new_text:
            session.xtest("--key", ch if ch != "-" else "minus")
        wait_mapped(session, 0.5)
        capture(ctx.display, shots / f"rename-{action}-typed.png")
        return {"icon": icon}

    try:
        old, located = create_folder("ok")
        info = rename_entry(old, NEW_NAME, "ok", located)
        session.xtest("--key", "Return")  # Enter confirms the rename
        wait_mapped(session, 1.5)
        after = sorted(os.listdir(desktop))
        capture(ctx.display, shots / "rename-ok-after.png")
        checks["ok"] = {"old": old, "entries": after, "renamed": NEW_NAME in after and old not in after,
                        "icon": info["icon"], "confirm": "Return"}

        second, located2 = create_folder("cancel")
        info2 = rename_entry(second, CANCEL_NAME, "cancel", located2)
        session.escape()  # Escape cancels the rename
        wait_mapped(session, 1.5)
        after2 = sorted(os.listdir(desktop))
        capture(ctx.display, shots / "rename-cancel-after.png")
        checks["cancel"] = {"old": second, "entries": after2, "kept": second in after2 and CANCEL_NAME not in after2,
                            "icon": info2["icon"], "cancel": "Escape"}
    except EditorNotDrawn as error:
        results["rename"] = {"status": FAIL, "reason": str(error), "checks": checks}
        return
    except RuntimeError as error:  # a control could not be located on screen: the run is incomplete, not a verdict
        results["rename"] = {"status": NOT_RUN, "reason": str(error), "checks": checks}
        return
    ok = checks["ok"]["renamed"] and checks["cancel"]["kept"]
    results["rename"] = {"status": PASS if ok else FAIL, "checks": checks,
                         "reason": "OK renamed on disk; Cancel left the entry unchanged" if ok else "disk mismatch"}


def cursor_digest(display: str) -> int | None:
    pixels = cursor_pixels_from_xfixes(display)
    return None if pixels is None else cursor_signature(pixels)


def section_live(session: cmw.Session, shots: Path, results: dict, args) -> None:
    ctx = session.ctx
    evidence: dict = {"cursor_digests": {}}
    try:
        (ctx.run / "home" / "Desktop" / LIVE_LAUNCHER_NAME).write_text(LIVE_LAUNCHER, encoding="ascii")
        # Restart the desktop with the session's XCURSOR_PATH so Breeze_Light and Adwaita resolve differently.
        ctx.stop(session.desktop)
        session.desktop = ctx.spawn("flamewm-desktop", [args.desktop_bin],
                                    session.session_env({"XCURSOR_PATH": DESKTOP_XCURSOR_PATH}))
        time.sleep(args.settle_seconds)
        session.xtest("--move", "900,300")  # pointer over the desktop so its cursor shape is the root cursor
        time.sleep(1.0)
        before_desktop = shots / "live-desktop-before.png"
        capture(ctx.display, before_desktop)
        evidence["cursor_digests"]["unset"] = cursor_digest(ctx.display)
        before_snapshot = gdbus(ctx, f"{IFACE}.GetSnapshot")
        accent = pick_other(base.gvariant_field_string(before_snapshot, "accent"), ACCENTS)
        icon_theme = pick_other(base.gvariant_field_string(before_snapshot, "iconTheme"), ICON_THEMES)
        evidence.update(accent_applied=accent, icon_theme_applied=icon_theme,
                        accent_before=base.gvariant_field_string(before_snapshot, "accent"),
                        icon_theme_before=base.gvariant_field_string(before_snapshot, "iconTheme"))
        settings = subprocess.Popen([args.settings_bin], env=session.session_env(), stdout=subprocess.DEVNULL,
                                    stderr=subprocess.DEVNULL, cwd=str(ctx.run))
        ctx.owned.append(base.Owned("flamewm-settings", settings))
        time.sleep(args.settle_seconds)
        capture(ctx.display, shots / "live-settings-before.png")
        apply(ctx, {"accent": text_value(accent), "iconTheme": text_value(icon_theme),
                    "cursorTheme": text_value("Breeze_Light")})
        time.sleep(2.0)
        capture(ctx.display, shots / "live-settings-after.png")
        evidence["cursor_digests"]["Breeze_Light"] = cursor_digest(ctx.display)
        # Close Settings so the desktop icon column and taskbar are not covered by the window.
        ctx.stop(next(o for o in ctx.owned if o.proc is settings))
        time.sleep(1.5)
        apply(ctx, {"cursorTheme": text_value("Adwaita")})
        time.sleep(2.0)
        evidence["cursor_digests"]["Adwaita"] = cursor_digest(ctx.display)
        after_png = shots / "live-after.png"
        capture(ctx.display, after_png)
        snapshot = gdbus(ctx, f"{IFACE}.GetSnapshot")
        evidence["accent_read_back"] = base.gvariant_field_string(snapshot, "accent")
        evidence["cursor_theme_read_back"] = base.gvariant_field_string(snapshot, "cursorTheme")
        evidence["icon_theme_read_back"] = base.gvariant_field_string(snapshot, "iconTheme")
        with base.Image.open(after_png) as image:
            full = (0, 0, *image.size)
        evidence["accent_changed_px"] = changed_pixels(crop_pixels(before_desktop, full), crop_pixels(after_png, full))
        icon_box = (0, 0, 200, 600)  # desktop icon column at top-left: file, folder and the launcher row
        evidence["icon_region_changed_px"] = changed_pixels(
            crop_pixels(before_desktop, icon_box), crop_pixels(after_png, icon_box))
    except Exception as error:
        results["live"] = {"status": FAIL if "Settings" in str(error) else NOT_RUN,
                           "reason": f"{type(error).__name__}: {error}", "evidence": evidence}
        return
    digests = [d for d in evidence["cursor_digests"].values() if d is not None]
    evidence["cursor_changed"] = len(set(digests)) > 1
    applied = (evidence["accent_read_back"] == evidence["accent_applied"]
               and evidence["icon_theme_read_back"] == evidence["icon_theme_applied"]
               and evidence["cursor_theme_read_back"] == "Adwaita")
    visible = evidence["accent_changed_px"] > PIXEL_THRESHOLD and evidence["icon_region_changed_px"] > PIXEL_THRESHOLD
    status = PASS if applied and evidence["cursor_changed"] and visible else FAIL
    results["live"] = {"status": status, "evidence": evidence,
                       "reason": "Settings.Apply read back; accent and icon region pixels changed; cursor digest "
                       "changed with cursorTheme" if status == PASS else "live propagation not evidenced (see evidence)"}


TASKBAR_STRIP = (1000, 748, 1280, 800)  # right side of the 1280x800 taskbar (tray, pager and clock)
CLOCK_BOX = (1196, 748, 1280, 800)
PIXEL_THRESHOLD = 200  # changed pixels needed to call a strip "changed"


def section_taskbar(session: cmw.Session, shots: Path, results: dict) -> None:
    ctx = session.ctx
    out: dict[str, dict] = {}
    try:
        base_png = shots / "taskbar-baseline.png"
        capture(ctx.display, base_png)
        baseline = crop_pixels(base_png, TASKBAR_STRIP)
        for item, key in TOGGLES.items():
            apply(ctx, {key: bool_value(False)})
            off_png = shots / f"taskbar-{item}-off.png"
            capture(ctx.display, off_png)
            apply(ctx, {key: bool_value(True)})
            on_png = shots / f"taskbar-{item}-on.png"
            capture(ctx.display, on_png)
            box = CLOCK_BOX if item == "clock" else TASKBAR_STRIP
            ref = crop_pixels(base_png, box)
            removed = changed_pixels(ref, crop_pixels(off_png, box))
            restored = changed_pixels(ref, crop_pixels(on_png, box))
            status = item_verdict(removed, restored, PIXEL_THRESHOLD)
            out[item] = {"key": key, "status": status, "removed_changed_px": removed,
                         "restored_changed_px": restored, "shots": [off_png.name, on_png.name]}
    except Exception as error:
        results["taskbar"] = {"status": FAIL, "reason": f"{type(error).__name__}: {error}", "items": out}
        return
    statuses = [v["status"] for v in out.values()]
    overall = FAIL if FAIL in statuses else (NOT_RUN if NOT_PRESENT in statuses else PASS)
    results["taskbar"] = {"status": overall, "items": out,
                          "reason": "per-item pixel verdicts; NOT_PRESENT = nothing rendered to toggle"}


# ---------------------------------------------------------------- run

def start_audio(ctx: base.Ctx) -> list[str]:
    """Private pipewire + pipewire-pulse under the test XDG_RUNTIME_DIR, so the Shell sees a pulse server."""
    started = []
    for name, argv in (("pipewire", ["pipewire"]), ("wireplumber", ["wireplumber"]),
                       ("pipewire-pulse", ["pipewire-pulse"])):
        ctx.spawn(name, argv, ctx.app_env)
        started.append(name)
        time.sleep(1.0)
    return started


def run(args: argparse.Namespace) -> int:
    run_dir = Path(args.run_dir).absolute()
    if not tray.display_allowed(args.display):
        print(f"refusing display {args.display!r}", file=sys.stderr)
        return 2
    if run_dir.exists() and any(run_dir.iterdir()):
        print(f"run dir must be new or empty: {run_dir}", file=sys.stderr)
        return 2
    for sub in ("home", "xdg-config", "xdg-data", "xdg-cache", "xdg-runtime", "xdg-state", "shots", "logs"):
        (run_dir / sub).mkdir(parents=True, exist_ok=True)
    os.chmod(run_dir / "xdg-runtime", 0o700)
    shots = run_dir / "shots"
    overrides = parse_override(args.override)
    ctx = base.Ctx(run_dir)
    results: dict[str, dict] = {}
    audio: list[str] = []
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
        only = set(args.only.split(",")) if args.only else {"rename", "live", "taskbar"}
        if "rename" in only:
            # The seeded file icon sits in the first grid cell; drop it so new entries do not stack on it.
            (run_dir / "home" / "Desktop" / cmw.FILE_NAME).unlink()
        if args.with_audio:
            audio = start_audio(ctx)
        session = cmw.Session(ctx, args)
        if not session.start():
            raise RuntimeError("WM, Shell or desktop did not start")
        desktop = run_dir / "home" / "Desktop"
        if "rename" in only:
            section_rename(session, shots, desktop, overrides, results)
        if "live" in only:
            section_live(session, shots, results, args)
        if "taskbar" in only:
            section_taskbar(session, shots, results)
    except Exception as error:
        results.setdefault("harness", {"status": NOT_RUN, "reason": f"{type(error).__name__}: {error}"})
    finally:
        teardown = ctx.teardown()
    verdict = ("FAIL" if any(r["status"] == FAIL for r in results.values())
               else ("PASS" if results and all(r["status"] == PASS for r in results.values()) else "INCOMPLETE"))
    doc = {"schema": SCHEMA, "verdict": verdict, "sections": results, "display": ctx.display,
           "audio_started": audio, "teardown": teardown}
    write_json(run_dir / "result.json", doc)
    print(json.dumps(doc, indent=2))
    return 0 if verdict == "PASS" else (1 if verdict == "FAIL" else 3)


def parse_args(argv: list[str]) -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--run-dir", required=True)
    p.add_argument("--display", default=DEFAULT_DISPLAY)
    p.add_argument("--geometry", default="1280x800")
    p.add_argument("--wm-bin", required=True)
    p.add_argument("--shell-bin", required=True)
    p.add_argument("--settings-bin", required=True)
    p.add_argument("--desktop-bin", required=True)
    p.add_argument("--client-bin", default="xclock")
    p.add_argument("--settle-seconds", type=float, default=3.0)
    p.add_argument("--only", default="")
    p.add_argument("--with-audio", action="store_true")
    p.add_argument("--override", action="append", default=[], metavar="NAME=X,Y")
    args = p.parse_args(argv)
    args.task_xy = args.pager_xy = args.file_xy = args.folder_xy = args.sticky_xy = args.weather_xy = None
    return args


def main(argv: list[str] | None = None) -> int:
    raw = sys.argv[1:] if argv is None else list(argv)
    args = parse_args(raw)
    if os.environ.get("FLAMEWM_DTA_INNER") != "1":
        # Re-exec on a private session bus so no host D-Bus service (or host pulse/pipewire) is reachable.
        if shutil.which("dbus-run-session") is None:
            print("error: dbus-run-session is required for a private session bus", file=sys.stderr)
            return 3
        child = subprocess.Popen(["dbus-run-session", "--", sys.executable, str(Path(__file__).resolve()), *raw],
                                 env=dict(os.environ, FLAMEWM_DTA_INNER="1"))
        return child.wait()
    return run(args)


if __name__ == "__main__":
    raise SystemExit(main())

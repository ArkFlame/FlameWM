#!/usr/bin/env python3
"""Native icon- and cursor-theme install acceptance (TI01..TI10) on a private Xephyr.

Usage:
  tools/flamewm_theme_install_acceptance.py --run-dir DIR [--geometry WxH] [--display :N]
      [--wm-bin PATH] [--shell-bin PATH] [--settings-bin PATH] [--settle-seconds S]

What it does:
  * creates a fresh private run directory DIR (must not exist or be empty);
  * re-execs itself once under `dbus-run-session`;
  * starts an owned Xephyr, the owned WM and Shell, and per flow one owned
    `flamewm-settings --page appearance` plus an owned fake FileChooser portal
    (tools/flamewm_settings_fake_portal.py) that returns the fixture source;
  * clicks the real "Install folder…" / "Install archive…" buttons (located by OCR after
    wheel scrolling), then checks the installed tree on disk, the status text, the icon
    theme picker listing and selection, and the authoritative Settings snapshot;
  * runs negative controls (escaping symlink folder, corrupt archive) that must be rejected
    with a visible error and must leave nothing installed;
  * writes DIR/result.json. Statuses: PASS, FAIL, NOT_RUN, ENVIRONMENT_BLOCKED.

Exit status: 0 when every TI entry is PASS, 1 when any entry FAILs, 2 usage error,
3 environment blocked or not run.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import secrets
import shutil
import struct
import subprocess
import sys
import tarfile
import time
from pathlib import Path

from flamewm_harness_common import write_json

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))

import flamewm_appearance_acceptance as ap  # noqa: E402  (Session, OCR helper, driver pattern)
import flamewm_settings_acceptance as base  # noqa: E402  (Ctx, capture, XTEST, snapshot readers)

try:
    from PIL import ImageOps
except ImportError:  # the run is ENVIRONMENT_BLOCKED before OCR when PIL is missing
    ImageOps = None

ROOT = base.ROOT
SCHEMA = "flamewm-theme-install-acceptance/1"
PORTAL_HELPER = TOOLS / "flamewm_settings_fake_portal.py"
IDS = tuple(f"TI{n:02d}" for n in range(1, 11))
TITLES = {
    "TI01": "theme fixtures: icon/cursor folders and tar.gz, escaping symlinks, corrupt archive and Xcursor",
    "TI02": "Install folder button installs the theme under XDG_DATA_HOME/icons",
    "TI03": "icon theme picker lists the installed theme and selecting it sets iconTheme",
    "TI04": "escaping-symlink folder is rejected with an error and nothing is installed",
    "TI05": "corrupt archive is rejected with an error and nothing is installed",
    "TI06": "Install archive button installs the tar.gz theme; picker selects it",
    "TI07": "Install folder button installs the cursor theme under XDG_DATA_HOME/icons",
    "TI08": "cursor theme picker lists the installed theme with a left_ptr preview and selecting it sets cursorTheme",
    "TI09": "Install archive button installs the cursor tar.gz theme",
    "TI10": "escaping cursors/left_ptr folder is rejected; corrupt Xcursor installs with its preview omitted",
}
PASS, FAIL = base.STATUS_PASS, base.STATUS_FAIL
NOT_RUN, ENV_BLOCKED = base.STATUS_NOT_RUN, base.STATUS_ENV_BLOCKED
STATUSES = (PASS, FAIL, NOT_RUN, ENV_BLOCKED)

THEME_ID = "AP-Test-Icons"
ESCAPE_ID = "AP-Test-Escape"
CORRUPT_ID = "AP-Test-Corrupt"
THEME_FILE = "folder.png"
SETTINGS_PAGE = "appearance"
SETTINGS_SIZE = (960, 640)
WHEEL_STEPS = 5
MAX_WHEEL_ROUNDS = 30
WAIT_SECONDS = 15.0
OCR_SCALE = 3
OCR_THRESHOLDS = (150, 110)  # dark Settings theme: OCR reads inverted, thresholded text; 150 reads the status line
INSTALL_WORDS = {"install"}
FOLDER_WORDS = {"folder"}
ARCHIVE_WORDS = {"archive"}
ROW_TOLERANCE_PX = 12
PICKER_ROW_WORDS = {THEME_ID.lower()}
CURSOR_ID = "AP-Test-Cursors"
CURSOR_ESCAPE_ID = "AP-Test-Cursor-Escape"
CURSOR_CORRUPT_ID = "AP-Test-Cursor-Corrupt"
CURSOR_FILE = "left_ptr"
CURSOR_ESCAPE_TARGET = "/etc/passwd"  # host file: only ever symlinked, never read by the runner
XCURSOR_SIZE = 32
XCURSOR_RGB = (60, 130, 220)  # the left_ptr preview colour the picker row must show
XCURSOR_MAGIC = b"Xcur"
XCURSOR_IMAGE_TYPE = 0xFFFD0002
PREVIEW_MIN_PIXELS = 40  # a 32x32 solid preview has ~1000 matching pixels; the placeholder has none
PREVIEW_LEFT_PX = 170  # preview crop: this far left of the row name, PREVIEW_HALF_PX above and below it
PREVIEW_HALF_PX = 22
PICKER_KINDS = {  # picker -> (OCR title word, settings snapshot key)
    "icon": ("icon", "iconTheme"),
    "cursor": ("cursor", "cursorTheme"),
}


# ---------------------------------------------------------------- pure helpers (unit-tested)

def theme_index_text(name: str) -> str:
    return ("[Icon Theme]\n"
            f"Name={name}\n"
            "Comment=FlameWM native theme install acceptance fixture\n"
            "Inherits=hicolor\n"
            "Directories=apps/48\n"
            "\n"
            "[apps/48]\n"
            "Size=48\n"
            "Context=Applications\n"
            "Type=Fixed\n")


def write_theme_folder(root: Path, name: str, escape_target: Path | None = None) -> Path:
    """A minimal valid icon theme tree; `escape_target` adds a symlink leaving the tree."""
    if base.Image is None:
        raise RuntimeError("PIL is required to build the icon fixture")
    (root / "apps" / "48").mkdir(parents=True, exist_ok=True)
    (root / "index.theme").write_text(theme_index_text(name), encoding="utf-8")
    base.Image.new("RGBA", (48, 48), (60, 130, 220, 255)).save(root / "apps" / "48" / THEME_FILE)
    if escape_target is not None:
        os.symlink(escape_target, root / "apps" / "48" / "escape.png")
    return root


def cursor_index_text(name: str) -> str:
    """No Directories= key: the importer classifies a folder with cursors/ and no icon dirs as a cursor theme."""
    return ("[Icon Theme]\n"
            f"Name={name}\n"
            "Comment=FlameWM native cursor theme install acceptance fixture\n")


def xcursor_bytes(size: int = XCURSOR_SIZE, rgb: tuple[int, int, int] = XCURSOR_RGB) -> bytes:
    """One opaque 'size'x'size' image frame: 16-byte file header, one TOC entry, 36-byte chunk header, ARGB pixels."""
    header = struct.pack("<4sIII", XCURSOR_MAGIC, 16, 0x00010000, 1)
    toc = struct.pack("<III", XCURSOR_IMAGE_TYPE, size, 16 + 12)
    chunk = struct.pack("<9I", 36, XCURSOR_IMAGE_TYPE, size, 1, size, size, 0, 0, 0)
    pixel = bytes((rgb[2], rgb[1], rgb[0], 255))  # little-endian ARGB words are stored as B, G, R, A
    return header + toc + chunk + pixel * (size * size)


def write_cursor_folder(root: Path, name: str, left_ptr: bytes | None = None, escape: bool = False) -> Path:
    """A minimal cursor theme: index.theme plus cursors/left_ptr and an 'arrow' link.

    `escape` replaces left_ptr with a symlink to an absolute host path, which the importer must refuse."""
    cursors = root / "cursors"
    cursors.mkdir(parents=True, exist_ok=True)
    (root / "index.theme").write_text(cursor_index_text(name), encoding="utf-8")
    if escape:
        os.symlink(CURSOR_ESCAPE_TARGET, cursors / CURSOR_FILE)
    else:
        (cursors / CURSOR_FILE).write_bytes(xcursor_bytes() if left_ptr is None else left_ptr)
        os.symlink(CURSOR_FILE, cursors / "arrow")
    return root


def preview_visible(pixel_count: int | None) -> bool:
    """True when the picker row shows the left_ptr preview colour (None means the crop was off-screen)."""
    return pixel_count is not None and pixel_count >= PREVIEW_MIN_PIXELS


def write_theme_archive(folder: Path, out: Path) -> Path:
    with tarfile.open(out, "w:gz") as archive:
        archive.add(folder, arcname=folder.name)
    return out


def write_corrupt_archive(out: Path) -> Path:
    out.write_bytes(b"\x1f\x8b not a gzip stream\n")
    return out


def normalize_word(token: str) -> str:
    """Lower-case OCR token without surrounding punctuation: 'folder...' and 'failed:' compare as 'folder', 'failed'."""
    return re.sub(r"^\W+|\W+$", "", token.strip().lower())


def locate_pair(centres: list[tuple[str, int, int]], label_words: set[str],
                anchor_words: set[str] = INSTALL_WORDS, row_tolerance: int = ROW_TOLERANCE_PX) -> tuple[int, int] | None:
    """Centre of the label word that sits right of an anchor word on the same OCR row, or None."""
    anchors = [(x, y) for word, x, y in centres if word in anchor_words]
    labels = sorted(((x, y) for word, x, y in centres if word in label_words), key=lambda p: (p[1], p[0]))
    for label_x, label_y in labels:
        if any(abs(label_y - anchor_y) <= row_tolerance and anchor_x < label_x for anchor_x, anchor_y in anchors):
            return label_x, label_y
    return None


def icon_title_point(centres: list[tuple[str, int, int]], title_word: str = "icon") -> tuple[int, int] | None:
    """The 'Icon theme' (or 'Cursor theme') title: its word directly above an 'Installed …' note at the same left edge.

    The page subtitle 'Accent, icon theme and live preview' also contains 'icon' but has no note below it.
    """
    installed = [(x, y) for word, x, y in centres if word == "installed"]
    for word, x, y in centres:
        if word == title_word and any(abs(nx - x) <= 8 and 8 <= ny - y <= 30 for nx, ny in installed):
            return x, y
    return None


def picker_row_point(centres: list[tuple[str, int, int]], theme_id: str = THEME_ID) -> tuple[int, int] | None:
    """A picker row name: the theme name on a row that is not the 'Installed <name>' status line."""
    status_rows = [y for word, _, y in centres if word == "installed"]
    for word, x, y in centres:
        if word == normalize_word(theme_id) and not any(abs(y - row) <= ROW_TOLERANCE_PX for row in status_rows):
            return x, y
    return None


def preview_rect(row: tuple[int, int]) -> tuple[int, int, int, int]:
    """The crop left of a picker row name, where the row's preview image sits (x, y, w, h)."""
    x, y = row
    return x - PREVIEW_LEFT_PX, y - PREVIEW_HALF_PX, PREVIEW_LEFT_PX - 4, 2 * PREVIEW_HALF_PX


def pointer_for(geometry: dict) -> str:
    return f"{geometry['x'] + geometry['width'] // 2},{geometry['y'] + geometry['height'] // 2}"


def expected_entry_status(observations: dict[str, bool | None], required: tuple[str, ...]) -> tuple[str, str]:
    missing = [key for key in required if observations.get(key) is None]
    failed = [key for key in required if observations.get(key) is False]
    if failed:
        return FAIL, "not satisfied: " + ", ".join(failed)
    if missing:
        return NOT_RUN, "not observed: " + ", ".join(missing)
    return PASS, "all observations satisfied"


def compose_document(results: dict[str, dict]) -> dict:
    """Every TI ID appears once; any FAIL wins, all PASS is 0, anything else is blocked (3)."""
    ordered = [results.get(sid, entry(sid, NOT_RUN, "not reached")) for sid in IDS]
    summary = {status: sum(1 for e in ordered if e["status"] == status) for status in STATUSES}
    if summary[FAIL]:
        verdict, code = "FAIL", 1
    elif summary[PASS] == len(IDS):
        verdict, code = "ALL_SCENARIOS_PASS", 0
    elif summary[ENV_BLOCKED] == len(IDS):
        verdict, code = "ENVIRONMENT_BLOCKED", 3
    else:
        verdict, code = "INCOMPLETE", 3
    return {"schema": SCHEMA, "results": ordered, "summary": summary, "verdict": verdict, "exit_code": code}


def entry(sid: str, status: str, reason: str = "", evidence: list | None = None, commands: list | None = None) -> dict:
    if status not in STATUSES:
        raise ValueError(f"unknown status {status!r}")
    return {"id": sid, "title": TITLES[sid], "status": status, "reason": reason,
            "evidence": evidence or [], "command": " && ".join(commands or []) or "none"}


def parse_args(argv: list[str]) -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--run-dir", required=True)
    p.add_argument("--geometry", default="1280x800")
    p.add_argument("--display", default=None)
    p.add_argument("--wm-bin", default=str(ROOT / ".target/release/flamewm"))
    p.add_argument("--shell-bin", default=str(ROOT / ".target/release/flamewm-shell"))
    p.add_argument("--settings-bin", default=str(ROOT / ".target/release/flamewm-settings"))
    p.add_argument("--settle-seconds", type=float, default=3.0)
    args = p.parse_args(argv)
    if not re.fullmatch(r"\d+x\d+", args.geometry):
        p.error("--geometry must be WxH")
    args.screen_w, args.screen_h = (int(v) for v in args.geometry.split("x"))
    return args


# ---------------------------------------------------------------- live helpers

def wait_for_report(report: Path, owned: base.Owned, timeout: float) -> dict | None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            state = json.loads(report.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            state = None
        if state and state.get("ready"):
            return state
        if owned.proc.poll() is not None:
            return None
        time.sleep(0.05)
    return None


def ocr_centres(png: Path, words: set[str]) -> list[tuple[str, int, int]]:
    """(word, x, y) screen-coordinate centres of wanted words in a dark Settings screenshot."""
    tesseract = shutil.which("tesseract")
    if not tesseract or base.Image is None or ImageOps is None:
        return []
    wanted = {normalize_word(word) for word in words}
    found = []
    with base.Image.open(png) as image:
        inverted = ImageOps.invert(ImageOps.autocontrast(image.convert("L")))
        scaled = inverted.resize((inverted.width * OCR_SCALE, inverted.height * OCR_SCALE))
    for threshold in OCR_THRESHOLDS:
        prepared = png.with_name(f"{png.stem}-ocr{threshold}.png")
        scaled.point(lambda v: 255 if v > threshold else 0).save(prepared)
        result = subprocess.run([tesseract, str(prepared), "stdout", "--psm", "11", "tsv"], capture_output=True,
                                text=True, timeout=120, check=False)
        for line in result.stdout.splitlines()[1:]:
            cols = line.split("\t")
            if len(cols) < 12 or normalize_word(cols[11]) not in wanted:
                continue
            left, top, width, height = (int(v) for v in cols[6:10])
            found.append((normalize_word(cols[11]), (left + width // 2) // OCR_SCALE, (top + height // 2) // OCR_SCALE))
    return found


def ocr_words(s: ap.Session, name: str, words: set[str]) -> tuple[list[tuple[str, int, int]], Path | None]:
    path, _ = s.shot(name)
    if path is None:
        return [], None
    return ocr_centres(path, words), path


def scroll_until(s: ap.Session, geometry: dict, words: set[str], locate, label: str, evidence: list,
                 commands: list, wheel: str = "down") -> tuple[int, int] | None:
    """Wheel the Appearance page (down for install buttons, up for a title above) until locate(centres) finds a point."""
    for round_index in range(MAX_WHEEL_ROUNDS + 1):
        if round_index:
            code, command = s.input(move=pointer_for(geometry), wheel=f"{wheel}:{WHEEL_STEPS}")
            commands.append(command)
            if code != 0:
                return None
            time.sleep(0.5)
        centres, path = ocr_words(s, f"ti-{label}-scroll-{round_index}", words)
        if path is not None:
            evidence.append(str(path))
        point = locate(centres)
        if point is not None:
            return point
    return None


def installed_status_seen(centres: list[tuple[str, int, int]], theme_id: str = THEME_ID) -> bool:
    """'Installed <name>' on one row. A bare 'Installed' also appears in the idle 'Installed icon themes' note."""
    installed_rows = [y for word, _, y in centres if word == "installed"]
    name_rows = [y for word, _, y in centres if word == normalize_word(theme_id)]
    return any(abs(a - b) <= ROW_TOLERANCE_PX for a in installed_rows for b in name_rows)


def failed_status_seen(centres: list[tuple[str, int, int]]) -> bool:
    return any(word == "failed" for word, _, _ in centres)


def await_status(s: ap.Session, name: str, words: set[str], done, evidence: list) -> tuple[bool, list]:
    """Poll screenshots until done(centres) holds or WAIT_SECONDS passes; returns (seen, last centres)."""
    deadline = time.monotonic() + WAIT_SECONDS
    centres: list[tuple[str, int, int]] = []
    while time.monotonic() < deadline:
        centres, path = ocr_words(s, name, words)
        if path is not None:
            evidence.append(str(path))
        if done(centres):
            return True, centres
        time.sleep(0.7)
    return False, centres


def settings_env(ctx: base.Ctx, empty_system_dirs: Path) -> dict[str, str]:
    """Settings sees only the private user data home, never the host's installed themes."""
    return dict(ctx.app_env, XDG_DATA_DIRS=str(empty_system_dirs))


def open_settings(s: ap.Session, flow: str, portal_mode: str, portal_image: Path, evidence: list,
                  commands: list) -> dict | None:
    """Start the owned portal and Settings for one flow; returns handles or None on failure."""
    ctx = s.ctx
    report = ctx.run / "logs" / f"{flow}-portal.json"
    portal_argv = [sys.executable, str(PORTAL_HELPER), "--mode", portal_mode, "--report", str(report),
                   "--image", str(portal_image)]
    portal = ctx.spawn(f"{flow}-portal", portal_argv, ctx.app_env)
    commands.append(base.shell(portal_argv))
    if wait_for_report(report, portal, 5.0) is None:
        ctx.stop(portal)
        return None
    settings_argv = [s.args.settings_bin, "--page", SETTINGS_PAGE,
                     "--window-size", f"{SETTINGS_SIZE[0]}x{SETTINGS_SIZE[1]}"]
    settings = ctx.spawn(f"{flow}-settings", settings_argv, settings_env(ctx, ctx.run / "xdg-system-empty"))
    commands.append(base.shell(settings_argv))
    time.sleep(s.args.settle_seconds)
    geometry = base.wait_for_settings_window(ctx, ctx.run / "logs" / f"{flow}-window.txt")
    if geometry is None or (geometry["width"], geometry["height"]) != SETTINGS_SIZE:
        ctx.stop(settings)
        ctx.stop(portal)
        return None
    evidence.append(str(ctx.run / "logs" / f"{flow}-window.txt"))
    return {"portal": portal, "settings": settings, "geometry": geometry, "report": report}


def click_install(s: ap.Session, handles: dict, flow: str, button: str, evidence: list, commands: list) -> dict:
    """Scroll to and click the Install folder/archive button; returns the observation fragment."""
    label_words = FOLDER_WORDS if button == "folder" else ARCHIVE_WORDS
    point = scroll_until(s, handles["geometry"], INSTALL_WORDS | label_words,
                         lambda centres: locate_pair(centres, label_words), button, evidence, commands)
    if point is None:
        return {"click_located": False}
    before, _ = s.shot(f"ti-{flow}-before-click")
    if before is not None:
        evidence.append(str(before))
    code, command = s.input(click=f"{point[0]},{point[1]}")
    commands.append(command)
    time.sleep(0.5)
    invocation = None
    report = handles["report"]
    deadline = time.monotonic() + WAIT_SECONDS
    while time.monotonic() < deadline:
        try:
            calls = json.loads(report.read_text(encoding="utf-8")).get("invocations", [])
        except (OSError, ValueError):
            calls = []
        if calls:
            invocation = calls[-1]
            break
        time.sleep(0.1)
    return {"click_located": True, "click_exit": code, "click": point,
            "portal_invocation": invocation,
            "portal_called": bool(invocation) and invocation.get("title") == "Choose Theme"}


def read_icons_dir(ctx: base.Ctx) -> Path:
    return ctx.run / "xdg-data" / "icons"


def staging_leftovers(icons: Path) -> list[str]:
    if not icons.is_dir():
        return []
    return sorted(p.name for p in icons.iterdir() if p.name.startswith((".flamewm-staging-", ".flamewm-replaced-")))


def installed_matches(source: Path, installed: Path) -> bool:
    """Every regular file of the source tree exists in the installed tree with identical bytes."""
    if not (installed / "index.theme").is_file():
        return False
    for path in source.rglob("*"):
        if path.is_file():
            target = installed / path.relative_to(source)
            if not target.is_file() or target.read_bytes() != path.read_bytes():
                return False
    return True


def picker_select(s: ap.Session, handles: dict, flow: str, kind: str, theme_id: str, select: bool,
                  evidence: list, commands: list) -> dict:
    """Open the icon or cursor picker, OCR-locate the installed name, check its preview; select it when asked."""
    geometry = handles["geometry"]
    word, key = PICKER_KINDS[kind]
    title = scroll_until(s, geometry, {word, "installed"}, lambda found: icon_title_point(found, word),
                         f"{flow}-{kind}-title", evidence, commands, wheel="up")
    if title is None:
        return {"picker_opened": False, "picker_listed": False}
    code, command = s.input(click=f"{geometry['x'] + geometry['width'] - 120},{title[1]}")
    commands.append(command)
    listed, centres = await_status(s, f"ti-{flow}-{kind}-picker", {normalize_word(theme_id), "installed"},
                                   lambda found: picker_row_point(found, theme_id) is not None, evidence)
    row = picker_row_point(centres, theme_id) if listed else None
    observations = {"picker_opened": code == 0 and listed, "picker_listed": row is not None}
    if row is not None and kind == "cursor":
        preview_path, _ = s.shot(f"ti-{flow}-{kind}-preview")
        if preview_path is not None:
            evidence.append(str(preview_path))
            observations["preview_pixels"] = base.image_region_color_count(preview_path, preview_rect(row), XCURSOR_RGB)
            observations["preview_row"] = list(row)
            observations["preview_visible"] = preview_visible(observations["preview_pixels"])
    if row is not None and select:
        select_code, command = s.input(click=f"{row[0]},{row[1]}")
        commands.append(command)
        time.sleep(1.5)
        text, query = base.dbus_settings_snapshot(s.ctx)
        commands.append(query)
        snapshot_path = s.ctx.run / "logs" / f"{flow}-settings-snapshot.txt"
        snapshot_path.write_text((text or "") + "\n", encoding="utf-8")
        evidence.append(str(snapshot_path))
        observations["selected_snapshot_value"] = base.gvariant_field_string(text or "", key)
        observations["select_exit"] = select_code
    observations["shell_alive"] = s.shell is not None and s.shell.proc.poll() is None and s.ctx.wm_dead() is False
    return observations


def run_install_flow(s: ap.Session, flow: str, button: str, portal_mode: str, source: Path,
                     evidence: list, commands: list, select: bool = False, reference: Path | None = None,
                     kind: str = "icon", picker: bool = False, theme_id: str = THEME_ID) -> dict:
    """Install one fixture through the real Settings button; then the picker (and selection) when it installed."""
    observations: dict = {}
    handles = open_settings(s, flow, portal_mode, source, evidence, commands)
    if handles is None:
        observations["settings_ready"] = False
        return observations
    try:
        observations["settings_ready"] = True
        observations.update(click_install(s, handles, flow, button, evidence, commands))
        if observations.get("click_located") and observations.get("portal_called"):
            status_words = {"installed", "failed", normalize_word(theme_id)}
            observations["status_installed"], _ = await_status(
                s, f"ti-{flow}-status", status_words, lambda found: installed_status_seen(found, theme_id), evidence)
            observations["status_failed"], _ = await_status(
                s, f"ti-{flow}-failure", status_words, failed_status_seen, evidence)
        installed = read_icons_dir(s.ctx) / theme_id
        # An archive is compared with the folder it was built from; a folder is compared with itself.
        reference = reference or source
        observations["installed_tree_matches_source"] = installed_matches(reference, installed) if reference.is_dir() else None
        if picker or select:
            observations.update(picker_select(s, handles, flow, kind, theme_id, select, evidence, commands))
        (s.ctx.run / "logs" / f"{flow}-observations.json").write_text(
            json.dumps(observations, indent=2, default=str) + "\n", encoding="utf-8")
    finally:
        s.ctx.stop(handles["settings"])
        s.ctx.stop(handles["portal"])
    return observations


# ---------------------------------------------------------------- scenarios

def ti01(fixtures: Path, evidence: list) -> dict:
    fixtures.mkdir(parents=True, exist_ok=True)
    folder = write_theme_folder(fixtures / "src" / THEME_ID, THEME_ID)
    archive = write_theme_archive(folder, fixtures / f"{THEME_ID}.tar.gz")
    outside = fixtures / "outside" / "host-file.png"
    outside.parent.mkdir(parents=True, exist_ok=True)
    outside.write_bytes(b"outside the theme tree\n")
    escape = write_theme_folder(fixtures / "src" / ESCAPE_ID, ESCAPE_ID, escape_target=outside)
    corrupt = write_corrupt_archive(fixtures / f"{CORRUPT_ID}.tar.gz")
    cursor = write_cursor_folder(fixtures / "src" / CURSOR_ID, CURSOR_ID)
    cursor_archive = write_theme_archive(cursor, fixtures / f"{CURSOR_ID}.tar.gz")
    cursor_escape = write_cursor_folder(fixtures / "src" / CURSOR_ESCAPE_ID, CURSOR_ESCAPE_ID, escape=True)
    cursor_corrupt = write_cursor_folder(fixtures / "src" / CURSOR_CORRUPT_ID, CURSOR_CORRUPT_ID,
                                         left_ptr=xcursor_bytes()[:40])
    with tarfile.open(archive, "r:gz") as tar:
        members = tar.getnames()
    with tarfile.open(cursor_archive, "r:gz") as tar:
        cursor_members = tar.getnames()
    cursor_left_ptr = cursor / "cursors" / CURSOR_FILE
    checks = {
        "folder_index": (folder / "index.theme").is_file() and f"Name={THEME_ID}" in (folder / "index.theme").read_text(),
        "archive_members": f"{THEME_ID}/index.theme" in members and f"{THEME_ID}/apps/48/{THEME_FILE}" in members,
        "escape_symlink": (escape / "apps" / "48" / "escape.png").is_symlink(),
        "corrupt_unreadable": True,
        "cursor_index": f"Name={CURSOR_ID}" in (cursor / "index.theme").read_text() and "Directories=" not in (
            cursor / "index.theme").read_text(),
        "cursor_left_ptr_xcursor": cursor_left_ptr.read_bytes()[:4] == XCURSOR_MAGIC,
        "cursor_archive_members": f"{CURSOR_ID}/index.theme" in cursor_members
                                  and f"{CURSOR_ID}/cursors/{CURSOR_FILE}" in cursor_members,
        "cursor_escape_symlink": os.readlink(cursor_escape / "cursors" / CURSOR_FILE) == CURSOR_ESCAPE_TARGET,
        "cursor_corrupt_xcursor": (cursor_corrupt / "cursors" / CURSOR_FILE).read_bytes()[:4] == XCURSOR_MAGIC
                                  and len((cursor_corrupt / "cursors" / CURSOR_FILE).read_bytes()) < 64,
    }
    try:
        with tarfile.open(corrupt, "r:gz") as tar:
            tar.getnames()
        checks["corrupt_unreadable"] = False
    except (tarfile.TarError, OSError, EOFError):
        pass
    evidence.extend([str(folder), str(archive), str(escape), str(corrupt), str(cursor_archive),
                     str(cursor), str(cursor_escape), str(cursor_corrupt)])
    status, reason = expected_entry_status({k: bool(v) for k, v in checks.items()}, tuple(checks))
    return entry("TI01", status, reason if status != PASS else "fixtures built and verified: " + ", ".join(checks),
                 evidence)


def ti_from_observations(sid: str, observations: dict, required: tuple[str, ...], evidence: list,
                         commands: list, passed_reason: str) -> dict:
    status, reason = expected_entry_status({k: (None if v is None else bool(v)) for k, v in observations.items()},
                                           required)
    return entry(sid, status, passed_reason if status == PASS else reason, evidence, commands)


def run_scenarios(s: ap.Session, run: Path, fixtures: Path, results: dict[str, dict]) -> None:
    evidence_all: list = []
    results["TI01"] = ti01(fixtures, evidence_all)
    if results["TI01"]["status"] != PASS:
        return
    icons = read_icons_dir(s.ctx)

    evidence: list = []
    commands: list = []
    folder_obs = run_install_flow(s, "folder", "folder", "folder", fixtures / "src" / THEME_ID, evidence, commands,
                                  select=True)
    results["TI02"] = ti_from_observations(
        "TI02", {k: folder_obs.get(k) for k in ("settings_ready", "click_located", "portal_called",
                                                 "status_installed", "installed_tree_matches_source")},
        ("settings_ready", "click_located", "portal_called", "status_installed", "installed_tree_matches_source"),
        evidence, commands, f"Install folder installed {THEME_ID} and the tree matches the source")
    results["TI03"] = ti_from_observations(
        "TI03", {k: folder_obs.get(k) for k in ("picker_opened", "picker_listed", "shell_alive")}
        | {"iconTheme_is_installed_theme": None if folder_obs.get("selected_snapshot_value") is None
           else folder_obs["selected_snapshot_value"] == THEME_ID},
        ("picker_opened", "picker_listed", "iconTheme_is_installed_theme", "shell_alive"),
        evidence, commands, "picker listed the theme and selecting it set iconTheme with the shell still running")

    evidence = []
    commands = []
    escape_obs = run_install_flow(s, "escape", "folder", "folder", fixtures / "src" / ESCAPE_ID, evidence, commands)
    no_escape = not (icons / ESCAPE_ID).exists() and not staging_leftovers(icons)
    results["TI04"] = ti_from_observations(
        "TI04", {"settings_ready": escape_obs.get("settings_ready"), "click_located": escape_obs.get("click_located"),
                 "status_failed": escape_obs.get("status_failed"), "nothing_installed": no_escape},
        ("settings_ready", "click_located", "status_failed", "nothing_installed"), evidence, commands,
        "escaping-symlink folder rejected with a failed status; nothing installed")

    evidence = []
    commands = []
    corrupt_obs = run_install_flow(s, "corrupt", "archive", "success", fixtures / f"{CORRUPT_ID}.tar.gz",
                                   evidence, commands)
    no_corrupt = not (icons / CORRUPT_ID).exists() and not staging_leftovers(icons)
    results["TI05"] = ti_from_observations(
        "TI05", {"settings_ready": corrupt_obs.get("settings_ready"), "click_located": corrupt_obs.get("click_located"),
                 "status_failed": corrupt_obs.get("status_failed"), "nothing_installed": no_corrupt},
        ("settings_ready", "click_located", "status_failed", "nothing_installed"), evidence, commands,
        "corrupt archive rejected with a failed status; nothing installed")

    evidence = []
    commands = []
    installed_tree = icons / THEME_ID
    if installed_tree.exists():
        shutil.rmtree(installed_tree)
        commands.append(f"rm -rf {installed_tree}")
    archive_obs = run_install_flow(s, "archive", "archive", "success", fixtures / f"{THEME_ID}.tar.gz",
                                   evidence, commands, select=True, reference=fixtures / "src" / THEME_ID)
    results["TI06"] = ti_from_observations(
        "TI06", {k: archive_obs.get(k) for k in ("settings_ready", "click_located", "portal_called", "status_installed",
                                                  "installed_tree_matches_source", "picker_listed",
                                                  "shell_alive")}
        | {"iconTheme_is_installed_theme": None if archive_obs.get("selected_snapshot_value") is None
           else archive_obs["selected_snapshot_value"] == THEME_ID},
        ("settings_ready", "click_located", "portal_called", "status_installed", "installed_tree_matches_source",
         "picker_listed", "iconTheme_is_installed_theme", "shell_alive"),
        evidence, commands, "Install archive installed the tar.gz theme; picker selected it with the shell running")

    evidence = []
    commands = []
    cursor_obs = run_install_flow(s, "cursor", "folder", "folder", fixtures / "src" / CURSOR_ID, evidence, commands,
                                  select=True, kind="cursor", theme_id=CURSOR_ID)
    results["TI07"] = ti_from_observations(
        "TI07", {k: cursor_obs.get(k) for k in ("settings_ready", "click_located", "portal_called", "status_installed",
                                                 "installed_tree_matches_source")},
        ("settings_ready", "click_located", "portal_called", "status_installed", "installed_tree_matches_source"),
        evidence, commands, f"Install folder installed cursor theme {CURSOR_ID} and the tree matches the source")
    results["TI08"] = ti_from_observations(
        "TI08", {k: cursor_obs.get(k) for k in ("picker_opened", "picker_listed", "preview_visible", "shell_alive")}
        | {"cursorTheme_is_installed_theme": None if cursor_obs.get("selected_snapshot_value") is None
           else cursor_obs["selected_snapshot_value"] == CURSOR_ID},
        ("picker_opened", "picker_listed", "preview_visible", "cursorTheme_is_installed_theme", "shell_alive"),
        evidence, commands, "cursor picker listed the theme with its left_ptr preview; selecting it set cursorTheme with the shell running")

    evidence = []
    commands = []
    cursor_archive = fixtures / f"{CURSOR_ID}.tar.gz"
    installed_cursor = icons / CURSOR_ID
    if installed_cursor.exists():
        shutil.rmtree(installed_cursor)
        commands.append(f"rm -rf {installed_cursor}")
    cursor_archive_obs = run_install_flow(s, "cursor-archive", "archive", "success", cursor_archive, evidence, commands,
                                          reference=fixtures / "src" / CURSOR_ID, kind="cursor", theme_id=CURSOR_ID)
    results["TI09"] = ti_from_observations(
        "TI09", {k: cursor_archive_obs.get(k) for k in ("settings_ready", "click_located", "portal_called",
                                                         "status_installed", "installed_tree_matches_source")},
        ("settings_ready", "click_located", "portal_called", "status_installed", "installed_tree_matches_source"),
        evidence, commands, "Install archive installed the cursor tar.gz theme")

    evidence = []
    commands = []
    escape_obs = run_install_flow(s, "cursor-escape", "folder", "folder", fixtures / "src" / CURSOR_ESCAPE_ID,
                                  evidence, commands, kind="cursor", theme_id=CURSOR_ESCAPE_ID)
    no_cursor_escape = not (icons / CURSOR_ESCAPE_ID).exists() and not staging_leftovers(icons)
    corrupt_obs = run_install_flow(s, "cursor-corrupt", "folder", "folder", fixtures / "src" / CURSOR_CORRUPT_ID,
                                   evidence, commands, kind="cursor", picker=True, theme_id=CURSOR_CORRUPT_ID)
    results["TI10"] = ti_from_observations(
        "TI10", {
            "escape_settings_ready": escape_obs.get("settings_ready"),
            "escape_click_located": escape_obs.get("click_located"),
            "escape_status_failed": escape_obs.get("status_failed"),
            "escape_nothing_installed": no_cursor_escape,
            "corrupt_status_installed": corrupt_obs.get("status_installed"),
            "corrupt_picker_listed": corrupt_obs.get("picker_listed"),
            "corrupt_preview_omitted": None if corrupt_obs.get("preview_visible") is None
            else not corrupt_obs["preview_visible"],
        },
        ("escape_settings_ready", "escape_click_located", "escape_status_failed", "escape_nothing_installed",
         "corrupt_status_installed", "corrupt_picker_listed", "corrupt_preview_omitted"),
        evidence, commands,
        "escaping cursors/left_ptr folder rejected with nothing installed; corrupt Xcursor installed with its preview omitted")


def run_inner(args: argparse.Namespace, run: Path) -> int:
    ctx = base.Ctx(run)
    results: dict[str, dict] = {}
    started = ap.utc_now()
    for sub in ("home", "xdg-config", "xdg-data", "xdg-cache", "xdg-runtime", "xdg-system-empty", "shots", "logs",
                "fixtures"):
        (run / sub).mkdir(parents=True, exist_ok=True)
    try:
        display = base.pick_display(args.display)
        if display is None:
            for sid in IDS:
                results[sid] = entry(sid, ENV_BLOCKED, "no free display in :90..:99")
        else:
            ctx.display = display
            ctx.build_app_env()
            ctx.token = secrets.token_hex(32)
            ctx.marker.write_text(f"{ctx.display}\n{ctx.token}\n", encoding="ascii")
            os.chmod(ctx.marker, 0o600)
            xephyr = ctx.spawn("xephyr", ["Xephyr", ctx.display, "-screen", args.geometry, "-ac", "-noreset", "-br"],
                               ctx.host_env)
            if not base.wait_for_display(ctx, xephyr):
                for sid in IDS:
                    results[sid] = entry(sid, ENV_BLOCKED, f"Xephyr did not become ready on {ctx.display}")
            else:
                session = ap.Session(ctx, args)
                if not session.restart({}):
                    for sid in IDS:
                        results[sid] = entry(sid, ENV_BLOCKED, "owned WM did not own the settings bus name")
                else:
                    try:
                        run_scenarios(session, run, run / "fixtures", results)
                    except Exception as error:  # a crashed flow is a FAIL with its reason, never a PASS
                        for sid in IDS:
                            if sid not in results:
                                results[sid] = entry(sid, FAIL, f"flow raised {type(error).__name__}: {error}")
    finally:
        teardown = ctx.teardown()
        doc = compose_document(results)
        doc.update({"run_dir": str(run), "display": ctx.display, "geometry": args.geometry,
                    "wm_bin": str(Path(args.wm_bin).resolve()), "shell_bin": str(Path(args.shell_bin).resolve()),
                    "started_utc": started, "finished_utc": ap.utc_now(), "teardown": teardown})
        if not teardown["ok"]:
            doc["verdict"], doc["exit_code"] = "FAIL", 1
        write_json(run / "result.json", doc)
    return doc["exit_code"]


def main(argv: list[str] | None = None) -> int:
    raw = sys.argv[1:] if argv is None else list(argv)
    args = parse_args(raw)
    run = Path(args.run_dir).expanduser().absolute()
    inner = os.environ.get("FLAMEWM_TI_INNER") == "1"
    if inner:
        if not run.is_dir() or [item.name for item in run.iterdir()] != [base.RUN_OWNER_MARKER]:
            raise SystemExit(f"error: inner run dir {run} must contain only the outer {base.RUN_OWNER_MARKER} marker")
    else:
        ap.validate_run_dir(run)
    missing = [name for name in ("Xephyr", "xwininfo", "xprop", "import", "identify", "xdpyinfo", "gdbus")
               if shutil.which(name) is None]
    if missing or not os.environ.get("DISPLAY") or base.Image is None:
        run.mkdir(parents=True, exist_ok=True)
        reason = "; ".join(filter(None, [f"missing tools: {missing}" if missing else "",
                                         "" if os.environ.get("DISPLAY") else "host DISPLAY unset",
                                         "" if base.Image is not None else "PIL unavailable"]))
        results = {sid: entry(sid, ENV_BLOCKED, reason) for sid in IDS}
        doc = compose_document(results) | {"run_dir": str(run)}
        write_json(run / "result.json", doc)
        print(run / "result.json")
        return 3
    for binary in (args.wm_bin, args.shell_bin, args.settings_bin):
        if not os.access(binary, os.X_OK):
            print(f"error: binary not executable: {binary}", file=sys.stderr)
            return 2
    run.mkdir(parents=True, exist_ok=True)
    os.chmod(run, 0o700)
    if inner:
        return run_inner(args, run)
    (run / base.RUN_OWNER_MARKER).write_text(f"owner-pid={os.getpid()}\ncreated={ap.utc_now()}\n", encoding="ascii")
    if shutil.which("dbus-run-session") is None:
        print("error: dbus-run-session not found on PATH", file=sys.stderr)
        return 3
    child = subprocess.Popen(["dbus-run-session", "--", sys.executable, str(Path(__file__).resolve()), *raw],
                             env=dict(os.environ, FLAMEWM_TI_INNER="1"))
    code = child.wait()
    print(run / "result.json")
    return code


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import os
import re
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
errors: list[str] = []


def require(path: str) -> Path:
    p = ROOT / path
    if not p.exists():
        errors.append(f"missing required path: {path}")
    return p


def read_required(path: str) -> str:
    p = ROOT / path
    return p.read_text() if p.exists() else ""


# ---------------------------------------------------------------------------
# Audit primitives.
#
# These live above the audit body so `--self-test` exercises the exact same
# predicates the default run uses instead of a parallel re-implementation.
# ---------------------------------------------------------------------------

_RUST_DELIMITERS = re.compile(r"""[/"']""")


def _raw_string_prefix_hashes(text: str, quote: int) -> int:
    """Classify the `"` at `quote` as a raw-string opener or a plain string.

    Returns the raw-string `#` count, or 0 when the quote is an ordinary
    string delimiter. Handles `r"..."`, `r#"..."#`, `br#"..."#`, `rb##"..."##`
    while refusing identifier lookalikes such as `myvar"..."`.
    """
    back = quote - 1
    while back >= 0 and text[back] == "#":
        back -= 1
    hashes = quote - 1 - back
    start = back
    while start >= 0 and text[start] in "rb":
        start -= 1
    if text[start + 1:back + 1] not in ("r", "br", "rb"):
        return 0
    if start >= 0 and (text[start].isalnum() or text[start] == "_"):
        return 0
    return -1 if hashes == 0 else hashes


def _strip_rust_comments(text: str) -> str:
    """Return Rust source with `//` and `/* */` comments removed.

    Correct Rust lexing rules, so a retired symbol mentioned inside a comment
    never leaks into the guarded source context:

    * `//` line comments and nested `/* /* */ */` block comments are removed,
      and newlines inside them are preserved so line numbers stay stable;
    * `"` strings keep `\\` escapes;
    * `'` CHAR literals (`'a'`, `'\\n'`, `'\\''`, `'{`) are preserved verbatim;
    * LIFETIMES (`'a`, `'static`, `'static mut`) never open a string;
    * raw strings `r"..."`, `r#"..."#`, `br#"..."#` are preserved verbatim.

    Stdlib only.
    """
    out: list[str] = []
    i = 0
    n = len(text)
    block_depth = 0
    while i < n:
        if block_depth:
            close = text.find("*/", i)
            nested = text.find("/*", i)
            if nested != -1 and (close == -1 or nested < close):
                block_depth += 1
                out.append("\n" * text.count("\n", i, nested))
                i = nested + 2
            elif close != -1:
                block_depth -= 1
                out.append("\n" * text.count("\n", i, close))
                i = close + 2
            else:
                out.append("\n" * text.count("\n", i))
                return "".join(out)
            continue
        match = _RUST_DELIMITERS.search(text, i)
        if match is None:
            out.append(text[i:])
            break
        start = match.start()
        if start > i:
            out.append(text[i:start])
            i = start
        ch = text[start]
        if ch == "/":
            nxt = text[i + 1] if i + 1 < n else ""
            if nxt == "/":
                end = text.find("\n", i)
                i = n if end == -1 else end  # the newline itself is kept
            elif nxt == "*":
                block_depth = 1
                i += 2
            else:
                out.append("/")
                i += 1
        elif ch == '"':
            hashes = _raw_string_prefix_hashes(text, i)
            if hashes < 0:
                end = text.find('"', i + 1)
                stop = n if end == -1 else end + 1
            else:
                terminator = '"' + "#" * hashes
                end = text.find(terminator, i + 1)
                stop = n if end == -1 else end + len(terminator)
            out.append(text[i:stop])
            i = stop
        else:
            if i + 1 < n and text[i + 1] == "\\":
                j = i + 1
                while j < n and text[j] != "'":
                    j += 2 if text[j] == "\\" else 1
                stop = n if j >= n else j + 1
                out.append(text[i:stop])
                i = stop
            elif text[i + 1:i + 2] and text[i + 2:i + 3] == "'":
                out.append(text[i:i + 3])
                i += 3
            else:
                out.append("'")  # lifetime: emit the quote, open nothing
                i += 1
    return "".join(out)


def _j11_code(text: str) -> str:
    return _strip_rust_comments(text)


def _j11_fn_body(code: str, fn_name: str) -> str:
    """Return the body of `fn <fn_name>(` up to the next top-level item."""
    match = re.search(r"fn\s+" + re.escape(fn_name) + r"\s*\(", code)
    if not match:
        return ""
    rest = code[match.end():]
    nxt = re.search(r"\n(?:pub\s+)?fn\s+", rest)
    return rest[:nxt.start()] if nxt else rest


_J11_MOVE_RESIZE_OWNER = "crates/flamewm-render-x11/src/surface_controller.rs"
_J11_XLIB_DECL = "crates/flamewm-render-x11/src/xlib.rs"
# J11-G01: move_resize is the single native geometry owner; size changes go
# through commit_geometry, and no other production file issues
# XMoveResizeWindow. xlib.rs only declares the FFI binding.
_J11_G01_EXEMPT = frozenset({_J11_MOVE_RESIZE_OWNER, _J11_XLIB_DECL})


def _j11_g01_message(rel: str) -> str:
    return f"raw native move outside move_resize owner: {rel} (J11-G01)"


def _j11_g01_nonowner_violation(rel: str, code: str) -> bool:
    """True when a non-owner production file issues a raw native move.

    `code` must already be comment-stripped by `_strip_rust_comments`.
    """
    if rel in _J11_G01_EXEMPT:
        return False
    return "XMoveResizeWindow" in code


def _j11_g01_owner_errors(owner_path: Path) -> list[str]:
    """J11-G01 checks on the canonical move_resize owner file."""
    found: list[str] = []
    if not owner_path.is_file():
        return found
    body = _j11_fn_body(_j11_code(owner_path.read_text(errors="ignore")), "move_resize")
    if "commit_geometry" not in body:
        found.append("move_resize bypasses commit_geometry (J11-G01)")
    if "XMoveResizeWindow" not in body:
        found.append("move_resize lost native move ownership (J11-G01)")
    return found


# Production shell topology, sourced from crates/flamewm-shell/build.rs: these
# seven templates (and flamewm.css) are the documents the compiled .rwr
# artifacts are actually built from.
PRODUCTION_SHELL_CSS = "ui/shell/flamewm.css"
PRODUCTION_SHELL_TEMPLATES: tuple[tuple[str, tuple[str, ...]], ...] = (
    ("ui/shell/panel.html",
     ("taskbar.surface", "task-start", "task-slot-", "workspace-pager", "clock-button")),
    ("ui/shell/start.html",
     ("start.menu.surface", "start-search-field", "start-category-", "start-app-slot-")),
    ("ui/shell/task-menu.html",
     ("task.menu.surface", "context-task-", "context-workspace-", "context-taskbar-")),
    ("ui/shell/media.html",
     ("popup.surface", "media.previous", "media.playpause", "media.next")),
    ("ui/shell/audio.html",
     ("volume-mute", "volume-track-fill", "audio-list")),
    ("ui/shell/network.html",
     ("network-slots", "network-secret", "network-scan")),
    ("ui/shell/calendar.html",
     ("calendar-month", "calendar-days", "calendar-day-")),
)
PRODUCTION_SHELL_BUILD_RS = "crates/flamewm-shell/build.rs"
PRODUCTION_UI_CONTRACT = "https://wm.arkflame.com"


def _production_topology_errors(base: Path) -> list[str]:
    """Audit the real multi-surface shell topology compiled by build.rs.

    `base` is a tree root, so callers may point this at a throwaway copy.
    """
    found: list[str] = []
    build_rs_path = base / PRODUCTION_SHELL_BUILD_RS
    if not build_rs_path.is_file():
        found.append(f"missing required path: {PRODUCTION_SHELL_BUILD_RS}")
        build_rs = ""
    else:
        build_rs = build_rs_path.read_text(errors="ignore")
    if not (base / PRODUCTION_SHELL_CSS).is_file():
        found.append(f"missing required path: {PRODUCTION_SHELL_CSS}")
    elif "flamewm.css" not in build_rs:
        found.append("shell build.rs must compile ui/shell/flamewm.css")
    audited = {rel.rsplit("/", 1)[-1] for rel, _ in PRODUCTION_SHELL_TEMPLATES}
    for rel, nodes in PRODUCTION_SHELL_TEMPLATES:
        name = rel.rsplit("/", 1)[-1]
        template = base / rel
        if not template.is_file():
            found.append(f"missing required path: {rel}")
        else:
            source = template.read_text(errors="ignore")
            for node in nodes:
                if node not in source:
                    found.append(
                        f"compiled shell template {rel} missing required node: {node}"
                    )
        if name not in build_rs:
            found.append(f"shell build.rs must compile {name}")
    # A newly staged template must be guarded like the rest of the topology.
    shell_dir = base / "ui/shell"
    if shell_dir.is_dir():
        for candidate in sorted(shell_dir.glob("*.html")):
            if candidate.name not in audited and candidate.name in build_rs:
                found.append(
                    "shell build.rs compiles unguarded template: "
                    f"ui/shell/{candidate.name}"
                )
    return found


# ui/index.html is the document scripts/check and scripts/verify-visual
# actually feed to the renderer, so it is a live gate input.
LIVE_GATE_INPUT = "ui/index.html"


def _live_gate_input_errors(base: Path) -> list[str]:
    """Cheap, source-grounded audit of the compiled prototype gate input."""
    found: list[str] = []
    path = base / LIVE_GATE_INPUT
    if not path.is_file():
        found.append(f"missing required path: {LIVE_GATE_INPUT}")
        return found
    text = path.read_text(errors="ignore")
    if not text.strip():
        found.append(f"{LIVE_GATE_INPUT} must not be empty")
        return found
    if "<html" not in text.lower():
        found.append(f"{LIVE_GATE_INPUT} must contain an <html> root")
    if "<style" not in text.lower() and not re.search(
        r'<link[^>]+stylesheet', text, re.I
    ):
        found.append(f"{LIVE_GATE_INPUT} must carry a <style> block or a linked stylesheet")
    if "data-scroll" not in text and 'id="taskbar"' not in text:
        found.append(
            f"{LIVE_GATE_INPUT} must expose a scrollable region or a taskbar surface action"
        )
    if re.search(r"<script\b", text, re.I):
        found.append(f"compiled production UI {LIVE_GATE_INPUT} must not embed JavaScript")
    if PRODUCTION_UI_CONTRACT not in text:
        found.append(f"{LIVE_GATE_INPUT} missing UI contract: {PRODUCTION_UI_CONTRACT}")
    return found


def _self_test() -> int:
    """Fixture-driven self-test of the audit's own predicates.

    Never reads or mutates the real repository files under test: the only
    filesystem work is a throwaway tree copy in a temporary directory.
    """
    import shutil
    import tempfile

    results: list[tuple[str, bool, str]] = []

    def record(name: str, ok: bool, detail: str = "") -> None:
        results.append((name, ok, detail))

    # 1. A lifetime followed by a comment-only retired symbol must NOT trip J11-G01.
    comment_only = (
        "pub fn configure_label() -> &'static str {\n"
        "    // No XMoveResizeWindow echo: the native move is owned by\n"
        "    // SurfaceController::move_resize only.\n"
        '    "geometry"\n'
        "}\n"
    )
    record(
        "comment-only-lifetime",
        not _j11_g01_nonowner_violation(
            "crates/flamewm-render-x11/src/app/events.rs",
            _strip_rust_comments(comment_only),
        ),
        "expect no J11-G01 violation for &'static str + comment-only symbol",
    )

    # 2. A real forbidden native move outside the owner MUST trip J11-G01.
    forbidden = (
        "pub fn nudge(display: Display, window: Window) {\n"
        "    unsafe { XMoveResizeWindow(display, window, 4, 8, 32, 32) };\n"
        "}\n"
    )
    hits = _j11_g01_nonowner_violation(
        "crates/flamewm-render-x11/src/app/layout.rs",
        _strip_rust_comments(forbidden),
    )
    record("real-forbidden-call", hits, _j11_g01_message("crates/flamewm-render-x11/src/app/layout.rs"))

    with tempfile.TemporaryDirectory(prefix="flamewm-static-audit-selftest-") as tmp:
        tmp_root = Path(tmp)

        # 3. The canonical owner shape stays legal.
        owner_fixture = (
            "impl SurfaceController {\n"
            "    fn move_resize(&mut self, window: Window, geometry: Geometry) {\n"
            "        self.commit_geometry(geometry);\n"
            "        unsafe {\n"
            "            XMoveResizeWindow(self.display, window, geometry.x, geometry.y,\n"
            "                              geometry.width, geometry.height)\n"
            "        };\n"
            "    }\n"
            "}\n"
        )
        owner_path = tmp_root / _J11_MOVE_RESIZE_OWNER
        owner_path.parent.mkdir(parents=True, exist_ok=True)
        owner_path.write_text(owner_fixture)
        owner_errors = _j11_g01_owner_errors(owner_path)
        owner_exempt = not _j11_g01_nonowner_violation(
            _J11_MOVE_RESIZE_OWNER, _strip_rust_comments(owner_fixture)
        )
        record(
            "legal-canonical-owner",
            not owner_errors and owner_exempt,
            f"owner_errors={owner_errors}",
        )

        # 4. Negative production fixture: deleting a required node from a copy
        #    of the real panel.html must fail the topology check.
        (tmp_root / "ui/shell").mkdir(parents=True)
        (tmp_root / "crates/flamewm-shell").mkdir(parents=True)
        shutil.copy2(ROOT / PRODUCTION_SHELL_BUILD_RS,
                     tmp_root / PRODUCTION_SHELL_BUILD_RS)
        shutil.copy2(ROOT / PRODUCTION_SHELL_CSS, tmp_root / PRODUCTION_SHELL_CSS)
        for rel, _nodes in PRODUCTION_SHELL_TEMPLATES:
            shutil.copy2(ROOT / rel, tmp_root / rel)
        pristine = _production_topology_errors(tmp_root)
        panel = tmp_root / "ui/shell/panel.html"
        panel.write_text(panel.read_text().replace('id="task-start"', "", 1))
        broken = _production_topology_errors(tmp_root)
        failure_line = next(
            (e for e in broken if "panel.html" in e and "task-start" in e), None
        )
        record(
            "negative-production-template",
            not pristine and bool(failure_line),
            f"pristine_copy_errors={len(pristine)} broken_copy_errors={len(broken)}"
            f" :: captured: {failure_line or broken}",
        )

    failed = 0
    for name, ok, detail in results:
        print(f"{'PASS' if ok else 'FAIL'} self-test: {name}" + (f" :: {detail}" if detail else ""))
        if not ok:
            failed += 1
    if failed:
        print(f"FAIL static audit self-test: {failed}/{len(results)} cases failed",
              file=sys.stderr)
        return 1
    print(f"OK static audit self-test: {len(results)} cases passed")
    return 0


# `--self-test` runs the fixtures and never audits the real tree; the default
# invocation falls through to the real audit below.
if "--self-test" in sys.argv[1:]:
    raise SystemExit(_self_test())

required = [
    "Cargo.toml",
    "FLAMEWM_VERSION",
    "README.md",
    "docs/TODO.md",
    "docs/ARCHITECTURE.md",
    LIVE_GATE_INPUT,
    "ui/shell/flamewm.css",
    "crates/flamewm-wm/Cargo.toml",
    "crates/flamewm-wm/src/main.rs",
    "crates/flamewm-wm-x11/Cargo.toml",
    "crates/flamewm-wm-x11/src/wm.rs",
    "crates/flamewm-shell/Cargo.toml",
    "crates/flamewm-shell/build.rs",
    "crates/flamewm-shell/src/main.rs",
    "crates/flamewm-render-core/Cargo.toml",
    "crates/flamewm-render-x11/Cargo.toml",
    "assets/branding/flamewm-start.svg",
    "assets/branding/flamewm-wallpaper.webp",
    "assets/branding/flamewm-wordmark.png",
    "scripts/doctor",
    "scripts/check",
    "scripts/xephyr",
    "scripts/verify-visual",
    "tools/verify_rwr_promotion.py",
]
for path in required:
    require(path)

version = read_required("FLAMEWM_VERSION").strip()
if not version:
    errors.append("project version in FLAMEWM_VERSION must not be empty")

root_manifest = ROOT / "Cargo.toml"
try:
    root_cargo = tomllib.loads(root_manifest.read_text())
except Exception as exc:
    errors.append(f"invalid root Cargo.toml: {exc}")
    root_cargo = {}
workspace = root_cargo.get("workspace", {})
package = workspace.get("package", {})
if package.get("version") != version:
    errors.append(f"workspace.package.version must match FLAMEWM_VERSION ({version})")
if package.get("rust-version") != "1.85":
    errors.append("workspace.package.rust-version must be 1.85")
if workspace.get("members") != ["crates/*"]:
    errors.append("workspace members must be exactly crates/*")
if "exclude" in workspace:
    errors.append("workspace must not carry an engine/vendor exclude list")
if (ROOT / "engine").exists():
    errors.append("active /engine tree is forbidden; all production code belongs under /crates")

vendor = {
    "flamewm-c-reference": 500,
    "RustWebRender-0.0.9": 150,
    "icewm-master": 500,
    "creamshell-main": 80,
    "creamui-main": 150,
}
vendor_root = ROOT / ".vendor"
allowed_vendor_dirs = set(vendor)
actual_vendor_dirs = {p.name for p in vendor_root.iterdir() if p.is_dir()} if vendor_root.exists() else set()
if actual_vendor_dirs != allowed_vendor_dirs:
    errors.append(
        f".vendor directories must be exactly {sorted(allowed_vendor_dirs)}, got {sorted(actual_vendor_dirs)}"
    )
for name, minimum in vendor.items():
    p = require(f".vendor/{name}")
    if p.exists():
        count = sum(1 for x in p.rglob("*") if x.is_file() or x.is_symlink())
        if count < minimum:
            errors.append(f"vendor tree {name} unexpectedly small: {count} < {minimum}")
if (rwr := ROOT / ".vendor/RustWebRender-0.0.9").exists():
    size = sum(p.stat().st_size for p in rwr.rglob("*") if p.is_file())
    if size > 12 * 1024 * 1024:
        errors.append(f"RustWebRender-0.0.9 vendor tree exceeds 12 MiB: {size} bytes")

# Every active crate must be a valid standalone manifest. Active source may never reach into reference corpora.
for manifest in sorted((ROOT / "crates").glob("*/Cargo.toml")):
    rel = manifest.relative_to(ROOT)
    try:
        data = tomllib.loads(manifest.read_text())
    except Exception as exc:
        errors.append(f"invalid manifest {rel}: {exc}")
        continue
    pkg = data.get("package", {})
    literal_rust = pkg.get("rust-version")
    if literal_rust == "0.0.1":
        errors.append(f"invalid copied rust-version 0.0.1 in {rel}")
    text = manifest.read_text()
    if re.search(r'(?im)^\s*path\s*=\s*["\'][^"\']*(?:\.vendor|/engine|engine/|icewm-rust)', text):
        errors.append(f"active path dependency reaches reference/legacy code: {rel}")
    for forbidden in ["wry", "webkit", "electron", "chromium", "tauri", "webview"]:
        if re.search(rf'(?im)^\s*{re.escape(forbidden)}\s*=', text):
            errors.append(f"forbidden browser/webview runtime dependency {forbidden} in {rel}")

def active_files(base: Path):
    """Yield source files while excluding generated Python cache entries."""
    for p in base.rglob("*"):
        if not p.is_file() or "target" in p.parts or "__pycache__" in p.parts:
            continue
        if p.suffix.lower() in {".pyc", ".pyo"}:
            continue
        yield p


active_roots = [ROOT / "crates", ROOT / "ui", ROOT / "scripts", ROOT / "tools"]
for base in active_roots:
    for p in active_files(base):
        if p.suffix.lower() not in {".rs", ".toml", ".html", ".css", ".py", ".sh", ""}:
            continue
        if p.resolve() == Path(__file__).resolve():
            continue
        text = p.read_text(errors="ignore").replace(".vendor/RustWebRender-0.0.9", ".vendor/<provenance>")
        if re.search(r'(include_bytes!|include_str!|path\s*=|src\s*=)[^\n]*\.vendor', text):
            errors.append(f"production dependency reaches into .vendor: {p.relative_to(ROOT)}")
        if "engine/icewm" in text or "engine/icewm-rust" in text:
            errors.append(f"legacy engine path referenced by active source: {p.relative_to(ROOT)}")

# Product surfaces use FlameWM identity. Historical vendor/provenance names are
# intentionally confined to the promotion verifier and vendor corpus.
for base in [ROOT / "crates", ROOT / "ui", ROOT / "scripts"]:
    for p in active_files(base):
        text = (
            p.read_text(errors="ignore")
            .replace(".vendor/RustWebRender-0.0.9", ".vendor/<provenance>")
            # Keep visual verifier's historical screenshot path operational without
            # treating its provenance filename as active product identity.
            .replace(
                "project_sources/visual/user-rwr-0.0.6-xephyr.png",
                "project_sources/visual/<historical-reference>.png",
            )
        )
        if re.search(r"RustWebRender|rustwebrender|(?<![A-Za-z])rwr-", text):
            errors.append(f"stale renderer identity in active product source: {p.relative_to(ROOT)}")

# No symlink in active paths may escape into reference corpora.
for base_name in ["crates", "ui", "assets", "scripts", "tools"]:
    base = ROOT / base_name
    for p in base.rglob("*"):
        if p.is_symlink():
            try:
                resolved = p.resolve()
            except OSError as exc:
                errors.append(f"broken active symlink {p.relative_to(ROOT)}: {exc}")
                continue
            if (ROOT / ".vendor") in resolved.parents or resolved == ROOT / ".vendor":
                errors.append(f"active symlink reaches .vendor: {p.relative_to(ROOT)}")

errors.extend(_production_topology_errors(ROOT))
errors.extend(_live_gate_input_errors(ROOT))
# LEGACY_PROTOTYPE_REFERENCE: ui/shell/index.html is an optional historical
# prototype, not a compiled template or product acceptance surface.

shell = read_required("crates/flamewm-shell/src/main.rs")
shell_runtime = read_required("crates/flamewm-shell/src/runtime.rs")
shell_build = read_required("crates/flamewm-shell/build.rs")
shell_manifest = read_required("crates/flamewm-shell/Cargo.toml")
if not re.search(r'(?im)^\s*name\s*=\s*["\']flamewm-shell["\']\s*$', shell_manifest):
    errors.append("canonical shell manifest must declare package flamewm-shell")
shell_host = read_required("crates/flamewm-shell/src/quick_controls/host.rs")
shell_artifacts = [
    ("flamewm-panel.rwr", "panel.html"),
    ("flamewm-start.rwr", "start.html"),
    ("flamewm-task-menu.rwr", "task-menu.html"),
    ("flamewm-media.rwr", "media.html"),
]
for artifact, source in shell_artifacts:
    if f'(\"{artifact}\", \"{source}\")' not in shell_build:
        errors.append(f"shell build must declare separate artifact {artifact} from {source}")
    if f'\"/{artifact}\"' not in shell_runtime:
        errors.append(f"shell runtime must include separate artifact {artifact}")
# J07: audio/network/calendar are helper-owned surfaces in the same
# flamewm-shell binary (role flag --quick-control-host). Build still
# compiles their artifacts; ownership lives in quick_controls/host.rs,
# and the parent ShellSurfaces must not own their native surfaces.
helper_artifacts = [
    ("flamewm-audio.rwr", "audio.html"),
    ("flamewm-network.rwr", "network.html"),
    ("flamewm-calendar.rwr", "calendar.html"),
]
for artifact, source in helper_artifacts:
    if f'(\"{artifact}\", \"{source}\")' not in shell_build:
        errors.append(f"shell build must declare helper artifact {artifact} from {source}")
    if f'\"/{artifact}\"' not in shell_host:
        errors.append(f"quick-control helper must own artifact {artifact}")
for owned in ["self.audio", "self.network", "self.calendar"]:
    if owned in shell_runtime:
        errors.append(f"parent ShellSurfaces must not own helper surface {owned} (J07)")
# J04: Start is one unified artifact (one native surface decision).
# The split start_submenu artifact must not return.
for forbidden_artifact in ["flamewm-start-submenu.rwr", "start-apps.html"]:
    if forbidden_artifact in shell_build or forbidden_artifact in shell_runtime:
        errors.append(f"split Start artifact forbidden; Start is unified as flamewm-start.rwr: {forbidden_artifact}")
for path, text in [
    ("crates/flamewm-shell/build.rs", shell_build),
    ("crates/flamewm-shell/src/main.rs", shell),
    ("crates/flamewm-shell/src/runtime.rs", shell_runtime),
]:
    if "flamewm-shell.rwr" in text:
        errors.append(f"combined shell artifact is forbidden: {path}")
if "SurfaceRuntime::new" not in shell:
    errors.append("shell must create surfaces through SurfaceRuntime")
for role in ["SurfaceRole::Dock", "SurfaceRole::PopupMenu", "SurfaceRole::DropdownMenu"]:
    if role not in shell_runtime:
        errors.append(f"shell runtime missing surface role: {role}")

desktop = read_required("crates/flamewm-desktop/src/main.rs")
if "flamewm_ui_x11" not in desktop or "UiWindowRole::Desktop" not in desktop:
    errors.append("desktop must use the flamewm-ui-x11 desktop boundary")
if "X11WindowRole::Desktop" in desktop:
    errors.append("desktop must not use raw X11WindowRole::Desktop")
if PRODUCTION_UI_CONTRACT not in read_required(LIVE_GATE_INPUT):
    errors.append(f"canonical shell UI missing contract: {PRODUCTION_UI_CONTRACT}")

stale_shell_name = "flamewm-" + "web-shell"
for base in [ROOT / "crates", ROOT / "scripts", ROOT / "tools"]:
    for p in active_files(base):
        if p.resolve() == Path(__file__).resolve():
            continue
        if stale_shell_name in p.read_text(errors="ignore"):
            errors.append(f"stale renamed shell reference in active source: {p.relative_to(ROOT)}")

wm_x11_path = "crates/flamewm-wm-x11/src/wm.rs"
wm_x11 = read_required(wm_x11_path)
for contract, anchor in {
    "SUBSTRUCTURE_REDIRECT": "EventMask::SUBSTRUCTURE_REDIRECT",
    "change_save_set": ".change_save_set(",
    "reparent_window": ".reparent_window(",
    "net_current_desktop": "self.atoms.net_current_desktop",
    "snap_geometry": "core_snap_geometry(",
}.items():
    if anchor not in wm_x11:
        errors.append(f"native X11 WM missing core contract: {contract} ({wm_x11_path})")

# No font binaries in active distributable paths. Runtime extraction belongs to ignored target/ only.
for p in ROOT.rglob("*"):
    if ".vendor" in p.parts or "target" in p.parts or "__pycache__" in p.parts or not p.is_file():
        continue
    if p.suffix.lower() in {".pyc", ".pyo"}:
        continue
    if p.suffix.lower() in {".ttf", ".otf", ".woff", ".woff2"}:
        errors.append(f"font binary must not be distributed in active tree: {p.relative_to(ROOT)}")

for script in ["scripts/doctor", "scripts/check", "scripts/xephyr", "scripts/verify-visual", "tools/verify_rwr_promotion.py"]:
    p = ROOT / script
    if p.exists() and not os.access(p, os.X_OK):
        errors.append(f"script is not executable: {script}")

# --- v62 framework-closure guards (G01-G10) ---
_compiled_templates = [
    "ui/shell/panel.html",
    "ui/shell/start.html",
    "ui/shell/start-apps.html",
    "ui/shell/task-menu.html",
    "ui/shell/media.html",
    "ui/shell/audio.html",
    "ui/shell/network.html",
    "ui/shell/calendar.html",
]
_known_keyed_ppms = {
    "task-start.ppm", "tray-volume.ppm", "tray-volume-muted.ppm", "tray-wifi.ppm",
    "tray-play.ppm", "tray-pause.ppm", "popup-volume.ppm", "popup-muted.ppm",
    "media-play.ppm", "media-pause.ppm", "media-prev.ppm", "media-next.ppm",
    "title-close.ppm", "title-maximize.ppm", "title-minimize.ppm", "title-restore.ppm",
}
# G01: compiled shell templates must not reference legacy keyed-magenta PPM roles.
for template in _compiled_templates:
    text = read_required(template)
    for ref in re.findall(r'assets/([A-Za-z0-9_./-]+\.ppm)', text):
        base = ref.rsplit("/", 1)[-1]
        if base in _known_keyed_ppms:
            errors.append(f"keyed-magenta PPM in active template {template}: {ref} (G01)")
# G02: no generic magenta-key comparison in production renderer (tests excluded).
for p in active_files(ROOT / "crates"):
    if p.suffix != ".rs" or "/tests" in p.as_posix() or p.name.endswith("_test.rs"):
        continue
    if "cfg(test)" in p.read_text(errors="ignore") and "mod tests" in p.read_text(errors="ignore"):
        pass
    text = p.read_text(errors="ignore")
    text_nt = re.sub(r"#\[cfg\(test\)\].*?\nmod tests \{.*?\n\}\n", "", text, flags=re.S)
    if re.search(r"(?i)(magenta.{0,40}key|key.{0,20}magenta|chroma.?key|color.?key.{0,20}transparent|transparent.{0,20}color.?key)", text_nt):
        if "no magenta" not in text_nt.lower() and "no color-key" not in text_nt.lower():
            errors.append(f"magenta-key heuristic in production renderer: {p.relative_to(ROOT)} (G02)")
# G03: shell must not import raw Pulse/NetworkManager provider APIs.
_shell_text = (read_required("crates/flamewm-shell/src/main.rs") + read_required("crates/flamewm-shell/src/runtime.rs"))
if re.search(r"use flamewm_integrations_linux::(pulse|network_manager)|use flamewm_platform::system::Pulse|NetworkManagerClient", _shell_text):
    errors.append("shell imports raw provider API; must use typed SystemAction only (G03)")
# G04: feature crates must not call XShape/XRender/Xcursor directly.
for crate in ["crates/flamewm-shell", "crates/flamewm-desktop", "crates/flamewm-settings",
              "crates/flamewm-platform", "crates/flamewm-api", "crates/flamewm-shell-core",
              "crates/flamewm-desktop-core"]:
    base = ROOT / crate
    if not base.exists():
        continue
    for p in active_files(base):
        if p.suffix != ".rs":
            continue
        if re.search(r"\b(XRender|XShape|Xcursor|Xft|XRenderComposite|XShapeCombine)\b", p.read_text(errors="ignore")):
            errors.append(f"feature crate calls native renderer API: {p.relative_to(ROOT)} (G04)")
# G05: wm-x11 must not use core image_text8 title path after C14 migration.
_wm_chrome = read_required("crates/flamewm-wm-x11/src/chrome.rs") + read_required("crates/flamewm-wm-x11/src/wm.rs")
if re.search(r"image_text8|XDrawString|9x15|TITLE_CHAR_ADVANCE", _wm_chrome):
    errors.append("wm-x11 still uses core image_text8 title path (G05)")
# G06: wm-x11 must not hand-sync duplicate chrome color constants.
if re.search(r"0x272a2d|0x46484b|0xe81123|TITLEBAR_BACKGROUND_RGB|CLOSE_HOVER_RGB", _wm_chrome):
    errors.append("wm-x11 has hand-synced duplicate chrome color constants (G06)")
# G07: ordinary active UI controls must not use cursor:pointer.
_shell_css = read_required("ui/shell/flamewm.css")
if "cursor:pointer" in _shell_css.replace(" ", ""):
    errors.append("ordinary UI CSS uses cursor:pointer (G07)")
# G08: no nmcli/pactl/wpctl subprocess control.
for p in active_files(ROOT / "crates"):
    if p.suffix != ".rs":
        continue
    if re.search(r'"(nmcli|pactl|wpctl)"|\bCommand::new\(\s*"(nmcli|pactl|wpctl)"', p.read_text(errors="ignore")):
        errors.append(f"subprocess system control in {p.relative_to(ROOT)} (G08)")
# G09: no fake KDE desktop identity.
for base in [ROOT / "crates", ROOT / "scripts"]:
    for p in active_files(base):
        text = p.read_text(errors="ignore")
        if re.search(r'XDG_CURRENT_DESKTOP["\']?\s*[,=]\s*["\']KDE["\']|set_var\(\s*"XDG_CURRENT_DESKTOP",\s*"KDE"\s*\)', text):
            errors.append(f"fake KDE desktop identity in {p.relative_to(ROOT)} (G09)")
# G10: architecture guard already rejects broad allow/unsafe suppression; static audit
# additionally rejects magenta PPM content in compiled-template image roles.
# J02: Xephyr harness must not impose GTK or Qt toolkit styles. Explicit inherited
# values are passed through by nested_env, but defaults would invalidate visual runs.
_J02_XEPHYR = read_required("scripts/xephyr")
if re.search(r'(?:GTK_THEME|QT_STYLE_OVERRIDE)\s*[:?+]?=\s*(?:Breeze(?:-Dark)?)', _J02_XEPHYR):
    errors.append("Xephyr harness forces GTK_THEME or QT_STYLE_OVERRIDE (J02)")
# --- J7 icon/alpha guards (J7-G1..J7-G8) ---
_J7_TEST_OWNERS = {
    "crates/flamewm-image-core",
    "crates/flamewm-integrations-linux",
    "crates/flamewm-render-core",
    "crates/flamewm-render-x11",
    "crates/flamewm-skin",
    "crates/flamewm-ui-core",
    "crates/flamewm-ui-x11",
}
_J7_KNOWN_KEYED_PPMS = set(_known_keyed_ppms)
_J7_MAGENTA_TRIPLE = re.compile(r"\[\s*255\s*,\s*0\s*,\s*255\s*\]")


def _j7_is_test_file(path: Path) -> bool:
    posix = path.as_posix()
    return "/tests/" in posix or "/tests/" in posix.replace("\\", "/") or path.name.endswith("_test.rs")


def _j7_production_rs(base: Path):
    for p in active_files(base):
        if p.suffix == ".rs" and not _j7_is_test_file(p):
            yield p


def _j7_guard_text(path: Path) -> str:
    text = path.read_text(errors="ignore")
    return re.sub(r"#\[cfg\(test\)\].*?\nmod tests \{.*?\n\}\n", "", text, flags=re.S)


# J7-G1: no normal production renderer path blends over black. Owner crates may
# keep the opaque-compositing helper/documented fallback; ordinary product code
# may not call it or reimplement the same compositing math.
for _p in _j7_production_rs(ROOT / "crates"):
    _t = _j7_guard_text(_p)
    _is_owner = any(str(_p.relative_to(ROOT)).startswith(owner + "/") for owner in
                    ("crates/flamewm-render-x11", "crates/flamewm-image-core"))
    if re.search(r"\bblend_over_black\b", _t) and not _is_owner:
        errors.append(f"normal production renderer blends over black: {_p.relative_to(ROOT)} (J7-G1)")
    if re.search(r"fallback-black|FallbackBlack|fallback_black", _t) and not _is_owner:
        errors.append(f"normal production renderer uses black fallback: {_p.relative_to(ROOT)} (J7-G1)")
# J7-G2: no canonical Flame surface constructor uses XCreateSimpleWindow; only
# the declared Xlib FFI binding owner may declare it.
for _p in _j7_production_rs(ROOT / "crates"):
    _t = _j7_guard_text(_p)
    if "XCreateSimpleWindow" in _t and _p.relative_to(ROOT).as_posix() != "crates/flamewm-render-x11/src/xlib.rs":
        errors.append(f"canonical Flame surface uses XCreateSimpleWindow: {_p.relative_to(ROOT)} (J7-G2)")
# J7-G3: active icon consumer may not reference known keyed PPM icon assets.
# Only real Rust sources are scanned: compiled shell templates are owned by
# the G01 template guard, and Markdown/docs plus the guard tables themselves
# are not icon consumers. Guard tables and test-only files are excluded.
_J7_G3_SKIP = {"tools/static_audit.py", "tools/architecture_guard.py"}
for _p in _j7_production_rs(ROOT / "crates"):
    _rel = _p.relative_to(ROOT).as_posix()
    if _rel in _J7_G3_SKIP or _p.suffix != ".rs":
        continue
    _t = _j7_guard_text(_p)
    for _m in re.findall(r"[A-Za-z0-9_./-]+\.ppm", _t):
        _base = _m.rsplit("/", 1)[-1]
        if _base in _J7_KNOWN_KEYED_PPMS:
            errors.append(f"active icon consumer references keyed PPM {_m}: {_p.relative_to(ROOT)} (J7-G3)")
# J7-G4: IconResolver generic fallback may not be .ppm.
for _line in read_required("crates/flamewm-integrations-linux/src/icons.rs").splitlines():
    _s = _line.strip()
    if _s.startswith("const GENERIC_FALLBACK") and ".ppm" in _s:
        errors.append("IconResolver generic fallback is PPM (J7-G4)")
# J7-G5: no production magenta-key/FF00FF color-key path.
for _p in _j7_production_rs(ROOT / "crates"):
    _t = _j7_guard_text(_p)
    _low = _t.lower()
    if ("ff00ff" in _low or "0xff00ff" in _low or "color-key" in _low or "color_key" in _low
            or "colorkey" in _low or "chroma" in _low):
        if "no magenta" not in _low and "no color-key" not in _low:
            errors.append(f"production magenta/FF00FF color-key path: {_p.relative_to(ROOT)} (J7-G5)")
# J7-G6: SymbolicForeground must not be inferred from filename.
for _p in _j7_production_rs(ROOT / "crates"):
    _t = _j7_guard_text(_p)
    if re.search(r"SymbolicForeground", _t) and re.search(
            r"(?i)(file.?name|extension|ends_with|\.svg|\.ppm|\.png).{0,80}SymbolicForeground|SymbolicForeground.{0,80}(file.?name|extension|\.svg|\.ppm|\.png)", _t):
        errors.append(f"SymbolicForeground inferred from filename: {_p.relative_to(ROOT)} (J7-G6)")
# J7-G7: real app icon path defaults to Original treatment.
_skin_icons = read_required("crates/flamewm-skin/src/icons.rs")
if "IconTreatment::Original" not in _skin_icons:
    errors.append("real app icon path missing Original default (J7-G7)")
if not re.search(r"Start\s*\|\s*.*Browser|Browser.*Start", _skin_icons):
    errors.append("real app icon roles missing Original treatment anchor (J7-G7)")
# J7-G8: shell symbolic icons consume semantic foreground, not embedded Breeze RGB.
_paint = read_required("crates/flamewm-render-core/src/paint.rs")
if "SymbolicForeground" in _paint and "resolve_color(style.color)" not in _paint:
    errors.append("shell symbolic icons do not consume semantic foreground (J7-G8)")
for _tpl in ["ui/shell/panel.html", "ui/shell/start.html"]:
    _tt = read_required(_tpl)
    if re.search(r"(?i)fill\s*:\s*#[0-9a-f]{6}|#[0-9a-f]{6}.*symbolic|symbolic.*#[0-9a-f]{6}", _tt):
        errors.append(f"embedded Breeze RGB in symbolic template {_tpl} (J7-G8)")
# --- J6 symbolic/migration guards (J6-G1..J6-G7) ---
# J6-G1 is hard-fail (already true): full-color Start role must stay Original.
# J6-G2..J6-G7 describe post-migration state; they currently fail
# pre-migration, so they are staged as WARN-only until migration lands.
_J6_WARNINGS: list[str] = []
# TODO(J12-PENDING): promote J6-G5/G6 to hard errors once packaged->system
# and z-index-store migrations land; currently WARN-only by design.


def _j6_code(text: str) -> str:
    return _strip_rust_comments(text)


def _j6_warn(message: str) -> None:
    _J6_WARNINGS.append(message)


_J6_SKIN = read_required("crates/flamewm-skin/src/icons.rs")
# J6-G1 (hard): Start category must not be SymbolicForeground.
if re.search(r"Self::Start[^\n]*SymbolicForeground|Start[^\n]{0,80}=>\s*IconTreatment::SymbolicForeground", _J6_SKIN):
    errors.append("full-color Start category uses SymbolicForeground (J6-G1)")
if "Self::Start" in _J6_SKIN and "IconTreatment::Original" not in _J6_SKIN:
    errors.append("Start category missing full-color Original treatment (J6-G1)")
# J6-G2 (enforced): local X11ChromePainter must be gone after migration.
_J6_CHROME = read_required("crates/flamewm-wm-x11/src/chrome.rs")
_J6_CHROME_CODE = _j6_code(_J6_CHROME)
if "struct X11ChromePainter" in _J6_CHROME_CODE:
    errors.append("local X11ChromePainter still present after migration (J6-G2)")
# J6-G3 (enforced): block-char title painter gone.
if re.search(r"fn draw_title_run", _J6_CHROME_CODE) and "poly_fill_rectangle" in _J6_CHROME_CODE:
    errors.append("block-char title painter still present (J6-G3)")
# J6-G4 (enforced): procedural control glyphs gone.
if re.search(r"fn draw_control_glyph", _J6_CHROME_CODE) and re.search(
    r"poly_line|poly_rectangle|poly_arc|poly_fill_arc", _J6_CHROME_CODE
):
    errors.append("procedural control glyphs still present (J6-G4)")
# J6-G5 (WARN-only): TODO(J12-PENDING) app icon packaged-before-system order.
_J6_ICONS = read_required("crates/flamewm-integrations-linux/src/icons.rs")
_m_pack = _j6_code(_J6_ICONS).find("packaged_candidates(name)")
_m_theme = _j6_code(_J6_ICONS).find("theme_candidates(name")
if _m_pack != -1 and _m_theme != -1 and _m_pack < _m_theme:
    _j6_warn("WARN TODO(J12-PENDING) app icon packaged-before-system order still present (J6-G5)")
# J6-G6 (WARN-only): TODO(J12-PENDING) new z-index store.
_J6_MODEL = read_required("crates/flamewm-render-core/src/model.rs")
if re.search(r"z_index_overrides|set_z_index|effective_z_index", _j6_code(_J6_MODEL)):
    _j6_warn("WARN TODO(J12-PENDING) new z-index store still present (J6-G6)")
# J6-G7 (enforced): root_pointer(0,0) placeholder gone.
_J6_SHELL_MAIN = read_required("crates/flamewm-shell/src/main.rs")
if re.search(r"root_pointer\(\s*0(\.0)?\s*,\s*0(\.0)?\s*\)", _j6_code(_J6_SHELL_MAIN)):
    errors.append("root_pointer(0,0) placeholder still present (J6-G7)")
# --- J12 focused guards (J12-G01..J12-G14): source-context only (comments stripped) ---
_J12_ROOT_DESKTOP_PROJ = read_required("crates/flamewm-desktop/src/projection.rs")
_J12_SHELL_PROJ = read_required("crates/flamewm-shell/src/projection.rs")
_J12_START_MENU = read_required("crates/flamewm-shell-core/src/start_menu.rs")
_J12_POPUP = read_required("crates/flamewm-ui-x11/src/popup.rs")
_J12_SURFACE_RT = read_required("crates/flamewm-ui-x11/src/surface_runtime.rs")
_J12_PAINT_DECOR = read_required("crates/flamewm-wm-x11/src/decoration/paint.rs")
_J12_MGR_DECOR = read_required("crates/flamewm-wm-x11/src/decoration/manager.rs")
_J12_SVG = read_required("crates/flamewm-image-core/src/svg.rs")
_J12_SESSION = read_required("crates/flamewm-session-core/src/lib.rs")
_J12_RENDER_PAINT = read_required("crates/flamewm-render-core/src/paint.rs")


def _j12_code(text: str) -> str:
    return _strip_rust_comments(text)


# J12-G01: invalid clamp(12.0,track) form must not appear in live code.
for _p in active_files(ROOT / "crates"):
    if _p.suffix != ".rs" or _j7_is_test_file(_p):
        continue
    if re.search(r"\bclamp\s*\(\s*12\.0", _j12_code(_p.read_text(errors="ignore"))):
        errors.append(f"invalid clamp(12.0,track) in live code: {_p.relative_to(ROOT)} (J12-G01)")
# J12-G02: replace_image must use node-local override, never mutate document.assets.
for _p in active_files(ROOT / "crates"):
    if _p.suffix != ".rs" or _j7_is_test_file(_p):
        continue
    _c = _j12_code(_p.read_text(errors="ignore"))
    if re.search(r"fn replace_image", _c) and re.search(
        r"\.assets\s*(\[[^\]]*\]\s*=|\.push\s*\(|\.insert\s*\(|\.clear\s*\()", _c
    ):
        errors.append(f"replace_image mutates document.assets: {_p.relative_to(ROOT)} (J12-G02)")
# J12-G03: no display:none on transient/popup/menu compiled templates (CSS/HTML context).
for _tpl in sorted((ROOT / "ui").rglob("*")):
    if not _tpl.is_file() or _tpl.suffix.lower() not in {".html", ".css"}:
        continue
    _t = re.sub(r"/\*.*?\*/", "", _tpl.read_text(errors="ignore"), flags=re.S)
    _t = re.sub(r"<!--.*?-->", "", _t, flags=re.S)
    if re.search(r"display\s*:\s*none", _t, re.I) and re.search(
        r"transient|popup-menu|context-menu|tooltip|dropdown", _t, re.I
    ):
        errors.append(f"display:none on transient node: {_tpl.relative_to(ROOT)} (J12-G03)")
# J12-G04: desktop root must sit on SurfaceBackground.
if "UiLayer::SurfaceBackground" not in _j12_code(_J12_ROOT_DESKTOP_PROJ):
    errors.append("desktop root missing SurfaceBackground layer (J12-G04)")
# J12-G05: semantic SVG themed render path present.
if "render_with_color_scheme" not in _j12_code(_J12_SVG):
    errors.append("semantic SVG themed render path missing (J12-G05)")
if "SymbolicForeground" not in _j12_code(_J12_RENDER_PAINT):
    errors.append("SymbolicForeground themed render missing in paint owner (J12-G05)")
# J12-G06: hidden create must not force redraw (body-bound).
_m = re.search(r"fn create_surface\(", _j12_code(_J12_SURFACE_RT))
if _m:
    _rest = _j12_code(_J12_SURFACE_RT)[_m.end():]
    _nxt = re.search(r"pub fn ", _rest)
    _body = _rest[:_nxt.start()] if _nxt else _rest[:800]
    if re.search(r"redraw|mark_full|Damage::Full", _body):
        errors.append("hidden create forces redraw (J12-G06)")
# J12-G07: popup raise policy = hide previous + show next.
if "runtime.hide" not in _j12_code(_J12_POPUP) or "runtime.show" not in _j12_code(_J12_POPUP):
    errors.append("popup raise policy missing hide-previous/show-next (J12-G07)")
# J12-G08: no sync IconResolver in category hot path.
if "IconResolver" in _j12_code(_J12_START_MENU):
    errors.append("sync IconResolver in category hot path (J12-G08)")
# J12-G09: Unknown category stays unbucketed (not visible).
if '_ => None' not in _j12_code(_J12_START_MENU):
    errors.append("Unknown category fallback missing (J12-G09)")
if 'presentation_key("Unknown"), None' not in _J12_START_MENU:
    errors.append("Unknown-not-visible anchor missing (J12-G09)")
# J12-G10: no image_text8/fixed-font normal decoration path in live code.
for _p in [ROOT / "crates/flamewm-wm-x11/src/decoration/paint.rs",
           ROOT / "crates/flamewm-wm-x11/src/decoration/manager.rs",
           ROOT / "crates/flamewm-wm-x11/src/wm.rs",
           ROOT / "crates/flamewm-wm-x11/src/chrome.rs"]:
    if _p.is_file() and re.search(
        r"image_text8|XDrawString|fixed_font|fixed-font|9x15|TITLE_CHAR_ADVANCE",
        _j12_code(_p.read_text(errors="ignore")),
    ):
        errors.append(f"legacy fixed-font decoration path: {_p.relative_to(ROOT)} (J12-G10)")
# J18 frame-engine owner (declared under flamewm-wm-x11).
_J12_FRAME_ENGINE_OWNER = "crates/flamewm-wm-x11"
_J12_GEOMETRY_OWNER = {
    "crates/flamewm-wm-x11/src/client.rs",
    "crates/flamewm-wm-x11/src/frame/controller.rs",
    "crates/flamewm-wm-x11/src/frame/geometry.rs",
    "crates/flamewm-wm-x11/src/frame/model.rs",
    "crates/flamewm-wm-x11/src/wm.rs",
}
_J12_HIT_OWNER = {
    "crates/flamewm-wm-x11/src/frame/input.rs",
    "crates/flamewm-wm-x11/src/frame/controller.rs",
    "crates/flamewm-wm-x11/src/decoration/interaction.rs",
    "crates/flamewm-wm-x11/src/decoration/manager.rs",
}
_J12_TITLE_OWNER = {
    "crates/flamewm-wm-x11/src/frame/chrome.rs",
    "crates/flamewm-render-x11/src/external_decoration.rs",
    "crates/flamewm-wm-x11/src/decoration/manager.rs",
    "crates/flamewm-wm-x11/src/decoration/paint.rs",
}
# J18-G01: direct outer-rect writes inside wm-x11 but outside the geometry
# owner bypass the frame engine.
for _p in sorted((ROOT / "crates/flamewm-wm-x11/src").rglob("*.rs")):
    if _j7_is_test_file(_p) or _p.relative_to(ROOT).as_posix() in _J12_GEOMETRY_OWNER:
        continue
    _t = _j12_code(_p.read_text(errors="ignore"))
    if re.search(r"\.outer\s*=", _t) or re.search(r"\.restore\s*=", _t):
        errors.append(f"direct outer rect write outside geometry owner: {_p.relative_to(ROOT)} (J18-G01)")
# J18-G02: exactly one live window-geometry/decoration owner pair.
_J18_GEO_COUNT = sum(
    len(re.findall(r"struct PlacementState", _j12_code(p.read_text(errors="ignore"))))
    for p in active_files(ROOT / "crates")
    if p.suffix == ".rs" and not _j7_is_test_file(p)
)
_J18_PLAN_COUNT = sum(
    len(re.findall(r"struct GeometryPlan", _j12_code(p.read_text(errors="ignore"))))
    for p in active_files(ROOT / "crates")
    if p.suffix == ".rs" and not _j7_is_test_file(p)
)
if _J18_GEO_COUNT != 1:
    errors.append(f"second live window-geometry owner: PlacementState count is {_J18_GEO_COUNT}, want 1 (J18-G02)")
if _J18_PLAN_COUNT != 1:
    errors.append(f"second live decoration owner: GeometryPlan count is {_J18_PLAN_COUNT}, want 1 (J18-G02)")
# J18-G03: single pointer-hit owner (semantic fn definitions, not filenames).
for _p in active_files(ROOT / "crates"):
    if _p.suffix != ".rs" or _j7_is_test_file(_p) or _p.relative_to(ROOT).as_posix() in _J12_HIT_OWNER:
        continue
    if re.search(r"fn\s+(resolve_pointer_intent|target_for_xid|pointer_intent_at)\s*\(", _j12_code(_p.read_text(errors="ignore"))):
        errors.append(f"second pointer hit owner: {_p.relative_to(ROOT)} (J18-G03)")
# J18-G04: no production old decoration paint call outside the title owner.
for _p in active_files(ROOT / "crates"):
    if _p.suffix != ".rs" or _j7_is_test_file(_p) or _p.relative_to(ROOT).as_posix() in _J12_TITLE_OWNER:
        continue
    if re.search(r"paint::paint_frame\s*\(", _j12_code(_p.read_text(errors="ignore"))):
        errors.append(f"old decoration paint call outside owner: {_p.relative_to(ROOT)} (J18-G04)")
# J18-G05: no new raw title renderer in wm-x11 outside the title owner.
for _p in active_files(ROOT / "crates"):
    if _p.suffix != ".rs" or _j7_is_test_file(_p) or _p.relative_to(ROOT).as_posix() in _J12_TITLE_OWNER:
        continue
    if re.search(r"\.draw_title\s*\(", _j12_code(_p.read_text(errors="ignore"))):
        errors.append(f"raw title renderer outside owner: {_p.relative_to(ROOT)} (J18-G05)")
_J12_FRAME_ENGINE_SENTINEL = _J12_FRAME_ENGINE_OWNER
# J12-G11: exactly one DecorationManager owner (J16 cutover: DecorationManager
# deleted; the frame engine FrameResources registry is the successor owner).
_j12_mgr_count = sum(
    len(re.findall(r"struct DecorationManager", _j12_code(p.read_text(errors="ignore"))))
    for p in active_files(ROOT / "crates")
    if p.suffix == ".rs" and not _j7_is_test_file(p)
)
_j12_frame_count = sum(
    len(re.findall(r"struct FrameResources", _j12_code(p.read_text(errors="ignore"))))
    for p in active_files(ROOT / "crates")
    if p.suffix == ".rs" and not _j7_is_test_file(p)
)
if not (_j12_mgr_count == 1 or (_j12_mgr_count == 0 and _j12_frame_count == 1)):
    errors.append(f"DecorationManager/frame-engine owner count is mgr={_j12_mgr_count} frame={_j12_frame_count}, want mgr=1 or frame-engine FrameResources=1 (J12-G11)")
# J12-G12: wallpaper and CPU paths stay separate (no cpu conflation in desktop owner).
if "wallpaper" not in _j12_code(_J12_ROOT_DESKTOP_PROJ).lower():
    errors.append("desktop wallpaper path missing (J12-G12)")
for _p in active_files(ROOT / "crates/flamewm-desktop/src"):
    if _p.suffix == ".rs" and re.search(r"(?i)\bcpu\b", _j12_code(_p.read_text(errors="ignore"))):
        errors.append(f"CPU conflated into desktop owner: {_p.relative_to(ROOT)} (J12-G12)")
# J12-G13: .performance evidence dir stays ignored.
if ".performance" not in (ROOT / ".gitignore").read_text(errors="ignore"):
    errors.append(".performance is not ignored (J12-G13)")
# J12-G14: Breeze cursor authority intact. Session-core owns the bundled
# FlameWM-Breeze-Dark theme + XCURSOR env authority; the frame engine owns
# the cursor-region binding via cursor_for_region (frame/resources.rs),
# applied by wm.rs bind_child_cursors.
if "FlameWM-Breeze-Dark" not in _J12_SESSION or "XCURSOR" not in _J12_SESSION:
    errors.append("Breeze cursor authority missing in session-core (J12-G14)")
_J12_FRAME_RES = read_required("crates/flamewm-wm-x11/src/frame/resources.rs")
if "cursor_for_region" not in _j12_code(_J12_FRAME_RES):
    errors.append("frame-engine cursor binding missing (cursor_for_region in frame/resources.rs) (J12-G14)")
if "Breeze-Dark" not in _J12_PAINT_DECOR and "CURSOR_THEME" not in _J12_PAINT_DECOR and "cursor_for_region" not in _J12_PAINT_DECOR:
    if not (ROOT / "crates/flamewm-wm-x11/src/decoration/paint.rs").exists():
        # J16 cutover removed decoration/paint.rs; Breeze anchor lives in
        # session-core + frame/resources.rs cursor binding instead.
        if "cursor_for_region" not in _j12_code(_J12_FRAME_RES):
            errors.append("Breeze cursor anchor missing in frame-engine cursor binding (J12-G14)")
    else:
        errors.append("Breeze cursor anchor missing in decoration intent side (J12-G14)")
# --- J12 handoff guards (J12H-G01..G15): source-context only (comments stripped) ---
_J12H_PAINT_CORE = read_required("crates/flamewm-render-core/src/paint.rs")
_J12H_START_MENU_CORE = read_required("crates/flamewm-shell-core/src/start_menu.rs")
_J12H_PROJ = read_required("crates/flamewm-shell/src/projection.rs")
_J12H_APP = read_required("crates/flamewm-shell/src/app.rs")
_J12H_RUNTIME = read_required("crates/flamewm-shell/src/runtime.rs")
_J12H_POPUP_CONTROLLER = read_required("crates/flamewm-shell/src/popup_controller.rs")
_J12H_STATUS_SURFACE = read_required("crates/flamewm-shell/src/runtime/status_surface.rs")
_J12H_MAIN = read_required("crates/flamewm-shell/src/main.rs")
_J12H_AUDIO = read_required("crates/flamewm-shell/src/taskbar/status/audio.rs")
_J12H_NETWORK = read_required("crates/flamewm-shell/src/taskbar/status/network.rs")
_J12H_FILE_ACTIONS = read_required("crates/flamewm-desktop-core/src/file_actions.rs")
_J12H_STICKY_PERSIST = read_required("crates/flamewm-desktop-core/src/sticky_persistence.rs")
_J12H_CHROME = read_required("crates/flamewm-wm-x11/src/chrome.rs")
_J12H_WM = read_required("crates/flamewm-wm-x11/src/wm.rs")
_J12H_EXT = read_required("crates/flamewm-render-x11/src/external_drawable.rs")
_J12H_PALETTE = read_required("crates/flamewm-skin/src/palette.rs")
_J12H_DESKTOP_CSS = read_required("ui/desktop/desktop.css")
_J12H_SPAN = read_required("crates/flamewm-profiler/src/span.rs")
# J12H-G01: StrokeRect carries explicit radius (no square-corner regression).
if "StrokeRect" not in _j12_code(_J12H_PAINT_CORE) or "radius" not in _j12_code(_J12H_PAINT_CORE):
    errors.append("StrokeRect radius missing (J12H-G01)")
# J12H-G02: exactly one StartModel owner.
_J12H_STARTMODEL_COUNT = sum(
    len(re.findall(r"struct StartModel", _j12_code(p.read_text(errors="ignore"))))
    for p in active_files(ROOT / "crates")
    if p.suffix == ".rs" and not _j7_is_test_file(p)
)
if _J12H_STARTMODEL_COUNT != 1:
    errors.append(f"StartModel owner count is {_J12H_STARTMODEL_COUNT}, want 1 (J12H-G02)")
# J12H-G03: borrowed query hot path exists (no clone-only query model).
for _anchor in ["fn filter_view", "fn query_view", "fn slot_view"]:
    if _anchor not in _j12_code(_J12H_START_MENU_CORE):
        errors.append(f"borrowed StartModel query view missing {_anchor} (J12H-G03)")
# J12H-G04: Power visibility is explicit (session-gated, not always-on).
if "StartCategory::Power" not in _j12_code(_J12H_PROJ) or "session" not in _j12_code(_J12H_PROJ).lower():
    errors.append("Power visibility not session-gated (J12H-G04)")
# J12H-G05: changed-slot icon reset is surface-scoped (no blanket reproject).
if "need_panel" not in _j12_code(_J12H_APP) or "need_submenu" not in _j12_code(_J12H_APP):
    errors.append("changed slot icon reset not surface-scoped (J12H-G05)")
# J12H-G06: production OpenPopover path is anchored (measured, not fixed rect).
# `open_status_anchored_id` enters the live status-surface transaction;
# popup_controller owns retained source resolution and node measurement.
if (
    "open_status_anchored_id" not in _j12_code(_J12H_RUNTIME)
    or "resolve_source_rect" not in _j12_code(_J12H_POPUP_CONTROLLER)
    or "measure_node" not in _j12_code(_J12H_POPUP_CONTROLLER)
    or "popup_controller::open_popup" not in _j12_code(_J12H_STATUS_SURFACE)
):
    errors.append("OpenPopover production path not anchored (J12H-G06)")
# J12H-G07: audio/network popovers have no fixed live authority (availability-gated).
if "ServiceAvailability::Available" not in _j12_code(_J12H_AUDIO):
    errors.append("audio live path not availability-gated (J12H-G07)")
if "ServiceAvailability::Available" not in _j12_code(_J12H_NETWORK):
    errors.append("network live path not availability-gated (J12H-G07)")
# J12H-G08: desktop context rows carry border 0.
if "border-width:0" not in _J12H_DESKTOP_CSS.replace(" ", ""):
    errors.append("desktop context border 0 missing (J12H-G08)")
# J12H-G09: rename is NOREPLACE (no silent overwrite).
if "RenameFlags::NOREPLACE" not in _j12_code(_J12H_FILE_ACTIONS):
    errors.append("NOREPLACE rename missing (J12H-G09)")
# J12H-G10: sticky state accepts v1 and v2 headers.
if "v1\\t" not in _J12H_STICKY_PERSIST or "v2\\t" not in _J12H_STICKY_PERSIST:
    errors.append("sticky v1+v2 headers missing (J12H-G10)")
# J12H-G11: WorkspacesChanged travels event-driven (no poll loop). Anchor
# lives in app.rs; check raw text plus revision-gated code path. The tick
# calls only it, event paths mark dirty instead of refreshing inline, and
# steady state documents never-poll/no-polling.
if ("refresh_dynamic" not in _j12_code(_J12H_APP)
        or ("never polls" not in _J12H_APP.lower() and "no polling" not in _J12H_RUNTIME.lower())):
    errors.append("WorkspacesChanged event-driven contract missing (J12H-G11)")
# J12H-G12: honest ExternalDrawable contract (safe-x11rb transport; geometry/material via skin).
if "ExternalDrawableTarget" not in (_J12_PAINT_DECOR + _J12H_CHROME) or "ExternalDrawableSession" not in _J12H_EXT:
    errors.append("ExternalDrawable delegation contract missing (J12H-G12)")
if "flamewm_skin" not in _j12_code(_J12H_CHROME) and "WINDOW_CHROME" not in _j12_code(_J12H_CHROME):
    errors.append("skin-metrics geometry/material contract missing (J12H-G12)")
# J12H-G13: no title-requires-xft normal path (measured intent is normal success).
_J12H_DECO_FILES = [
    ROOT / "crates/flamewm-wm-x11/src/decoration/paint.rs",
    ROOT / "crates/flamewm-wm-x11/src/chrome.rs",
    ROOT / "crates/flamewm-wm-x11/src/wm.rs",
]
for _p in _J12H_DECO_FILES:
    if _p.is_file():
        _t = _j12_code(_p.read_text(errors="ignore"))
        if "image_text8" in _t or "XDrawString" in _t:
            errors.append(f"legacy title path in decoration intent: {_p.relative_to(ROOT)} (J12H-G13)")
if (ROOT / "crates/flamewm-wm-x11/src/decoration/paint.rs").is_file():
    if "title_drawn = true" not in _j12_code(_J12_PAINT_DECOR):
        errors.append("measured title intent success missing (J12H-G13)")
# J12H-G14: PANEL stays #1b1e20.
if "0x1b1e20" not in _j12_code(_J12H_PALETTE).lower():
    errors.append("PANEL #1b1e20 missing in skin palette (J12H-G14)")
# J12H-G15: profiler spans are scoped static labels.
_J12H_PROF_FILES = [
    ROOT / "crates/flamewm-profiler/src/span.rs",
    ROOT / "crates/flamewm-profiler/src/point.rs",
    ROOT / "crates/flamewm-profiler/src/lib.rs",
]
_J12H_SPAN_ALL = "".join(f.read_text(errors="ignore") for f in _J12H_PROF_FILES if f.is_file())
if _J12H_SPAN_ALL and "&'static str" not in _J12H_SPAN_ALL:
    errors.append("scoped profiler span labels missing (J12H-G15)")
if 'ProfilePoint::new("shell.' not in _J12H_MAIN and "flamewm_profiler::start(" not in _J12H_MAIN:
    errors.append("scoped shell profiler spans missing (J12H-G15)")
# --- J11 convergence guards (J11-G01..G10): source-context only (comments stripped) ---
# Each guard passes on the converged tree and hard-fails if the retired
# pattern is reintroduced.


def _j11_prod_rs(base: Path):
    for p in active_files(base):
        if p.suffix == ".rs" and not _j7_is_test_file(p):
            yield p


def _j11_strip_test_items(text: str) -> str:
    """Drop `#[cfg(test)]` items (test fns and test modules) from code context."""
    lines = text.splitlines(keepends=True)
    out: list[str] = []
    i = 0
    while i < len(lines):
        if lines[i].strip().startswith("#[cfg(test)]"):
            i += 1
            while i < len(lines) and not lines[i].strip():
                out.append(lines[i])
                i += 1
            # Skip one item: find opening brace, then balance to its close.
            start = i
            while i < len(lines) and "{" not in lines[i]:
                i += 1
            if i >= len(lines):
                break
            depth = 0
            while i < len(lines):
                depth += lines[i].count("{") - lines[i].count("}")
                i += 1
                if depth <= 0:
                    break
            continue
        out.append(lines[i])
        i += 1
    return "".join(out)


# J11-G01: move_resize is the single native geometry owner; size changes go
# through commit_geometry, and no other production file issues XMoveResizeWindow.
_move_resize_src = ROOT / _J11_MOVE_RESIZE_OWNER
if _move_resize_src.is_file():
    _mr_body = _j11_fn_body(_j11_code(_move_resize_src.read_text(errors="ignore")), "move_resize")
    if "commit_geometry" not in _mr_body:
        errors.append("move_resize bypasses commit_geometry (J11-G01)")
    if "XMoveResizeWindow" not in _mr_body:
        errors.append("move_resize lost native move ownership (J11-G01)")
for _p in _j11_prod_rs(ROOT / "crates"):
    _rel = _p.relative_to(ROOT).as_posix()
    if _rel in (_J11_MOVE_RESIZE_OWNER, _J11_XLIB_DECL):
        continue
    _t = re.sub(r"#\[cfg\(test\)\].*?\nmod tests \{.*?\n\}\n", "",
                _p.read_text(errors="ignore"), flags=re.S)
    if _j11_g01_nonowner_violation(_rel, _strip_rust_comments(_t)):
        errors.append(f"raw native move outside move_resize owner: {_rel} (J11-G01)")
# J11-G02: Start is one unified surface; the split start_submenu runtime
# surface must not return (projection helper fn names carry the substring
# but are not surfaces; only standalone member uses trip this).
for _path in ["crates/flamewm-shell/src/runtime.rs", "crates/flamewm-shell/src/main.rs"]:
    _t = _j11_code(read_required(_path))
    if re.search(r"(?<![A-Za-z0-9_])start_submenu(?![A-Za-z0-9_])", _t):
        errors.append(f"split start_submenu runtime surface reintroduced: {_path} (J11-G02)")
# J11-G03: no procedural solid-red fallback raster in production code.
# Test-only modules are stripped; the converged Flame fallback is brand
# [239,64,72], never solid [255,0,0,255].
for _p in _j11_prod_rs(ROOT / "crates"):
    _t = re.sub(r"#\[cfg\(test\)\].*?\nmod tests \{.*?\n\}\n", "",
                _p.read_text(errors="ignore"), flags=re.S)
    _t = _j11_strip_test_items(_strip_rust_comments(_t))
    _t = _j11_strip_test_items(_strip_rust_comments(_t))
    if re.search(r"255\s*,\s*0\s*,\s*0\s*,\s*255", _t):
        errors.append(f"procedural solid-red fallback in production code: {_p.relative_to(ROOT)} (J11-G03)")
# J11-G04: desktop cold miss paints the Flame fallback synchronously (never
# blank, never stale); project_row must keep the fallback arm.
_j11_desk = _j11_code(read_required("crates/flamewm-desktop/src/projection.rs"))
_j11_row = _j11_fn_body(_j11_desk, "project_row")
if "project_row" not in _j11_desk or "flame_fallback" not in _j11_row:
    errors.append("desktop cold miss lost synchronous fallback (J11-G04)")
# J11-G05: no sync IconResolver in WM draw/manage; the decoration manager
# owns resolution, wm.rs only passes already-resolved sources. The
# IconService<IconResolver> generic import is async-service use, not sync
# resolution, so only direct prepare_*/resolve calls in wm.rs trip this.
_j11_wm = _j11_code(read_required("crates/flamewm-wm-x11/src/wm.rs"))
_j11_chrome_runtime = _j11_code(read_required("crates/flamewm-wm-x11/src/wm/chrome_runtime.rs"))
if (
    "prepare_application" in _j11_wm
    or "prepare_semantic" in _j11_wm
    or "prepare_path(" in _j11_wm
    or "resolve_app_icon_via" in _j11_wm
):
    errors.append("sync IconResolver in WM draw/manage path (J11-G05)")
# J11-G06: frame chrome paints through the frame-engine owner (paint_chrome /
# chrome_runtime cached scene_for+paint_cached bridge); no raw non-renderer
# paint path (direct paint:: calls or core text draws). The retired
# Wm::draw_frame painter was deleted in the J16 frame-engine cutover.
_has_draw = "draw_frame" in _j11_wm
_has_chrome = "fn paint_chrome" in _j11_wm
if not _has_draw and not _has_chrome:
    errors.append("frame chrome paint path missing (need draw_frame or paint_chrome) (J11-G06)")
elif _has_draw:
    if "self.decorations" not in _j11_draw:
        errors.append("Wm::draw_frame bypasses DecorationManager (J11-G06)")
    elif re.search(r"paint::paint_frame|image_text8|XDrawString|XRenderComposite", _j11_draw):
        errors.append("Wm::draw_frame uses raw non-renderer paint path (J11-G06)")
if _has_chrome:
    _j11_chrome_body = _j11_fn_body(_j11_wm, "paint_chrome")
    if re.search(r"paint::paint_frame|image_text8|XDrawString|XRenderComposite", _j11_chrome_body):
        errors.append("paint_chrome uses raw non-renderer paint path (J11-G06)")
    elif not (
        "frame_chrome::plan_scene" in _j11_chrome_body
        or "frame_chrome::render" in _j11_chrome_body
        or (
            "runtime.scene_for" in _j11_chrome_body
            and "runtime.paint_cached" in _j11_chrome_body
            and "pub fn scene_for" in _j11_chrome_runtime
            and "pub fn paint_cached" in _j11_chrome_runtime
        )
    ):
        errors.append("paint_chrome bypasses frame chrome owner (J11-G06)")
# J11-G07: performance retention cleanup defaults ON (opt-out, not opt-in).
_j11_ret = read_required("tools/performance_retention.py")
if 'os.environ.get("FLAMEWM_PERFORMANCE_CLEANUP", "1")' not in _j11_ret:
    errors.append("performance cleanup default is not opt-out-on (J11-G07)")
# J11-G08: no unprefixed env reads outside the platform allowlist.
# FLAMEWM_* product vars plus well-known OS/desktop/build names are fine;
# any other bare literal is an unprefixed env dependency.
_J11_ENV_OK = re.compile(
    r"^(FLAMEWM_[A-Z0-9_]+|XDG_[A-Z0-9_]+|XCURSOR_[A-Z0-9_]+|DESKTOP_SESSION"
    r"|HOME|PATH|TERMINAL|CARGO_MANIFEST_DIR|OUT_DIR)$"
)
for _p in _j11_prod_rs(ROOT / "crates"):
    _t = _strip_rust_comments(_p.read_text(errors="ignore"))
    for _lit in re.findall(r'env::(?:var|var_os)\(\s*"([^"]+)"', _t):
        if not _J11_ENV_OK.match(_lit):
            errors.append(f"unprefixed env literal {_lit!r}: {_p.relative_to(ROOT)} (J11-G08)")
    for _lit in re.findall(r'(?:option_env!|env!)\(\s*"([^"]+)"', _t):
        if not _J11_ENV_OK.match(_lit):
            errors.append(f"unprefixed env macro literal {_lit!r}: {_p.relative_to(ROOT)} (J11-G08)")
# J11-G09: profile log truncates once per process generation; the winning
# init owns it, and no other site wipes the log.
_j11_rep = read_required("crates/flamewm-profiler/src/report.rs")
_j11_rep_code = _j11_code(_j11_rep)
_j11_trunc = _j11_fn_body(_j11_rep_code, "truncate_profile_log_once")
if "truncate_profile_log_once" not in _j11_rep_code or ".truncate(true)" not in _j11_trunc:
    errors.append("profile log missing once-per-generation truncate (J11-G09)")
elif _j11_rep_code.count(".truncate(true)") != _j11_trunc.count(".truncate(true)"):
    errors.append("profile truncate outside once-per-generation owner (J11-G09)")
if "truncate_profile_log_once(name)" not in _j11_rep_code:
    errors.append("profile truncate not owned by winning init (J11-G09)")
# J11-G10: memory gauges are live (constructed gauges are set; set() stores).
_j11_mem = _j11_code(read_required("crates/flamewm-profiler/src/memory.rs"))
_j11_set = _j11_fn_body(_j11_mem, "set")
if ".store(" not in _j11_set:
    errors.append("MemoryGauge::set is a no-op (J11-G10)")
for _p in _j11_prod_rs(ROOT / "crates"):
    if _p.name == "memory.rs":
        continue
    _t = _strip_rust_comments(_p.read_text(errors="ignore"))
    if "MemoryGauge::new" in _t and ".set(" not in _t:
        errors.append(f"constructed gauge never set (no-op gauge): {_p.relative_to(ROOT)} (J11-G10)")
for _w in _J6_WARNINGS:
    print(_w, file=sys.stderr)
# --- J10 anti-regression guards (J10-G01..G09): source-context only ---
# Hard-fail where the converged tree already holds; WARN-only where the
# pre-J07 tree has not migrated yet (explicit reason, never a hard fail).
_J10_WARNINGS: list[str] = []


def _j10_warn(message: str) -> None:
    _J10_WARNINGS.append(message)


_J10_XEPHYR = read_required("scripts/xephyr")
# J10-G01 (hard): Xephyr private D-Bus default present.
if "FLAMEWM_XEPHYR_PRIVATE_DBUS:-1" not in _J10_XEPHYR or "dbus-run-session" not in _J10_XEPHYR:
    errors.append("Xephyr private D-Bus default missing (J10-G01)")
# J10-G02 (hard): shared host-session fallback forbidden in perf gate.
for _perf in ["tools/profile_summary.py", "tools/performance_bundle.py", "tools/performance_retention.py"]:
    _pt = read_required(_perf)
    if re.search(r"DBUS_SESSION_BUS_ADDRESS|host.session|host_session", _pt):
        errors.append(f"shared host-session fallback in perf gate {_perf} (J10-G02)")
# J10-G03 (WARN-only): pre-J07 tree still owns audio/network/calendar
# native surfaces in parent ShellSurfaces; parent migration to the
# standalone quick-control host has not landed, so this cannot hard-fail.
_j10_rt = _j12_code(read_required("crates/flamewm-shell/src/runtime.rs"))
if re.search(r"struct ShellSurfaces", _j10_rt) and all(
    k in _j10_rt for k in ["self.audio", "self.network", "self.calendar"]
):
    _j10_warn("WARN TODO(J07-PENDING) parent ShellSurfaces still owns audio/network/calendar surfaces (J10-G03)")
# J10-G04 (hard): helper cannot own NetworkManager/Pulse provider directly.
for _hp in active_files(ROOT / "crates/flamewm-shell/src/quick_controls"):
    if _hp.suffix != ".rs" or _j7_is_test_file(_hp):
        continue
    _ht = _j12_code(_hp.read_text(errors="ignore"))
    if re.search(r"NetworkManagerProvider|PulseProvider|PulseAudioProvider|pulse::|network_manager::", _ht):
        errors.append(f"quick-control helper owns native provider: {_hp.relative_to(ROOT)} (J10-G04)")
# J10-G05 (WARN-only): Start runtime still uses the measured path;
# fitted_start_placement migration has not landed in runtime.rs.
if "fitted_start_placement" not in _j10_rt:
    _j10_warn("WARN TODO(J07-PENDING) Start runtime does not call fitted_start_placement (J10-G05)")
# J10-G06 (hard): no start_submenu native surface (artifact/surface-scoped;
# projection helper fns are not surfaces and must not trip this).
for _sp, _st in [
    ("crates/flamewm-shell/src/runtime.rs", _j10_rt),
    ("crates/flamewm-shell/src/main.rs", _j12_code(read_required("crates/flamewm-shell/src/main.rs"))),
    ("crates/flamewm-shell/build.rs", read_required("crates/flamewm-shell/build.rs")),
]:
    if re.search(r"flamewm-start-submenu|start_submenu.*Surface|Surface.*start_submenu|create_surface\([^)]*submenu", _st):
        errors.append(f"split start_submenu surface reintroduced: {_sp} (J10-G06)")
# J10-G07 (hard): no blocking Child::wait on shell event owner.
# Allowed: wait_timeout, try_wait, or kill-then-wait reap idiom.
for _wp in active_files(ROOT / "crates/flamewm-shell/src"):
    if _wp.suffix != ".rs" or _j7_is_test_file(_wp):
        continue
    _wt = _j12_code(_wp.read_text(errors="ignore"))
    if re.search(r"\.wait_timeout\s*\(", _wt):
        continue
    for _wm in re.finditer(r"\.wait\s*\(\s*\)", _wt):
        _ctx = _wt[max(0, _wm.start() - 600):_wm.end() + 200]
        if "try_wait" in _ctx or "wait_timeout" in _ctx or ".kill()" in _ctx or "kill()" in _wt:
            continue
        errors.append(f"blocking Child::wait on shell event owner: {_wp.relative_to(ROOT)} (J10-G07)")
        break
# J10-G08 (hard): no shell interpolation in quick-control protocol.
# String literals and #[cfg(test)] modules are stripped: protocol tests
# embed shell metacharacters as rejected-input fixtures, not live use.
def _j10_unquoted(text: str) -> str:
    text = re.sub(r"#\[cfg\(test\)\].*?\nmod tests \{.*?\n\}\n", "", text, flags=re.S)
    return re.sub(r'"(?:[^"\\]|\\.)*"', '""', text)


for _qp in active_files(ROOT / "crates/flamewm-shell/src/quick_controls"):
    if _qp.suffix != ".rs" or _j7_is_test_file(_qp):
        continue
    _qt = _j10_unquoted(_j12_code(_qp.read_text(errors="ignore")))
    if re.search(r'sh\s+-c|Command::new\(\s*"sh"|/bin/sh|\beval\s*\(|\bsystem\s*\(|`[^`]*`|\$\(', _qt):
        errors.append(f"shell interpolation in quick-control protocol: {_qp.relative_to(ROOT)} (J10-G08)")
_j10_proto = read_required("crates/flamewm-shell/src/quick_controls/protocol.rs")
if "split_ascii_whitespace" not in _j10_proto and "split_whitespace" not in _j10_proto:
    errors.append("quick-control protocol lost whitespace-split decode (J10-G08)")
# J10-G09 (hard): no dynamic profiler labels on hot paths.
# format!-built labels detected only when scoped to profiler calls.
_J10_PROF_CALL = re.compile(
    r"(CounterPoint::new|ProfilePoint::new|flamewm_profiler::start|CounterSlot::new)\s*\(\s*(?:&)?format!\s*\("
)
for _pp in active_files(ROOT / "crates"):
    if _pp.suffix != ".rs" or _j7_is_test_file(_pp):
        continue
    _ptx = _j12_code(_pp.read_text(errors="ignore"))
    if _J10_PROF_CALL.search(_ptx):
        errors.append(f"dynamic profiler label on hot path: {_pp.relative_to(ROOT)} (J10-G09)")
for _w in _J10_WARNINGS:
    print(_w, file=sys.stderr)
if errors:
    for error in errors:
        print(f"ERROR {error}", file=sys.stderr)
    raise SystemExit(1)

print(f"OK FlameWM Rust WebRender {version} static audit")
print("OK J7 icon/alpha guards G1-G8 clean")

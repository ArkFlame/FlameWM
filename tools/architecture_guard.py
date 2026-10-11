#!/usr/bin/env python3
"""Enforce the active product-layer architecture boundary."""

from pathlib import Path
import re
import sys
import tomllib

try:
    from static_audit import _strip_rust_comments
except ImportError:
    from .static_audit import _strip_rust_comments


PRODUCT_CRATES = {
    "flamewm-api",
    "flamewm-applications",
    "flamewm-control-core",
    "flamewm-control-dbus",
    "flamewm-control-wire",
    "flamewm-dbus-reactor",
    "flamewm-desktop-core",
    "flamewm-shell",
    "flamewm-shell-core",
    "flamewm-desktop",
    "flamewm-settings",
    "flamewm-settings-core",
    "flamewm-platform",
    "flamewm-wm",
    "flamewm-reactor",
    "flamewm-skin",
    "flamewm-ui",
    "flamewm-ui-core",
    "flamewm-window-core",
    "flamewm-session-core",
    "flamewm-integrations-core",
    "flamewm-render-compiler",
    "flamewm-render-core",
    "flamewm-render-x11",
    "flamewm-ui-x11",
    "flamewm-wm-x11",
}
# These crates are the canonical native/render owners. Their low-level APIs are
# intentionally allowed to mention the capabilities guarded elsewhere.
OWNER_CRATES = {
    "flamewm-render-compiler",
    "flamewm-render-core",
    "flamewm-render-x11",
    "flamewm-ui-x11",
    "flamewm-wm-x11",
}
FORBIDDEN_DEPENDENCIES = {"flamewm-render-core", "flamewm-render-x11"}
FORBIDDEN_RENDERER_SOURCE = re.compile(
    r"RuntimeDocument|flamewm_render_(?:core|x11)::|"
    r"(?:XOpenDisplay|XMapWindow|XGrabPointer|XRender|Xft)|"
    r"\bdocument\.(?:set_|clear_|update_|remove_|add_|insert_)"
)
FORBIDDEN_X11_SOURCE = re.compile(r"\bx11rb::")
# Product crates may depend on a typed owner, but may not grow a second native
# boundary or hide warning debt behind a crate-wide suppression.
RAW_FFI_SOURCE = re.compile(
    r'extern\s+"C"|std::os::raw|\b(?:libc|libloading)::|\b(?:dlopen|dlsym)\s*\('
)
BROAD_SUPPRESSION = re.compile(
    r"#\s*!?\s*\[allow\((?:warnings|unused(?:_[a-z_]+)?|clippy::(?:all|pedantic))\)\]"
)
UNSAFE_FUNCTION = re.compile(r"\bunsafe\s+fn\s+[A-Za-z_][A-Za-z0-9_]*")

# These are diagnostic budgets, not ownership rules. They stop a facade or
# core from becoming an unreviewable second owner while leaving the deliberate
# native/render owner implementations outside this product-facade guard.
RESPONSIBILITY_LOC_LIMITS = {
    "core": 2400,
    "facade": 5000,
    "compiler": 2400,
    "reactor": 1200,
}


def responsibility(crate_name):
    if crate_name.endswith("-core"):
        return "core"
    if crate_name.endswith("-reactor") or crate_name == "flamewm-reactor":
        return "reactor"
    if crate_name == "flamewm-render-compiler":
        return "compiler"
    return "facade"


def source_loc(paths):
    # Production responsibility size: #[cfg(test)] items are not counted.
    return sum(
        1
        for path in paths
        for line in _j11_strip_cfgs(path.read_text(encoding="utf-8")).splitlines()
        if line.strip() and not line.lstrip().startswith("//")
    )


def relative_path(root, path):
    return path.relative_to(root).as_posix()


def product_manifests(root):
    manifests = {}
    for manifest in sorted((root / "crates").glob("*/Cargo.toml")):
        with manifest.open("rb") as stream:
            data = tomllib.load(stream)
        name = data.get("package", {}).get("name")
        if name in PRODUCT_CRATES and name not in OWNER_CRATES:
            manifests[name] = manifest
    return manifests


def dependency_names(manifest):
    with manifest.open("rb") as stream:
        data = tomllib.load(stream)
    names = set()
    def add_sections(table):
        for section in ("dependencies", "dev-dependencies", "build-dependencies"):
            for name, value in table.get(section, {}).items():
                # A dependency may be renamed locally with package = "...".
                names.add(value.get("package", name) if isinstance(value, dict) else name)

    add_sections(data)
    for target in data.get("target", {}).values():
        add_sections(target)
    return names


def violations(root):
    found = set()
    for manifest in product_manifests(root).values():
        crate_name = manifest.parent.name
        path = relative_path(root, manifest)
        dependencies = dependency_names(manifest)
        if dependencies & FORBIDDEN_DEPENDENCIES:
            found.add((path, "renderer-dependency"))
        if "x11rb" in dependencies:
            found.add((path, "x11rb-dependency"))

        source_paths = sorted((manifest.parent / "src").rglob("*.rs"))
        build_script = manifest.parent / "build.rs"
        if build_script.is_file():
            source_paths.append(build_script)
        for source in source_paths:
            text = source.read_text(encoding="utf-8")
            source_path = relative_path(root, source)
            if FORBIDDEN_X11_SOURCE.search(text):
                found.add((source_path, "x11rb-symbol"))
            if FORBIDDEN_RENDERER_SOURCE.search(_j6_strip_comments(text)):
                found.add((source_path, "renderer-symbol"))
            if RAW_FFI_SOURCE.search(text):
                found.add((source_path, "raw-ffi-boundary"))
            for unsafe_match in UNSAFE_FUNCTION.finditer(text):
                # Keep the contract local to the function; a Safety section
                # elsewhere in a large module must not satisfy this tripwire.
                prefix = text[max(0, unsafe_match.start() - 600) : unsafe_match.start()]
                if "# Safety" not in prefix:
                    found.add((source_path, "unsafe-fn-safety-contract"))

        role = responsibility(crate_name)
        limit = RESPONSIBILITY_LOC_LIMITS[role]
        loc = source_loc(source_paths)
        if loc > limit:
            found.add((path, f"{role}-source-loc>{limit}"))

    # Suppressions are checked across owners too: native declarations may use
    # naming/dead-code allowances, but never blanket warning suppression.
    for source in sorted((root / "crates").rglob("*.rs")):
        text = source.read_text(encoding="utf-8")
        if BROAD_SUPPRESSION.search(text):
            found.add((relative_path(root, source), "broad-warning-suppression"))
    found |= j7_violations(root)
    return found


# --- J7 icon/alpha guards (J7-G1..J7-G8), capability-tagged for the exception ratchet ---
J7_KNOWN_KEYED_PPMS = {
    "task-start.ppm", "tray-volume.ppm", "tray-volume-muted.ppm", "tray-wifi.ppm",
    "tray-play.ppm", "tray-pause.ppm", "popup-volume.ppm", "popup-muted.ppm",
    "media-play.ppm", "media-pause.ppm", "media-prev.ppm", "media-next.ppm",
    "title-close.ppm", "title-maximize.ppm", "title-minimize.ppm", "title-restore.ppm",
}
J7_OWNER_PREFIXES = (
    "crates/flamewm-render-x11/",
    "crates/flamewm-image-core/",
)
J7_FFI_DECLARATION = "crates/flamewm-render-x11/src/xlib.rs"
J7_FALLBACK_OWNER = "crates/flamewm-integrations-linux/src/icons.rs"
J7_TREATMENT_OWNER = "crates/flamewm-skin/src/icons.rs"
J7_PAINT_OWNER = "crates/flamewm-render-core/src/paint.rs"
J7_SURFACE_OWNER = "crates/flamewm-render-x11/src/native/target.rs"
J7_MAGENTA_KEY = re.compile(r"ff00ff|color[-_ ]?key|chromakey|chroma[-_ ]?key", re.IGNORECASE)


def _j7_is_test(path):
    return "/tests/" in path.as_posix() or path.name.endswith("_test.rs")


def _j7_production_sources(root):
    for path in sorted((root / "crates").rglob("*.rs")):
        if not _j7_is_test(path):
            yield path


def _j7_declared(text):
    body = re.sub(r"#\[cfg\(test\)\].*?\nmod tests \{.*?\n\}\n", "", text, flags=re.S)
    return body


def j7_violations(root):
    found = set()

    def rel(path):
        return relative_path(root, path)

    def owned(path, prefixes=J7_OWNER_PREFIXES):
        return rel(path).startswith(prefixes)

    # J7-G1: only the render/native owner may blend over black or keep the
    # documented opaque fallback.
    for path in _j7_production_sources(root):
        text = _j7_declared(path.read_text(encoding="utf-8"))
        if re.search(r"\bblend_over_black\b", text) and not owned(path):
            found.add((rel(path), "j7-g1-no-blend-over-black"))
        if re.search(r"fallback-black|fallback_black|FallbackBlack", text) and not owned(path):
            found.add((rel(path), "j7-g1-no-blend-over-black"))
    # J7-G2: only the Xlib FFI declaration owner may declare
    # XCreateSimpleWindow; canonical Flame surface state lives in target.rs.
    for path in _j7_production_sources(root):
        if rel(path) == J7_FFI_DECLARATION:
            continue
        if "XCreateSimpleWindow" in _j7_declared(path.read_text(encoding="utf-8")):
            found.add((rel(path), "j7-g2-no-xcreate-simple-window"))
    # J7-G3: no active icon consumer references known keyed PPM assets.
    # Compiled shell templates are owned by the G01 template guard; the guard
    # tables themselves are not icon consumers. Test-only files are excluded.
    for path in _j7_production_sources(root):
        if path.suffix != ".rs":
            continue;
        text = _j7_declared(path.read_text(encoding="utf-8"))
        for match in re.findall(r"[A-Za-z0-9_./-]+\.ppm", text):
            if match.rsplit("/", 1)[-1] in J7_KNOWN_KEYED_PPMS:
                found.add((rel(path), "j7-g3-no-keyed-ppm-asset"))
                break
    # J7-G4: generic IconResolver fallback must not be .ppm (checked at owner).
    owner = root / J7_FALLBACK_OWNER
    if owner.is_file():
        for line in owner.read_text(encoding="utf-8").splitlines():
            if line.strip().startswith("const GENERIC_FALLBACK") and ".ppm" in line:
                found.add((J7_FALLBACK_OWNER, "j7-g4-no-ppm-fallback"))
    # J7-G5: no production magenta-key/FF00FF color-key path.
    for path in _j7_production_sources(root):
        if owned(path):
            continue
        text = _j7_declared(path.read_text(encoding="utf-8")).lower()
        if J7_MAGENTA_KEY.search(text) and "no magenta" not in text and "no color-key" not in text:
            found.add((rel(path), "j7-g5-no-magenta-colorkey"))
    # J7-G6: SymbolicForeground must not be inferred from filename.
    for path in _j7_production_sources(root):
        text = _j7_declared(path.read_text(encoding="utf-8"))
        if "SymbolicForeground" in text and re.search(
            r"(?i)(filename|extension|ends_with|\.svg|\.ppm|\.png).{0,80}SymbolicForeground"
            r"|SymbolicForeground.{0,80}(filename|extension|\.svg|\.ppm|\.png)",
            text,
        ):
            found.add((rel(path), "j7-g6-no-filename-treatment"))
    # J7-G7: real app icon path defaults to Original (checked at skin owner).
    skin = root / J7_TREATMENT_OWNER
    if skin.is_file():
        text = skin.read_text(encoding="utf-8")
        if "IconTreatment::Original" not in text or not re.search(
            r"Start\s*\|.*Browser|Browser.*Start", text
        ):
            found.add((J7_TREATMENT_OWNER, "j7-g7-original-app-icons"))
    # J7-G8: shell symbolic icons consume semantic foreground, not Breeze RGB.
    paint = root / J7_PAINT_OWNER
    if paint.is_file() and "SymbolicForeground" in paint.read_text(encoding="utf-8"):
        if "resolve_color(style.color)" not in paint.read_text(encoding="utf-8"):
            found.add((J7_PAINT_OWNER, "j7-g8-semantic-foreground"))
    surface = root / J7_SURFACE_OWNER
    if surface.is_file() and "resolve_color(style.color)" not in paint.read_text(encoding="utf-8"):
        found.add((J7_SURFACE_OWNER, "j7-g8-semantic-foreground"))
    found |= j6_violations(root)
    found |= j12_violations(root)
    found |= j11_violations(root)
    print_j6_warnings(root)
    return found


# --- J6 symbolic/migration guards (J6-G1..J6-G7), exception-ratchet tagged ---
# J6-G1 is hard-fail (already true). J6-G2..J6-G7 describe post-migration
# state; they currently fail pre-migration, so they are staged as WARN-only
# (printed to stderr) and never added to the violation set until migration.
J6_SKIN_OWNER = "crates/flamewm-skin/src/icons.rs"
J6_CHROME_FILE = "crates/flamewm-wm-x11/src/chrome.rs"
J6_ICONS_FILE = "crates/flamewm-integrations-linux/src/icons.rs"
J6_MODEL_FILE = "crates/flamewm-render-core/src/model.rs"
J6_SHELL_MAIN = "crates/flamewm-shell/src/main.rs"
J6_PENDING: list[str] = []
# TODO(J12-PENDING): promote J6-G5/G6 to violation entries once migrations land.


def _j6_strip_comments(text: str) -> str:
    return _strip_rust_comments(text)


def print_j6_warnings(root):
    del root
    for warning in J6_PENDING:
        print(warning, file=sys.stderr)


def j6_violations(root):
    found = set()
    skin = root / J6_SKIN_OWNER
    if skin.is_file():
        text = skin.read_text(encoding="utf-8")
        if re.search(
            r"Self::Start[^\n]*SymbolicForeground|Start[^\n]{0,80}=>\s*IconTreatment::SymbolicForeground",
            text,
        ):
            found.add((J6_SKIN_OWNER, "j6-g1-start-full-color"))
        if "Self::Start" in text and "IconTreatment::Original" not in text:
            found.add((J6_SKIN_OWNER, "j6-g1-start-full-color"))
    chrome = root / J6_CHROME_FILE
    if chrome.is_file():
        text = _j6_strip_comments(chrome.read_text(encoding="utf-8"))
        if "struct X11ChromePainter" in text:
            found.add((J6_CHROME_FILE, "j6-g2-no-local-chrome-painter"))
        if re.search(r"fn draw_title_run", text) and "poly_fill_rectangle" in text:
            found.add((J6_CHROME_FILE, "j6-g3-no-block-title"))
        if re.search(r"fn draw_control_glyph", text) and re.search(
            r"poly_line|poly_rectangle|poly_arc|poly_fill_arc", text
        ):
            found.add((J6_CHROME_FILE, "j6-g4-no-procedural-glyphs"))
    icons = root / J6_ICONS_FILE
    if icons.is_file():
        text = _j6_strip_comments(icons.read_text(encoding="utf-8"))
        pack = text.find("packaged_candidates(name)")
        theme = text.find("theme_candidates(name")
        if pack != -1 and theme != -1 and pack < theme:
            J6_PENDING.append(f"WARN TODO(J12-PENDING) j6-g5-system-before-packaged: {J6_ICONS_FILE}")
    model = root / J6_MODEL_FILE
    if model.is_file() and re.search(
        r"z_index_overrides|set_z_index|effective_z_index",
        _j6_strip_comments(model.read_text(encoding="utf-8")),
    ):
        J6_PENDING.append(f"WARN TODO(J12-PENDING) j6-g6-no-new-zindex-store: {J6_MODEL_FILE}")
    shell = root / J6_SHELL_MAIN
    if shell.is_file() and re.search(
        r"root_pointer\(\s*0(\.0)?\s*,\s*0(\.0)?\s*\)",
        _j6_strip_comments(shell.read_text(encoding="utf-8")),
    ):
        found.add((J6_SHELL_MAIN, "j6-g7-no-root-pointer-zero"))
    return found


# --- J11 convergence guards (J11-G01..G10), exception-ratchet tagged ---
J11_MOVE_RESIZE_OWNER = "crates/flamewm-render-x11/src/surface_controller.rs"
J11_XLIB_DECL = "crates/flamewm-render-x11/src/xlib.rs"
J11_ENV_OK = re.compile(
    r"^(FLAMEWM_[A-Z0-9_]+|XDG_[A-Z0-9_]+|XCURSOR_[A-Z0-9_]+|DESKTOP_SESSION"
    r"|HOME|PATH|TERMINAL|CARGO_MANIFEST_DIR|OUT_DIR)$"
)


def _j11_fn_body(code, fn_name):
    match = re.search(r"fn\s+" + re.escape(fn_name) + r"\s*\(", code)
    if not match:
        return ""
    rest = code[match.end():]
    nxt = re.search(r"\n(?:pub\s+)?fn\s+", rest)
    return rest[:nxt.start()] if nxt else rest


def _j11_strip_cfgs(text):
    """Drop `#[cfg(test)]` items (test fns and test modules) from code context."""
    lines = text.splitlines(keepends=True)
    out = []
    i = 0
    while i < len(lines):
        if lines[i].strip().startswith("#[cfg(test)]"):
            i += 1
            while i < len(lines) and not lines[i].strip():
                out.append(lines[i])
                i += 1
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


def j11_violations(root):
    found = set()
    code = _j6_strip_comments

    def live(path):
        return code((root / path).read_text(encoding="utf-8")) if (root / path).is_file() else ""

    def prod(path):
        return code(path.read_text(encoding="utf-8"))

    # J11-G01: move_resize owns commit_geometry + native move; no other
    # production file issues XMoveResizeWindow.
    mr = live(J11_MOVE_RESIZE_OWNER)
    mr_body = _j11_fn_body(mr, "move_resize")
    if "commit_geometry" not in mr_body:
        found.add((J11_MOVE_RESIZE_OWNER, "j11-g01-move-resize-commits-geometry"))
    if "XMoveResizeWindow" not in mr_body:
        found.add((J11_MOVE_RESIZE_OWNER, "j11-g01-move-resize-commits-geometry"))
    for path in _j7_production_sources(root):
        if path.suffix != ".rs" or relative_path(root, path) in (J11_MOVE_RESIZE_OWNER, J11_XLIB_DECL):
            continue
        if "XMoveResizeWindow" in code(path.read_text(encoding="utf-8")):
            found.add((relative_path(root, path), "j11-g01-move-resize-commits-geometry"))
    # J11-G02: no split start_submenu runtime surface.
    for target in ("crates/flamewm-shell/src/runtime.rs", "crates/flamewm-shell/src/main.rs"):
        if re.search(r"(?<![A-Za-z0-9_])start_submenu(?![A-Za-z0-9_])", live(target)):
            found.add((target, "j11-g02-no-submenu-surface"))
    # J11-G03: no procedural solid-red fallback in production code.
    for path in _j7_production_sources(root):
        if path.suffix != ".rs":
            continue
        text = _j11_strip_cfgs(code(path.read_text(encoding="utf-8")))
        if re.search(r"255\s*,\s*0\s*,\s*0\s*,\s*255", text):
            found.add((relative_path(root, path), "j11-g03-no-solid-red-fallback"))
    # J11-G04: desktop cold miss keeps the synchronous fallback arm.
    desk = live("crates/flamewm-desktop/src/projection.rs")
    if "flame_fallback" not in _j11_fn_body(desk, "project_row"):
        found.add(("crates/flamewm-desktop/src/projection.rs", "j11-g04-desktop-cold-fallback"))
    # J11-G05: no sync IconResolver in WM draw/manage (generic
    # IconService<IconResolver> import is async-service use, not sync).
    wm = live("crates/flamewm-wm-x11/src/wm.rs")
    if (
        "prepare_application" in wm
        or "prepare_semantic" in wm
        or "prepare_path(" in wm
        or "resolve_app_icon_via" in wm
    ):
        found.add(("crates/flamewm-wm-x11/src/wm.rs", "j11-g05-no-sync-icon-resolver"))
    # J11-G06: frame chrome paints through the frame-engine owner
    # (paint_chrome / chrome_runtime cached scene_for+paint_cached bridge).
    # The retired Wm::draw_frame painter was deleted in the J16 cutover.
    draw = _j11_fn_body(wm, "draw_frame")
    has_draw = "draw_frame" in wm
    has_chrome = "fn paint_chrome" in wm
    if not has_draw and not has_chrome:
        found.add(("crates/flamewm-wm-x11/src/wm.rs", "j11-g06-renderer-frame-path"))
    elif has_draw:
        if "self.decorations" not in draw:
            found.add(("crates/flamewm-wm-x11/src/wm.rs", "j11-g06-renderer-frame-path"))
        elif re.search(r"paint::paint_frame|image_text8|XDrawString|XRenderComposite", draw):
            found.add(("crates/flamewm-wm-x11/src/wm.rs", "j11-g06-renderer-frame-path"))
    if has_chrome:
        chrome_body = _j11_fn_body(wm, "paint_chrome")
        chrome_runtime = live("crates/flamewm-wm-x11/src/wm/chrome_runtime.rs")
        if re.search(r"paint::paint_frame|image_text8|XDrawString|XRenderComposite", chrome_body):
            found.add(("crates/flamewm-wm-x11/src/wm.rs", "j11-g06-renderer-frame-path"))
        elif not (
            "frame_chrome::plan_scene" in chrome_body
            or "frame_chrome::render" in chrome_body
            or (
                "runtime.scene_for" in chrome_body
                and "runtime.paint_cached" in chrome_body
                and "pub fn scene_for" in chrome_runtime
                and "pub fn paint_cached" in chrome_runtime
            )
        ):
            found.add(("crates/flamewm-wm-x11/src/wm.rs", "j11-g06-renderer-frame-path"))
    # J11-G07: retention cleanup defaults opt-out-on (checked in tools).
    ret = root / "tools/performance_retention.py"
    if ret.is_file() and 'os.environ.get("FLAMEWM_PERFORMANCE_CLEANUP", "1")' not in ret.read_text(encoding="utf-8"):
        found.add(("tools/performance_retention.py", "j11-g07-cleanup-default-on"))
    # J11-G08: no unprefixed env literals.
    for path in _j7_production_sources(root):
        if path.suffix != ".rs":
            continue
        text = code(path.read_text(encoding="utf-8"))
        for lit in re.findall(r'env::(?:var|var_os)\(\s*"([^"]+)"', text):
            if not J11_ENV_OK.match(lit):
                found.add((relative_path(root, path), "j11-g08-prefixed-env"))
                break
        for lit in re.findall(r'(?:option_env!|env!)\(\s*"([^"]+)"', text):
            if not J11_ENV_OK.match(lit):
                found.add((relative_path(root, path), "j11-g08-prefixed-env"))
                break
    # J11-G09: once-per-generation profile truncate, single owner.
    rep_path = "crates/flamewm-profiler/src/report.rs"
    rep = live(rep_path)
    trunc = _j11_fn_body(rep, "truncate_profile_log_once")
    if "truncate_profile_log_once" not in rep or ".truncate(true)" not in trunc:
        found.add((rep_path, "j11-g09-profile-truncate-once"))
    elif rep.count(".truncate(true)") != trunc.count(".truncate(true)"):
        found.add((rep_path, "j11-g09-profile-truncate-once"))
    if "truncate_profile_log_once(name)" not in rep:
        found.add((rep_path, "j11-g09-profile-truncate-once"))
    # J11-G10: gauges are live, never no-op.
    mem_path = "crates/flamewm-profiler/src/memory.rs"
    mem = live(mem_path)
    if ".store(" not in _j11_fn_body(mem, "set"):
        found.add((mem_path, "j11-g10-live-gauges"))
    for path in _j7_production_sources(root):
        if path.suffix != ".rs" or path.name == "memory.rs":
            continue
        text = code(path.read_text(encoding="utf-8"))
        if "MemoryGauge::new" in text and ".set(" not in text:
            found.add((relative_path(root, path), "j11-g10-live-gauges"))
    return found


# --- J12 focused guards (J12-G01..J12-G14), exception-ratchet tagged ---
J12_DESKTOP_PROJ = "crates/flamewm-desktop/src/projection.rs"
J12_START_MENU = "crates/flamewm-shell-core/src/start_menu.rs"
J12_POPUP = "crates/flamewm-ui-x11/src/popup.rs"
J12_SURFACE_RT = "crates/flamewm-ui-x11/src/surface_runtime.rs"
J12_SVG = "crates/flamewm-image-core/src/svg.rs"
J12_SESSION = "crates/flamewm-session-core/src/lib.rs"
J12_PAINT_OWNER = "crates/flamewm-render-core/src/paint.rs"


def j12_violations(root):
    found = set()
    code = _j6_strip_comments

    def live(path):
        return code((root / path).read_text(encoding="utf-8")) if (root / path).is_file() else ""

    # J12-G01: invalid clamp(12.0,track) in live code.
    for path in _j7_production_sources(root):
        if path.suffix != ".rs":
            continue
        if re.search(r"\bclamp\s*\(\s*12\.0", code(path.read_text(encoding="utf-8"))):
            found.add((relative_path(root, path), "j12-g01-no-invalid-clamp"))
    # J12-G02: replace_image must not mutate document.assets.
    for path in _j7_production_sources(root):
        if path.suffix != ".rs":
            continue
        body = code(path.read_text(encoding="utf-8"))
        if re.search(r"fn replace_image", body) and re.search(
            r"\.assets\s*(\[[^\]]*\]\s*=|\.push\s*\(|\.insert\s*\(|\.clear\s*\()", body
        ):
            found.add((relative_path(root, path), "j12-g02-no-assets-mutation"))
    # J12-G03: no display:none on transient/popup/menu templates (ui context).
    for tpl in sorted((root / "ui").rglob("*")):
        if not tpl.is_file() or tpl.suffix.lower() not in {".html", ".css"}:
            continue
        text = re.sub(r"/\*.*?\*/", "", tpl.read_text(encoding="utf-8"), flags=re.S)
        text = re.sub(r"<!--.*?-->", "", text, flags=re.S)
        if re.search(r"display\s*:\s*none", text, re.I) and re.search(
            r"transient|popup-menu|context-menu|tooltip|dropdown", text, re.I
        ):
            found.add((relative_path(root, tpl), "j12-g03-no-display-none-transient"))
    # J12-G04: desktop root on SurfaceBackground.
    if "UiLayer::SurfaceBackground" not in live(J12_DESKTOP_PROJ):
        found.add((J12_DESKTOP_PROJ, "j12-g04-surface-background-root"))
    # J12-G05: semantic SVG themed render.
    if "render_with_color_scheme" not in live(J12_SVG):
        found.add((J12_SVG, "j12-g05-semantic-svg-render"))
    if "SymbolicForeground" not in live(J12_PAINT_OWNER):
        found.add((J12_PAINT_OWNER, "j12-g05-semantic-svg-render"))
    # J12-G06: hidden create must not force redraw (body-bound).
    match = re.search(r"fn create_surface\(", live(J12_SURFACE_RT))
    if match:
        rest = live(J12_SURFACE_RT)[match.end():]
        nxt = re.search(r"pub fn ", rest)
        body = rest[:nxt.start()] if nxt else rest[:800]
        if re.search(r"redraw|mark_full|Damage::Full", body):
            found.add((J12_SURFACE_RT, "j12-g06-hidden-create-no-redraw"))
    # J12-G07: popup raise policy hide-previous + show-next.
    popup = live(J12_POPUP)
    if "runtime.hide" not in popup or "runtime.show" not in popup:
        found.add((J12_POPUP, "j12-g07-popup-raise-policy"))
    # J12-G08: no sync IconResolver in category hot path.
    if "IconResolver" in live(J12_START_MENU):
        found.add((J12_START_MENU, "j12-g08-no-sync-icon-resolver"))
    # J12-G09: Unknown stays unbucketed.
    start = live(J12_START_MENU)
    if "_ => None" not in start:
        found.add((J12_START_MENU, "j12-g09-unknown-not-visible"))
    # J12-G10: no image_text8/fixed-font normal decoration path.
    for deco in [
        "crates/flamewm-wm-x11/src/decoration/paint.rs",
        "crates/flamewm-wm-x11/src/decoration/manager.rs",
        "crates/flamewm-wm-x11/src/wm.rs",
        "crates/flamewm-wm-x11/src/chrome.rs",
    ]:
        text = live(deco)
        if text and re.search(
            r"image_text8|XDrawString|fixed_font|fixed-font|9x15|TITLE_CHAR_ADVANCE", text
        ):
            found.add((deco, "j12-g10-no-fixed-font-decoration"))
    # J12-G11: exactly one DecorationManager owner (J16 cutover: deleted;
    # successor is the frame-engine FrameResources registry).
    count = sum(
        len(re.findall(r"struct DecorationManager", code(p.read_text(encoding="utf-8"))))
        for p in _j7_production_sources(root)
        if p.suffix == ".rs"
    )
    frame_count = sum(
        len(re.findall(r"struct FrameResources", code(p.read_text(encoding="utf-8"))))
        for p in _j7_production_sources(root)
        if p.suffix == ".rs"
    )
    if not (count == 1 or (count == 0 and frame_count == 1)):
        found.add(("crates/flamewm-wm-x11/src/decoration/manager.rs", "j12-g11-single-decoration-manager"))
    # J12-G12: wallpaper present, CPU out of desktop owner.
    if "wallpaper" not in live(J12_DESKTOP_PROJ).lower():
        found.add((J12_DESKTOP_PROJ, "j12-g12-wallpaper-not-cpu"))
    desk = root / "crates/flamewm-desktop/src"
    if desk.exists():
        for path in sorted(desk.rglob("*.rs")):
            if _j7_is_test(path):
                continue
            if re.search(r"(?i)\bcpu\b", code(path.read_text(encoding="utf-8"))):
                found.add((relative_path(root, path), "j12-g12-wallpaper-not-cpu"))
                break
    # J12-G13: .performance stays ignored.
    gitignore = root / ".gitignore"
    if not gitignore.is_file() or ".performance" not in gitignore.read_text(encoding="utf-8"):
        found.add((".gitignore", "j12-g13-performance-ignored"))
    # J12-G14: Breeze cursor authority intact. Session-core owns the bundled
    # theme + XCURSOR env; frame engine owns cursor-region binding via
    # cursor_for_region (frame/resources.rs). J16 cutover deleted
    # decoration/paint.rs, so the frame-engine binding is the successor anchor.
    session = live(J12_SESSION)
    if "FlameWM-Breeze-Dark" not in session or "XCURSOR" not in session:
        found.add((J12_SESSION, "j12-g14-breeze-cursor-intact"))
    frame_res = live("crates/flamewm-wm-x11/src/frame/resources.rs")
    paint_decor_path = root / "crates/flamewm-wm-x11/src/decoration/paint.rs"
    paint_decor = live("crates/flamewm-wm-x11/src/decoration/paint.rs")
    if "cursor_for_region" not in frame_res:
        found.add((
            "crates/flamewm-wm-x11/src/frame/resources.rs",
            "j12-g14-breeze-cursor-intact",
        ))
    elif paint_decor_path.is_file():
        if "Breeze-Dark" not in paint_decor:
            found.add((
                "crates/flamewm-wm-x11/src/decoration/paint.rs",
                "j12-g14-breeze-cursor-intact",
            ))
    found |= j12h_violations(root)
    found |= j10_violations(root)
    found |= j18_violations(root)
    return found


# --- J10 anti-regression guards (J10-G01..G09), exception-ratchet tagged ---
# Hard violations where the converged tree holds; WARN-only (J6_PENDING)
# where the pre-J07 tree has not migrated (explicit reason, never a fail).
J10_PROF_CALL = re.compile(
    r"(CounterPoint::new|ProfilePoint::new|flamewm_profiler::start|CounterSlot::new)\s*\(\s*(?:&)?format!\s*\("
)


def j10_violations(root):
    found = set()
    code = _j6_strip_comments

    def live(path):
        return code((root / path).read_text(encoding="utf-8")) if (root / path).is_file() else ""

    def prod_sources(base):
        for path in sorted((root / base).rglob("*.rs")):
            if not _j7_is_test(path):
                yield path

    # J10-G01: Xephyr private D-Bus default present.
    xep = root / "scripts/xephyr"
    xept = xep.read_text(encoding="utf-8") if xep.is_file() else ""
    if "FLAMEWM_XEPHYR_PRIVATE_DBUS:-1" not in xept or "dbus-run-session" not in xept:
        found.add(("scripts/xephyr", "j10-g01-private-dbus-default"))
    # J10-G02: shared host-session fallback forbidden in perf gate.
    for perf in ("tools/profile_summary.py", "tools/performance_bundle.py", "tools/performance_retention.py"):
        text = (root / perf).read_text(encoding="utf-8") if (root / perf).is_file() else ""
        if re.search(r"DBUS_SESSION_BUS_ADDRESS|host\.session|host_session", text):
            found.add((perf, "j10-g02-no-host-session-in-perf"))
    # J10-G03 (WARN-only): parent ShellSurfaces still owns helper surfaces pre-J07.
    rt = live("crates/flamewm-shell/src/runtime.rs")
    if "struct ShellSurfaces" in rt and all(k in rt for k in ("self.audio", "self.network", "self.calendar")):
        J6_PENDING.append("WARN TODO(J07-PENDING) j10-g03-parent-native-surfaces: crates/flamewm-shell/src/runtime.rs")
    # J10-G04: helper cannot own NetworkManager/Pulse provider directly.
    for path in prod_sources("crates/flamewm-shell/src/quick_controls"):
        if path.suffix != ".rs":
            continue
        text = code(path.read_text(encoding="utf-8"))
        if re.search(r"NetworkManagerProvider|PulseProvider|PulseAudioProvider|pulse::|network_manager::", text):
            found.add((relative_path(root, path), "j10-g04-helper-no-provider"))
    # J10-G05 (WARN-only): fitted_start_placement migration not in runtime yet.
    if "fitted_start_placement" not in rt:
        J6_PENDING.append("WARN TODO(J07-PENDING) j10-g05-fitted-start-placement: crates/flamewm-shell/src/runtime.rs")
    # J10-G06: no start_submenu native surface.
    for target in ("crates/flamewm-shell/src/runtime.rs", "crates/flamewm-shell/src/main.rs",
                   "crates/flamewm-shell/build.rs"):
        text = live(target) if target.endswith(".rs") else ((root / target).read_text(encoding="utf-8") if (root / target).is_file() else "")
        if re.search(r"flamewm-start-submenu|start_submenu.*Surface|Surface.*start_submenu|create_surface\([^)]*submenu", text):
            found.add((target, "j10-g06-no-submenu-surface"))
    # J10-G07: no blocking Child::wait (allow wait_timeout/try_wait/kill+wait).
    for path in prod_sources("crates/flamewm-shell/src"):
        if path.suffix != ".rs":
            continue
        text = code(path.read_text(encoding="utf-8"))
        if re.search(r"\.wait_timeout\s*\(", text):
            continue
        for m in re.finditer(r"\.wait\s*\(\s*\)", text):
            ctx = text[max(0, m.start() - 600):m.end() + 200]
            if "try_wait" in ctx or "wait_timeout" in ctx or "kill()" in text:
                continue
            found.add((relative_path(root, path), "j10-g07-no-blocking-wait"))
            break
    # J10-G08: no shell interpolation in quick-control protocol.
    # String literals and #[cfg(test)] modules are stripped: protocol
    # tests embed shell metacharacters as rejected-input fixtures.
    def unquoted(text):
        text = re.sub(r"#\[cfg\(test\)\].*?\nmod tests \{.*?\n\}\n", "", text, flags=re.S)
        return re.sub(r'"(?:[^"\\]|\\.)*"', '""', text)

    for path in prod_sources("crates/flamewm-shell/src/quick_controls"):
        if path.suffix != ".rs":
            continue
        text = unquoted(code(path.read_text(encoding="utf-8")))
        if re.search(r'sh\s+-c|Command::new\(\s*"sh"|/bin/sh|\beval\s*\(|\bsystem\s*\(|`[^`]*`|\$\(', text):
            found.add((relative_path(root, path), "j10-g08-no-shell-interpolation"))
    proto = live("crates/flamewm-shell/src/quick_controls/protocol.rs")
    if proto and "split_ascii_whitespace" not in proto and "split_whitespace" not in proto:
        found.add(("crates/flamewm-shell/src/quick_controls/protocol.rs", "j10-g08-no-shell-interpolation"))
    # J10-G09: no dynamic profiler labels on hot paths.
    for path in sorted((root / "crates").rglob("*.rs")):
        if _j7_is_test(path) or path.suffix != ".rs":
            continue
        if J10_PROF_CALL.search(code(path.read_text(encoding="utf-8"))):
            found.add((relative_path(root, path), "j10-g09-static-profiler-labels"))
    return found


# --- J18 frame-engine guards (J18-G01..G05), exception-ratchet tagged ---
# Semantic owner checks for the Flame X11 frame engine under flamewm-wm-x11.
J18_FRAME_OWNER = "crates/flamewm-wm-x11"
J18_GEOMETRY_OWNER_FILES = (
    "crates/flamewm-wm-x11/src/client.rs",
    "crates/flamewm-wm-x11/src/frame/controller.rs",
    "crates/flamewm-wm-x11/src/frame/geometry.rs",
    "crates/flamewm-wm-x11/src/frame/model.rs",
    "crates/flamewm-wm-x11/src/wm.rs",
)
J18_HIT_OWNER_FILES = (
    "crates/flamewm-wm-x11/src/frame/input.rs",
    "crates/flamewm-wm-x11/src/frame/controller.rs",
    "crates/flamewm-wm-x11/src/decoration/interaction.rs",
    "crates/flamewm-wm-x11/src/decoration/manager.rs",
)
J18_TITLE_OWNER_FILES = (
    "crates/flamewm-wm-x11/src/frame/chrome.rs",
    "crates/flamewm-render-x11/src/external_decoration.rs",
    "crates/flamewm-wm-x11/src/decoration/manager.rs",
    "crates/flamewm-wm-x11/src/decoration/paint.rs",
)


def j18_violations(root):
    found = set()
    code = _j6_strip_comments

    def live(path):
        return code((root / path).read_text(encoding="utf-8")) if (root / path).is_file() else ""

    # J18-G01: window-geometry authority lives only in the frame engine.
    # Direct outer-rect writes inside the live wm-x11 crate but outside the
    # geometry owner are rejected (legacy-mirror bypass). Pure policy crates
    # (e.g. window-core snap math over their own state) are out of scope.
    wm11 = root / "crates/flamewm-wm-x11/src"
    if wm11.is_dir():
        for path in sorted(wm11.rglob("*.rs")):
            if _j7_is_test(path):
                continue
            rel = relative_path(root, path)
            if rel in J18_GEOMETRY_OWNER_FILES:
                continue
            text = code(path.read_text(encoding="utf-8"))
            if re.search(r"\.outer\s*=", text) or re.search(r"\.restore\s*=", text):
                found.add((rel, "j18-g01-frame-geometry-owner"))
    # J18-G02: exactly one live window-geometry/decoration owner pair.
    # A second PlacementState or GeometryPlan struct is a second live owner.
    geo_count = sum(
        len(re.findall(r"struct PlacementState", code(p.read_text(encoding="utf-8"))))
        for p in _j7_production_sources(root)
        if p.suffix == ".rs"
    )
    plan_count = sum(
        len(re.findall(r"struct GeometryPlan", code(p.read_text(encoding="utf-8"))))
        for p in _j7_production_sources(root)
        if p.suffix == ".rs"
    )
    if geo_count != 1:
        found.add(("crates/flamewm-wm-x11/src/frame/model.rs", "j18-g02-single-geometry-owner"))
    if plan_count != 1:
        found.add(("crates/flamewm-wm-x11/src/frame/geometry.rs", "j18-g02-single-geometry-owner"))
    # J18-G03: single pointer-hit owner. A second hit resolver
    # (resolve_pointer_intent / target_for_xid / pointer_intent_at definition)
    # outside the hit owner is rejected.
    hit_defs = []
    for path in _j7_production_sources(root):
        if path.suffix != ".rs":
            continue
        rel = relative_path(root, path)
        if rel in J18_HIT_OWNER_FILES:
            continue
        text = code(path.read_text(encoding="utf-8"))
        if re.search(r"fn\s+(resolve_pointer_intent|target_for_xid|pointer_intent_at)\s*\(", text):
            hit_defs.append(rel)
    for rel in hit_defs:
        found.add((rel, "j18-g03-single-pointer-hit-owner"))
    # J18-G04: no production old decoration paint call. Direct
    # paint::paint_frame invocations outside the title owner are rejected
    # (the manager facade is the live caller; tests construct outcomes).
    for path in _j7_production_sources(root):
        if path.suffix != ".rs":
            continue
        rel = relative_path(root, path)
        if rel in J18_TITLE_OWNER_FILES:
            continue
        text = code(path.read_text(encoding="utf-8"))
        if re.search(r"paint::paint_frame\s*\(", text):
            found.add((rel, "j18-g04-no-old-decoration-paint"))
    # J18-G05: no new raw title renderer in wm-x11. Direct renderer
    # draw_title calls outside the title owner are rejected.
    for path in _j7_production_sources(root):
        if path.suffix != ".rs":
            continue
        rel = relative_path(root, path)
        if rel in J18_TITLE_OWNER_FILES:
            continue
        text = code(path.read_text(encoding="utf-8"))
        if re.search(r"\.draw_title\s*\(", text):
            found.add((rel, "j18-g05-no-raw-title-renderer"))
    return found


# --- J12 handoff guards (J12H-G01..G15), exception-ratchet tagged ---
def j12h_violations(root):
    found = set()
    code = _j6_strip_comments

    def live(path):
        return code((root / path).read_text(encoding="utf-8")) if (root / path).is_file() else ""

    def desktop_css():
        for name in ("ui/desktop/desktop.css", "ui/shell/flamewm.css"):
            path = root / name
            if path.is_file():
                return path.read_text(encoding="utf-8")
        return ""

    paint_core = live("crates/flamewm-render-core/src/paint.rs")
    start_menu = live("crates/flamewm-shell-core/src/start_menu.rs")
    proj = live("crates/flamewm-shell/src/projection.rs")
    app = live("crates/flamewm-shell/src/app.rs")
    runtime = live("crates/flamewm-shell/src/runtime.rs")
    popup_controller = live("crates/flamewm-shell/src/popup_controller.rs")
    status_surface = live("crates/flamewm-shell/src/runtime/status_surface.rs")
    main = live("crates/flamewm-shell/src/main.rs")
    audio = live("crates/flamewm-shell/src/taskbar/status/audio.rs")
    network = live("crates/flamewm-shell/src/taskbar/status/network.rs")
    file_actions = live("crates/flamewm-desktop-core/src/file_actions.rs")
    persist = (root / "crates/flamewm-desktop-core/src/sticky_persistence.rs").read_text(
        encoding="utf-8"
    ) if (root / "crates/flamewm-desktop-core/src/sticky_persistence.rs").is_file() else ""
    ext = live("crates/flamewm-render-x11/src/external_drawable.rs")
    chrome = live("crates/flamewm-wm-x11/src/chrome.rs")
    paint_decor = live("crates/flamewm-wm-x11/src/decoration/paint.rs")
    palette = live("crates/flamewm-skin/src/palette.rs")
    span = live("crates/flamewm-profiler/src/span.rs")
    # J12H-G01: StrokeRect radius.
    if "StrokeRect" not in paint_core or "radius" not in paint_core:
        found.add(("crates/flamewm-render-core/src/paint.rs", "j12h-g01-strokerect-radius"))
    # J12H-G02: one StartModel.
    count = sum(
        len(re.findall(r"struct StartModel", code(p.read_text(encoding="utf-8"))))
        for p in _j7_production_sources(root)
        if p.suffix == ".rs"
    )
    if count != 1:
        found.add(("crates/flamewm-shell-core/src/start_menu.rs", "j12h-g02-single-startmodel"))
    # J12H-G03: borrowed query hot path.
    for anchor in ("fn filter_view", "fn query_view", "fn slot_view"):
        if anchor not in start_menu:
            found.add(("crates/flamewm-shell-core/src/start_menu.rs", "j12h-g03-borrowed-query"))
            break
    # J12H-G04: Power explicit.
    if "StartCategory::Power" not in proj or "session" not in proj.lower():
        found.add(("crates/flamewm-shell/src/projection.rs", "j12h-g04-power-explicit"))
    # J12H-G05: surface-scoped icon reset.
    if "need_panel" not in app or "need_submenu" not in app:
        found.add(("crates/flamewm-shell/src/app.rs", "j12h-g05-scoped-icon-reset"))
    # J12H-G06: anchored OpenPopover. `open_status_anchored_id` enters the
    # status-surface transaction; popup_controller owns retained source
    # resolution and node measurement.
    if (
        "open_status_anchored_id" not in runtime
        or "resolve_source_rect" not in popup_controller
        or "measure_node" not in popup_controller
        or "popup_controller::open_popup" not in status_surface
    ):
        found.add(("crates/flamewm-shell/src/runtime.rs", "j12h-g06-anchored-popover"))
    # J12H-G07: audio/network availability-gated.
    if "ServiceAvailability::Available" not in audio:
        found.add(("crates/flamewm-shell/src/taskbar/status/audio.rs", "j12h-g07-no-live-authority"))
    if "ServiceAvailability::Available" not in network:
        found.add(("crates/flamewm-shell/src/taskbar/status/network.rs", "j12h-g07-no-live-authority"))
    # J12H-G08: context border 0.
    if "border-width:0" not in desktop_css().replace(" ", ""):
        found.add(("ui/desktop/desktop.css", "j12h-g08-context-border-zero"))
    # J12H-G09: NOREPLACE rename.
    if "RenameFlags::NOREPLACE" not in file_actions:
        found.add(("crates/flamewm-desktop-core/src/file_actions.rs", "j12h-g09-noreplace-rename"))
    # J12H-G10: sticky v1+v2 (source holds backslash-t escapes).
    if "v1\\t" not in persist or "v2\\t" not in persist:
        found.add(("crates/flamewm-desktop-core/src/sticky_persistence.rs", "j12h-g10-sticky-v1-v2"))
    # J12H-G11: event-driven, no poll. Contract anchor lives in doc
    # comments (stripped from code context); refresh_dynamic is owned by
    # app.rs and reconciles the runtime's signal-driven dirty state.
    raw_main = (root / "crates/flamewm-shell/src/main.rs").read_text(encoding="utf-8") if (root / "crates/flamewm-shell/src/main.rs").is_file() else ""
    raw_rt = (root / "crates/flamewm-shell/src/runtime.rs").read_text(encoding="utf-8") if (root / "crates/flamewm-shell/src/runtime.rs").is_file() else ""
    raw_app = (root / "crates/flamewm-shell/src/app.rs").read_text(encoding="utf-8") if (root / "crates/flamewm-shell/src/app.rs").is_file() else ""
    if ("refresh_dynamic" not in app or "register_timer" in main and "workspace" in main.lower().split("register_timer")[0][-200:]
            or ("never polls" not in raw_main.lower() and "no polling" not in raw_rt.lower()
                and "never polls" not in raw_app.lower() and "no polling" not in raw_app.lower())):
        found.add(("crates/flamewm-shell/src/app.rs", "j12h-g11-no-workspace-poll"))
    # J12H-G12: honest ExternalDrawable + skin metrics contract.
    if "ExternalDrawableTarget" not in (paint_decor + chrome) or "ExternalDrawableSession" not in ext:
        found.add(("crates/flamewm-wm-x11/src/decoration/paint.rs", "j12h-g12-external-delegation"))
    if "flamewm_skin" not in chrome and "WINDOW_CHROME" not in chrome:
        found.add(("crates/flamewm-wm-x11/src/chrome.rs", "j12h-g12-skin-metrics"))
    # J12H-G13: no legacy title path; measured intent success (missing-tolerant).
    for deco in ("crates/flamewm-wm-x11/src/decoration/paint.rs", "crates/flamewm-wm-x11/src/chrome.rs", "crates/flamewm-wm-x11/src/wm.rs"):
        text = live(deco)
        if text and ("image_text8" in text or "XDrawString" in text):
            found.add((deco, "j12h-g13-no-legacy-title"))
    if (root / "crates/flamewm-wm-x11/src/decoration/paint.rs").is_file() and "title_drawn = true" not in paint_decor:
        found.add(("crates/flamewm-wm-x11/src/decoration/paint.rs", "j12h-g13-measured-title-intent"))
    # J12H-G14: PANEL #1b1e20.
    if "0x1b1e20" not in palette.lower():
        found.add(("crates/flamewm-skin/src/palette.rs", "j12h-g14-panel-1b1e20"))
    # J12H-G15: scoped profiler spans.
    span_all = span + live("crates/flamewm-profiler/src/point.rs") + live("crates/flamewm-profiler/src/lib.rs")
    if span_all and "&'static str" not in span_all:
        found.add(("crates/flamewm-profiler/src/span.rs", "j12h-g15-scoped-spans"))
    if 'ProfilePoint::new("shell.' not in main and "flamewm_profiler::start(" not in main:
        found.add(("crates/flamewm-shell/src/main.rs", "j12h-g15-scoped-spans"))
    return found


def load_exceptions(root):
    path = root / "docs/ARCHITECTURE_EXCEPTIONS.toml"
    with path.open("rb") as stream:
        data = tomllib.load(stream)
    exceptions = set()
    errors = []
    for index, entry in enumerate(data.get("exception", []), 1):
        required = ("path", "capability", "reason", "canonical_target", "remove_by")
        missing = [key for key in required if not isinstance(entry.get(key), str) or not entry[key]]
        if missing:
            errors.append(f"exception {index}: missing required fields: {', '.join(missing)}")
            continue
        exception_path = entry["path"]
        if Path(exception_path).is_absolute() or any(token in exception_path for token in ("*", "?", "[", "]")):
            errors.append(f"exception {index}: path must be exact and relative: {exception_path}")
            continue
        if Path(exception_path).as_posix() != exception_path:
            errors.append(f"exception {index}: path must use relative POSIX form: {exception_path}")
            continue
        exceptions.add((exception_path, entry["capability"]))
    return exceptions, errors


def main():
    root = Path(__file__).resolve().parents[1]
    expected = violations(root)
    exceptions, errors = load_exceptions(root)
    stale = sorted(exceptions - expected)
    uncovered = sorted(expected - exceptions)
    for error in errors:
        print(f"ERROR architecture exception: {error}", file=sys.stderr)
    for path, capability in stale:
        print(f"ERROR stale architecture exception: {path} [{capability}]", file=sys.stderr)
    for path, capability in uncovered:
        print(f"ERROR architecture boundary violation: {path} [{capability}]", file=sys.stderr)
    if errors or stale or uncovered:
        return 1
    print(f"OK architecture boundary guard ({len(expected)} ratified violations)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

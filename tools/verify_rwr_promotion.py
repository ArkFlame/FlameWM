#!/usr/bin/env python3
"""Verify the identity-preserving RustWebRender 0.0.9 promotion.

This is a development/provenance gate only. Production never loads `.vendor`.
The post-mortem lesson is enforced mechanically: source owners that FlameWM did
not need to extend must retain equivalent Rust mechanics after the crate-name rename.
"""
from pathlib import Path
import hashlib
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
REF = ROOT / ".vendor/RustWebRender-0.0.9"
errors: list[str] = []


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


# Accepted-baseline pins. The user accepted the current renderer source as the
# new baseline: these owners are no longer compared token-by-token against
# 0.0.9. Each pin is an exact SHA-256, so any further change fails until it is
# re-accepted here. Non-pinned owners keep full token-identity checks.
ACCEPTED_OWNERS: dict[str, dict[str, str]] = {
    "crates/flamewm-render-core/src/lib.rs": {
        "sha256": "8af861a9971a7be225749a7f8a0899bf61360ff7f483ecab69aee66c32b9ea1a",
        "delta": "adds PixelBackend, PixelOp, RoundedRect and RenderAcceleration (shared CPU/GPU renderer domain) and the surface_material module and re-exports on top of text_layout exports, plus the hash module (fast integer-keyed maps)",
        "reason": "re-accepted after reviewing merged PR #15 (2026-10-30)",
    },
    "crates/flamewm-render-core/src/codec.rs": {
        "sha256": "4a197baf032b3de42c1a935dc3b2f5d82a5db6af3ff59433c9ffd23e3af1628f",
        "delta": "shares identical node styles when decoding compiled documents; clippy cleanups",
        "reason": "re-accepted after reviewed post-baseline changes (2026-10-28)",
    },
    "crates/flamewm-render-compiler/src/compile.rs": {
        "sha256": "0fcab5b20b7df30f6d9a4ba95e6f01240b4414c765a67e3d42a6f686c9f4977a",
        "delta": "shares identical node styles on lowering; rustfmt after upstream merge",
        "reason": "re-accepted after reviewed post-baseline changes (2026-10-28)",
    },
    "crates/flamewm-render-x11/src/xlib.rs": {
        "sha256": "46b00226cc1d82999e295863d7a024b53f89944670dcc1b8732ac659f06565cb",
        "delta": "adds CW_BORDER_PIXEL and further Xlib FFI constants (ARGB border pixel fix); return_large_frees_to_system also sets a single malloc arena",
        "reason": "re-accepted after reviewed post-baseline changes (2026-10-29)",
    },
    "crates/flamewm-render-x11/src/xft.rs": {
        "sha256": "dc1fcf075108436305c73c80e16ded1fae0a27ab45d85d1e95eab16b0de5f171",
        "delta": "text_alpha_mask plus clip, scroll and backdrop support added since 0.0.9",
        "reason": "re-accepted after reviewed post-baseline changes (2026-10-28)",
    },
    "crates/flamewm-render-x11/src/xrender.rs": {
        "sha256": "7399150d9a44138f6ae0d88d659cf3583f0a5085b17665c95c9cd5972518cebb",
        "delta": "clippy cleanups and memory-footprint changes since 0.0.9",
        "reason": "re-accepted after reviewed post-baseline changes (2026-10-28)",
    },
    "crates/flamewm-render-compiler/src/css.rs": {
        "sha256": "04092a882c05e37af5d03dd4eecf482585887f9e05fdbea69ae32dba45c9745e",
        "delta": "clippy and debloat rewrites (iterator chains, is_some_and) since 0.0.9",
        "reason": "re-accepted after reviewed post-baseline changes (2026-10-28)",
    },
    "crates/flamewm-render-compiler/src/html.rs": {
        "sha256": "0066cbd5597997934025a20a9ccf94904958ead7127b667595587be1fcd06b4d",
        "delta": "clippy and debloat rewrites (iterator style and link collectors) since 0.0.9",
        "reason": "re-accepted after reviewed post-baseline changes (2026-10-28)",
    },
    "crates/flamewm-render-compiler/src/bin/flamewm-render-inspect.rs": {
        "sha256": "dadff8cbd8e98fa4b674f9e4b10433927adcc4eb04fd24bd1459a88ba3cb278e",
        "delta": "rwr-inspect renamed with FlameWM Render naming and later updates",
        "reason": "re-accepted after reviewed post-baseline changes (2026-10-28)",
    },
}

# Split items pinned by SHA-256 of the exact item body at the current owner.
ACCEPTED_ITEMS: dict[tuple[str, str], dict[str, str]] = {
    ("crates/flamewm-render-x11/src/config.rs", "pub struct X11Config"): {
        "sha256": "bfbcb053b0db6217ee6b2decb7c69857c37748f0b3a7eff1fcb2734952923582",
        "delta": "X11Config fields grew since 0.0.9 (xdnd, backdrop, acceleration settings)",
        "reason": "re-accepted after reviewed post-baseline changes (2026-10-28)",
    },
    ("crates/flamewm-render-x11/src/config.rs", "impl Default for X11Config"): {
        "sha256": "85ca9f3208b8341ab3ded8de2379585e8df3278e61a56801e2da75ad2ce5a8d3",
        "delta": "X11Config defaults follow the added fields",
        "reason": "re-accepted after reviewed post-baseline changes (2026-10-28)",
    },
    ("crates/flamewm-render-x11/src/app/events.rs", "fn keyboard_text"): {
        "sha256": "b3dd46897484d141462d6deaff89771939e7e6a17d06268918489a79048cd1ff",
        "delta": "keyboard_text moved into app/events.rs with later input fixes",
        "reason": "re-accepted after reviewed post-baseline changes (2026-10-28)",
    },
    ("crates/flamewm-render-x11/src/runtime.rs", "pub fn run_with_controller<F>"): {
        "sha256": "b0f49a5f79b362b0cd2d2ab41b9997201e3b865944438569a8a7488ac02aae11",
        "delta": "run_with_controller body differs from 0.0.9 (controller/event-seam wiring)",
        "reason": "user accepted current renderer source as the new baseline (2026-10-09)",
    },
    ("crates/flamewm-render-x11/src/app/mod.rs", "struct CachedImage"): {
        "sha256": "d97edbf389f26b82fc5d39984fe7b89200d63d76087c69ec47cdb3fbaeba0e87",
        "delta": "CachedImage carries pixmap, picture, mask and mask_bytes fields",
        "reason": "user accepted current renderer source as the new baseline (2026-10-09)",
    },
    ("crates/flamewm-render-x11/src/app/mod.rs", "struct ImageCacheKey"): {
        "sha256": "77f46f17a8ce1bdfadd95d8c5159e5835f42417c49bd9481e056094be451f446",
        "delta": "ImageCacheKey includes the revision field (pinned at the app/mod.rs owner)",
        "reason": "user accepted current renderer source as the new baseline (2026-10-09)",
    },
    ("crates/flamewm-render-x11/src/app/color.rs", "fn blend_over_black"): {
        "sha256": "08edd0cc128fdfb08618fd6d7c248007dfbfdf19552fd5eac6dcbd750bc70567",
        "delta": "blend_over_black delegates to flatten_opaque_fallback instead of 0.0.9 alpha-multiply arithmetic",
        "reason": "user accepted current renderer source as the new baseline (2026-10-09)",
    },
}


NAMESPACE_TRANSLATIONS = {
    "rustwebrender_core": "flamewm_render_core",
    "rustwebrender_compiler": "flamewm_render_compiler",
    "rustwebrender_x11": "flamewm_render_x11",
}


def translate(text: str, translations: tuple[tuple[str, str], ...]) -> str:
    for source, target in translations:
        text = text.replace(source, target)
    return text


def tokens(text: str) -> tuple[str, ...]:
    pattern = re.compile(
        r'//[^\n]*|/\*.*?\*/|"(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\])*\'|'
        r'[A-Za-z_][A-Za-z0-9_]*|[0-9]+(?:\.[0-9]+)?|==|!=|<=|>=|=>|&&|\|\||'
        r'::|->|\.\.|[{}()\[\];,.:?+*/%&|^!<>=\-~]',
        re.DOTALL,
    )
    raw = tuple(token for token in pattern.findall(text) if not token.startswith(("//", "/*")))
    return tuple(
        token
        for index, token in enumerate(raw)
        if not (token == "," and index + 1 < len(raw) and raw[index + 1] in "})]")
    )


def normalized_tokens(text: str) -> tuple[str, ...]:
    """Compare Rust after formatter-only and Rust 2024 unsafe-wrapper changes."""
    # Current rustfmt also reorders names inside a braced use item while the
    # 0.0.9 source predates that formatting. Keep that layout-only change out
    # of the provenance comparison.
    def sort_use(match: re.Match[str]) -> str:
        names = sorted(
            (part.strip() for part in match.group(2).split(",") if part.strip()),
            key=str.casefold,
        )
        return f"use {match.group(1)}::{{{', '.join(names)}}};"

    text = re.sub(
        r"use (flamewm_render_core|rustwebrender_core)::\{([^{}]*)\};",
        sort_use,
        text,
    )
    # Split modules require restricted visibility where the monolith used private
    # items; rustfmt layout and that visibility do not change item mechanics.
    text = re.sub(r"\bpub\((?:crate|super)\)\s+", "", text)
    # Rust 2024 requires explicit unsafe blocks around individual FFI calls.
    # Remove only those wrapper pairs; unsafe item signatures remain significant.
    previous = None
    while text != previous:
        previous = text
        text = re.sub(r"\bunsafe\s*\{([^{}]*)\}", r"\1", text, flags=re.DOTALL)
    try:
        formatted = subprocess.run(
            ["rustfmt", "--edition", "2021", "--emit", "stdout"],
            input=text,
            text=True,
            capture_output=True,
            check=True,
        ).stdout
    except (OSError, subprocess.CalledProcessError):
        formatted = text
    return tokens(formatted)


def strip_patterns(text: str, patterns: tuple[str, ...]) -> str:
    for pattern in patterns:
        text, count = re.subn(pattern, "", text, flags=re.DOTALL | re.MULTILINE)
        if count != 1:
            errors.append(f"declared extension anchor missing or duplicated: {pattern}")
    return text


CORE_EVENTS = r"\s*#\[derive\(Clone, Copy, Debug, PartialEq, Eq\)\]\s*pub enum ActionPhase \{.*?\}\s*#\[derive\(Clone, Debug, PartialEq\)\]\s*pub struct ActionEvent \{.*?\}\s*#\[derive\(Clone, Debug, PartialEq\)\]\s*pub enum ControllerEvent \{.*?\}\s*"
CONFIG_X11_FIELDS = (r"\s*pub x: i32,\s*", r"\s*pub y: i32,\s*", r"\s*pub role: X11WindowRole,\s*", r"\s*/// Optional ICCCM WM_CLASS identity\. `None` leaves WM_CLASS unset\.\s*pub identity: Option<X11WindowIdentity>,\s*")
CONFIG_X11_DEFAULT = (r"\s*x: 0,\s*", r"\s*y: 0,\s*", r"\s*role: X11WindowRole::Normal,\s*", r"\s*identity: None,\s*")
APP_IMAGE_REVISION = r"\s*revision: u64,\s*"
XLIB_EXTENSIONS = (
    r"\s*#\[repr\(C\)\]\s*pub struct XSetWindowAttributes \{.*?\}\s*",
    r"\s*pub const PROP_MODE_REPLACE: c_int = 0;\s*",
    r"\s*pub const XA_ATOM: Atom = 4;\s*",
    r"\s*pub const CW_OVERRIDE_REDIRECT: c_ulong = 1 << 9;\s*",
    r"\s*pub const GRAB_MODE_ASYNC: c_int = 1;\s*",
    r"\s*pub const CURRENT_TIME: Time = 0;\s*",
    r"\s*pub fn XChangeWindowAttributes\(.*?\)\s*->\s*c_int;\s*",
    r"\s*pub fn XUnmapWindow\(.*?\)\s*->\s*c_int;\s*",
    r"\s*pub fn XRaiseWindow\(.*?\)\s*->\s*c_int;\s*",
    r"\s*pub fn XMoveResizeWindow\(.*?\)\s*->\s*c_int;\s*",
    r"\s*pub fn XGrabPointer\(.*?\)\s*->\s*c_int;\s*",
    r"\s*pub fn XUngrabPointer\(.*?\)\s*->\s*c_int;\s*",
    r"\s*pub fn XDisplayWidth\(.*?\)\s*->\s*c_int;\s*",
    r"\s*pub fn XDisplayHeight\(.*?\)\s*->\s*c_int;\s*",
    r"\s*pub fn XConnectionNumber\(.*?\)\s*->\s*c_int;\s*",
    r"\s*pub fn XPutBackEvent\(.*?\)\s*->\s*c_int;\s*",
    r"\s*pub fn XChangeProperty\(.*?\)\s*->\s*c_int;\s*",
)


def exact(
    ref: str,
    active: str,
    extensions: tuple[tuple[str, str], ...] = (),
    ref_extensions: tuple[str, ...] = (),
    active_extensions: tuple[str, ...] = (),
) -> None:
    ref_path = REF / ref
    active_path = ROOT / active
    if not ref_path.is_file():
        errors.append(f"vendor promotion anchor missing: {ref}")
        return
    if not active_path.is_file():
        errors.append(f"canonical promoted owner missing: {active}")
        return
    if active in ACCEPTED_OWNERS:
        # Accepted baseline: the whole active file is pinned by SHA-256 instead
        # of token identity. Any further change fails until re-accepted.
        if sha256_file(active_path) != ACCEPTED_OWNERS[active]["sha256"]:
            errors.append(f"accepted baseline hash mismatch: {active} (re-accept or revert)")
        return
    r = strip_patterns(translate(ref_path.read_text(), extensions), ref_extensions)
    a = strip_patterns(active_path.read_text(), active_extensions)
    if normalized_tokens(r) != normalized_tokens(a):
        errors.append(f"promoted owner drifted beyond declared translations: {active} <- {ref}")


def rust_item(text: str, identity: str) -> str | None:
    """Return one named Rust item, rejecting absent or duplicate identities."""
    matches = list(re.finditer(re.escape(identity), text))
    if len(matches) != 1:
        return None
    start = text.rfind("\n", 0, matches[0].start()) + 1
    brace = text.find("{", matches[0].end())
    semi = text.find(";", matches[0].end())
    if semi != -1 and (brace == -1 or semi < brace):
        return text[start : semi + 1]
    if brace == -1:
        return None
    depth = 0
    for index in range(brace, len(text)):
        if text[index] == "{":
            depth += 1
        elif text[index] == "}":
            depth -= 1
            if depth == 0:
                return text[start : index + 1]
    return None


def exact_split_owner(
    ref: str,
    active: str,
    ref_identity: str,
    active_identity: str | None = None,
    translations: tuple[tuple[str, str], ...] = (),
    ref_extensions: tuple[str, ...] = (),
    active_extensions: tuple[str, ...] = (),
) -> None:
    """Compare one stable vendor item at its current split active owner."""
    ref_path = REF / ref
    active_path = ROOT / active
    if not ref_path.is_file() or not active_path.is_file():
        errors.append(f"split promoted owner missing: {active} <- {ref}")
        return
    ref_item = rust_item(translate(ref_path.read_text(), translations), ref_identity)
    active_item = rust_item(active_path.read_text(), active_identity or ref_identity)
    if ref_item is None or active_item is None:
        errors.append(f"split promoted item missing or duplicated: {active_identity or ref_identity} in {active}")
        return
    pin = ACCEPTED_ITEMS.get((active, active_identity or ref_identity))
    if pin is not None:
        # Accepted baseline: the item body is pinned by SHA-256 instead of token identity.
        if hashlib.sha256(active_item.encode()).hexdigest() != pin["sha256"]:
            errors.append(f"accepted baseline item hash mismatch: {active_identity or ref_identity} in {active}")
        return
    r = strip_patterns(ref_item, ref_extensions)
    a = strip_patterns(active_item, active_extensions)
    if normalized_tokens(r) != normalized_tokens(a):
        errors.append(f"split promoted item drifted: {active_identity or ref_identity} in {active} <- {ref_identity} in {ref}")


def require_anchor(path: str, anchor: str, description: str) -> None:
    owner = ROOT / path
    if not owner.is_file() or anchor not in owner.read_text():
        errors.append(f"required {description} anchor missing: {path}: {anchor}")


if not (REF / "Cargo.toml").is_file() or 'version = "0.0.9"' not in (REF / "Cargo.toml").read_text():
    errors.append("vendor manifest is not RustWebRender 0.0.9")

exact(
    "crates/rustwebrender-core/src/lib.rs",
    "crates/flamewm-render-core/src/lib.rs",
    tuple(NAMESPACE_TRANSLATIONS.items()),
    active_extensions=(CORE_EVENTS,),
)
exact(
    "crates/rustwebrender-core/src/codec.rs",
    "crates/flamewm-render-core/src/codec.rs",
    tuple(NAMESPACE_TRANSLATIONS.items())
    + (
        ("not a RustWebRender compiled document", "not a FlameWM Render compiled document"),
        ("unsupported RustWebRender format version", "unsupported FlameWM Render format version"),
    ),
)

for rel in ["compile.rs", "css.rs", "html.rs", "lib.rs"]:
    exact(
        f"crates/rustwebrender-compiler/src/{rel}",
        f"crates/flamewm-render-compiler/src/{rel}",
        tuple(NAMESPACE_TRANSLATIONS.items()) + (("RustWebRender", "FlameWM Render"),),
    )
exact(
    "crates/rustwebrender-compiler/src/bin/rwr-inspect.rs",
    "crates/flamewm-render-compiler/src/bin/flamewm-render-inspect.rs",
    tuple(NAMESPACE_TRANSLATIONS.items())
    + (("rwr-inspect", "flamewm-render-inspect"),),
)

# X11 lib.rs was intentionally split by owner. Compare stable promoted items
# at their current owners rather than ignoring the former monolithic module.
X11_TRANSLATIONS = tuple(NAMESPACE_TRANSLATIONS.items()) + (
    ("RustWebRender", "FlameWM Render"),
    ("RWR_ACTION", "FLAMEWM_RENDER_ACTION"),
)
for identity, active in [
    ("pub struct X11Config", "crates/flamewm-render-x11/src/config.rs"),
    ("impl Default for X11Config", "crates/flamewm-render-x11/src/config.rs"),
    ("pub fn run(", "crates/flamewm-render-x11/src/runtime.rs"),
    ("pub fn run_with_handler", "crates/flamewm-render-x11/src/runtime.rs"),
    ("pub fn run_with_controller<F>", "crates/flamewm-render-x11/src/runtime.rs"),
    ("struct CachedImage", "crates/flamewm-render-x11/src/app/mod.rs"),
    ("fn scale_rect", "crates/flamewm-render-x11/src/app/primitives.rs"),
    ("fn keyboard_text", "crates/flamewm-render-x11/src/app/events.rs"),
    ("fn cursor_shape", "crates/flamewm-render-x11/src/app/cursor.rs"),
    ("fn root_background", "crates/flamewm-render-x11/src/app/color.rs"),
    ("fn blend_over_black", "crates/flamewm-render-x11/src/app/color.rs"),
    ("fn rgb_to_pixel", "crates/flamewm-render-x11/src/app/color.rs"),
    ("fn channel_to_mask", "crates/flamewm-render-x11/src/app/color.rs"),
]:
    exact_split_owner(
        "crates/rustwebrender-x11/src/lib.rs",
        active,
        identity,
        translations=X11_TRANSLATIONS,
        active_extensions=CONFIG_X11_FIELDS if identity == "pub struct X11Config" else (
            CONFIG_X11_DEFAULT if identity == "impl Default for X11Config" else ()
        ),
    )
exact_split_owner(
    "crates/rustwebrender-x11/src/lib.rs",
    "crates/flamewm-render-x11/src/app/mod.rs",
    "struct ImageCacheKey",
    translations=X11_TRANSLATIONS,
    active_extensions=(APP_IMAGE_REVISION,),
)

# Xlib stays monolithic. These are the exact audited FFI additions needed for
# roles, popup placement/grabs, reactor integration, and root geometry.
exact(
    "crates/rustwebrender-x11/src/xlib.rs",
    "crates/flamewm-render-x11/src/xlib.rs",
    (("extern \"C\" {", "unsafe extern \"C\" {"),),
    active_extensions=XLIB_EXTENSIONS,
)
for rel in ["xft.rs", "xrender.rs"]:
    exact(
        f"crates/rustwebrender-x11/src/{rel}",
        f"crates/flamewm-render-x11/src/{rel}",
        tuple(NAMESPACE_TRANSLATIONS.items())
    + (
        ("RustWebRender", "FlameWM Render"),
        ('extern "C" {', 'unsafe extern "C" {'),
            ("RWR_FONTCONFIG_WARNING", "FLAMEWM_RENDER_FONTCONFIG_WARNING"),
            ("RWR_UI_FONT", "FLAMEWM_RENDER_UI_FONT"),
            ("RWR_FONT_FILES", "FLAMEWM_RENDER_FONT_FILES"),
         ),
      )
exact(
    "crates/rustwebrender-x11/src/bin/rwr-x11.rs",
    "crates/flamewm-render-x11/src/bin/flamewm-render-x11.rs",
    tuple(NAMESPACE_TRANSLATIONS.items())
    + (("rwr-x11:", "flamewm-render-x11:"), ("usage: rwr-x11", "usage: flamewm-render-x11")),
)

# Deliberately extended owners must retain exact 0.0.9 mechanics and audited
# FlameWM seams at their current split owners.
for path, anchor, description in [
    ("crates/flamewm-render-x11/src/app/events.rs", "XEventsQueued(self.display, QUEUED_AFTER_READING)", "motion coalescing"),
    ("crates/flamewm-render-x11/src/app/events.rs", "XPeekEvent(self.display", "motion coalescing"),
    ("crates/flamewm-render-x11/src/app/events.rs", "ActionPhase::Motion", "motion dispatch"),
    ("crates/flamewm-render-x11/src/app/events.rs", "hover == Some(index),", "motion dispatch"),
    ("crates/flamewm-render-x11/src/app/mod.rs", "XSetGraphicsExposures(display, gc, 0)", "image copy"),
    ("crates/flamewm-render-x11/src/app/render.rs", "XCopyArea(\n                    self.display,\n                    self.backbuffer,", "present copy"),
    ("crates/flamewm-render-x11/src/app/mod.rs", "_NET_WM_WINDOW_TYPE", "EWMH role"),
    ("crates/flamewm-render-x11/src/app/mod.rs", "attributes.override_redirect = 1;", "popup override redirect"),
    ("crates/flamewm-render-x11/src/app/images.rs", "revision: u64", "image revision cache"),
    ("crates/flamewm-render-x11/src/app/events.rs", "self.single_event", "single-event lifecycle"),
    ("crates/flamewm-render-x11/src/app/events.rs", "self.close_requested", "close lifecycle"),
    ("crates/flamewm-render-x11/src/runtime.rs", "run_with_controller_events_with_reactor", "reactor event seam"),
    ("crates/flamewm-render-x11/src/runtime.rs", "pub fn root_geometry", "root geometry"),
    ("crates/flamewm-render-x11/src/config.rs", "pub enum X11WindowRole", "role enum"),
    ("crates/flamewm-render-x11/src/config.rs", "pub struct SurfaceConfig", "surface lifecycle"),
    ("crates/flamewm-render-x11/src/config.rs", "pub struct SurfaceId", "surface lifecycle"),
    ("crates/flamewm-render-x11/src/config.rs", "pub struct SurfaceControllerEvent", "surface lifecycle"),
    ("crates/flamewm-render-x11/src/surface_controller.rs", "pub struct SurfaceController", "surface lifecycle"),
    ("crates/flamewm-render-x11/src/xlib.rs", "pub fn XChangeProperty", "Xlib property FFI"),
    ("crates/flamewm-render-x11/src/xlib.rs", "pub fn XGrabPointer", "Xlib pointer-grab FFI"),
]:
    require_anchor(path, anchor, description)

for anchor in ["pub enum ActionPhase", "pub struct ActionEvent", "pub enum ControllerEvent"]:
    require_anchor("crates/flamewm-render-core/src/lib.rs", anchor, "public renderer event seam")

if errors:
    for error in errors:
        print(f"ERROR {error}", file=sys.stderr)
    raise SystemExit(1)
print("OK FlameWM Render 0.0.9 provenance promotion")

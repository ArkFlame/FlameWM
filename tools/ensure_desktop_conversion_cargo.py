#!/usr/bin/env python3
"""Ensure Cargo builds the desktop-core API from this checkout, not a stale rlib.

A source-only check cannot prove what Cargo will link. In particular, extracted
archives can restore source mtimes older than previously compiled artifacts;
Cargo's mtime fingerprints can then report an old desktop-core as fresh.

This pre-build gate validates the exact path dependency via Cargo metadata and
invalidates only the desktop-core RELEASE artifacts when the content fingerprint
has not yet been invalidated. Never deletes target trees, source, or other crates.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path

# `python3 -I` used by the Xephyr harness removes the script directory from
# sys.path. Import only the adjacent trusted checker, never checkout input.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from verify_desktop_conversion_contract import contract_errors

CORE = "flamewm-desktop-core"
DESKTOP = "flamewm-desktop"
# Source files which define the RWR embedded by flamewm-desktop/build.rs.
# An extracted archive can preserve OLD mtimes while carrying NEW content,
# causing Cargo to reuse an obsolete OUT_DIR/flamewm-desktop.rwr.
DESKTOP_UI_INPUTS = (
    "crates/flamewm-desktop/build.rs",
    "crates/flamewm-desktop/ui_contract.rs",
    "ui/desktop/index.html",
    "ui/desktop/desktop.css",
    "ui/shared/menu.css",
    "crates/flamewm-render-compiler/src/lib.rs",
    "crates/flamewm-render-compiler/src/compile.rs",
    "crates/flamewm-render-compiler/src/html.rs",
    "crates/flamewm-render-compiler/src/css.rs",
)

INPUTS = (
    "crates/flamewm-desktop-core/Cargo.toml",
    "crates/flamewm-desktop-core/src/lib.rs",
    "crates/flamewm-desktop/Cargo.toml",
)
# Every core source file is an input, so a new or unlisted module with an
# old mtime and new bytes still invalidates the core without a forced clean.
CORE_SOURCE_DIR = "crates/flamewm-desktop-core/src"


class BuildGateError(RuntimeError):
    """A source, Cargo resolution or targeted invalidation boundary failed."""


def core_inputs(root: Path) -> tuple[str, ...]:
    sources = (p.relative_to(root).as_posix() for p in (root / CORE_SOURCE_DIR).rglob("*.rs") if p.is_file())
    return tuple(sorted(set(INPUTS) | set(sources)))


def digest_files(root: Path, files: tuple[str, ...]) -> str:
    result = hashlib.sha256()
    for rel in files:
        path = root / rel
        try:
            data = path.read_bytes()
        except OSError as error:
            raise BuildGateError(f"missing or unreadable build input {rel}: {error}") from error
        result.update(rel.encode("utf-8") + b"\0")
        result.update(len(data).to_bytes(8, "big"))
        result.update(data)
    return result.hexdigest()


def content_digest(root: Path) -> str:
    return digest_files(root, core_inputs(root))


def desktop_ui_digest(root: Path) -> str:
    return digest_files(root, DESKTOP_UI_INPUTS)


def validate_metadata(root: Path, data: dict) -> Path:
    """Validate Cargo's own resolution and return the resolved target directory."""
    try:
        actual_root = Path(data["workspace_root"]).resolve()
        packages = {p["name"]: p for p in data["packages"] if p["name"] in (DESKTOP, CORE)}
        core = packages[CORE]
        desktop = packages[DESKTOP]
        members = set(data["workspace_members"])
        target_dir = Path(data["target_directory"]).resolve()
    except (KeyError, TypeError, ValueError) as error:
        raise BuildGateError(f"Cargo metadata missing workspace/core/desktop/target: {error}") from error
    if actual_root != root:
        raise BuildGateError(f"Cargo workspace root mismatch: expected {root}, got {actual_root}")
    for name, info in ((CORE, core), (DESKTOP, desktop)):
        actual_manifest = Path(info["manifest_path"]).resolve()
        expected = (root / "crates" / name / "Cargo.toml").resolve()
        if actual_manifest != expected:
            raise BuildGateError(f"Cargo resolves {name} to {actual_manifest}, expected {expected}")
        if info.get("id") not in members:
            raise BuildGateError(f"Cargo workspace does not include local {name} package")
    source = (root / "crates/flamewm-desktop-core/src/lib.rs").resolve()
    lib_targets = [t for t in core.get("targets", ()) if "lib" in t.get("kind", ())]
    if len(lib_targets) != 1 or Path(lib_targets[0]["src_path"]).resolve() != source:
        raise BuildGateError(f"Cargo core library target does not point to {source}")
    deps = [d for d in desktop.get("dependencies", ()) if d.get("name") == CORE]
    if len(deps) != 1 or not deps[0].get("path") or Path(deps[0]["path"]).resolve() != source.parent.parent:
        raise BuildGateError(f"Cargo desktop's {CORE} dependency is not the local path crate")
    return target_dir


def invalidate_if_changed(
    state_dir: Path, *, root: Path, package: str, profile: str, digest: str, marker: str,
    force: bool, cargo: str, executor,
) -> bool:
    # Markers live beside the resolved Cargo target dir, so a digest stamped for one
    # target (or a full `cargo clean`, which removes it) never vouches for another.
    state = state_dir / marker
    prior = state.read_text(encoding="ascii").strip() if state.is_file() else None
    if prior == digest and not force:
        print(f"OK FlameWM {package} Cargo path and {profile} content fingerprint unchanged")
        return False

    clean = [cargo, "clean", "-p", package]
    if profile == "release":
        clean.append("--release")
    print(
        f"FLAMEWM-CONVERT-CARGO: invalidate {profile} artifacts for {package} "
        "(old archive timestamps may hide changed source from Cargo)", flush=True,
    )
    try:
        result = executor(clean, cwd=root, timeout=180)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise BuildGateError(f"targeted Cargo clean failed for {package}: {error}") from error
    if result.returncode:
        raise BuildGateError(
            f"targeted Cargo clean failed for {package} with exit {result.returncode}; "
            "cannot trust cached build outputs"
        )
    state.parent.mkdir(parents=True, exist_ok=True)
    tmp = state.with_name(f".{state.name}.{os.getpid()}.tmp")
    try:
        tmp.write_text(digest + "\n", encoding="ascii")
        os.replace(tmp, state)
    finally:
        tmp.unlink(missing_ok=True)
    print(f"OK targeted Cargo {profile} artifacts invalidated for {package}")
    return True


def run(root: Path, *, profile: str = "release", force: bool = False,
        include_ui: bool = False, cargo: str = "cargo", executor=subprocess.run) -> bool:
    root = root.resolve(strict=True)
    if profile not in ("dev", "release"):
        raise BuildGateError(f"unknown Cargo profile: {profile}")
    errors = contract_errors(root)
    if errors:
        raise BuildGateError("source API mismatch: " + "; ".join(errors))
    cmd = [cargo, "metadata", "--format-version", "1", "--no-deps", "--offline"]
    try:
        result = executor(cmd, cwd=root, text=True, capture_output=True, timeout=90)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise BuildGateError(f"Cargo metadata could not run: {error}") from error
    if result.returncode:
        raise BuildGateError(f"Cargo metadata failed (exit {result.returncode}): {result.stderr.strip()[-2500:]}")
    try:
        target_dir = validate_metadata(root, json.loads(result.stdout))
    except (ValueError, TypeError) as error:
        raise BuildGateError(f"malformed Cargo metadata JSON: {error}") from error
    state_dir = target_dir / "flamewm"

    # Validate ALL digests before cleaning anything: absent build inputs fail closed.
    core_digest = content_digest(root)
    ui_digest = desktop_ui_digest(root) if include_ui else None
    changed = invalidate_if_changed(
        state_dir, root=root, package=CORE, profile=profile, digest=core_digest,
        marker=f"desktop-core-{profile}-cache-invalidated.sha256",
        force=force, cargo=cargo, executor=executor,
    )
    if ui_digest is not None:
        changed |= invalidate_if_changed(
            state_dir, root=root, package=DESKTOP, profile=profile, digest=ui_digest,
            marker=f"desktop-rwr-{profile}-cache-invalidated.sha256",
            force=force, cargo=cargo, executor=executor,
        )
    return changed


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parent.parent)
    parser.add_argument("--profile", choices=("dev", "release"), default="release")
    parser.add_argument("--force", action="store_true", help="force one additional targeted clean")
    parser.add_argument("--include-ui", action="store_true",
                        help="also invalidate flamewm-desktop when compiled RWR inputs change")
    args = parser.parse_args()
    try:
        run(args.root, profile=args.profile, force=args.force, include_ui=args.include_ui)
        return 0
    except BuildGateError as error:
        print("FLAMEWM-CONVERT-CARGO: BLOCKED: " + str(error), file=sys.stderr)
        print("Inspect `cargo metadata --no-deps --format-version 1` and the configured Cargo target directory; do not overwrite unrelated source.", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())

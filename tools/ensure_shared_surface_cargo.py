#!/usr/bin/env python3
"""Refresh only changed shared-surface and Task Manager release artifacts.

Extracting a source tarball can preserve mtimes older than Cargo's cached rlibs.
Cargo may then reuse old definitions even though the source modules are correct.
A content digest, scoped to the crates affected by Pass 9/10, prevents that.
"""
from __future__ import annotations

import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CRATES = ("flamewm-skin", "flamewm-ui-core", "flamewm-ui-x11", "flamewm-task-manager")
UI_INPUTS = ("ui/task-manager/task-manager.css", "ui/task-manager/task-manager.html")


def content_digest(root: Path, crate: str) -> str:
    directory = root / "crates" / crate
    sources = [directory / "Cargo.toml"] + sorted(directory.rglob("*.rs"))
    if crate == "flamewm-task-manager":
        sources += [root / name for name in UI_INPUTS]
    digest = hashlib.sha256()
    for source in sources:
        if not source.is_file():
            raise RuntimeError(f"Missing source input: {source}")
        relative = source.relative_to(root).as_posix().encode()
        payload = source.read_bytes()
        digest.update(len(relative).to_bytes(4, "big"))
        digest.update(relative)
        digest.update(len(payload).to_bytes(8, "big"))
        digest.update(payload)
    return digest.hexdigest()


def run(root: Path = ROOT, *, cargo: str = "cargo", executor=subprocess.run) -> list[str]:
    root = root.resolve()
    metadata = executor(
        [cargo, "metadata", "--format-version", "1", "--no-deps", "--offline"],
        cwd=root, capture_output=True, text=True, timeout=90,
    )
    if metadata.returncode:
        raise RuntimeError(f"cargo metadata failed: {metadata.stderr.strip()[-1000:]}")
    details = json.loads(metadata.stdout)
    if Path(details["workspace_root"]).resolve() != root:
        raise RuntimeError("cargo workspace root differs from checkout")
    packages = {package["name"]: package for package in details["packages"]}
    for crate in CRATES:
        if crate not in packages or Path(packages[crate]["manifest_path"]).resolve() != (
            root / "crates" / crate / "Cargo.toml"
        ):
            raise RuntimeError(f"{crate} is not resolved as a local workspace crate")
    state_dir = Path(details["target_directory"]).resolve() / "flamewm"
    # Digest everything before changing any build state.
    expected = {crate: content_digest(root, crate) for crate in CRATES}
    changed = []
    for crate, digest in expected.items():
        marker = state_dir / f"{crate}-release-source.sha256"
        prior = marker.read_text().strip() if marker.is_file() else ""
        if prior == digest:
            print(f"OK {crate} release sources unchanged")
            continue
        print(f"FLAMEWM-CARGO: invalidating stale release artifacts: {crate}", flush=True)
        clean = executor([cargo, "clean", "-p", crate, "--release"], cwd=root, timeout=180)
        if clean.returncode:
            raise RuntimeError(f"cargo clean failed for {crate} (exit {clean.returncode})")
        state_dir.mkdir(parents=True, exist_ok=True)
        temporary = marker.with_name(f".{marker.name}.{os.getpid()}.tmp")
        try:
            temporary.write_text(digest + "\n")
            os.replace(temporary, marker)
        finally:
            temporary.unlink(missing_ok=True)
        changed.append(crate)
    return changed


if __name__ == "__main__":
    try:
        run()
    except (OSError, RuntimeError, ValueError, KeyError, subprocess.TimeoutExpired) as error:
        print(f"FLAMEWM-CARGO: blocked: {error}", file=sys.stderr)
        raise SystemExit(2) from error

#!/usr/bin/env python3
"""Validate an already staged FlameWM prefix and launch its installed Settings.

This runner does not build or extract packages. Extract an official package into
an owned temporary prefix first, then pass that prefix and a fresh run directory.
The native Settings harness owns a private Xephyr, D-Bus session, HOME, and XDG
directories; it receives only binaries from the staged prefix.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
RUNNER = ROOT / "tools" / "flamewm_settings_acceptance.py"
REQUIRED_BINARIES = (
    "flamewm", "flamewm-session", "flamewm-desktop", "flamewm-shell",
    "flamewm-settings", "flamewm-render-x11", "flamewm-render-compile",
    "flamewm-render-inspect",
)
REQUIRED_FILES = tuple(f"usr/bin/{binary}" for binary in REQUIRED_BINARIES) + (
    "usr/share/flamewm/VERSION",
    "usr/share/flamewm/assets/web/flamewm-icon.svg",
    "usr/share/flamewm/assets/web/github.svg",
    "usr/share/flamewm/assets/web/paypal.svg",
    "usr/share/icons/FlameWM-Breeze-Dark/index.theme",
)
REQUIRED_DIRS = (
    "usr/share/flamewm/assets/web/breeze",
    "usr/share/flamewm/assets/branding",
    "usr/share/icons/FlameWM-Breeze-Dark/cursors",
)
def inside(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
        return True
    except ValueError:
        return False


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prefix", required=True, help="extracted package root containing usr/bin and usr/share")
    parser.add_argument("--run-dir", required=True, help="fresh private directory for native evidence")
    parser.add_argument("--geometry", default="1280x800", help="private Xephyr screen size")
    parser.add_argument("--display", help="optional free display :90..:99")
    parser.add_argument("--settle-seconds", type=float, default=3.0)
    parser.add_argument("--hotkey-click")
    parser.add_argument("--hotkey-key")
    parser.add_argument("--hotkey-row-rect")
    parser.add_argument("--hotkey-error-rect")
    return parser.parse_args(argv)


def validate_prefix(raw: str) -> tuple[Path, dict[str, str], str]:
    prefix = Path(raw).expanduser().resolve(strict=True)
    if not prefix.is_dir() or prefix == Path("/") or prefix == Path("/usr"):
        raise ValueError("--prefix must be an owned extracted package directory, never / or /usr")
    expected = (ROOT / "FLAMEWM_VERSION").read_text(encoding="utf-8").strip()
    version_path = prefix / "usr/share/flamewm/VERSION"
    if version_path.read_text(encoding="utf-8").strip() != expected:
        raise ValueError(f"staged VERSION does not match worktree FLAMEWM_VERSION ({expected})")
    for relative in REQUIRED_FILES:
        path = prefix / relative
        if not path.is_file() or not inside(path.resolve(), prefix):
            raise ValueError(f"required staged file missing or escapes prefix: {relative}")
    for binary in REQUIRED_BINARIES:
        if not os.access(prefix / f"usr/bin/{binary}", os.X_OK):
            raise ValueError(f"staged executable is not executable: usr/bin/{binary}")
    for relative in REQUIRED_DIRS:
        path = prefix / relative
        if not path.is_dir() or not inside(path.resolve(), prefix):
            raise ValueError(f"required staged directory missing or escapes prefix: {relative}")
        if not any(item.is_file() for item in path.rglob("*")):
            raise ValueError(f"required staged directory is empty: {relative}")
    files = {relative: sha256(prefix / relative) for relative in REQUIRED_FILES}
    return prefix, files, expected


def run(args: argparse.Namespace) -> tuple[dict[str, Any], int]:
    prefix, hashes, version = validate_prefix(args.prefix)
    run_dir = Path(args.run_dir).expanduser().resolve()
    if run_dir == prefix or prefix in run_dir.parents or run_dir in prefix.parents:
        raise ValueError("--run-dir and --prefix must be separate trees")
    if run_dir.exists() and (not run_dir.is_dir() or any(run_dir.iterdir())):
        raise ValueError("--run-dir must not exist or must be empty")
    run_dir.mkdir(parents=True, exist_ok=True)
    os.chmod(run_dir, 0o700)
    acceptance_dir = run_dir / "native"
    command = [
        sys.executable, str(RUNNER), "--run-dir", str(acceptance_dir),
        "--wm-bin", str(prefix / "usr/bin/flamewm"),
        "--settings-bin", str(prefix / "usr/bin/flamewm-settings"),
        "--geometry", args.geometry,
    ]
    for flag, value in (
        ("--display", args.display), ("--hotkey-click", args.hotkey_click),
        ("--hotkey-key", args.hotkey_key), ("--hotkey-row-rect", args.hotkey_row_rect),
        ("--hotkey-error-rect", args.hotkey_error_rect),
    ):
        if value is not None:
            command.extend((flag, value))
    command.extend(("--settle-seconds", str(args.settle_seconds)))
    completed = subprocess.run(command, cwd=run_dir, text=True, capture_output=True, timeout=1800, check=False)
    native_file = acceptance_dir / "result.json"
    native = json.loads(native_file.read_text(encoding="utf-8")) if native_file.is_file() else None
    status = "NOT_RUN"
    reason = "native runner did not produce a result document"
    if native is not None:
        expected_bins = {
            "wm_bin": str(prefix / "usr/bin/flamewm"),
            "settings_bin": str(prefix / "usr/bin/flamewm-settings"),
        }
        identity_ok = all(native.get(key) == value for key, value in expected_bins.items())
        for key, value in expected_bins.items():
            if not identity_ok:
                break
            if sha256(Path(value)) != hashes[f"usr/bin/{Path(value).name}"]:
                identity_ok = False
        pages = {item.get("id"): item for item in native.get("scenarios", [])}
        page_status = [pages.get("SS01", {}).get("status")]
        if not identity_ok:
            status, reason = "FAIL", "native result did not identify unchanged staged binaries"
        elif "FAIL" in page_status or native.get("verdict") == "FAIL" or completed.returncode != 0:
            status, reason = "FAIL", "installed-prefix native page launch failed; see native result"
        elif "ENVIRONMENT_BLOCKED" in page_status:
            status, reason = "ENVIRONMENT_BLOCKED", "native page launch blocked by a reported environment capability"
        elif "NOT_RUN" in page_status or "PASS" not in page_status:
            status, reason = "NOT_RUN", "installed-prefix page launch did not reach PASS; see native result"
        else:
            status, reason = "PASS", "staged version/assets verified and staged binaries opened all Settings pages in private native session"
    # The overall verdict is the native verdict once the installed launch gate passes; any
    # non-PASS gate (FAIL, NOT_RUN, ENVIRONMENT_BLOCKED) stays the overall verdict.
    verdict = native.get("verdict", "INCOMPLETE") if status == "PASS" else status
    document = {
        "schema": "flamewm-settings-installed-acceptance-v1",
        "verdict": verdict,
        "launch_gate": status,
        "version": version,
        "prefix": str(prefix),
        "installed_files_sha256": hashes,
        "binary_identity": {
            "wm": str(prefix / "usr/bin/flamewm"),
            "settings": str(prefix / "usr/bin/flamewm-settings"),
        },
        "isolated_working_directory": str(acceptance_dir),
        "native_result": str(native_file) if native_file.exists() else None,
        "native_verdict": native.get("verdict") if native else None,
        "reason": reason,
        "command": command,
        "runner_exit_code": completed.returncode,
        "runner_stdout": completed.stdout,
        "runner_stderr": completed.stderr,
    }
    (run_dir / "installed-result.json").write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
    return document, 0 if status == "PASS" else (1 if status == "FAIL" else 3)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        document, code = run(args)
    except (OSError, ValueError, subprocess.SubprocessError, json.JSONDecodeError) as error:
        print(f"ERROR installed acceptance: {error}", file=sys.stderr)
        return 2
    print(json.dumps(document, indent=2))
    return code


if __name__ == "__main__":
    raise SystemExit(main())

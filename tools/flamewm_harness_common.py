"""Shared helpers for FlameWM acceptance harnesses (stdlib only).

Imported as a sibling module; harnesses and their tests run as `python3 tools/<name>.py`,
which puts this directory on sys.path.
"""

from __future__ import annotations

import hashlib
import json
import subprocess
from pathlib import Path

X11_SOCKET_DIR = Path("/tmp/.X11-unix")


def write_json(path: Path, doc) -> None:
    path.write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def display_in_use(number: str) -> bool:
    return (X11_SOCKET_DIR / f"X{number}").exists() or Path(f"/tmp/.X{number}-lock").exists()


def stop_process(proc: subprocess.Popen, grace: float = 5) -> None:
    if proc.poll() is not None:
        return
    proc.terminate()
    try:
        proc.wait(timeout=grace)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait(timeout=grace)

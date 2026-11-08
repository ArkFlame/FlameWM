"""Shared helpers for FlameWM acceptance harnesses (stdlib only).

Imported as a sibling module; harnesses and their tests run as `python3 tools/<name>.py`,
which puts this directory on sys.path.
"""

from __future__ import annotations

import hashlib
import json
import os
import socket
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


def _lock_owner_alive(lock: Path) -> bool:
    try:
        pid = int(lock.read_text(encoding="ascii").strip())
    except (OSError, ValueError):
        return False
    try:
        os.kill(pid, 0)
    except PermissionError:
        return True
    except OSError:
        return False
    return True


def _socket_accepts(socket_path: Path) -> bool:
    probe = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    probe.settimeout(0.5)
    try:
        probe.connect(str(socket_path))
    except OSError:
        return False
    finally:
        probe.close()
    return True


def display_in_use(number: str) -> bool:
    """A display is busy only while its lock owner is alive or its socket accepts connections.

    Stale /tmp/.X<n>-lock and socket files left by dead servers must not block a display.
    """
    lock = Path(f"/tmp/.X{number}-lock")
    socket_path = X11_SOCKET_DIR / f"X{number}"
    if lock.exists() and _lock_owner_alive(lock):
        return True
    return socket_path.exists() and _socket_accepts(socket_path)


def stop_process(proc: subprocess.Popen, grace: float = 5) -> None:
    if proc.poll() is not None:
        return
    proc.terminate()
    try:
        proc.wait(timeout=grace)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait(timeout=grace)

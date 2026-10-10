"""Shared implementation for bounded run-directory retention."""
from __future__ import annotations

import os
import re
import time
from pathlib import Path

RUN_RE = re.compile(r"^\d{8}T\d{6}Z-\d+$")
RETENTION_FILE = "retention.txt"


def env_int(name: str, default: int) -> int:
    try:
        return max(0, int(os.environ.get(name, str(default))))
    except ValueError:
        return default


def latest_target(directory: Path) -> str:
    link = directory / "latest"
    try:
        if link.is_symlink():
            target = os.readlink(link)
            resolved = ((directory / target).resolve()
                        if not os.path.isabs(target) else Path(target))
            try:
                name = resolved.relative_to(directory.resolve()).parts[0]
            except ValueError:
                return resolved.name
            return name
    except OSError:
        pass
    return ""


def retain(directory: Path, current: str, keep_runs: int,
           max_age_days: int) -> tuple[list[str], list[str], list[str]]:
    """Return (kept, deleted, protected) run names. Performs deletion."""
    latest = latest_target(directory)
    try:
        directory_real = directory.resolve()
    except OSError:
        return [], [], []
    entries: list[tuple[str, float]] = []
    protected: list[str] = []
    for child in sorted(directory.iterdir(), key=lambda p: p.name):
        name = child.name
        if not RUN_RE.match(name):
            continue
        if child.is_symlink() or not child.is_dir():
            protected.append(name)
            continue
        try:
            if (child.resolve() != directory_real / name
                    or directory_real not in child.resolve().parents):
                protected.append(name)
                continue
        except OSError:
            protected.append(name)
            continue
        if name == current or name == latest or (child / ".keep").exists():
            protected.append(name)
            continue
        try:
            mtime = child.stat().st_mtime
        except OSError:
            protected.append(name)
            continue
        entries.append((name, mtime))
    entries.sort(key=lambda e: e[1], reverse=True)
    now = time.time()
    max_age_secs = max_age_days * 86400
    current_counts_toward_floor = bool(
        current and RUN_RE.match(current)
        and (directory / current).is_dir()
        and not (directory / current).is_symlink()
        and not (directory / current / ".keep").exists()
    )
    historical_slots = max(0, keep_runs - int(current_counts_toward_floor))
    kept = [name for name, _ in entries[:historical_slots]]
    deleted: list[str] = []
    for name, mtime in entries[historical_slots:]:
        if now - mtime < max_age_secs:
            kept.append(name)
            continue
        target = directory / name
        try:
            if target.is_symlink() or not target.is_dir():
                protected.append(name)
                continue
            if target.resolve() != directory_real / name:
                protected.append(name)
                continue
            import shutil
            shutil.rmtree(target)
            deleted.append(name)
        except OSError:
            protected.append(name)
    return kept, deleted, sorted(set(protected))


def write_retention(directory: Path, kept, deleted, protected, keep_runs,
                    max_age_days, current: str = "") -> None:
    lines = [
        f"keep_runs={keep_runs}",
        f"max_age_days={max_age_days}",
        f"kept={' '.join(sorted(set(kept)))}",
        f"deleted={' '.join(sorted(deleted))}",
        f"protected={' '.join(sorted(protected))}",
    ]
    text = "\n".join(lines) + "\n"
    targets: list[Path] = []
    if current and RUN_RE.match(current):
        targets.append(directory / current / RETENTION_FILE)
    targets.append(directory / RETENTION_FILE)
    for target in targets:
        try:
            target.write_text(text)
        except OSError:
            pass

#!/usr/bin/env python3
"""Retain bounded .performance evidence runs.

Policy (env overridable):
  FLAMEWM_PERFORMANCE_CLEANUP=1   (default on; anything else disables)
  FLAMEWM_PERFORMANCE_KEEP_RUNS=1 (keep newest N matching runs)
  FLAMEWM_PERFORMANCE_MAX_AGE_DAYS=0 (delete matching runs older than this,
                                  but never below the KEEP_RUNS floor)

Only directories directly under the perf dir whose name matches
^\\d{8}T\\d{6}Z-\\d+$ are candidates. Everything else is protected:
  - files/symlinks (e.g. `latest`) are never deleted
  - names not matching the regex are never deleted
  - runs containing a `.keep` file are pinned and never deleted
  - the run targeted by the `latest` symlink is never deleted
  - the current run (--current RUN_ID or FLAMEWM_CURRENT_RUN) is never deleted
  - candidates resolving outside the perf dir are never deleted

Writes `retention.txt` into `.performance/<current-run>/retention.txt`
on every active run (falls back to the perf dir when current is unknown).
"""
from __future__ import annotations

import argparse
import os
import sys
import time
from pathlib import Path

if __package__:
    from . import retention_core
else:
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    try:
        import retention_core
    finally:
        sys.path.pop(0)

RUN_RE = retention_core.RUN_RE
RETENTION_FILE = retention_core.RETENTION_FILE


def _env_int(name: str, default: int) -> int:
    return retention_core.env_int(name, default)


def retain(perf: Path, current: str = "", keep_runs: int = 12,
           max_age_days: int = 7) -> tuple[list[str], list[str], list[str]]:
    return retention_core.retain(perf, current, keep_runs, max_age_days)


def write_retention(perf: Path, kept, deleted, protected, keep_runs, max_age_days,
                      current: str = "") -> None:
    retention_core.write_retention(perf, kept, deleted, protected, keep_runs,
                                   max_age_days, current)


def self_test() -> int:
    import shutil
    import tempfile
    tmp = Path(tempfile.mkdtemp(prefix="flamewm-retention-test-"))
    try:
        perf = tmp / "perf"
        perf.mkdir()
        old = time.time() - 10 * 86400
        now = time.time()

        def mk(name, mtime=None):
            d = perf / name
            d.mkdir()
            (d / "x.txt").write_text("x")
            if mtime is not None:
                os.utime(d, (mtime, mtime))
            return d

        mk("20200101T000000Z-1", old)
        mk("20200102T000000Z-2", old)
        pinned = mk("20200103T000000Z-3", old)
        (pinned / ".keep").write_text("pin")
        cur = mk("20200104T000000Z-4", old)
        latest_run = mk("20200105T000000Z-5", old)
        (perf / "latest").symlink_to(latest_run.name)
        (perf / "notes.txt").write_text("keep me")
        (perf / "latest-file-link").symlink_to("notes.txt")
        (perf / "weird-name").mkdir()
        fresh = mk("20200106T000000Z-6", now)
        esc = tmp / "escape"
        esc.mkdir()
        (perf / "20200107T000000Z-7").symlink_to(str(esc))

        kept, deleted, protected = retain(perf, current=cur.name,
                                          keep_runs=1, max_age_days=7)
        assert (perf / "20200101T000000Z-1").exists() is False, "oldest must go"
        assert (perf / "20200102T000000Z-2").exists() is False, "second oldest must go"
        assert pinned.exists(), "pinned .keep must survive"
        assert cur.exists(), "current must survive"
        assert latest_run.exists(), "latest target must survive"
        assert (perf / "notes.txt").exists(), "nonmatching file must survive"
        assert (perf / "weird-name").exists(), "nonmatching dir must survive"
        assert fresh.exists(), "fresh run within max age must survive"
        assert esc.exists() and (perf / "20200107T000000Z-7").is_symlink(), \
            "symlink must survive"
        assert "20200101T000000Z-1" in deleted
        write_retention(perf, kept, deleted, protected, 1, 7)
        assert (perf / RETENTION_FILE).is_file(), "retention.txt must be written"
        # Idempotence: second pass deletes nothing new.
        _, deleted2, _ = retain(perf, current=cur.name, keep_runs=1, max_age_days=7)
        assert not (set(deleted2) - set(deleted)), "second pass must not delete more"

        # KEEP_RUNS includes current, not one extra historical unpinned run.
        exact = tmp / "exact"
        exact.mkdir()
        def exact_mk(name):
            d = exact / name
            d.mkdir()
            (d / "x.txt").write_text("x")
            os.utime(d, (old, old))
            return d

        old_run = exact_mk("20200101T000000Z-1")
        current_run = exact_mk("20200102T000000Z-2")
        pin_one = exact_mk("20200103T000000Z-3")
        pin_two = exact_mk("20200104T000000Z-4")
        (pin_one / ".keep").write_text("pin")
        (pin_two / ".keep").write_text("pin")
        retain(exact, current=current_run.name, keep_runs=1, max_age_days=0)
        assert not old_run.exists(), "current must consume KEEP_RUNS slot"
        assert current_run.exists() and pin_one.exists() and pin_two.exists(), \
            "only current and pinned runs must survive"
        print("OK performance_retention self-test")
        return 0
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description="Bound .performance evidence retention.")
    ap.add_argument("perf_dir", nargs="?", default=".performance")
    ap.add_argument("--current", default=os.environ.get("FLAMEWM_CURRENT_RUN", ""))
    ap.add_argument("--self-test", action="store_true")
    args = ap.parse_args(argv)
    if args.self_test:
        return self_test()
    if os.environ.get("FLAMEWM_PERFORMANCE_CLEANUP", "1") != "1":
        return 0
    perf = Path(args.perf_dir)
    if not perf.is_dir():
        return 0
    keep_runs = _env_int("FLAMEWM_PERFORMANCE_KEEP_RUNS", 1)
    max_age_days = _env_int("FLAMEWM_PERFORMANCE_MAX_AGE_DAYS", 0)
    kept, deleted, protected = retain(perf, current=args.current,
                                      keep_runs=keep_runs,
                                      max_age_days=max_age_days)
    write_retention(perf, kept, deleted, protected, keep_runs, max_age_days,
                    args.current)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

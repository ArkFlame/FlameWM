#!/usr/bin/env python3
"""Summarize present-interval lines from FLAMEWM_PROFILE=1 logs.

Parses `$FLAMEWM_PROFILE_DIR/<process>.profile.log` files written by
crates/flamewm-profiler/src/report.rs. Each window with presents contains:

    PROCESS name=<name> cpu_total=... window=<ms>
    PRESENT_INTERVAL:
     present-interval count=<n> p50=<ms> p95=<ms> max=<ms>

Per-window values are the window's own p50/p95/max. Per-process values are
nearest-rank percentiles over the window-level p50/p95 values, the max of
window maxima, and the summed count. Exact cross-window percentiles cannot be
recovered from summaries, so the per-process line is an approximation.

Stdlib only. Does not touch any display.
"""

from __future__ import annotations

import argparse
import math
import re
import sys
from pathlib import Path

PROCESS_RE = re.compile(r"^PROCESS name=(\S+)\b")
WINDOW_START = "FLAMEWM_PROFILE_SUMMARY"
PRESENT_RE = re.compile(
    r"^\s*present-interval count=(\d+) p50=([\d.]+)ms p95=([\d.]+)ms max=([\d.]+)ms\s*$"
)


def parse_log(text: str, process: str) -> list[dict]:
    """Return one record per window that carried a present-interval line.

    Each record: process, window (1-based index among all windows in the log),
    count, p50_ms, p95_ms, max_ms. Windows without the line are skipped.
    """
    records: list[dict] = []
    window_no = 0
    current_process = process
    for line in text.splitlines():
        if line.startswith(WINDOW_START):
            window_no += 1
            continue
        proc = PROCESS_RE.match(line)
        if proc:
            current_process = proc.group(1)
            continue
        match = PRESENT_RE.match(line)
        if match and window_no:
            count, p50, p95, mx = match.groups()
            records.append(
                {
                    "process": current_process,
                    "window": window_no,
                    "count": int(count),
                    "p50_ms": float(p50),
                    "p95_ms": float(p95),
                    "max_ms": float(mx),
                }
            )
    return records


def nearest_rank(values: list[float], pct: float) -> float:
    ordered = sorted(values)
    index = max(0, math.ceil(pct / 100.0 * len(ordered)) - 1)
    return ordered[index]


def summarize_process(records: list[dict]) -> dict:
    return {
        "windows": len(records),
        "count": sum(r["count"] for r in records),
        "p50_ms": nearest_rank([r["p50_ms"] for r in records], 50),
        "p95_ms": nearest_rank([r["p95_ms"] for r in records], 95),
        "max_ms": max(r["max_ms"] for r in records),
    }


def load_logs(log_dir: Path) -> list[dict]:
    records: list[dict] = []
    for path in sorted(log_dir.glob("*.profile.log")):
        records.extend(parse_log(path.read_text(encoding="utf-8", errors="replace"), path.name.split(".")[0]))
    return records


def format_report(records: list[dict]) -> str:
    if not records:
        return "no present-interval lines found\n"
    lines = []
    by_process: dict[str, list[dict]] = {}
    for rec in records:
        by_process.setdefault(rec["process"], []).append(rec)
    lines.append("PROCESS SUMMARY (p50/p95 approximated across window summaries)")
    for name in sorted(by_process):
        s = summarize_process(by_process[name])
        lines.append(
            f"  {name} windows={s['windows']} count={s['count']} "
            f"p50={s['p50_ms']:.2f}ms p95={s['p95_ms']:.2f}ms max={s['max_ms']:.2f}ms"
        )
    lines.append("WINDOWS")
    for name in sorted(by_process):
        for rec in by_process[name]:
            lines.append(
                f"  {name} window={rec['window']} count={rec['count']} "
                f"p50={rec['p50_ms']:.2f}ms p95={rec['p95_ms']:.2f}ms max={rec['max_ms']:.2f}ms"
            )
    return "\n".join(lines) + "\n"


ORCHESTRATE_STEPS = """\
Orchestration (documented only; nothing is executed by this script):

1. Pick a private nested display in :97..:119 and start the nested Xephyr
   harness that writes the owner token and owner marker for that display.
2. Start the WM on that display with profiling enabled:
     FLAMEWM_PROFILE=1 FLAMEWM_PROFILE_INTERVAL=10 \\
     FLAMEWM_PROFILE_DIR=<private-dir> DISPLAY=<display> <flamewm binary>
   Logs appear at <private-dir>/<process>.profile.log.
3. Drive a drag over a window with the xtest helper (owner token and marker
   come from step 1):
     python3 tools/flamewm_xtest.py --display <display> \\
       --owner-token <token> --owner-marker <marker> \\
       --drag X1,Y1,X2,Y2:STEPS:HOLD_MS
4. Wait for at least one full report interval, then stop the WM.
5. Summarize the logs:
     python3 -I tools/flamewm_frame_interval.py <private-dir>
"""


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("log_dir", nargs="?", type=Path, help="directory containing *.profile.log files")
    parser.add_argument("--orchestrate", action="store_true", help="print the manual run procedure and exit")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    if args.orchestrate:
        sys.stdout.write(ORCHESTRATE_STEPS)
        return 0
    if args.log_dir is None:
        print("error: log_dir is required unless --orchestrate is given", file=sys.stderr)
        return 2
    if not args.log_dir.is_dir():
        print(f"error: not a directory: {args.log_dir}", file=sys.stderr)
        return 2
    sys.stdout.write(format_report(load_logs(args.log_dir)))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

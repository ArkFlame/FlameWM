#!/usr/bin/env python3
"""10-second, read-only FlameWM /proc/PID/smaps ownership comparison.

Reports per-process and per-mapping PSS/USS/RSS, with baseline, midpoint and end.
No root access, daemon, injected code, or workload automation. This is mapping
attribution, NOT a Rust allocation call-stack profiler or XRes pixmap ledger.
"""
from __future__ import annotations

import argparse
from collections import defaultdict
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import time

NAMES = frozenset(("flamewm", "flamewm-shell", "flamewm-desktop", "flamewm-settings",
                   "flamewm-terminal", "flamewm-explorer", "flamewm-task-manager",
                   "flamewm-wayland", "flamewm-screenshot", "flamewm-lock"))
HEADER = re.compile(r"^[0-9a-fA-F]+-[0-9a-fA-F]+\s+([rwxps-]{4})\s+[0-9a-fA-F]+\s+\S+\s+\d+\s*(.*)$")
METRICS = ("Pss", "Rss", "Private_Clean", "Private_Dirty", "Swap", "Anonymous")


def classify(path: str, executable: str) -> str:
    if path == "[heap]":
        return "heap"
    if path.startswith("[stack"):
        return "stack"
    if path.startswith("/dev/shm") or path.startswith("/SYSV") or "memfd:" in path:
        return "shared-memory"
    if not path or path.startswith("[anon"):
        return "anonymous-mappings"
    if path == executable or path == executable + " (deleted)":
        return "executable"
    if path.startswith("/usr/") or path.startswith("/lib/"):
        if ".so" in path or "/lib" in path:
            return "shared-libraries"
        return "other-files"
    if path.startswith("["):
        return "kernel-special"
    return "other-files"


def parse_smaps(text: str, executable: str) -> dict[str, dict[str, int]]:
    groups: dict[str, dict[str, int]] = defaultdict(lambda: defaultdict(int))
    category = None
    for line in text.splitlines():
        match = HEADER.match(line)
        if match:
            name = match.group(2).strip()
            # Keep both broad category and mapping pathname for drilldown.
            category = (classify(name, executable), name or "[anonymous]")
            continue
        if category is None:
            continue
        key, sep, val = line.partition(":")
        if sep and key in METRICS:
            parts = val.split()
            if len(parts) == 2 and parts[1] == "kB" and parts[0].isdigit():
                groups[category][key] += int(parts[0])
    return {f"{kind} | {name}": dict(metrics) for (kind, name), metrics in groups.items()}


def discover(proc: Path, only_pids: set[int] | None) -> list[tuple[int, str, str]]:
    result = []
    for child in proc.iterdir():
        if not child.name.isdigit():
            continue
        pid = int(child.name)
        if only_pids is not None and pid not in only_pids:
            continue
        try:
            if child.stat().st_uid != os.getuid():
                continue
            comm = (child / "comm").read_text().strip()
            # Linux comm is truncated at 15 bytes (e.g. flamewm-task-manager).
            # Trust the full /proc/PID/exe basename instead of that truncated name.
            exe = os.readlink(child / "exe").removesuffix(" (deleted)")
            # Prevent unrelated same-named processes from being counted.
            if only_pids is None and Path(exe).name not in NAMES:
                continue
            result.append((pid, comm, exe))
        except (OSError, ValueError):
            continue
    return sorted(result)


def snapshot(proc: Path, only_pids: set[int] | None) -> dict:
    rows = {}
    for pid, comm, exe in discover(proc, only_pids):
        try:
            stat = (proc / str(pid) / "stat").read_text()
            start = stat.rsplit(")", 1)[-1].split()[19]
            maps = parse_smaps((proc / str(pid) / "smaps").read_text(), exe)
            totals = {m: sum(v.get(m, 0) for v in maps.values()) for m in METRICS}
            totals["USS"] = totals["Private_Clean"] + totals["Private_Dirty"]
            rows[f"{pid}:{start}"] = {"pid": pid, "comm": comm, "exe": exe,
                                      "totals_kib": totals, "maps_kib": maps}
        except (OSError, ValueError, IndexError):
            continue
    return rows


def summarize(captures: list[dict], top: int) -> None:
    before, after = captures[0], captures[-1]
    ids = set(before) | set(after)
    total_start = sum(p["totals_kib"]["Pss"] for p in before.values())
    total_end = sum(p["totals_kib"]["Pss"] for p in after.values())
    print(f"FlameWM mapping memory: {total_start / 1024:.2f} -> {total_end / 1024:.2f} MiB PSS"
          f" ({(total_end-total_start)/1024:+.2f} MiB); excludes Xorg/XRes server pixmaps")
    for identity in sorted(ids, key=lambda key: after.get(key, before.get(key))["totals_kib"]["Pss"], reverse=True):
        end = after.get(identity)
        old = before.get(identity)
        entry = end or old
        a = old["totals_kib"]["Pss"] if old else 0
        b = end["totals_kib"]["Pss"] if end else 0
        print(f"\n{entry['comm']} PID {entry['pid']} PSS {a/1024:.2f} -> {b/1024:.2f} MiB"
              f" ({(b-a)/1024:+.2f}), USS {((end or old)['totals_kib']['USS'])/1024:.2f} MiB")
        oldmaps = old["maps_kib"] if old else {}
        newmaps = end["maps_kib"] if end else {}
        category = defaultdict(lambda: [0, 0])
        for key in set(oldmaps) | set(newmaps):
            kind = key.split(" | ", 1)[0]
            category[kind][0] += oldmaps.get(key, {}).get("Pss", 0)
            category[kind][1] += newmaps.get(key, {}).get("Pss", 0)
        for kind, (v0, v1) in sorted(category.items(), key=lambda pair: pair[1][1], reverse=True):
            print(f"  {kind:22s} {v1/1024:7.2f} MiB   delta {(v1-v0)/1024:+7.2f}")
        print("  Largest mapped objects (end PSS):")
        for name, metrics in sorted(newmaps.items(), key=lambda pair: pair[1].get("Pss", 0), reverse=True)[:top]:
            print(f"    {metrics.get('Pss', 0)/1024:7.2f} MiB  {name[:95]}")
    if not ids:
        print("NO FLAMEWM PROCESSES FOUND. Run inside the active FlameWM session or use --pid PID.")
    print("\nNOTE: /proc/smaps locates mapped memory, not Rust allocation call sites."
          " For call stacks use heaptrack on a test session; avoid runtime attach to production.")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seconds", type=float, default=10.0)
    parser.add_argument("--pid", type=int, action="append", help="specific PID (repeatable)")
    parser.add_argument("--top", type=int, default=8)
    parser.add_argument("--output", type=Path, help="JSON report location")
    parser.add_argument("--proc", type=Path, default=Path("/proc"), help=argparse.SUPPRESS)
    args = parser.parse_args(argv)
    if not (0 < args.seconds <= 120) or not (1 <= args.top <= 100):
        parser.error("--seconds must be between 0 and 120; --top between 1 and 100")
    output = args.output or Path(f"flamewm-memory-owners-{datetime.now(timezone.utc):%Y%m%dT%H%M%SZ}.json")
    pids = set(args.pid) if args.pid else None
    start = time.monotonic()
    snaps = []
    for step in range(3):
        deadline = start + args.seconds * step / 2
        delay = deadline - time.monotonic()
        if delay > 0:
            time.sleep(delay)
        snaps.append(snapshot(args.proc, pids))
    report = {"schema": "flamewm-memory-map-owners/1", "duration_seconds": args.seconds,
              "sampling": "start/mid/end", "units": "KiB", "limitations":
              "Mapping PSS is not Rust allocation site or X11 server pixmap ownership",
              "captures": snaps}
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n")
    summarize(snaps, args.top)
    print(f"\nReport: {output}")
    return 0 if any(snaps) else 2


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Sample memory of explicitly named processes from /proc (stdlib only).

Usage:
  python3 -I tools/flamewm_mem_sample.py --pid wm=1234 --pid shell=5678 \
      [--samples N] [--interval SECONDS]

Each sample round reads every named PID at one instant (threads released
together by a barrier). Output is JSON on stdout with median/p95 per metric,
plus a combined (sum over all named PIDs) RSS/PSS line in MiB and bytes,
compared against the 34 MiB (35651584 B) target.
"""

import argparse
import json
import math
import os
import statistics
import sys
import threading
import time

TARGET_MIB = 34
TARGET_BYTES = TARGET_MIB * 1024 * 1024  # 35651584

SMAPS_KEYS = {
    "Rss": "rss",
    "Pss": "pss",
    "Private_Dirty": "private_dirty",
    "Private_Clean": "private_clean",
}
MEM_METRICS = ("rss", "pss", "private_dirty", "private_clean", "vmhwm")
ALL_METRICS = MEM_METRICS + ("threads",)


def parse_smaps_rollup(text):
    """Return {rss, pss, private_dirty, private_clean} in bytes."""
    out = {}
    for line in text.splitlines():
        key, sep, rest = line.partition(":")
        if not sep or key not in SMAPS_KEYS:
            continue
        fields = rest.split()
        if len(fields) < 1 or not fields[0].isdigit():
            raise ValueError(f"bad smaps_rollup line: {line!r}")
        unit = fields[1] if len(fields) > 1 else "kB"
        if unit != "kB":
            raise ValueError(f"unexpected unit in smaps_rollup: {line!r}")
        out[SMAPS_KEYS[key]] = int(fields[0]) * 1024
    missing = sorted(set(SMAPS_KEYS.values()) - set(out))
    if missing:
        raise ValueError(f"smaps_rollup missing fields: {missing}")
    return out


def parse_status(text):
    """Return {vmhwm (bytes), threads (count)} from /proc/<pid>/status."""
    out = {}
    for line in text.splitlines():
        key, sep, rest = line.partition(":")
        if not sep:
            continue
        fields = rest.split()
        if key == "VmHWM" and fields and fields[0].isdigit():
            out["vmhwm"] = int(fields[0]) * 1024
        elif key == "Threads" and fields and fields[0].isdigit():
            out["threads"] = int(fields[0])
    missing = sorted({"vmhwm", "threads"} - set(out))
    if missing:
        raise ValueError(f"status missing fields: {missing}")
    return out


def parse_pid_arg(value):
    """Parse NAME=PID into (name, pid)."""
    name, sep, pid_text = value.partition("=")
    if not sep or not name or not pid_text.isdigit() or int(pid_text) <= 0:
        raise argparse.ArgumentTypeError(f"expected NAME=PID, got {value!r}")
    return name, int(pid_text)


def percentile95(values):
    """Nearest-rank 95th percentile."""
    ordered = sorted(values)
    rank = math.ceil(0.95 * len(ordered))
    return ordered[max(rank, 1) - 1]


def summarize(values):
    return {
        "median": statistics.median(values),
        "p95": percentile95(values),
    }


def read_pid(pid):
    """Read all metrics for one PID. Raises OSError/ValueError on failure."""
    base = f"/proc/{pid}"
    with open(f"{base}/smaps_rollup", encoding="ascii") as f:
        sample = parse_smaps_rollup(f.read())
    with open(f"{base}/status", encoding="ascii") as f:
        sample.update(parse_status(f.read()))
    with open(f"{base}/comm", encoding="ascii") as f:
        comm = f.read().strip()
    try:
        exe = os.readlink(f"{base}/exe")
    except OSError:
        exe = None
    sample["comm"] = comm
    sample["exe"] = exe
    return sample


def sample_round(pids):
    """Read every PID at (as near as possible) the same instant."""
    barrier = threading.Barrier(len(pids))
    results = {}
    errors = {}

    def worker(name, pid):
        try:
            barrier.wait()
            results[name] = read_pid(pid)
        except (OSError, ValueError) as exc:
            errors[name] = f"{pid}: {exc}"

    threads = [
        threading.Thread(target=worker, args=(name, pid))
        for name, pid in pids
    ]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    if errors:
        raise RuntimeError("; ".join(f"{n}: {e}" for n, e in sorted(errors.items())))
    return results


def mib(value):
    return round(value / (1024 * 1024), 3)


def build_report(pids, rounds, samples, interval):
    per_process = {}
    for name, pid in pids:
        series = {m: [r[name][m] for r in rounds] for m in ALL_METRICS}
        per_process[name] = {
            "pid": pid,
            "comm": rounds[-1][name]["comm"],
            "exe": rounds[-1][name]["exe"],
            "metrics": {m: summarize(series[m]) for m in ALL_METRICS},
        }

    combined = {}
    for metric in ("rss", "pss"):
        totals = [sum(r[n][metric] for n, _ in pids) for r in rounds]
        stats = summarize(totals)
        combined[metric] = {
            "median_bytes": stats["median"],
            "median_mib": mib(stats["median"]),
            "p95_bytes": stats["p95"],
            "p95_mib": mib(stats["p95"]),
            "target_bytes": TARGET_BYTES,
            "target_mib": TARGET_MIB,
            "median_reached": stats["median"] <= TARGET_BYTES,
            "p95_reached": stats["p95"] >= TARGET_BYTES,
        }

    return {
        "samples": samples,
        "interval_s": interval,
        "processes": per_process,
        "combined": combined,
    }


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument(
        "--pid", action="append", type=parse_pid_arg, required=True,
        metavar="NAME=PID", help="named process to sample (repeatable)",
    )
    parser.add_argument("--samples", type=int, default=1)
    parser.add_argument("--interval", type=float, default=1.0)
    args = parser.parse_args(argv)
    if args.samples < 1:
        parser.error("--samples must be >= 1")
    if args.interval < 0:
        parser.error("--interval must be >= 0")
    names = [n for n, _ in args.pid]
    if len(set(names)) != len(names):
        parser.error("duplicate --pid NAME")

    rounds = []
    for i in range(args.samples):
        try:
            rounds.append(sample_round(args.pid))
        except RuntimeError as exc:
            print(f"sample failed: {exc}", file=sys.stderr)
            return 2
        if i + 1 < args.samples and args.interval > 0:
            time.sleep(args.interval)

    report = build_report(args.pid, rounds, args.samples, args.interval)
    json.dump(report, sys.stdout, indent=2, sort_keys=True)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())

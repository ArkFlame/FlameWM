#!/usr/bin/env python3
"""Sample FlameWM-owned processes over repeatable workload phases."""
from __future__ import annotations

import argparse
import csv
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
from datetime import datetime, timezone

ROOT = Path(__file__).resolve().parents[1]
INTERVAL_MIN_MS = 200
INTERVAL_MAX_MS = 500
ROLLUP_METRICS = ("Pss", "Rss", "Private_Dirty", "Private_Clean", "Anonymous", "Swap")
METRICS = (*ROLLUP_METRICS, "USS")
PROCESS_NAMES = {
    "flamewm": "wm",
    "flamewm-session": "session",
    "flamewm-shell": "shell",
    "flamewm-desktop": "desktop",
    "flamewm-settings": "settings",
    "flamewm-lock": "lock",
}
HELPER_ARGS = {"flamewm-desktop-services", "flamewm_desktop_services.py"}
PHASES = {"startup", "idle", "transient", "steady", "shutdown"}
WORKLOADS = {
    "idle": "60 s settled idle",
    "start-close-100": "Start and close 100 times",
    "context-menus-100": "Open and close context menus 100 times",
    "taskbar-1-display": "Taskbar with one display",
    "taskbar-2-displays": "Taskbar with two displays",
    "theme-accent": "Change theme and accent",
    "icons-250": "Show 250 diverse icons and exercise cache reuse",
    "minimize-8": "Minimize and restore eight 1080p clients",
    "wallpaper-1080p-4k": "Rapidly switch 1080p and 4K wallpapers",
    "screenshot-edit": "Capture and edit a screenshot region",
    "shutdown": "Normal FlameWM shutdown",
}
ENV_KEYS = (
    "DISPLAY", "XDG_SESSION_TYPE", "XDG_CURRENT_DESKTOP", "XDG_MENU_PREFIX",
    "XDG_CONFIG_HOME", "XDG_CONFIG_DIRS", "XDG_DATA_HOME", "XDG_DATA_DIRS",
    "XDG_CACHE_HOME", "FLAMEWM_CONFIG_DIR", "FLAMEWM_PROFILE_DIR",
    "FLAMEWM_PROFILE_MODE", "FLAMEWM_PROFILE_INTERVAL", "FLAMEWM_PROFILE_TOP",
    "MALLOC_ARENA_MAX", "MALLOC_TRIM_THRESHOLD_", "RWR_UI_FONT",
    "XCURSOR_THEME", "XCURSOR_SIZE", "XCURSOR_PATH",
)


def parse_phase(value: str) -> tuple[str, float]:
    name, sep, raw_seconds = value.partition("=")
    try:
        seconds = float(raw_seconds)
    except ValueError as exc:
        raise argparse.ArgumentTypeError("expected PHASE=SECONDS") from exc
    if not sep or name not in PHASES or seconds <= 0:
        raise argparse.ArgumentTypeError(
            f"expected PHASE=positive-seconds, phase in {', '.join(sorted(PHASES))}"
        )
    return name, seconds


def parse_smaps_rollup(text: str) -> dict[str, int]:
    values: dict[str, int] = {}
    for line in text.splitlines():
        key, sep, rest = line.partition(":")
        if sep and key in ROLLUP_METRICS:
            fields = rest.split()
            if len(fields) < 2 or fields[1] != "kB" or not fields[0].isdigit():
                raise ValueError(f"invalid {key} field")
            values[key] = int(fields[0])
    missing = sorted(set(ROLLUP_METRICS) - values.keys())
    if missing:
        raise ValueError(f"smaps_rollup missing: {', '.join(missing)}")
    values["USS"] = values["Private_Dirty"] + values["Private_Clean"]
    return values


def parse_status(text: str) -> dict[str, int | None]:
    keys = {
        "Threads": "threads",
        "voluntary_ctxt_switches": "voluntary_ctxt_switches",
        "nonvoluntary_ctxt_switches": "nonvoluntary_ctxt_switches",
    }
    result: dict[str, int | None] = {value: None for value in keys.values()}
    for line in text.splitlines():
        key, sep, raw = line.partition(":")
        if sep and key in keys:
            fields = raw.split()
            if fields and fields[0].isdigit():
                result[keys[key]] = int(fields[0])
    return result


def parse_proc_stat(text: str) -> tuple[int, int]:
    """Return parent PID and start-time ticks despite parentheses in comm."""
    tail = text.rsplit(")", 1)[-1].split()
    if len(tail) <= 19 or not tail[1].isdigit() or not tail[19].isdigit():
        raise ValueError("malformed proc stat")
    return int(tail[1]), int(tail[19])


def _read_text(path: Path) -> str:
    return path.read_text(encoding="ascii", errors="replace")


def scan_processes(proc_root: Path, uid: int | None = None) -> dict[int, dict]:
    """Discover exact FlameWM executables and their same-UID descendants."""
    uid = os.getuid() if uid is None else uid
    processes: dict[int, dict] = {}
    for entry in proc_root.iterdir():
        if not entry.name.isdigit():
            continue
        pid = int(entry.name)
        try:
            parent, start_ticks = parse_proc_stat(_read_text(entry / "stat"))
            status = _read_text(entry / "status")
            uid_line = next(line for line in status.splitlines() if line.startswith("Uid:"))
            real_uid = int(uid_line.split()[1])
            if real_uid != uid:
                continue
            comm = _read_text(entry / "comm").strip()
            exe = os.readlink(entry / "exe").removesuffix(" (deleted)")
            argv = (entry / "cmdline").read_bytes().decode(errors="replace").split("\0")
            argv = [arg for arg in argv if arg]
        except (OSError, StopIteration, ValueError, IndexError):
            continue
        executable = Path(exe).name
        role = PROCESS_NAMES.get(executable)
        expected_comm = executable[:15]
        if role and comm != expected_comm:
            role = None
        if (executable.startswith("python3") and comm.startswith("python3") and len(argv) > 1
                and Path(argv[1]).name in HELPER_ARGS):
            role = "helper"
        elif executable in HELPER_ARGS and comm == executable[:15]:
            role = "helper"
        processes[pid] = {
            "pid": pid,
            "ppid": parent,
            "start_ticks": start_ticks,
            "uid": real_uid,
            "comm": comm,
            "exe": exe,
            "role": role,
        }

    roots = {pid for pid, item in processes.items() if item["role"]}
    descendants: set[int] = set(roots)
    changed = True
    while changed:
        changed = False
        for pid, item in processes.items():
            if pid not in descendants and item["ppid"] in descendants:
                descendants.add(pid)
                changed = True
    owned = {pid: processes[pid] for pid in descendants}
    for pid, item in owned.items():
        if item["role"] is None:
            item["role"] = f"child:{item['comm']}"
    return owned


def read_metrics(proc_root: Path, item: dict) -> tuple[dict, str | None]:
    base = proc_root / str(item["pid"])
    try:
        before = parse_proc_stat(_read_text(base / "stat"))[1]
        if before != item["start_ticks"]:
            return {}, "pid_reused"
        metrics = parse_smaps_rollup(_read_text(base / "smaps_rollup"))
        status = parse_status(_read_text(base / "status"))
        try:
            sched = _read_text(base / "sched")
        except OSError:
            sched = ""
        for name in ("nr_wakeups", "nr_switches"):
            match = re.search(rf"^\s*{name}\s*:\s*(\d+)", sched, re.MULTILINE)
            metrics[name] = int(match.group(1)) if match else None
        after = parse_proc_stat(_read_text(base / "stat"))[1]
        if after != item["start_ticks"]:
            return {}, "pid_reused"
        metrics.update(status)
        return metrics, None
    except (OSError, ValueError, IndexError) as exc:
        return {}, f"{type(exc).__name__}:{exc}"


def sample_snapshot(proc_root: Path, uid: int | None = None) -> tuple[list[dict], dict]:
    owned = scan_processes(proc_root, uid)
    rows: list[dict] = []
    unique: set[tuple[int, int]] = set()
    for item in owned.values():
        key = (item["pid"], item["start_ticks"])
        if key in unique:
            continue
        unique.add(key)
        metrics, error = read_metrics(proc_root, item)
        rows.append({**item, **metrics, "error": error})
    totals = {metric: 0 for metric in METRICS}
    totals.update({"threads": 0, "nr_wakeups": 0, "nr_switches": 0})
    valid = [row for row in rows if row["error"] is None]
    for row in valid:
        for metric in METRICS:
            totals[metric] += row[metric]
        for metric in ("threads", "nr_wakeups", "nr_switches"):
            if row.get(metric) is not None:
                totals[metric] += row[metric]
    totals.update({"process_count": len(valid), "child_count": sum(not row["role"] in PROCESS_NAMES.values() and row["role"] != "helper" for row in valid)})
    return rows, totals


def parse_profile_gauges(profile_dir: Path | None) -> dict:
    """Read the real FlameWM MEMORY_TOP log format; retain process and source."""
    gauges: list[dict] = []
    if profile_dir is None or not profile_dir.is_dir():
        return {"status": "unavailable", "items": [], "source": None}
    for path in sorted(profile_dir.glob("*.profile.log")):
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        current_process = path.name.removesuffix(".profile.log")
        blocks = [index for index, line in enumerate(text.splitlines()) if line.strip() == "MEMORY_TOP:"]
        if not blocks:
            continue
        lines = text.splitlines()
        block = []
        for line in lines[blocks[-1] + 1:]:
            if line and not line[0].isspace():
                break
            match = re.fullmatch(r"\s+([^\s]+) current=(\d+)B peak=(\d+)B\s*", line)
            if match:
                block.append({
                    "process": current_process,
                    "label": match.group(1),
                    "current_bytes": int(match.group(2)),
                    "peak_bytes": int(match.group(3)),
                    "source": str(path),
                })
        gauges.extend(block)
    return {"status": "latest_available" if gauges else "no_gauges_found", "items": gauges,
            "source": str(profile_dir), "freshness": "profile format has no timestamp; values are from the last MEMORY_TOP block"}


def _fingerprint(path: str) -> dict:
    item = {"path": path, "sha256": None}
    try:
        digest = hashlib.sha256()
        with open(path, "rb") as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                digest.update(chunk)
        item["sha256"] = digest.hexdigest()
    except OSError:
        pass
    return item


def _revision() -> str | None:
    try:
        result = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True,
                                text=True, timeout=2, check=False)
        return result.stdout.strip() if result.returncode == 0 else None
    except (OSError, subprocess.TimeoutExpired):
        return None


def plan(args: argparse.Namespace) -> dict:
    default_phase = {"idle": "idle", "shutdown": "shutdown"}.get(args.workload, "transient")
    phases = args.phase or [(default_phase, args.duration)]
    return {
        "workload": args.workload,
        "workload_instruction": WORKLOADS[args.workload],
        "phases": [{"name": name, "duration_seconds": seconds} for name, seconds in phases],
        "interval_ms": args.interval_ms,
        "experiment": args.experiment or args.workload,
        "repetition": args.repetition,
        "operator_annotations": dict(item.split("=", 1) for item in args.annotation),
        "resolution": args.resolution,
        "environment": {key: os.environ.get(key) for key in ENV_KEYS},
        "workload_matrix": WORKLOADS,
    }


CSV_FIELDS = ["timestamp_utc", "elapsed_seconds", "phase", "workload", "sample_kind",
              "pid", "start_ticks", "ppid", "role", "comm", "exe", *METRICS,
              "threads", "voluntary_ctxt_switches", "nonvoluntary_ctxt_switches",
              "nr_wakeups", "nr_switches", "process_count", "child_count", "error"]


def run(args: argparse.Namespace) -> int:
    run_id = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ") + f"-{os.getpid()}"
    output = args.output or ROOT / ".target" / "memory" / run_id
    phases = plan(args)["phases"]
    output.parent.mkdir(parents=True, exist_ok=True)
    output.mkdir(parents=True, exist_ok=False)
    started = time.monotonic()
    latest: dict[tuple[int, int], dict] = {}
    seen_executables: set[str] = set()
    process_peaks: dict[tuple[int, int], dict] = {}
    stage_summaries = []
    global_peak = {"pss_kib": 0, "timestamp_utc": None, "phase": None, "processes": 0}
    sample_count = 0
    observed_processes: set[tuple[int, int]] = set()
    with (output / "samples.csv").open("w", newline="", encoding="utf-8") as stream:
        writer = csv.DictWriter(stream, fieldnames=CSV_FIELDS, extrasaction="ignore")
        writer.writeheader()
        for phase in phases:
            phase_start = time.monotonic()
            phase_end = phase_start + phase["duration_seconds"]
            phase_started_at = datetime.now(timezone.utc).isoformat()
            phase_count = 0
            phase_peak = 0
            while True:
                now = time.monotonic()
                if now >= phase_end and phase_count:
                    break
                timestamp = datetime.now(timezone.utc).isoformat()
                elapsed = now - started
                rows, totals = sample_snapshot(Path(args.proc_root))
                sample_count += 1
                phase_count += 1
                latest = {}
                for row in rows:
                    if row["error"] is None:
                        key = (row["pid"], row["start_ticks"])
                        latest[key] = row
                        observed_processes.add(key)
                        if row["role"] in (*PROCESS_NAMES.values(), "helper"):
                            seen_executables.add(row["exe"])
                        peak = process_peaks.setdefault(key, {"pid": row["pid"],
                            "start_ticks": row["start_ticks"], "role": row["role"],
                            "comm": row["comm"], "exe": row["exe"], "Pss": 0})
                        if row["Pss"] >= peak["Pss"]:
                            peak.update({key: row[key] for key in ("Pss", "Rss", "Private_Dirty",
                                "Private_Clean", "Anonymous", "Swap", "threads")})
                            peak["phase"] = phase["name"]
                            peak["timestamp_utc"] = timestamp
                    writer.writerow({"timestamp_utc": timestamp, "elapsed_seconds": f"{elapsed:.3f}",
                                     "phase": phase["name"], "workload": args.workload,
                                     "sample_kind": "process", **row})
                writer.writerow({"timestamp_utc": timestamp, "elapsed_seconds": f"{elapsed:.3f}",
                                 "phase": phase["name"], "workload": args.workload,
                                 "sample_kind": "aggregate", **totals})
                stream.flush()
                phase_peak = max(phase_peak, totals["Pss"])
                if totals["Pss"] >= global_peak["pss_kib"]:
                    global_peak = {"pss_kib": totals["Pss"], "timestamp_utc": timestamp,
                                   "phase": phase["name"], "processes": totals["process_count"]}
                remaining = phase_end - time.monotonic()
                if remaining <= 0:
                    break
                time.sleep(min(args.interval_ms / 1000, remaining))
            phase_finished_at = datetime.now(timezone.utc).isoformat()
            stage_summaries.append({"phase": phase["name"],
                                    "requested_duration_seconds": phase["duration_seconds"],
                                    "actual_duration_seconds": round(time.monotonic() - phase_start, 3),
                                    "started_at": phase_started_at, "finished_at": phase_finished_at,
                                    "samples": phase_count, "observed_peak_pss_kib": phase_peak})

    executables = [_fingerprint(path) for path in sorted(seen_executables)]
    total_current = {metric: sum(row[metric] for row in latest.values()) for metric in METRICS}
    total_current.update({
        "process_count": len(latest),
        "child_count": sum(row["role"].startswith("child:") for row in latest.values()),
        "threads": sum(row["threads"] or 0 for row in latest.values()),
        "nr_wakeups": sum(row["nr_wakeups"] or 0 for row in latest.values()),
        "nr_switches": sum(row["nr_switches"] or 0 for row in latest.values()),
    })
    summary = {
        "schema": "flamewm-memory-timeline/1",
        "run_id": run_id,
        "experiment": args.experiment or args.workload,
        "repetition": args.repetition,
        "operator_annotations": dict(item.split("=", 1) for item in args.annotation),
        "revision": _revision(),
        "workload": args.workload,
        "workload_instruction": WORKLOADS[args.workload],
        "environment": {**plan(args)["environment"], "resolution": args.resolution},
        "memory_units": {"Pss": "KiB", "Rss": "KiB", "USS": "KiB",
                         "USS_definition": "Private_Clean + Private_Dirty from smaps_rollup"},
        "interval_ms": args.interval_ms,
        "sample_count": sample_count,
        "observed_peak_aggregate_pss_kib": global_peak,
        "latest_unique_process_aggregate": total_current,
        "latest_processes": [
            {key: row.get(key) for key in ("pid", "start_ticks", "role", "comm", "exe", *METRICS,
                                           "threads", "nr_wakeups", "nr_switches")}
            for row in sorted(latest.values(), key=lambda item: (item["role"], item["pid"]))
        ],
        "observed_process_peaks": sorted(process_peaks.values(), key=lambda item: (item["role"], item["pid"])),
        "executable_fingerprints": executables,
        "phases": stage_summaries,
        "allocator_gauges": parse_profile_gauges(Path(args.profile_dir or os.environ.get("FLAMEWM_PROFILE_DIR", ""))
                                                   if args.profile_dir or os.environ.get("FLAMEWM_PROFILE_DIR") else None),
        "x11_server_resources": {
            "status": "unavailable",
            "reason": "No XRes or external X11 resource ledger interface is configured; process PSS excludes server-owned pixmaps. Unreadable/non-owned proc entries are skipped; per-PID read errors remain in samples.csv.",
        },
        "sampling_note": "Peak is the maximum observed at the requested interval, not an instantaneous maximum.",
        "aggregate_note": "Aggregate PSS sums unique active owned PIDs. Aggregate RSS also sums PIDs and can double-count shared pages.",
    }
    (output / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(output)
    return 0 if observed_processes else 3


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workload", choices=WORKLOADS, default="idle")
    parser.add_argument("--phase", action="append", type=parse_phase, metavar="PHASE=SECONDS",
                        help="timeline stage; repeat for startup/idle/transient/steady/shutdown")
    parser.add_argument("--duration", type=float, default=60.0, help="default single-phase duration")
    parser.add_argument("--interval-ms", type=int, default=250)
    parser.add_argument("--output", type=Path, help="new output directory (must not already exist)")
    parser.add_argument("--experiment", help="shared identifier for before/after runs")
    parser.add_argument("--repetition", type=int, default=1)
    parser.add_argument("--resolution", help="explicit monitor geometry for comparability")
    parser.add_argument("--profile-dir", help="FLAMEWM_PROFILE_DIR for real profiler gauge snapshots")
    parser.add_argument("--annotation", action="append", default=[], metavar="NAME=VALUE",
                        help="record externally measured latency/checksum metadata; repeatable")
    parser.add_argument("--proc-root", default="/proc", help=argparse.SUPPRESS)
    parser.add_argument("--dry-run", action="store_true", help="print run plan without reading /proc or writing files")
    args = parser.parse_args(argv)
    if not INTERVAL_MIN_MS <= args.interval_ms <= INTERVAL_MAX_MS:
        parser.error(f"--interval-ms must be {INTERVAL_MIN_MS}..{INTERVAL_MAX_MS}")
    if args.duration <= 0 or args.repetition < 1:
        parser.error("--duration must be positive and --repetition must be >= 1")
    if any("=" not in item or not item.split("=", 1)[0] for item in args.annotation):
        parser.error("--annotation expects NAME=VALUE")
    if args.dry_run:
        print(json.dumps(plan(args), indent=2, sort_keys=True))
        return 0
    try:
        return run(args)
    except FileExistsError as exc:
        print(f"output directory already exists: {exc.filename}", file=sys.stderr)
        return 2
    except OSError as exc:
        print(f"measurement failed: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())

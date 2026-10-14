#!/usr/bin/env python3
"""Compare idle voluntary wakeups and CPU of two FlameWM release builds.

For each run an isolated stack is started through scripts/xephyr on a private
Xephyr display (default :102) with a fresh HOME under --work-dir. After a settle
period the per-thread counters of the WM, desktop and shell processes are read
from /proc/<pid>/task/<tid>/{status,stat} over an idle window. Builds alternate
per run. Only the PIDs this runner discovered (by exact binary path under the
given target dir and the display name in their environment) are signalled.

Run with:
    python3 -I tools/flamewm_idle_wakeups_compare.py \
        --baseline-target /tmp/claude-1000/baseline-target \
        --current-target .target --work-dir /tmp/claude-1000/idle-wk --runs 5
"""

from __future__ import annotations

import argparse
import json
import os
import re
import signal
import statistics
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PROCESSES = ("flamewm", "flamewm-desktop", "flamewm-shell")
CLK_TCK = os.sysconf("SC_CLK_TCK")


def parse_status(text: str) -> dict[str, int]:
    vol = re.search(r"^voluntary_ctxt_switches:\s+(\d+)$", text, re.MULTILINE)
    nonvol = re.search(r"^nonvoluntary_ctxt_switches:\s+(\d+)$", text, re.MULTILINE)
    if vol is None or nonvol is None:
        raise ValueError("missing ctxt_switches fields in thread status")
    return {"vol": int(vol.group(1)), "nonvol": int(nonvol.group(1))}


def parse_stat_ticks(text: str) -> int:
    # comm may contain spaces and parentheses; fields resume after the last ')'.
    fields = text[text.rfind(")") + 2 :].split()
    return int(fields[11]) + int(fields[12])  # utime + stime (stat fields 14, 15)


def read_threads(pid: int) -> dict[str, dict] | None:
    base = Path(f"/proc/{pid}/task")
    try:
        tids = os.listdir(base)
    except OSError:
        return None
    out: dict[str, dict] = {}
    for tid in tids:
        try:
            comm = (base / tid / "comm").read_text(encoding="utf-8").strip()
            counters = parse_status((base / tid / "status").read_text(encoding="ascii"))
            ticks = parse_stat_ticks((base / tid / "stat").read_text(encoding="ascii"))
        except (OSError, ValueError):
            continue  # thread exited while sampling
        out[tid] = {"comm": comm, "ticks": ticks, **counters}
    return out


def window_delta(start: dict[str, dict], end: dict[str, dict]) -> dict:
    """Per-thread counter deltas over the window, aggregated by thread name.

    Threads created inside the window count from zero; threads that exited are
    dropped (their final counts are not observable).
    """
    threads: dict[str, dict[str, int]] = {}
    total = {"vol": 0, "nonvol": 0, "cpu_ticks": 0}
    for tid, now in end.items():
        before = start.get(tid, {"vol": 0, "nonvol": 0, "ticks": 0})
        d = {
            "vol": now["vol"] - before["vol"],
            "nonvol": now["nonvol"] - before["nonvol"],
            "cpu_ticks": now["ticks"] - before["ticks"],
        }
        agg = threads.setdefault(now["comm"], {"vol": 0, "nonvol": 0, "cpu_ticks": 0})
        for key, value in d.items():
            agg[key] += value
            total[key] += value
    return {"total": total, "threads": threads}


def find_pids(target: Path, display: str) -> dict[str, int]:
    release = (target / "release").resolve()
    found: dict[str, int] = {}
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        try:
            exe = os.readlink(f"/proc/{entry}/exe")
            environ = Path(f"/proc/{entry}/environ").read_bytes()
        except OSError:
            continue
        for name in PROCESSES:
            if exe == str(release / name) and f"DISPLAY={display}".encode() in environ.split(b"\0"):
                found.setdefault(name, int(entry))
    return found


def find_xephyr(display: str) -> int | None:
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        try:
            argv = Path(f"/proc/{entry}/cmdline").read_bytes().split(b"\0")
        except OSError:
            continue
        if argv and argv[0].endswith(b"Xephyr") and display.encode() in argv[1:2]:
            return int(entry)
    return None


def wait_for(predicate, timeout: float):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.5)
    return None


def run_once(label: str, target: Path, index: int, args: argparse.Namespace) -> dict:
    run_dir = Path(args.work_dir) / f"{label}-{index}"
    home = run_dir / "home"
    for sub in ("home", "xdg-config", "xdg-data", "xdg-cache", "xdg-runtime", "out"):
        (run_dir / sub).mkdir(parents=True, exist_ok=True)
    (run_dir / "xdg-runtime").chmod(0o700)
    release = target / "release"
    if args.taskbar_opacity is not None:
        config = run_dir / "xdg-config" / "flamewm"
        config.mkdir(parents=True, exist_ok=True)
        (config / "settings.toml").write_text(f"surfaceOpacity={args.taskbar_opacity}\n", encoding="utf-8")
    env = dict(os.environ)
    env.update(
        {
            "HOME": str(home),
            "XDG_CONFIG_HOME": str(run_dir / "xdg-config"),
            "XDG_DATA_HOME": str(run_dir / "xdg-data"),
            "XDG_CACHE_HOME": str(run_dir / "xdg-cache"),
            "XDG_RUNTIME_DIR": str(run_dir / "xdg-runtime"),
            "FLAMEWM_XEPHYR_DISPLAY": args.display,
            "FLAMEWM_XEPHYR_BUILD": "0",
            "FLAMEWM_XEPHYR_TEST_APPS": "0",
            # Match the product-start defaults used by the AP runner (debug and profiler off).
            "FLAMEWM_DEBUG": "0",
            "FLAMEWM_PROFILE": "0",
            "FLAMEWM_XEPHYR_CANARY": "0",
            "FLAMEWM_XEPHYR_WM_BIN": str(release / "flamewm"),
            "FLAMEWM_XEPHYR_DESKTOP_BIN": str(release / "flamewm-desktop"),
            "FLAMEWM_XEPHYR_SHELL_BIN": str(release / "flamewm-shell"),
            "FLAMEWM_XEPHYR_CANARY_BIN": str(Path(args.canary_bin)),
            "FLAMEWM_XEPHYR_OUTPUT_DIR": str(run_dir / "out"),
            "FLAMEWM_XEPHYR_EVIDENCE_ROOT": str(run_dir / "performance"),
            "FLAMEWM_XEPHYR_DEBUG_ROOT": str(run_dir / "debug"),
            "PATH": f"{release}:{env['PATH']}",
        }
    )
    log = open(run_dir / "xephyr-script.log", "wb")
    proc = subprocess.Popen(
        [str(ROOT / "scripts" / "xephyr")],
        cwd=ROOT,
        env=env,
        stdout=log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )
    result: dict = {"label": label, "run": index, "status": "FAIL", "reason": ""}
    try:
        pids = wait_for(
            lambda: (lambda f: f if len(f) == len(PROCESSES) else None)(find_pids(target, args.display)),
            args.startup_timeout,
        )
        if pids is None:
            result["reason"] = f"processes not found: {sorted(set(PROCESSES) - set(find_pids(target, args.display)))}"
            return result
        time.sleep(args.settle_seconds)
        start = {name: read_threads(pid) for name, pid in pids.items()}
        time.sleep(args.window_seconds)
        end = {name: read_threads(pid) for name, pid in pids.items()}
        missing = [n for n in PROCESSES if start.get(n) is None or end.get(n) is None]
        if missing:
            result["reason"] = f"exited during window: {missing}"
            return result
        result["processes"] = {n: window_delta(start[n], end[n]) for n in PROCESSES}
        result["status"] = "OK"
        return result
    finally:
        ours = [*find_pids(target, args.display).values()]
        xephyr = find_xephyr(args.display)
        try:
            os.killpg(proc.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            proc.wait(timeout=20)
        except subprocess.TimeoutExpired:
            os.killpg(proc.pid, signal.SIGKILL)
            proc.wait()
        log.close()
        for pid in [*ours, xephyr]:
            if pid is None:
                continue
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass


def summarize(runs: list[dict]) -> dict:
    out: dict[str, dict] = {}
    for label in sorted({r["label"] for r in runs}):
        ok = [r for r in runs if r["label"] == label and r["status"] == "OK"]
        procs: dict[str, dict] = {}
        for name in PROCESSES:
            totals = [r["processes"][name]["total"] for r in ok]
            comms = sorted({c for r in ok for c in r["processes"][name]["threads"]})
            procs[name] = {
                "runs": len(ok),
                "vol_median": median([t["vol"] for t in totals]),
                "nonvol_median": median([t["nonvol"] for t in totals]),
                "cpu_s_median": median([t["cpu_ticks"] / CLK_TCK for t in totals]),
                "threads_vol_median": {
                    c: median([r["processes"][name]["threads"].get(c, {"vol": 0})["vol"] for r in ok])
                    for c in comms
                },
            }
        out[label] = procs
    return out


def median(values: list[float]) -> float | None:
    return statistics.median(values) if values else None


def parse_args(argv: list[str]) -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--baseline-target", required=True)
    p.add_argument("--current-target", required=True)
    p.add_argument("--canary-bin", default=str(ROOT / ".target/release/flamewm-x11-canary"))
    p.add_argument("--work-dir", required=True, help="fresh directory for per-run HOME/XDG/logs")
    p.add_argument("--display", default=":102")
    p.add_argument("--runs", type=int, default=5)
    p.add_argument("--startup-timeout", type=float, default=90.0)
    p.add_argument("--settle-seconds", type=float, default=20.0)
    p.add_argument("--window-seconds", type=float, default=30.0)
    p.add_argument("--taskbar-opacity", type=int, default=None, help="seed surfaceOpacity in settings.toml (taskbar uses the surface policy)")
    return p.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    work = Path(args.work_dir)
    if work.exists() and any(work.iterdir()):
        print(f"error: --work-dir must not exist or be empty: {work}", file=sys.stderr)
        return 2
    work.mkdir(parents=True, exist_ok=True)
    builds = {"baseline": Path(args.baseline_target), "current": Path(args.current_target)}
    runs: list[dict] = []
    for i in range(args.runs):
        order = ("baseline", "current") if i % 2 == 0 else ("current", "baseline")
        for label in order:
            result = run_once(label, builds[label], i, args)
            print(f"{label} run {i}: {result['status']} {result['reason']}", flush=True)
            runs.append(result)
    summary = summarize(runs)
    (work / "runs.json").write_text(json.dumps(runs, indent=2), encoding="utf-8")
    (work / "summary.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")
    print(json.dumps(summary, indent=2))
    return 0 if all(r["status"] == "OK" for r in runs) else 1


if __name__ == "__main__":
    sys.exit(main())

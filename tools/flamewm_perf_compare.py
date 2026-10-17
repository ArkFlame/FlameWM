#!/usr/bin/env python3
"""Old-vs-new FlameWM performance comparison on a private Xephyr (reuses the appearance harness).

Usage:
  tools/flamewm_perf_compare.py --old-bin-dir DIR --new-bin-dir DIR [--run-dir DIR] [--reps N]
      [--idle-seconds S] [--start-cycles N] [--minimize-cycles N] [--settle-seconds S]
      [--geometry WxH] [--display :N]

Each repetition runs both binary sets (order alternates per repetition) with a fresh owned WM + Shell:
  (a) idle window: WM and Shell CPU seconds (utime+stime), voluntary+nonvoluntary context switches,
      VmHWM peak RSS at the end of the window;
  (b) Start menu open/close cycles via the Super key (XTEST through tools/flamewm_xtest.py):
      WM+Shell CPU seconds and wall time;
  (c) xclock minimize cycles (fresh xclock each): WM+Shell CPU seconds and wall time. Restore is not driven:
      it fails in both binary sets (taskbar: StaleRevision; DBus RestoreWindow leaves the window unmapped).

Not collected: X11 request counters, allocation counters, perf/ftrace. Output: DIR/result.json and
DIR/result.md; the markdown table is printed on stdout. Exit: 0 all phases complete, 1 PARTIAL/FAILED (partial summary still written), 2 usage, 3 environment blocked.
"""

from __future__ import annotations

import argparse
import math
import os
import re
import secrets
import shutil
import statistics
import subprocess
import sys
import time
from pathlib import Path

from flamewm_harness_common import sha256_file as sha256, write_json

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))

import flamewm_appearance_acceptance as ap  # noqa: E402
import flamewm_settings_acceptance as base  # noqa: E402

ROOT = base.ROOT
SCHEMA = "flamewm-perf-compare/1"
LABELS = ("old", "new")
PROCESSES = ("flamewm", "flamewm-shell")
CYCLE_GAP_SECONDS = 0.5      # fixed pause after each Super toggle, identical for both binary sets
MAP_TIMEOUT_SECONDS = 10.0
CLIENT_TIMEOUT_SECONDS = 10.0
METRICS = (
    ("idle_cpu_s", "idle 30 s WM+Shell CPU seconds"),
    ("idle_ctx_switches", "idle 30 s context switches (vol+nonvol)"),
    ("idle_vmhwm_kb", "idle VmHWM peak RSS sum (kB)"),
    ("start_cpu_s", "40 Start open/close WM+Shell CPU seconds"),
    ("start_wall_s", "40 Start open/close wall seconds"),
    ("minimize_cpu_s", "10 xclock minimize (restore not driven) WM+Shell CPU s"),
    ("minimize_wall_s", "10 xclock minimize (restore not driven) wall s"),
)


# ---------------------------------------------------------------- pure helpers (unit-tested)

def order_for_rep(rep: int) -> tuple[str, str]:
    """Alternate which binary set runs first so drift does not favour one side."""
    return LABELS if rep % 2 == 0 else (LABELS[1], LABELS[0])


def parse_stat_ticks(stat_text: str) -> int:
    """utime + stime in clock ticks from /proc/<pid>/stat (comm may contain spaces or parentheses)."""
    fields = stat_text[stat_text.rfind(")") + 2:].split()
    return int(fields[11]) + int(fields[12])


def parse_status_counters(status_text: str) -> dict[str, int | None]:
    """Context switches and VmHWM from /proc/<pid>/status; missing keys are None."""
    def grab(key: str) -> int | None:
        match = re.search(rf"^{key}:\s+(\d+)", status_text, re.MULTILINE)
        return int(match.group(1)) if match else None
    vol, nonvol, hwm = grab("voluntary_ctxt_switches"), grab("nonvoluntary_ctxt_switches"), grab("VmHWM")
    switches = None if vol is None or nonvol is None else vol + nonvol
    return {"ctx_switches": switches, "vmhwm_kb": hwm}


def nearest_rank_p95(values: list[float]) -> float:
    ordered = sorted(values)
    return ordered[max(0, math.ceil(0.95 * len(ordered)) - 1)]


def summarize(values: list[float]) -> dict[str, float | int]:
    """min / median / nearest-rank p95 over repetitions; refuses an empty sample."""
    if not values:
        raise ValueError("no repetitions to summarize")
    return {"n": len(values), "min": min(values), "median": statistics.median(values),
            "p95": nearest_rank_p95(values)}


def ratio(new: float, old: float) -> float | None:
    return None if old == 0 else new / old


def render_markdown(summary: dict[str, dict], reps: int, problems: list[str]) -> str:
    """summary[metric][label] -> summarize() dict or None; summary[metric]['ratio'] -> new/old median."""
    lines = [
        f"Repetitions per binary set: {reps} (order alternated). Problems: "
        f"{'; '.join(problems) if problems else 'none'}.",
        "",
        "| metric | old min | old median | old p95 | new min | new median | new p95 | new/old (median) |",
        "|---|---|---|---|---|---|---|---|",
    ]
    for key, title in METRICS:
        row = summary[key]
        cells = []
        for label in LABELS:
            st = row[label]
            if st is None:
                cells += ["n/a", "n/a", "n/a"]
            else:
                cells += [f"{st['min']:.3f}", f"{st['median']:.3f}", f"{st['p95']:.3f}"]
        r = row["ratio"]
        counts = "/".join(str(row[label]["n"]) if row[label] else "0" for label in LABELS)
        lines.append(f"| {title} (n={counts}) | " + " | ".join(cells) + f" | {'n/a' if r is None else f'{r:.3f}'} |")
    lines += ["", f"Noise: shared host ({os.cpu_count()} CPUs), nested Xephyr, no CPU pinning; p95 is nearest-rank "
              "over the repetitions (small N makes p95 close to max). X11 request and allocation counters were NOT collected."]
    return "\n".join(lines) + "\n"


# ---------------------------------------------------------------- live helpers

def sample(pid: int) -> dict | None:
    proc = Path("/proc") / str(pid)
    try:
        ticks = parse_stat_ticks((proc / "stat").read_text(encoding="ascii"))
        counters = parse_status_counters((proc / "status").read_text(encoding="ascii"))
    except (OSError, ValueError, IndexError):
        return None
    if counters["ctx_switches"] is None or counters["vmhwm_kb"] is None:
        return None
    return {"ticks": ticks, **counters}


def snapshot(pids: dict[str, int]) -> dict[str, dict] | None:
    snaps = {name: sample(pid) for name, pid in pids.items()}
    return snaps if all(v is not None for v in snaps.values()) else None


def window_map_state(s: ap.Session, win_id: str) -> str | None:
    out = subprocess.run(["xwininfo", "-display", s.ctx.display, "-id", win_id], env=s.ctx.host_env,
                         capture_output=True, text=True, timeout=20, check=False).stdout
    return ap.map_state(out)


def wait_map_state(s: ap.Session, win_id: str, wanted: tuple[str, ...]) -> bool:
    deadline = time.monotonic() + MAP_TIMEOUT_SECONDS
    while time.monotonic() < deadline:
        if window_map_state(s, win_id) in wanted:
            return True
        time.sleep(0.05)
    return False


def wait_client_frame(s: ap.Session, timeout: float = CLIENT_TIMEOUT_SECONDS) -> tuple[dict | None, dict | None]:
    """Poll the X tree until the owned xclock and its frame exist (readiness only, not timed)."""
    deadline = time.monotonic() + timeout
    while True:
        client, frame = s.client_frame(s.tree("perf-minimize"))
        if (client is not None and frame is not None) or time.monotonic() >= deadline:
            return client, frame
        time.sleep(0.25)


def toggle_start(s: ap.Session) -> bool:
    code, _ = s.input(key=base.SHORTCUT_DEFAULT_BINDING)
    return code == 0


def cpu_delta(before: dict[str, dict], after: dict[str, dict]) -> dict[str, float]:
    return {name: (after[name]["ticks"] - before[name]["ticks"]) / ap.CLK_TCK for name in PROCESSES}


def run_set(s: ap.Session, label: str, bin_dir: Path, args: argparse.Namespace) -> dict:
    """One binary set on a fresh WM + Shell. Each phase is kept or dropped on its own; see metric_values."""
    args.wm_bin, args.shell_bin = str(bin_dir / "flamewm"), str(bin_dir / "flamewm-shell")
    if not s.restart({"surfaceOpacity": "100"}):
        return {"label": label, "reason": "WM or shell exited during restart", "phases": {}}
    time.sleep(args.settle_seconds)
    pids = {"flamewm": s.ctx.wm.pid, "flamewm-shell": s.shell.pid}
    out: dict = {"label": label, "reason": "", "pids": pids, "phases": {}}
    problems: list[str] = []

    # (a) idle window
    before = snapshot(pids)
    time.sleep(args.idle_seconds)
    after = snapshot(pids)
    if before is None or after is None:
        problems.append("idle: process not observable")
    else:
        cpu = cpu_delta(before, after)
        out["idle"] = {
            "cpu_s": {n: cpu[n] for n in PROCESSES},
            "ctx_switches": {n: after[n]["ctx_switches"] - before[n]["ctx_switches"] for n in PROCESSES},
            "vmhwm_kb": {n: after[n]["vmhwm_kb"] for n in PROCESSES},
        }

    # (b) Start open/close cycles
    before = snapshot(pids)
    t0 = time.monotonic()
    fails = 0
    for _ in range(args.start_cycles):
        opened = toggle_start(s)
        time.sleep(CYCLE_GAP_SECONDS)
        closed = toggle_start(s)
        time.sleep(CYCLE_GAP_SECONDS)
        fails += (not opened) + (not closed)
    wall = time.monotonic() - t0
    after = snapshot(pids)
    if before is None or after is None:
        problems.append("start: process not observable")
    elif fails:
        problems.append(f"start: {fails} Start toggle inputs failed")
    else:
        out["start"] = {"cpu_s": cpu_delta(before, after), "wall_s": wall}

    # (c) xclock minimize cycles: a fresh xclock per cycle, timed from the minimize click until unmapped.
    # Restore is NOT driven: taskbar restore fails with StaleRevision and DBus RestoreWindow leaves the window
    # unmapped in both binary sets, so each cycle needs its own client.
    cpu_total = {n: 0.0 for n in PROCESSES}
    wall_total, minimized, step = 0.0, 0, "ok"
    for _ in range(args.minimize_cycles):
        s.spawn_client()
        client, frame = wait_client_frame(s)
        if client is None or frame is None:
            step = "xclock or frame not found within readiness timeout"
            break
        minimize_at = f"{frame['x'] + frame['w'] - 3 * ap.BUTTON_WIDTH + ap.BUTTON_WIDTH // 2},{frame['y'] + 15}"
        before = snapshot(pids)
        t0 = time.monotonic()
        code, _ = s.input(click=minimize_at)
        unmapped = code == 0 and wait_map_state(s, client["id"], ("IsUnMapped", "IsUnviewable"))
        wall_total += time.monotonic() - t0
        after = snapshot(pids)
        base.Ctx.stop(s.client)
        if before is None or after is None:
            step = "process not observable during minimize cycle"
            break
        if not unmapped:
            step = f"minimize click rc={code} or no unmap within {MAP_TIMEOUT_SECONDS:g}s"
            break
        for name, value in cpu_delta(before, after).items():
            cpu_total[name] += value
        minimized += 1
    out["minimize"] = {"cpu_s": cpu_total, "wall_s": wall_total, "minimized": minimized,
                       "requested": args.minimize_cycles, "step": step}
    if minimized != args.minimize_cycles:
        problems.append(f"minimize: only {minimized}/{args.minimize_cycles} cycles completed: {step}")
    out["reason"] = "; ".join(problems)
    return out


def phase_rows(sets: list[dict], label: str) -> dict[str, list[dict]]:
    """Per-phase rows for one label; a phase counts only when it completed in full."""
    rows = [r for r in sets if r["label"] == label]
    return {
        "idle": [r["idle"] for r in rows if "idle" in r],
        "start": [r["start"] for r in rows if "start" in r],
        "minimize": [r["minimize"] for r in rows
                     if "minimize" in r and r["minimize"]["minimized"] == r["minimize"]["requested"]],
    }


def metric_values(sets: list[dict], label: str) -> dict[str, list[float]]:
    rows = phase_rows(sets, label)
    return {
        "idle_cpu_s": [sum(r["cpu_s"].values()) for r in rows["idle"]],
        "idle_ctx_switches": [sum(r["ctx_switches"].values()) for r in rows["idle"]],
        "idle_vmhwm_kb": [sum(r["vmhwm_kb"].values()) for r in rows["idle"]],
        "start_cpu_s": [sum(r["cpu_s"].values()) for r in rows["start"]],
        "start_wall_s": [r["wall_s"] for r in rows["start"]],
        "minimize_cpu_s": [sum(r["cpu_s"].values()) for r in rows["minimize"]],
        "minimize_wall_s": [r["wall_s"] for r in rows["minimize"]],
    }


def build_summary(sets: list[dict]) -> dict[str, dict]:
    """Per metric: summarize() per label (None when no completed repetition), plus new/old median ratio."""
    values = {label: metric_values(sets, label) for label in LABELS}
    summary: dict[str, dict] = {}
    for key, _ in METRICS:
        old = summarize(values["old"][key]) if values["old"][key] else None
        new = summarize(values["new"][key]) if values["new"][key] else None
        both = old is not None and new is not None
        summary[key] = {"old": old, "new": new, "ratio": ratio(new["median"], old["median"]) if both else None}
    return summary


def parse_args(argv: list[str]) -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--old-bin-dir", default="/tmp/claude-1000/perf-base-target/release")
    p.add_argument("--new-bin-dir", default=str(ROOT / ".target/release"))
    p.add_argument("--run-dir", default=None, help="default /tmp/claude-1000/perf-runs/<utc-id>")
    p.add_argument("--reps", type=int, default=5)
    p.add_argument("--idle-seconds", type=float, default=30.0)
    p.add_argument("--start-cycles", type=int, default=40)
    p.add_argument("--minimize-cycles", type=int, default=10)
    p.add_argument("--settle-seconds", type=float, default=3.0)
    p.add_argument("--geometry", default="1280x800")
    p.add_argument("--display", default=None)
    p.add_argument("--client-bin", default="xclock")
    args = p.parse_args(argv)
    if args.reps < 1 or args.start_cycles < 1 or args.minimize_cycles < 1 or args.idle_seconds <= 0:
        p.error("reps, cycles and idle-seconds must be positive")
    if not re.fullmatch(r"\d+x\d+", args.geometry):
        p.error("--geometry must be WxH")
    args.screen_w, args.screen_h = (int(v) for v in args.geometry.split("x"))
    return args


def run_inner(args: argparse.Namespace, run: Path) -> int:
    ctx = base.Ctx(run)
    sets: list[dict] = []
    started = ap.utc_now()
    bins = {"old": Path(args.old_bin_dir), "new": Path(args.new_bin_dir)}
    for sub in ("home", "xdg-config", "xdg-data", "xdg-cache", "xdg-runtime", "logs"):
        (run / sub).mkdir(parents=True, exist_ok=True)
    doc: dict = {"schema": SCHEMA, "verdict": "BLOCKED", "exit_code": 3}
    try:
        display = base.pick_display(args.display)
        if display is None:
            doc["reason"] = "no free display in :90..:99"
            return 3
        ctx.display = display
        ctx.build_app_env()
        ctx.token = secrets.token_hex(32)
        ctx.marker.write_text(f"{ctx.display}\n{ctx.token}\n", encoding="ascii")
        os.chmod(ctx.marker, 0o600)
        xephyr = ctx.spawn("xephyr", ["Xephyr", ctx.display, "-screen", args.geometry, "-ac", "-noreset", "-br"],
                           ctx.host_env)
        if not base.wait_for_display(ctx, xephyr):
            doc["reason"] = f"Xephyr did not become ready on {ctx.display}"
            return 3
        session = ap.Session(ctx, args)
        for rep in range(args.reps):
            for label in order_for_rep(rep):
                result = run_set(session, label, bins[label], args) | {"rep": rep}
                sets.append(result)
                print(f"rep {rep} {label}: {result['reason'] or 'complete'}", file=sys.stderr)
        problems = [f"rep{r['rep']}-{r['label']}: {r['reason']}" for r in sets if r["reason"]]
        if len(sets) != 2 * args.reps:
            problems.append(f"run incomplete: {len(sets)}/{2 * args.reps} binary-set repetitions")
        summary = build_summary(sets)
        complete = not problems
        doc |= {"verdict": "MEASURED" if complete else "PARTIAL", "exit_code": 0 if complete else 1,
                "problems": problems, "summary": summary,
                "markdown": render_markdown(summary, args.reps, problems)}
        return doc["exit_code"]
    finally:
        teardown = ctx.teardown()
        doc |= {"run_dir": str(run), "display": ctx.display, "geometry": args.geometry, "reps": args.reps,
                "idle_seconds": args.idle_seconds, "start_cycles": args.start_cycles,
                "minimize_cycles": args.minimize_cycles, "cycle_gap_s": CYCLE_GAP_SECONDS,
                "binaries": {label: {name: {"path": str(bins[label] / name),
                                            "sha256": sha256(bins[label] / name)}
                                     for name in ("flamewm", "flamewm-shell")}
                             for label in LABELS},
                "started_utc": started, "finished_utc": ap.utc_now(), "teardown": teardown, "sets": sets}
        if not teardown["ok"]:
            doc["verdict"], doc["exit_code"] = "FAILED", 1
        write_json(run / "result.json", doc)
        if doc.get("markdown"):
            (run / "result.md").write_text(doc["markdown"], encoding="utf-8")
            print(doc["markdown"])


def main(argv: list[str] | None = None) -> int:
    raw = sys.argv[1:] if argv is None else list(argv)
    args = parse_args(raw)
    inner = os.environ.get("FLAMEWM_PERF_INNER") == "1"
    if args.run_dir is None:
        # Pin the run id before the re-exec so the child (which validates the run dir) sees the same path.
        args.run_dir = f"/tmp/claude-1000/perf-runs/{ap.utc_now()}"
        raw = [*raw, "--run-dir", args.run_dir]
    run = Path(args.run_dir).expanduser().absolute()
    if inner:
        ap.validate_inner_run_dir(run)
    else:
        ap.validate_run_dir(run)
    for label in LABELS:
        for name in ("flamewm", "flamewm-shell"):
            binary = Path(getattr(args, f"{label}_bin_dir")) / name
            if not os.access(binary, os.X_OK):
                print(f"error: binary not executable: {binary}", file=sys.stderr)
                return 2
    missing = [name for name in ("Xephyr", "xwininfo", "xprop", "xdpyinfo", "gdbus") if shutil.which(name) is None]
    if missing or not os.environ.get("DISPLAY"):
        print(f"ENVIRONMENT_BLOCKED: missing tools {missing} or host DISPLAY unset", file=sys.stderr)
        return 3
    run.mkdir(parents=True, exist_ok=True)
    os.chmod(run, 0o700)
    if inner:
        return run_inner(args, run)
    (run / base.RUN_OWNER_MARKER).write_text(f"owner-pid={os.getpid()}\ncreated={ap.utc_now()}\n", encoding="ascii")
    if shutil.which("dbus-run-session") is None:
        print("error: dbus-run-session not found on PATH", file=sys.stderr)
        return 3
    child = subprocess.Popen(["dbus-run-session", "--", sys.executable, str(Path(__file__).resolve()), *raw],
                             env=dict(os.environ, FLAMEWM_PERF_INNER="1"))
    code = child.wait()
    print(run / "result.json")
    return code


if __name__ == "__main__":
    raise SystemExit(main())

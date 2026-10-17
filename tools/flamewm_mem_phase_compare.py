#!/usr/bin/env python3
"""Old-vs-new FlameWM memory per phase on a private Xephyr (reuses perf_compare and mem_sample helpers).

Usage:
  tools/flamewm_mem_phase_compare.py --old-bin-dir DIR --new-bin-dir DIR [--run-dir DIR] [--reps N]
      [--idle-seconds S] [--idle-samples N] [--samples N] [--start-cycles N] [--minimize-cycles N]
      [--drag-cycles N] [--settle-seconds S] [--geometry WxH] [--display :N]
      [--icon-theme-dir PATH | --synthetic-icons N] [--scenario NAME]

Scenario mode (opt-in, --scenario NAME): each repetition restarts WM + Shell once and runs only the phases of
NAME (see SCENARIOS). Phases that keep the client alive sample it too (pid reported as "client"); the
client-destroy phase samples alive, stops the client, waits for it to unmanage, then samples again (its
alive samples are kept as alive_samples). Without --scenario the full sequence above runs unchanged.

Icon theme scenario (opt-in, default none): --icon-theme-dir symlinks an existing theme dir into the run's
private XDG_DATA_HOME/icons; --synthetic-icons N generates an N-file theme there. Either way the theme name
is written to kdeglobals and gtk-3.0/settings.ini under the private XDG_CONFIG_HOME, which is where the
product reads them. The scenario is recorded in result.json (icon_theme) and the first line of result.md.

Each repetition runs both binary sets (order alternates) on a fresh owned WM + Shell. Every phase reads
WM, Shell and the owned Xephyr at one instant per sample (mem_sample.sample_round, barrier-aligned):
  cold      after settle
  idle      --idle-samples reads spread over the --idle-seconds window
  start     after --start-cycles Super open/close cycles
  minimize  after --minimize-cycles fresh-xclock minimize cycles (restore not driven)
  drag      after --drag-cycles titlebar drags of one fresh xclock (XTEST via flamewm_xtest.py)
A phase that cannot complete is recorded as NOT_RUN with its reason.

Per repetition the phase value is the median of its samples. Across repetitions: median and nearest-rank
p95. Verdict: combined WM+Shell PSS and RSS against 34 MiB (35651584 B), evaluated on the median per phase
(a value at or below the target is WITHIN). Xephyr PSS is reported on its own line and is not in the verdict.
Output: DIR/result.json and DIR/result.md; the markdown is printed on stdout.
Exit: 0 all phases complete, 1 PARTIAL/FAILED, 2 usage, 3 environment blocked.
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import re
import secrets
import shutil
import statistics
import subprocess
import sys
import time
from pathlib import Path

from flamewm_harness_common import write_json

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))

import flamewm_appearance_acceptance as ap  # noqa: E402
import flamewm_mem_sample as mem  # noqa: E402
import flamewm_perf_compare as perf  # noqa: E402
import flamewm_settings_acceptance as base  # noqa: E402

SCHEMA = "flamewm-mem-phase-compare/1"
LABELS = ("old", "new")
PHASES = ("cold", "idle", "start", "minimize", "drag")
ALL_PHASES = (*PHASES, "destroy")
# Allocator tuning the session applies to its WM and Shell children (flamewm-session.rs); the mem-phase
# harness passes it to those children only, leaving the client processes untouched.
ALLOCATOR_TUNING = (("MALLOC_ARENA_MAX", "2"), ("MALLOC_TRIM_THRESHOLD_", "131072"),
                    ("MALLOC_MMAP_THRESHOLD_", "131072"))
WM_SHELL_SPAWNS = frozenset({"flamewm", "flamewm-restart", "flamewm-shell"})
SCENARIOS = {
    "idle-only": ("idle",),
    "minimize-only": ("minimize",),
    "drag-only": ("drag",),
    "minimize-then-drag": ("minimize", "drag"),
    "drag-then-minimize": ("drag", "minimize"),
    "client-destroy": ("destroy",),
}
PROC_NAMES = ("flamewm", "flamewm-shell", "xephyr", "client")
PROC_METRICS = ("rss", "pss", "private_dirty")
WS_METRICS = ("rss", "pss", "private_dirty", "threads")   # combined WM+Shell
XE_METRICS = ("pss", "private_dirty")                      # Xephyr, separate line
VERDICT_METRICS = ("pss", "rss")
SAMPLE_SPACING_SECONDS = 0.5
DRAG_STEP_PX = 120
DRAG_DROP_PX = 60
SYNTHETIC_THEME = "FlameWM-Synthetic-Icons"
SYNTHETIC_SIZES = (16, 22, 24, 32, 48, 64, 128, 256)
SCALABLE_DIR = "scalable/apps"
# 1x1 transparent PNG; the synthetic theme repeats it so every file is a real, decodable image.
SYNTHETIC_PNG = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=")
SYNTHETIC_SVG = (b'<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16">'
                 b'<rect width="16" height="16" fill="#777"/></svg>\n')


# ---------------------------------------------------------------- pure helpers (unit-tested)

def combine_ws(round_sample: dict) -> dict[str, int]:
    """Sum WM and Shell metrics for one sample_round() result; Xephyr is excluded."""
    wm, shell = round_sample["flamewm"], round_sample["flamewm-shell"]
    return {m: wm[m] + shell[m] for m in WS_METRICS}


def per_process_rep(samples: list[dict]) -> dict[str, dict[str, float]]:
    """Median per process over one repetition's samples; a process appears only when it was sampled."""
    out = {}
    for name in PROC_NAMES:
        rows = [s[name] for s in samples if name in s]
        if rows:
            out[name] = {m: statistics.median([r[m] for r in rows]) for m in PROC_METRICS}
    return out


def phase_rep_value(samples: list[dict]) -> dict[str, dict[str, float]]:
    """Median over one repetition's samples of one phase: combined WM+Shell, Xephyr and per-process."""
    combos = [combine_ws(s) for s in samples]
    return {
        "wm_shell": {m: statistics.median([c[m] for c in combos]) for m in WS_METRICS},
        "xephyr": {m: statistics.median([s["xephyr"][m] for s in samples]) for m in XE_METRICS},
        "per_process": per_process_rep(samples),
    }


def phase_stats(rep_values: list[dict]) -> dict | None:
    """summarize() (min/median/p95) per metric across repetitions; None when no repetition completed."""
    if not rep_values:
        return None
    per_process = {}
    for name in PROC_NAMES:
        rows = [r["per_process"][name] for r in rep_values if name in r["per_process"]]
        if rows:
            per_process[name] = {m: perf.summarize([r[m] for r in rows]) for m in PROC_METRICS}
    return {
        "wm_shell": {m: perf.summarize([r["wm_shell"][m] for r in rep_values]) for m in WS_METRICS},
        "xephyr": {m: perf.summarize([r["xephyr"][m] for r in rep_values]) for m in XE_METRICS},
        "per_process": per_process,
    }


def run_phase_list(names: list[str] | tuple[str, ...], runners: dict) -> dict[str, dict]:
    """Run each named phase's runner in order; a runner returns a dict with 'samples' (plus extras).
    A phase that raises a live-part error is recorded as NOT_RUN with its reason and the next phase still runs."""
    phases: dict[str, dict] = {}
    for name in names:
        try:
            phases[name] = {"status": "ok", **runners[name]()}
        except (RuntimeError, OSError, ValueError) as exc:
            phases[name] = {"status": f"NOT_RUN: {exc}", "samples": []}
    return phases


def phases_for(args: argparse.Namespace) -> tuple[str, ...]:
    """The phases one binary set runs: the named scenario, or the full sequence when none is given."""
    return SCENARIOS[args.scenario] if args.scenario else PHASES


def aggregate(sets: list[dict], label: str) -> dict[str, dict | None]:
    """phase -> phase_stats() for one label. A repetition counts for a phase only when that phase is ok."""
    out: dict[str, dict | None] = {}
    for phase in ALL_PHASES:
        reps = [phase_rep_value(r["phases"][phase]["samples"])
                for r in sets
                if r["label"] == label and r.get("phases", {}).get(phase, {}).get("status") == "ok"]
        out[phase] = phase_stats(reps)
    return out


def mib_text(value: float) -> str:
    return f"{mem.mib(value):.3f} MiB"


def verdict(agg: dict[str, dict | None], target: int = mem.TARGET_BYTES, phases: tuple[str, ...] = PHASES) -> dict:
    """OVER if any measured phase has a combined WM+Shell PSS or RSS median above target, else WITHIN.
    NOT_RUN when no phase was measured; a status gets '+PARTIAL' when some phases were not measured."""
    lines: list[str] = []
    over = False
    not_run = [p for p in phases if agg.get(p) is None]
    for phase in phases:
        st = agg.get(phase)
        if st is None:
            lines.append(f"{phase}: NOT_RUN")
            continue
        parts = []
        for metric in VERDICT_METRICS:
            s = st["wm_shell"][metric]
            above = s["median"] > target
            over = over or above
            flag = "OVER" if above else "WITHIN"
            parts.append(f"{metric.upper()} median {mib_text(s['median'])}, p95 {mib_text(s['p95'])}, {flag}")
        lines.append(f"{phase}: " + "; ".join(parts))
    if len(not_run) == len(phases):
        status = "NOT_RUN"
    else:
        status = "OVER" if over else "WITHIN"
        if not_run:
            status += "+PARTIAL"
    return {"status": status, "lines": lines, "not_run_phases": not_run}


def ratio_or_none(new: dict | None, old: dict | None, metric: str) -> float | None:
    if new is None or old is None:
        return None
    return perf.ratio(new["wm_shell"][metric]["median"], old["wm_shell"][metric]["median"])


def fmt_value(metric: str, value: float) -> str:
    return f"{value:.0f}" if metric == "threads" else mib_text(value)


def render_markdown(summary: dict, verdicts: dict, reps: int, problems: list[str]) -> str:
    lines = [
        f"Repetitions per binary set: {reps} (order alternated). Problems: "
        f"{'; '.join(problems) if problems else 'none'}.",
        "",
        "Combined WM+Shell (per phase: median and p95 across repetitions):",
        "",
        "| phase | metric | old median | old p95 | new median | new p95 | new/old (median) |",
        "|---|---|---|---|---|---|---|",
    ]
    for phase in ALL_PHASES:
        for metric in WS_METRICS:
            cells = []
            for label in LABELS:
                st = summary[label][phase]
                if st is None:
                    cells += ["n/a", "n/a"]
                else:
                    cells += [fmt_value(metric, st["wm_shell"][metric]["median"]),
                              fmt_value(metric, st["wm_shell"][metric]["p95"])]
            r = ratio_or_none(summary["new"][phase], summary["old"][phase], metric)
            lines.append(f"| {phase} | {metric} | " + " | ".join(cells)
                         + f" | {'n/a' if r is None else f'{r:.3f}'} |")
    lines += ["", "Per process (median across repetitions of the per-repetition median; not in the verdict):", "",
              "| phase | process | label | RSS median | PSS median | Private_Dirty median |",
              "|---|---|---|---|---|---|"]
    for phase in ALL_PHASES:
        for name in PROC_NAMES:
            for label in LABELS:
                st = summary[label][phase]
                pp = None if st is None else st.get("per_process", {}).get(name)
                if pp is not None:
                    lines.append(f"| {phase} | {name} | {label} | {mib_text(pp['rss']['median'])} "
                                 f"| {mib_text(pp['pss']['median'])} | {mib_text(pp['private_dirty']['median'])} |")
    lines += ["", "Xephyr (private, separate from the verdict):", "",
              "| phase | label | Xephyr PSS median | Xephyr PSS p95 | Xephyr Private_Dirty median | Xephyr Private_Dirty p95 |",
              "|---|---|---|---|---|---|"]
    for phase in ALL_PHASES:
        for label in LABELS:
            st = summary[label][phase]
            if st is None:
                lines.append(f"| {phase} | {label} | n/a | n/a | n/a | n/a |")
            else:
                x = st["xephyr"]
                lines.append(f"| {phase} | {label} | {mib_text(x['pss']['median'])} | {mib_text(x['pss']['p95'])} "
                             f"| {mib_text(x['private_dirty']['median'])} | {mib_text(x['private_dirty']['p95'])} |")
    lines.append("")
    for label in LABELS:
        v = verdicts[label]
        lines.append(f"VERDICT {label}: {v['status']} vs 34 MiB (35651584 B) on combined WM+Shell PSS and RSS "
                     f"(median per phase across repetitions; p95 shown; <= target is WITHIN).")
        lines += [f"- {row}" for row in v["lines"]]
    lines += ["", f"Noise: shared host ({os.cpu_count()} CPUs), nested Xephyr, no CPU pinning; p95 is nearest-rank "
              "over the repetitions (small N makes p95 close to max)."]
    return "\n".join(lines) + "\n"


def icon_scenario(args: argparse.Namespace) -> dict:
    """The icon theme scenario the run stages, recorded verbatim in result.json."""
    if args.icon_theme_dir:
        src = Path(args.icon_theme_dir).expanduser().absolute()
        return {"mode": "dir", "name": src.name, "source": str(src)}
    if args.synthetic_icons:
        return {"mode": "synthetic", "name": SYNTHETIC_THEME, "files": args.synthetic_icons}
    return {"mode": "none"}


def scenario_text(scenario: dict) -> str:
    if scenario["mode"] == "dir":
        return f"icon theme {scenario['name']} from {scenario['source']}"
    if scenario["mode"] == "synthetic":
        return f"synthetic icon theme {scenario['name']} with {scenario['files']} files"
    return "none (default, no icon theme staged)"


def write_synthetic_theme(icons_root: Path, count: int) -> Path:
    """Write index.theme and `count` small PNG/SVG icons under icons_root/SYNTHETIC_THEME, round-robin over
    the fixed size dirs and the scalable dir."""
    theme = icons_root / SYNTHETIC_THEME
    dirs = [f"{size}x{size}/apps" for size in SYNTHETIC_SIZES] + [SCALABLE_DIR]
    for d in dirs:
        (theme / d).mkdir(parents=True, exist_ok=True)
    sections = [f"[{d}]\nSize={SYNTHETIC_SIZES[i] if i < len(SYNTHETIC_SIZES) else 48}\nContext=Apps\n"
                + ("Type=Scalable\nMinSize=8\nMaxSize=512\n" if d == SCALABLE_DIR else "Type=Fixed\n")
                for i, d in enumerate(dirs)]
    index = (f"[Icon Theme]\nName={SYNTHETIC_THEME}\nComment=Generated memory-scenario icon theme\n"
             f"Inherits=hicolor\nDirectories={','.join(dirs)}\n\n" + "\n".join(sections))
    (theme / "index.theme").write_text(index, encoding="ascii")
    for i in range(count):
        d = dirs[i % len(dirs)]
        if d == SCALABLE_DIR:
            (theme / d / f"icon-{i:06d}.svg").write_bytes(SYNTHETIC_SVG)
        else:
            (theme / d / f"icon-{i:06d}.png").write_bytes(SYNTHETIC_PNG)
    return theme


def stage_icon_scenario(run: Path, args: argparse.Namespace) -> None:
    """Stage the selected icon theme: the theme under XDG_DATA_HOME/icons and its name in the private
    XDG_CONFIG_HOME (kdeglobals and gtk-3.0/settings.ini, the files the product reads). No-op by default."""
    scenario = icon_scenario(args)
    if scenario["mode"] == "none":
        return
    icons = run / "xdg-data" / "icons"
    icons.mkdir(parents=True, exist_ok=True)
    if scenario["mode"] == "dir":
        (icons / scenario["name"]).symlink_to(scenario["source"])
    else:
        write_synthetic_theme(icons, scenario["files"])
    config = run / "xdg-config"
    (config / "gtk-3.0").mkdir(parents=True, exist_ok=True)
    (config / "kdeglobals").write_text(f"[Icons]\nTheme={scenario['name']}\n", encoding="utf-8")
    (config / "gtk-3.0" / "settings.ini").write_text(
        f"[Settings]\ngtk-icon-theme-name={scenario['name']}\n", encoding="utf-8")


# ---------------------------------------------------------------- live helpers

def timed_samples(pids: list[tuple[str, int]], count: int, spacing: float) -> list[dict]:
    """count concurrent sample_round() reads, each after `spacing` seconds."""
    out = []
    for _ in range(count):
        time.sleep(spacing)
        out.append(mem.sample_round(pids))
    return out


def start_phase(s: ap.Session, pids: list[tuple[str, int]], args: argparse.Namespace) -> list[dict]:
    fails = 0
    for _ in range(args.start_cycles):
        opened = perf.toggle_start(s)
        time.sleep(perf.CYCLE_GAP_SECONDS)
        closed = perf.toggle_start(s)
        time.sleep(perf.CYCLE_GAP_SECONDS)
        fails += (not opened) + (not closed)
    if fails:
        raise RuntimeError(f"{fails} Start toggle inputs failed")
    return timed_samples(pids, args.samples, SAMPLE_SPACING_SECONDS)


def live_samples(s: ap.Session, pids: list[tuple[str, int]], args: argparse.Namespace) -> list[dict]:
    """Samples with the owned client still alive (pid as 'client'), then the client is stopped."""
    try:
        return timed_samples([*pids, ("client", s.client.proc.pid)], args.samples, SAMPLE_SPACING_SECONDS)
    finally:
        base.Ctx.stop(s.client)


def wait_client_gone(s: ap.Session) -> bool:
    """Poll the X tree until the owned client and its frame are both gone (unmanaged)."""
    deadline = time.monotonic() + perf.MAP_TIMEOUT_SECONDS
    while time.monotonic() < deadline:
        client, frame = s.client_frame(s.tree("perf-destroy"))
        if client is None and frame is None:
            return True
        time.sleep(0.05)
    return False


def minimize_phase(s: ap.Session, pids: list[tuple[str, int]], args: argparse.Namespace,
                   live: bool = False) -> list[dict]:
    for n in range(args.minimize_cycles):
        s.spawn_client()
        client, frame = perf.wait_client_frame(s)
        if client is None or frame is None:
            raise RuntimeError(f"minimize cycle {n}: xclock or frame not found")
        at = f"{frame['x'] + frame['w'] - 3 * ap.BUTTON_WIDTH + ap.BUTTON_WIDTH // 2},{frame['y'] + 15}"
        code, _ = s.input(click=at)
        unmapped = code == 0 and perf.wait_map_state(s, client["id"], ("IsUnMapped", "IsUnviewable"))
        keep = live and unmapped and n == args.minimize_cycles - 1
        if not keep:
            base.Ctx.stop(s.client)
        if not unmapped:
            raise RuntimeError(f"minimize cycle {n}: no unmap within {perf.MAP_TIMEOUT_SECONDS:g}s")
    if live:
        return live_samples(s, pids, args)
    return timed_samples(pids, args.samples, SAMPLE_SPACING_SECONDS)


def drag_phase(s: ap.Session, pids: list[tuple[str, int]], args: argparse.Namespace,
               live: bool = False) -> list[dict]:
    """Drag one fresh xclock by its titlebar; each drag starts where the previous one ended."""
    s.spawn_client()
    client, frame = perf.wait_client_frame(s)
    if client is None or frame is None:
        raise RuntimeError("xclock or frame not found")
    x, y = frame["x"] + frame["w"] // 2, frame["y"] + 15
    for i in range(args.drag_cycles):
        step = DRAG_STEP_PX if i % 2 == 0 else -DRAG_STEP_PX
        end_x, end_y = x + step, y + (DRAG_DROP_PX if i % 2 == 0 else 0)
        s.input(drag=f"{x},{y},{end_x},{end_y}")
        x, y = end_x, end_y
    if live:
        return live_samples(s, pids, args)
    base.Ctx.stop(s.client)
    return timed_samples(pids, args.samples, SAMPLE_SPACING_SECONDS)


def destroy_phase(s: ap.Session, pids: list[tuple[str, int]], args: argparse.Namespace) -> dict:
    """Sample the live client, stop it, wait for unmanage, then sample again. 'samples' is the post-unmanage
    set; 'alive_samples' is the set taken while the client was alive."""
    s.spawn_client()
    client, frame = perf.wait_client_frame(s)
    if client is None or frame is None:
        raise RuntimeError("xclock or frame not found")
    alive = timed_samples([*pids, ("client", s.client.proc.pid)], args.samples, SAMPLE_SPACING_SECONDS)
    base.Ctx.stop(s.client)
    if not wait_client_gone(s):
        raise RuntimeError(f"client not unmanaged within {perf.MAP_TIMEOUT_SECONDS:g}s")
    return {"samples": timed_samples(pids, args.samples, SAMPLE_SPACING_SECONDS), "alive_samples": alive}


def surface_settings(args: argparse.Namespace) -> dict[str, str]:
    """Settings snapshot values for the surface; blur is written only when --surface-blur is given."""
    values = {"surfaceOpacity": str(args.surface_opacity)}
    if args.surface_blur is not None:
        values["surfaceBlurPercent"] = str(args.surface_blur)
    return values


def run_set(s: ap.Session, label: str, bin_dir: Path, args: argparse.Namespace, xephyr_pid: int) -> dict:
    """One binary set on a fresh WM + Shell. Each phase is kept or marked NOT_RUN on its own."""
    args.wm_bin, args.shell_bin = str(bin_dir / "flamewm"), str(bin_dir / "flamewm-shell")
    names = phases_for(args)
    out: dict = {"label": label, "phases": {}}
    if not s.restart(surface_settings(args)):
        out["phases"] = {p: {"status": "NOT_RUN: WM or shell exited during restart", "samples": []} for p in names}
        out["reason"] = "WM or shell exited during restart"
        return out
    time.sleep(args.settle_seconds)
    pids = [("flamewm", s.ctx.wm.pid), ("flamewm-shell", s.shell.pid), ("xephyr", xephyr_pid)]
    live = args.scenario is not None
    runners = {
        "cold": lambda: {"samples": timed_samples(pids, args.samples, SAMPLE_SPACING_SECONDS)},
        "idle": lambda: {"samples": timed_samples(pids, args.idle_samples, args.idle_seconds / args.idle_samples)},
        "start": lambda: {"samples": start_phase(s, pids, args)},
        "minimize": lambda: {"samples": minimize_phase(s, pids, args, live)},
        "drag": lambda: {"samples": drag_phase(s, pids, args, live)},
        "destroy": lambda: destroy_phase(s, pids, args),
    }
    out["phases"] = run_phase_list(names, runners)
    not_ok = [f"{p}: {v['status']}" for p, v in out["phases"].items() if v["status"] != "ok"]
    out["reason"] = "; ".join(not_ok)
    return out


# ---------------------------------------------------------------- CLI and run

def parse_args(argv: list[str]) -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--old-bin-dir", default="/tmp/claude-1000/perf-base-target/release")
    p.add_argument("--new-bin-dir", default=str(base.ROOT / ".target/release"))
    p.add_argument("--run-dir", default=None, help="default /tmp/claude-1000/mem-phase-runs/<utc-id>")
    p.add_argument("--reps", type=int, default=5)
    p.add_argument("--idle-seconds", type=float, default=30.0)
    p.add_argument("--idle-samples", type=int, default=5)
    p.add_argument("--samples", type=int, default=3, help="samples per non-idle phase")
    p.add_argument("--start-cycles", type=int, default=40)
    p.add_argument("--minimize-cycles", type=int, default=10)
    p.add_argument("--drag-cycles", type=int, default=10)
    p.add_argument("--settle-seconds", type=float, default=3.0)
    p.add_argument("--geometry", default="1280x800")
    p.add_argument("--display", default=None)
    p.add_argument("--client-bin", default="xclock")
    p.add_argument("--scenario", choices=sorted(SCENARIOS), default=None,
                   help="run only this phase list on a fresh WM+Shell per binary set (default: full sequence)")
    icons = p.add_mutually_exclusive_group()
    icons.add_argument("--icon-theme-dir", default=None, metavar="PATH",
                       help="existing icon theme dir (contains index.theme), symlinked into the private run")
    icons.add_argument("--synthetic-icons", type=int, default=None, metavar="N",
                       help="generate an N-file synthetic icon theme in the private run")
    p.add_argument("--surface-opacity", type=int, default=100, metavar="N",
                   help="taskbar surfaceOpacity written into the settings snapshot (0..100, default 100)")
    p.add_argument("--surface-blur", type=int, default=None, metavar="N",
                   help="taskbar surfaceBlurPercent written into the settings snapshot (0..100; default: not written)")
    args = p.parse_args(argv)
    if not 0 <= args.surface_opacity <= 100:
        p.error("--surface-opacity must be 0..100")
    if args.surface_blur is not None and not 0 <= args.surface_blur <= 100:
        p.error("--surface-blur must be 0..100")
    counts = (args.reps, args.idle_samples, args.samples, args.start_cycles, args.minimize_cycles, args.drag_cycles)
    if min(counts) < 1 or args.idle_seconds <= 0:
        p.error("reps, sample, cycle counts and idle-seconds must be positive")
    if not re.fullmatch(r"\d+x\d+", args.geometry):
        p.error("--geometry must be WxH")
    if args.synthetic_icons is not None and args.synthetic_icons < 1:
        p.error("--synthetic-icons must be a positive file count")
    if args.icon_theme_dir and not (Path(args.icon_theme_dir).expanduser() / "index.theme").is_file():
        p.error("--icon-theme-dir must be a directory containing index.theme")
    args.screen_w, args.screen_h = (int(v) for v in args.geometry.split("x"))
    return args


def build_doc(sets: list[dict], args: argparse.Namespace) -> tuple[dict, list[str]]:
    problems = [f"rep{r['rep']}-{r['label']}: {r['reason']}" for r in sets if r["reason"]]
    if len(sets) != 2 * args.reps:
        problems.append(f"run incomplete: {len(sets)}/{2 * args.reps} binary-set repetitions")
    summary = {label: aggregate(sets, label) for label in LABELS}
    verdicts = {label: verdict(summary[label], phases=phases_for(args)) for label in LABELS}
    markdown = f"Icon theme scenario: {scenario_text(icon_scenario(args))}.\n" + \
        render_markdown(summary, verdicts, args.reps, problems)
    doc = {"summary": summary, "verdicts": verdicts, "problems": problems, "markdown": markdown}
    return doc, problems


def allocator_env(name: str, env: dict[str, str]) -> dict[str, str]:
    if name not in WM_SHELL_SPAWNS:
        return env
    return {**env, **dict(ALLOCATOR_TUNING)}


def tune_wm_shell_spawns(ctx: base.Ctx) -> None:
    spawn = ctx.spawn

    def tuned(name: str, argv: list[str], env: dict[str, str]) -> base.Owned:
        return spawn(name, argv, allocator_env(name, env))

    ctx.spawn = tuned


def run_inner(args: argparse.Namespace, run: Path) -> int:
    ctx = base.Ctx(run)
    tune_wm_shell_spawns(ctx)
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
        stage_icon_scenario(run, args)
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
            for label in perf.order_for_rep(rep):
                result = run_set(session, label, bins[label], args, xephyr.pid) | {"rep": rep}
                sets.append(result)
                print(f"rep {rep} {label}: {result['reason'] or 'complete'}", file=sys.stderr)
        built, problems = build_doc(sets, args)
        complete = not problems
        doc |= built | {"verdict": "MEASURED" if complete else "PARTIAL", "exit_code": 0 if complete else 1,
                        "problems": problems}
        return doc["exit_code"]
    finally:
        teardown = ctx.teardown()
        doc |= {"run_dir": str(run), "display": ctx.display, "geometry": args.geometry, "reps": args.reps,
                "idle_seconds": args.idle_seconds, "idle_samples": args.idle_samples, "samples": args.samples,
                "start_cycles": args.start_cycles, "minimize_cycles": args.minimize_cycles,
                "drag_cycles": args.drag_cycles, "scenario": args.scenario or "default",
                "icon_theme": icon_scenario(args),
                "settings": {"surfaceOpacity": args.surface_opacity,
                             "surfaceBlurPercent": args.surface_blur},
                "target_bytes": mem.TARGET_BYTES,
                "binaries": {label: {name: {"path": str(bins[label] / name),
                                            "sha256": perf.sha256(bins[label] / name)}
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
        args.run_dir = f"/tmp/claude-1000/mem-phase-runs/{ap.utc_now()}"
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

#!/usr/bin/env python3
"""Whole-session memory measurement for an installed FlameWM session (stdlib only).

This is the session mode behind `tools/bench_mem.sh --session`. It answers the
question the legacy per-process listing could not answer: what does the
*complete* running session cost, including the Python session services and the
X server that owns the graphical resources, at a declared resolution with open
applications?

Policy:

* every FlameWM process on the display is reported per PID with PSS and RSS;
* `flamewm-desktop-services` (the Python session companion) is reported
  explicitly and its absence is stated, never swallowed;
* the X server that actually serves the display (`Xorg`, `Xephyr` or
  `Xwayland`) is included in the session total; it is identified by owning the
  `/tmp/.X11-unix/X<n>` socket, with the command line as a fallback;
* an absent optional component, an unreadable process, or an absent baseline
  desktop produces a printed `SKIPPED`/`excluded` line instead of a failure or
  an invented number;
* a baseline (XFCE or IceWM) is compared only when its binaries exist and a
  live baseline display is supplied; otherwise the line says why.

Nothing is fabricated: every number comes from `/proc/<pid>/smaps_rollup` of a
PID that belongs to the measured display, and every omission is printed.

Usage:
  python3 -I tools/bench_flamewm_session_memory.py --display :0
  python3 -I tools/bench_flamewm_session_memory.py --display :167 --apps 3 --launch \\
      --baseline xfce --baseline-display :168 --json /tmp/session.json

Exit codes: 0 measured, 2 usage error, 3 no FlameWM process matched the display.
"""
from __future__ import annotations

import argparse
import json
from datetime import datetime, timezone
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time

DEFAULT_RESOLUTION = "1920x1080"
DEFAULT_APPS = 3
DEFAULT_SOCKET_DIR = Path("/tmp/.X11-unix")
MIB = 1024

# exe basename -> role
FLAMEWM_ROLES = {
    "flamewm": "wm",
    "flamewm-desktop": "desktop",
    "flamewm-shell": "shell",
    "flamewm-settings": "settings",
    "flamewm-xsettings": "xsettings",
    "flamewm-session": "session",
    "flamewm-wayland": "wayland",
    "flamewm-audio-helper": "audio-helper",
    "flamewm-terminal": "terminal",
    "flamewm-explorer": "explorer",
    "flamewm-lock": "lock",
    "flamewm-screenshot": "screenshot",
    "flamewm-task-manager": "task-manager",
    "flamewm-finder": "finder",
}
SERVER_EXECUTABLES = {"Xorg": "xorg", "Xephyr": "xephyr", "Xwayland": "xwayland"}
# Session components: expected in every session and never an application window.
CORE_ROLES = ("session", "wm", "desktop", "shell", "desktop-services",
              "xsettings", "audio-helper", "wayland")
APP_ROLES = ("terminal", "explorer", "settings", "lock", "screenshot",
             "task-manager", "finder")
# A total that omits one of these is not a session total.
REQUIRED_CORE = ("wm", "desktop", "shell", "desktop-services")
# The installed bridge is a Python program: usr/libexec/flamewm/flamewm-desktop-services
# (shebang) or `python3 tools/flamewm_desktop_services.py` in a checkout.
HELPER_NAMES = {"flamewm-desktop-services", "flamewm_desktop_services.py"}
APP_CANDIDATES = ("flamewm-terminal", "flamewm-explorer", "flamewm-settings",
                  "xterm", "xclock", "xcalc", "xeyes")
BASELINES = {"xfce": ("startxfce4", "xfce4-session"),
             "icewm": ("icewm-session", "icewm")}
# We can attribute these to an ancestor only if the ancestor chain is in scope,
# which is why the FlameWM processes are matched from their own DISPLAY.
MAX_ANCESTOR_HOPS = 5


# ---------------------------------------------------------------- /proc truth


def _read_text(path: Path) -> str | None:
    try:
        return path.read_text(encoding="ascii", errors="replace")
    except OSError:
        return None


def exe_basename(proc_root: Path, pid: int) -> str | None:
    try:
        return Path(os.readlink(proc_root / str(pid) / "exe").replace(" (deleted)", "")).name
    except OSError:
        return None


def comm_of(proc_root: Path, pid: int) -> str | None:
    text = _read_text(proc_root / str(pid) / "comm")
    return text.strip() if text else None


def argv_of(proc_root: Path, pid: int) -> list[str]:
    try:
        raw = (proc_root / str(pid) / "cmdline").read_bytes()
    except OSError:
        return []
    return [arg for arg in raw.decode("utf-8", "replace").split("\0") if arg]


def proc_stat(proc_root: Path, pid: int) -> tuple[int | None, int | None]:
    """(ppid, start_ticks), robust against parentheses in comm."""
    text = _read_text(proc_root / str(pid) / "stat")
    if not text:
        return None, None
    tail = text.rsplit(")", 1)[-1].split()
    if len(tail) < 20 or not tail[1].isdigit() or not tail[19].isdigit():
        return None, None
    return int(tail[1]), int(tail[19])


def real_uid(proc_root: Path, pid: int) -> int | None:
    text = _read_text(proc_root / str(pid) / "status") or ""
    for line in text.splitlines():
        if line.startswith("Uid:"):
            fields = line.split()
            if len(fields) > 1 and fields[1].isdigit():
                return int(fields[1])
            return None
    return None


def environ_has_display(proc_root: Path, pid: int, display: str) -> bool:
    try:
        raw = (proc_root / str(pid) / "environ").read_bytes()
    except OSError:
        return False
    return f"DISPLAY={display}".encode() in raw.split(b"\0")


def display_socket_inodes(display: str, *, socket_dir: Path = DEFAULT_SOCKET_DIR,
                          net_unix: Path = Path("/proc/net/unix")) -> set[int]:
    """Kernel socket inodes listening on /tmp/.X11-unix/X<n>.

    A filesystem `st_ino` is NOT the socket inode: the kernel socket has its own
    inode in /proc/net/unix (`ss -x` shows the same value). Using the file inode
    silently matches nothing, which is how an X server "disappears" from a
    memory total. Only the /proc/net/unix inode can be matched against
    /proc/<pid>/fd -> `socket:[inode]`.
    """
    match = re.fullmatch(r":(\d+)(?:\.\d+)?", display or "")
    if not match:
        return set()
    name = f"X{match.group(1)}"
    paths = {f"{socket_dir}/{name}", f"@{socket_dir}/{name}"}
    text = _read_text(net_unix) or ""
    inodes: set[int] = set()
    for line in text.splitlines()[1:]:
        fields = line.split()
        if len(fields) >= 8 and fields[7] in paths and fields[6].isdigit():
            inodes.add(int(fields[6]))
    if not inodes:
        # No /proc/net/unix entry (hidden by a namespace or an older kernel):
        # the server is then matched only by environment and command line.
        return set()
    return inodes


def owns_display_socket(proc_root: Path, pid: int, inodes: set[int]) -> bool:
    """True when `pid` holds a listening unix socket for this display."""
    if not inodes:
        return False
    try:
        descriptors = list((proc_root / str(pid) / "fd").iterdir())
    except OSError:
        return False
    wanted = {f"socket:[{inode}]" for inode in inodes}
    for descriptor in descriptors:
        try:
            if os.readlink(descriptor) in wanted:
                return True
        except OSError:
            continue
    return False


def read_rollup(proc_root: Path, pid: int) -> dict[str, int]:
    """PSS/RSS in KiB from smaps_rollup (the kernel prints these in kB)."""
    text = _read_text(proc_root / str(pid) / "smaps_rollup")
    if text is None:
        raise OSError("smaps_rollup unreadable")
    values: dict[str, int] = {}
    for line in text.splitlines():
        key, sep, rest = line.partition(":")
        if not sep or key not in ("Rss", "Pss"):
            continue
        fields = rest.split()
        if not fields or not fields[0].isdigit():
            raise ValueError(f"invalid {key} field")
        values[key] = int(fields[0])
    if "Rss" not in values or "Pss" not in values:
        raise ValueError("smaps_rollup missing Rss/Pss")
    return {"rss_kib": values["Rss"], "pss_kib": values["Pss"]}


def classify(executable: str | None, comm: str | None,
             argv: list[str]) -> tuple[str | None, str | None]:
    """Return (role, class); (None, None) when the process is not ours.

    A server owned by another user has no readable /proc/<pid>/exe, so the
    kernel comm name is the fallback identity. It is reported but never
    silently counted with a made-up number.
    """
    if executable in SERVER_EXECUTABLES:
        return SERVER_EXECUTABLES[executable], "server"
    role = FLAMEWM_ROLES.get(executable or "")
    if role:
        return role, ("core" if role in CORE_ROLES else "app")
    if executable and executable.startswith("python"):
        # Installed bridge: argv[0] is the script; checkout: argv[1] is the script.
        if any(Path(arg).name in HELPER_NAMES for arg in argv[:2]):
            return "desktop-services", "core"
    if executable is None and comm in SERVER_EXECUTABLES:
        return SERVER_EXECUTABLES[comm], "server"
    return None, None


def collect(proc_root: Path = Path("/proc"), display: str = "", *,
            uid: int | None = None, socket_dir: Path = DEFAULT_SOCKET_DIR,
            net_unix: Path = Path("/proc/net/unix")) -> tuple[list[dict], list[dict]]:
    """Measure every known FlameWM/X-server process belonging to `display`.

    Returns (rows, skipped). `skipped` holds processes this display owns whose
    memory could not be read; they are printed as exclusions, never as zeros.
    """
    uid = os.getuid() if uid is None else uid
    inodes = display_socket_inodes(display, socket_dir=socket_dir, net_unix=net_unix)
    candidates: dict[int, dict] = {}
    identify_errors: list[dict] = []
    for entry in proc_root.iterdir():
        if not entry.name.isdigit():
            continue
        pid = int(entry.name)
        executable = exe_basename(proc_root, pid)
        comm = comm_of(proc_root, pid)
        argv = argv_of(proc_root, pid)
        role, klass = classify(executable, comm, argv)
        if role is None:
            continue
        owner = real_uid(proc_root, pid)
        if owner is not None and owner != uid:
            # Another user's process. Only an X server can still be ours by
            # socket ownership, and its memory is unreadable either way.
            if klass == "server" and owns_display_socket(proc_root, pid, inodes):
                identify_errors.append({
                    "pid": pid, "role": role, "class": klass,
                    "reason": f"foreign-uid-{owner}-smaps_rollup-and-environ-unreadable",
                })
            continue
        if owner is None and klass != "server":
            continue
        parent, start = proc_stat(proc_root, pid)
        matched = (environ_has_display(proc_root, pid, display)
                   or display in argv
                   or (klass == "server" and owns_display_socket(proc_root, pid, inodes)))
        candidates[pid] = {"pid": pid, "ppid": parent, "start_ticks": start,
                           "role": role, "class": klass, "exe": executable or comm,
                           "argv": argv, "matched": matched}
    # A helper inherits the display from a matched FlameWM ancestor; that is the
    # only reason to pull in a process with no DISPLAY of its own.
    for _ in range(MAX_ANCESTOR_HOPS):
        changed = False
        for item in candidates.values():
            if item["matched"] or candidates.get(item["ppid"], {}).get("matched") is not True:
                continue
            item["matched"] = True
            changed = True
        if not changed:
            break
    rows: list[dict] = []
    skipped = identify_errors
    for item in sorted(candidates.values(), key=lambda value: value["pid"]):
        if not item["matched"]:
            continue
        try:
            rollup = read_rollup(proc_root, item["pid"])
        except (OSError, ValueError) as exc:
            skipped.append({"pid": item["pid"], "role": item["role"],
                            "class": item["class"], "reason": f"{type(exc).__name__}:{exc}"})
            continue
        rows.append({**item, **rollup})
    rows.sort(key=lambda row: (row["class"] != "server", row["role"], row["pid"]))
    return rows, skipped


# --------------------------------------------------------------- measurement


def application_selection(count: int, which=shutil.which) -> tuple[list[str], list[str]]:
    """First `count` application binaries this machine has, plus the ones it lacks."""
    chosen: list[str] = []
    missing: list[str] = []
    for name in APP_CANDIDATES:
        if len(chosen) >= count:
            missing.append(name)
            continue
        if which(name):
            chosen.append(name)
        else:
            missing.append(name)
    return chosen, missing


def launch_applications(display: str, count: int, *, which=shutil.which,
                        spawn=subprocess.Popen, settle: float = 5.0) -> dict:
    """Start up to `count` applications on the display. Never fatal."""
    chosen, missing = application_selection(count, which)
    launched: list[str] = []
    failed: list[str] = []
    for name in chosen:
        path = which(name)
        if not path:
            missing.append(name)
            continue
        try:
            spawn([path], env={**os.environ, "DISPLAY": display},
                  stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                  start_new_session=True, cwd="/")
            launched.append(name)
        except OSError:
            failed.append(name)
    if launched and settle > 0:
        time.sleep(settle)
    return {"requested": count, "launched": launched, "failed": failed,
            "missing": missing}


def summarize(rows: list[dict]) -> dict:
    def group(items: list[dict]) -> dict:
        return {"processes": len(items),
                "pss_kib": sum(row["pss_kib"] for row in items),
                "rss_kib": sum(row["rss_kib"] for row in items)}

    core = [row for row in rows if row["class"] == "core"]
    apps = [row for row in rows if row["class"] == "app"]
    servers = [row for row in rows if row["class"] == "server"]
    present = {row["role"] for row in rows}
    flamewm = core + apps
    return {
        "core": group(core),
        "apps": group(apps),
        "servers": group(servers),
        "flamewm": group(flamewm),
        "session": group(flamewm + servers),
        "present_roles": sorted(present),
        "missing_required_core": [role for role in REQUIRED_CORE if role not in present],
        "missing_server": not servers,
    }


def manifest(rows: list[dict], skipped: list[dict], summary: dict) -> dict:
    measured = []
    for row in sorted(rows, key=lambda row: (row["class"], row["role"], row["pid"])):
        measured.append(f"measured=role={row['role']} class={row['class']} pid={row['pid']} "
                        f"pss_kib={row['pss_kib']} rss_kib={row['rss_kib']}")
    excluded = []
    for role in REQUIRED_CORE:
        if role in summary["present_roles"]:
            continue
        reason = ("not-running-python-session-services-bridge-omitted-from-total"
                  if role == "desktop-services" else "not-running-required-session-component")
        excluded.append(f"excluded=role={role} reason={reason}")
    for role in CORE_ROLES:
        if role not in REQUIRED_CORE and role not in summary["present_roles"]:
            excluded.append(f"excluded=role={role} reason=not-running-in-this-session")
    for role in APP_ROLES:
        if role not in summary["present_roles"]:
            excluded.append(f"excluded=role={role} reason=no-open-application")
    if summary["missing_server"]:
        excluded.append("excluded=role=x-server reason=no-readable-Xorg/Xephyr/Xwayland-owns-"
                        "this-display-so-server-side-graphics-are-not-in-the-total")
    for item in skipped:
        excluded.append(f"excluded=role={item['role']} pid={item['pid']} "
                        f"reason={item['reason']}")
    return {"measured": measured, "excluded": excluded}


def render(rows: list[dict], summary: dict, manifest_report: dict, *, display: str,
           label: str, resolution: str, apps_requested: int, apps: dict | None,
           warnings: list[str]) -> list[str]:
    stamp = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    out = [f"## FlameWM whole session: {label} (display {display}, {resolution}, {stamp})"]
    for warning in warnings:
        out.append(f"WARNING: {warning}")
    out.append("")
    out.append("| process | role | class | pid | PSS MiB | RSS MiB |")
    out.append("|---|---|---|---|---|---|")
    for row in sorted(rows, key=lambda row: row["pss_kib"], reverse=True):
        out.append(f"| {row['exe']} | {row['role']} | {row['class']} | {row['pid']} "
                   f"| {row['pss_kib'] / MIB:.1f} | {row['rss_kib'] / MIB:.1f} |")
    out.append("")
    out.append("| total | what it covers | PSS MiB | RSS MiB | processes |")
    out.append("|---|---|---|---|---|")
    coverage = (
        ("core", "FlameWM session components (WM, desktop, shell, session, Python services)"),
        ("apps", "applications open on the display"),
        ("flamewm", "FlameWM total: core + applications"),
        ("servers", "the X server serving this display"),
        ("session", "SESSION TOTAL: FlameWM + X server"),
    )
    for key, what in coverage:
        entry = summary[key]
        out.append(f"| `{key}` | {what} | {entry['pss_kib'] / MIB:.1f} "
                   f"| {entry['rss_kib'] / MIB:.1f} | {entry['processes']} |")
    out.append("")
    services = next((row for row in rows if row["role"] == "desktop-services"), None)
    if services is None:
        out.append("desktop_services=ABSENT (flamewm-desktop-services is NOT running on this "
                   "display: the session total above omits the Python session services)")
    else:
        out.append(f"desktop_services=present pid={services['pid']} "
                   f"pss_kib={services['pss_kib']} rss_kib={services['rss_kib']}")
    if apps is not None:
        out.append(f"apps=requested:{apps['requested']} launched:{len(apps['launched'])} "
                   f"measured:{summary['apps']['processes']} "
                   f"launched_names:{','.join(apps['launched']) or 'none'} "
                   f"missing_binaries:{','.join(apps['missing']) or 'none'}")
    else:
        out.append(f"apps=requested:{apps_requested} launched:0 "
                   f"measured:{summary['apps']['processes']} "
                   f"launched_names:none (pass --launch to open applications)")
    complete = not summary["missing_required_core"] and not summary["missing_server"]
    out.append("total_scope=" + ("COMPLETE" if complete else "PARTIAL")
               + " missing=" + (",".join(summary["missing_required_core"]) or "none")
               + " x_server=" + ("missing" if summary["missing_server"] else "included")
               + " resolution_source=declared")
    out.extend(manifest_report["measured"])
    out.extend(manifest_report["excluded"])
    return out


# ------------------------------------------------------------------ baselines


def resolve_baseline(requested: str, which=shutil.which) -> tuple[str | None, list[str]]:
    """Pick an installed baseline desktop. Returns (name or None, missing deps)."""
    if requested == "none":
        return None, []
    order = ["xfce", "icewm"] if requested == "auto" else [requested]
    absent: list[str] = []
    for name in order:
        present = [dep for dep in BASELINES[name] if which(dep)]
        if present:
            return name, [dep for dep in BASELINES[name] if not which(dep)]
        absent.extend(BASELINES[name])
    return None, absent


def baseline_lines(requested: str, resolution: str, apps_requested: int, *,
                   which=shutil.which, baseline_display: str | None,
                   baseline_rows: list[dict] | None,
                   baseline_apps: dict | None) -> list[str]:
    if requested == "none":
        return ["baseline=none reason=not-requested comparison=NOT-MEASURED"]
    name, missing = resolve_baseline(requested, which)
    if name is None:
        return [f"SKIPPED baseline={requested} reason=missing-dependency:"
                f"{','.join(sorted(set(missing))) or 'xfce4-session,startxfce4,icewm,icewm-session'}"
                f" comparison=NOT-MEASURED no_baseline_desktop_installed_on_this_machine"]
    lines = []
    if missing:
        lines.append(f"WARN baseline={name} partial-install missing_optional:"
                     f"{','.join(sorted(missing))}")
    if baseline_rows is None:
        launcher = BASELINES[name][0]
        lines.append(f"baseline={name} status=READY resolution={resolution} "
                     f"apps={apps_requested} comparison=NOT-MEASURED "
                     f"next: start it on a display of the same resolution and rerun with "
                     f"--baseline {name} --baseline-display :167 --launch "
                     f"(for example `Xephyr :167 -screen {resolution} & "
                     f"DISPLAY=:167 {launcher}`)")
        return lines
    summary = summarize(baseline_rows)
    lines.append(f"baseline={name} resolution={resolution} "
                 f"apps_measured={summary['apps']['processes']} "
                 f"session_pss_kib={summary['session']['pss_kib']} "
                 f"session_rss_kib={summary['session']['rss_kib']} "
                 f"x_server={'included' if not summary['missing_server'] else 'missing'}")
    if baseline_apps is not None:
        lines.append(f"baseline_apps={name} launched={','.join(baseline_apps['launched']) or 'none'}"
                     f" missing_binaries={','.join(baseline_apps['missing']) or 'none'}")
    return lines


def comparison_table(flamewm_summary: dict, baseline_name: str,
                     baseline_summary: dict, *, resolution: str) -> list[str]:
    out = ["", "| session | resolution | session PSS MiB | session RSS MiB | processes |",
           "|---|---|---|---|---|"]
    for name, summary in (("FlameWM", flamewm_summary), (baseline_name, baseline_summary)):
        out.append(f"| {name} | {resolution} | {summary['session']['pss_kib'] / MIB:.1f} "
                   f"| {summary['session']['rss_kib'] / MIB:.1f} "
                   f"| {summary['session']['processes']} |")
    out.append("One sample on one machine. This is not a ranking.")
    return out


# ----------------------------------------------------------------------- main


def parse_resolution(text: str) -> str:
    match = re.fullmatch(r"(\d{3,5})x(\d{3,5})", (text or "").strip())
    if not match:
        raise argparse.ArgumentTypeError("expected WIDTHxHEIGHT, for example 1920x1080")
    return f"{int(match.group(1))}x{int(match.group(2))}"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--display", default=os.environ.get("BENCH_DISPLAY")
                        or os.environ.get("DISPLAY"),
                        help="X display of the session (default $BENCH_DISPLAY or $DISPLAY)")
    parser.add_argument("--label", default="session")
    parser.add_argument("--resolution", type=parse_resolution, default=DEFAULT_RESOLUTION,
                        help=f"declared session resolution (default {DEFAULT_RESOLUTION})")
    parser.add_argument("--apps", type=int, default=DEFAULT_APPS,
                        help=f"applications to open/expect (default {DEFAULT_APPS})")
    parser.add_argument("--launch", action="store_true",
                        help="start the selected applications on the display before measuring")
    parser.add_argument("--settle", type=float, default=5.0,
                        help="seconds to wait after launching applications")
    parser.add_argument("--baseline", default="auto", choices=("auto", "xfce", "icewm", "none"))
    parser.add_argument("--baseline-display", default=None,
                        help="display of an already running baseline session to measure identically")
    parser.add_argument("--json", default=None, help="write the machine-readable report here")
    parser.add_argument("--proc-root", default="/proc", help=argparse.SUPPRESS)
    parser.add_argument("--socket-dir", default=str(DEFAULT_SOCKET_DIR), help=argparse.SUPPRESS)
    parser.add_argument("--net-unix", default="/proc/net/unix", help=argparse.SUPPRESS)
    args = parser.parse_args(argv)
    if args.apps < 0:
        parser.error("--apps must be >= 0")
    if not args.display:
        parser.error("--display is required, or set BENCH_DISPLAY/DISPLAY")

    proc_root = Path(args.proc_root)
    socket_dir = Path(args.socket_dir)
    net_unix = Path(args.net_unix)
    warnings: list[str] = []
    apps = None
    if args.launch and args.apps:
        apps = launch_applications(args.display, args.apps, settle=args.settle)
        if apps["missing"]:
            warnings.append("application binaries missing, skipped: "
                            + ", ".join(apps["missing"]))
        if apps["failed"]:
            warnings.append("applications failed to start: " + ", ".join(apps["failed"]))

    rows, skipped = collect(proc_root, args.display, socket_dir=socket_dir,
                            net_unix=net_unix)
    if not rows and not skipped:
        print(f"NO_FLAMEWM_PROCESSES display={args.display} "
              f"(no FlameWM process and no readable X server belongs to it)", file=sys.stderr)
        return 3
    summary = summarize(rows)
    if summary["missing_required_core"]:
        warnings.append("the session total is PARTIAL; missing: "
                        + ", ".join(summary["missing_required_core"]))
    if summary["missing_server"]:
        warnings.append(f"no readable X server owns display {args.display}; server-side "
                        f"graphical memory is inside none of the totals below")
    report = manifest(rows, skipped, summary)

    baseline_name = resolve_baseline(args.baseline)[0]
    baseline_rows = None
    baseline_apps = None
    if baseline_name and args.baseline_display:
        if args.launch and args.apps:
            baseline_apps = launch_applications(args.baseline_display, args.apps,
                                                settle=args.settle)
        candidate_rows, _ = collect(proc_root, args.baseline_display,
                                    socket_dir=socket_dir, net_unix=net_unix)
        if candidate_rows:
            baseline_rows = candidate_rows
        else:
            warnings.append(f"baseline display {args.baseline_display} exposed no measurable "
                            f"process; no comparison was produced")

    lines = render(rows, summary, report, display=args.display, label=args.label,
                   resolution=args.resolution, apps_requested=args.apps, apps=apps,
                   warnings=warnings)
    lines.extend(baseline_lines(args.baseline, args.resolution, args.apps,
                                baseline_display=args.baseline_display,
                                baseline_rows=baseline_rows, baseline_apps=baseline_apps))
    if baseline_rows:
        lines.extend(comparison_table(summary, baseline_name, summarize(baseline_rows),
                                      resolution=args.resolution))
    print("\n".join(lines))

    if args.json:
        document = {
            "schema": "flamewm-session-memory/1",
            "measured_at": datetime.now(timezone.utc).isoformat(),
            "display": args.display,
            "label": args.label,
            "resolution": {"value": args.resolution, "source": "declared",
                           "verified": False},
            "scope": {"nested_xephyr": False, "installed_session": True,
                      "x_server_included": not summary["missing_server"],
                      "python_services_included":
                          "desktop-services" in summary["present_roles"]},
            "apps_requested": args.apps,
            "apps": apps,
            "processes": [{key: row.get(key) for key in
                           ("pid", "role", "class", "exe", "pss_kib", "rss_kib")}
                          for row in rows],
            "totals_kib": {key: summary[key] for key in
                           ("core", "apps", "flamewm", "servers", "session")},
            "missing_required_core": summary["missing_required_core"],
            "manifest": report,
            "baseline": {"requested": args.baseline, "resolved": baseline_name,
                         "display": args.baseline_display,
                         "totals_kib": summarize(baseline_rows) if baseline_rows else None},
            "units": "KiB as the kernel reports in smaps_rollup",
        }
        Path(args.json).write_text(json.dumps(document, indent=2, sort_keys=True) + "\n",
                                   encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Ten-second CPU and memory benchmark of a whole FlameWM desktop.

Runs the FlameWM Wayland compositor nested (winit) inside the current desktop, with the shell and
desktop on its XWayland display, in a private D-Bus session with temporary home/config/cache.
The host display and runtime socket are retained only to host the nested compositor. Then:

  open   Starts the terminal, explorer and settings windows ("just opening stuff") and, once they
         settle, records per-process memory: PSS/RSS/USS from smaps_rollup and a breakdown of the
         anonymous memory (heap, other anonymous, file-backed). XWayland is listed too, because
         X11 pixmaps (backbuffers, icons, backdrops) live in the X server, not in the client.
  cpu    A fixed workload for --seconds (default 10): a full-screen `top -d 0.2` in the terminal,
         pointer sweeps over the taskbar and desktop and opening/closing the start menu, all
         injected with XTEST on the nested XWayland display (the host pointer never moves). CPU
         time per process and per thread is sampled from /proc.

The FlameWM profiler runs in every process (FLAMEWM_PROFILE=1); the per-span reports of the
processes that write them land in the output directory next to the JSON report.

Usage:
  python3 -I tools/bench-flamewm-session.py --bin .target/release --out /tmp/flame-bench-001

--out must be a new directory inside the system temp directory or working directory.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

HZ = os.sysconf("SC_CLK_TCK")
PAGE = os.sysconf("SC_PAGE_SIZE")
ROLE_BASENAMES = {
    "flamewm-wayland": "compositor",
    "flamewm-desktop": "desktop",
    "flamewm-shell": "shell",
    "flamewm-terminal": "terminal",
    "flamewm-explorer": "explorer",
    "flamewm-settings": "settings",
    "Xwayland": "xwayland",
}
READY_RE = re.compile(rb"XWayland ready on DISPLAY=:(\d+)")
# Only display/bootstrap configuration crosses into the benchmark processes. In particular,
# never pass API tokens, shell startup hooks, preload libraries or other session secrets.
DISPLAY_ENV = (
    "PATH", "LANG", "LANGUAGE", "LC_ALL", "LC_CTYPE", "LC_MESSAGES",
    "DISPLAY", "WAYLAND_DISPLAY", "XAUTHORITY", "XDG_RUNTIME_DIR",
    "DBUS_SESSION_BUS_ADDRESS", "XCURSOR_THEME", "XCURSOR_SIZE",
)


def ensure_safe_output_dir(path: Path) -> Path:
    """Exclusively create a private output directory; never reuse/delete user data."""
    target = path.expanduser().resolve(strict=False)
    roots = (Path(tempfile.gettempdir()).resolve(), Path.cwd().resolve())
    if not any(target != root and target.is_relative_to(root) for root in roots):
        raise SystemExit(f"Refusing benchmark output outside the temp directory or cwd: {path}")
    try:
        target.mkdir(mode=0o700, parents=True, exist_ok=False)
    except FileExistsError as exc:
        raise SystemExit(f"Refusing existing benchmark output directory: {target}") from exc
    return target


def benchmark_environment(home: Path, out: Path, binaries: Path) -> dict[str, str]:
    """Keep host display connectivity but isolate state and drop unrelated credentials."""
    env = {key: os.environ[key] for key in DISPLAY_ENV if key in os.environ}
    # Changing HOME must not make the host X11 authorization cookie undiscoverable.
    if "XAUTHORITY" not in env and os.environ.get("HOME"):
        authority = Path(os.environ["HOME"]) / ".Xauthority"
        if authority.is_file():
            env["XAUTHORITY"] = str(authority)
    env.update({
        "HOME": str(home),
        "FLAMEWM_WAYLAND_BACKEND": "winit",
        "FLAMEWM_WAYLAND_SOCKET": f"wayland-flame-bench-{os.getpid()}",
        "FLAMEWM_WAYLAND_AUTOSTART": f"{binaries}/flamewm-desktop:{binaries}/flamewm-shell",
        "FLAMEWM_PROFILE": "1",
        "FLAMEWM_PROFILE_INTERVAL": "10",
        "FLAMEWM_PROFILE_DIR": str(out),
        "FLAMEWM_CONFIG_DIR": str(home / "config/flamewm"),
        "XDG_CONFIG_HOME": str(home / "config"),
        "XDG_DATA_HOME": str(home / "data"),
        "XDG_CACHE_HOME": str(home / "cache"),
        "XDG_STATE_HOME": str(home / "state"),
    })
    return env


def validate_xwayland(display: str, compositor_pid: int, host_display: str | None) -> None:
    """Fail closed before injecting XTEST events when the reported display is suspect."""
    host = re.fullmatch(r":(\d+)(?:\.\d+)?", host_display or "")
    if host and display == f":{host.group(1)}":
        raise RuntimeError("Refusing XTEST on the host X11 display")
    if not any(exe_basename(pid) == "Xwayland" for pid in children_of(compositor_pid)):
        raise RuntimeError("No Xwayland descendant of the benchmark compositor; refusing XTEST")


def exe_basename(pid: int) -> str | None:
    try:
        return os.path.basename(os.readlink(f"/proc/{pid}/exe").replace(" (deleted)", ""))
    except OSError:
        return None


def children_of(root: int) -> list[int]:
    """Every live descendant of `root` (and `root`), from /proc/*/stat parent links."""
    parents: dict[int, int] = {}
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        try:
            stat = Path(f"/proc/{entry}/stat").read_text()
        except OSError:
            continue
        fields = stat[stat.rfind(")") + 2 :].split()
        parents[int(entry)] = int(fields[1])
    found = [root]
    frontier = [root]
    while frontier:
        parent = frontier.pop()
        for pid, ppid in parents.items():
            if ppid == parent and pid not in found:
                found.append(pid)
                frontier.append(pid)
    return [pid for pid in found if Path(f"/proc/{pid}").exists()]


def roles(*roots: int) -> dict[int, str]:
    out = {}
    for pid in (pid for root in roots for pid in children_of(root)):
        role = ROLE_BASENAMES.get(exe_basename(pid) or "")
        if role:
            out[pid] = role
    return out


def cpu_ticks(pid: int) -> int:
    stat = Path(f"/proc/{pid}/stat").read_text()
    fields = stat[stat.rfind(")") + 2 :].split()
    return int(fields[11]) + int(fields[12])


def thread_ticks(pid: int) -> dict[str, int]:
    out: dict[str, int] = {}
    for tid in os.listdir(f"/proc/{pid}/task"):
        try:
            stat = Path(f"/proc/{pid}/task/{tid}/stat").read_text()
        except OSError:
            continue
        name = stat[stat.find("(") + 1 : stat.rfind(")")]
        fields = stat[stat.rfind(")") + 2 :].split()
        out[name] = out.get(name, 0) + int(fields[11]) + int(fields[12])
    return out


def memory(pid: int) -> dict[str, int]:
    """kB: smaps_rollup totals plus anonymous memory split into heap, other anon and file."""
    rollup: dict[str, int] = {}
    for line in Path(f"/proc/{pid}/smaps_rollup").read_text().splitlines()[1:]:
        key, _, value = line.partition(":")
        if value.strip().endswith("kB"):
            rollup[key.strip()] = int(value.split()[0])
    split = {"heap_pss": 0, "anon_pss": 0, "file_pss": 0, "gpu_pss": 0}
    mappings: dict[str, int] = {}
    current = None
    label = ""
    for line in Path(f"/proc/{pid}/smaps").read_text().splitlines():
        head = line.split()
        if head and re.match(r"^[0-9a-f]+-[0-9a-f]+$", head[0]):
            path = head[5] if len(head) > 5 else ""
            start, end = (int(part, 16) for part in head[0].split("-"))
            label = path or f"[anon {((end - start) // 1024)} kB]"
            if path == "[heap]":
                current = "heap_pss"
            elif path.startswith("/dev/dri") or "drm" in path:
                current = "gpu_pss"
            elif path.startswith("/") and not path.startswith("/memfd"):
                current = "file_pss"
            else:
                current = "anon_pss"
        elif line.startswith("Pss:") and current:
            split[current] += int(line.split()[1])
            mappings[label] = mappings.get(label, 0) + int(line.split()[1])
    uss = rollup.get("Private_Clean", 0) + rollup.get("Private_Dirty", 0)
    return {
        "pss": rollup.get("Pss", 0),
        "rss": rollup.get("Rss", 0),
        "uss": uss,
        **split,
        "top_mappings_pss": dict(sorted(mappings.items(), key=lambda kv: -kv[1])[:10]),
    }


def xtest(display: str, script, stop: threading.Event) -> None:
    from Xlib import X, XK, display as xdisplay
    from Xlib.ext import xtest as xt

    conn = xdisplay.Display(display)
    screen = conn.screen()
    width, height = screen.width_in_pixels, screen.height_in_pixels

    def move(x: int, y: int) -> None:
        xt.fake_input(conn, X.MotionNotify, x=int(x), y=int(y))
        conn.sync()

    def click(x: int, y: int) -> None:
        move(x, y)
        xt.fake_input(conn, X.ButtonPress, 1)
        xt.fake_input(conn, X.ButtonRelease, 1)
        conn.sync()

    def key(name: str) -> None:
        code = conn.keysym_to_keycode(XK.string_to_keysym(name))
        xt.fake_input(conn, X.KeyPress, code)
        xt.fake_input(conn, X.KeyRelease, code)
        conn.sync()

    def type_text(text: str) -> None:
        names = {" ": "space", "-": "minus", ".": "period", "\n": "Return"}
        for char in text:
            key(names.get(char, char))

    script(width, height, move, click, key, type_text, stop)
    conn.close()


def workload(width, height, move, click, key, type_text, stop) -> None:
    """Fixed, repeatable desktop use: `top` in the terminal, taskbar and desktop hover, start menu."""
    click(width // 2, height // 2)
    type_text("top -d 0.2\n")
    step = 0
    while not stop.is_set():
        phase = step % 40
        if phase < 20:
            move(20 + (width - 40) * phase / 19, height - 12)
        elif phase < 30:
            move(width * 0.2 + width * 0.06 * (phase - 20), height * 0.3)
        elif phase == 30:
            click(18, height - 12)
        elif phase == 35:
            key("Escape")
        step += 1
        stop.wait(0.05)


def wait_ready(log: Path, deadline: float) -> str:
    while time.monotonic() < deadline:
        match = READY_RE.search(log.read_bytes()) if log.exists() else None
        if match:
            return f":{match.group(1).decode()}"
        time.sleep(0.1)
    raise SystemExit("compositor never reported XWayland ready")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--bin", type=Path, default=Path(".target/release"))
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--seconds", type=float, default=10.0)
    parser.add_argument("--settle", type=float, default=3.0)
    args = parser.parse_args()

    if os.environ.get("FLAMEWM_BENCH_PRIVATE_BUS") != "1":
        env = dict(os.environ, FLAMEWM_BENCH_PRIVATE_BUS="1")
        return subprocess.call(["dbus-run-session", "--", sys.executable, "-I", *sys.argv], env=env)

    binaries = args.bin.resolve()
    out = ensure_safe_output_dir(args.out)
    home = Path(tempfile.mkdtemp(prefix="flamewm-bench-"))
    env = benchmark_environment(home, out, binaries)
    log = out / "compositor.log"
    compositor: subprocess.Popen | None = None
    report: dict = {"seconds": args.seconds}
    opened: list[subprocess.Popen] = []
    try:
        with log.open("wb") as stream:
            compositor = subprocess.Popen(
                [str(binaries / "flamewm-wayland")],
                env=env,
                stdout=stream,
                stderr=subprocess.STDOUT,
                start_new_session=True,
            )
        display = wait_ready(log, time.monotonic() + 20)
        validate_xwayland(display, compositor.pid, os.environ.get("DISPLAY"))
        report["display"] = display
        client_env = dict(env, DISPLAY=display)
        client_env.pop("WAYLAND_DISPLAY", None)
        time.sleep(args.settle)
        for program in ("flamewm-terminal", "flamewm-explorer", "flamewm-settings"):
            with (out / f"{program}.log").open("wb") as stream:
                opened.append(
                    subprocess.Popen(
                        [str(binaries / program)],
                        env=client_env,
                        stdout=stream,
                        stderr=subprocess.STDOUT,
                        start_new_session=True,
                    )
                )
        time.sleep(args.settle)
        procs = roles(compositor.pid, *(proc.pid for proc in opened))
        report["open"] = {
            f"{role}:{pid}": memory(pid) for pid, role in sorted(procs.items(), key=lambda kv: kv[1])
        }

        procs = roles(compositor.pid, *(proc.pid for proc in opened))
        start = {pid: cpu_ticks(pid) for pid in procs}
        start_threads = {pid: thread_ticks(pid) for pid in procs}
        stop = threading.Event()
        driver = threading.Thread(
            target=xtest, args=(display, workload, stop), daemon=True
        )
        began = time.monotonic()
        driver.start()
        time.sleep(args.seconds)
        stop.set()
        driver.join(timeout=5)
        elapsed = time.monotonic() - began
        cpu = {}
        for pid, role in procs.items():
            try:
                ticks = cpu_ticks(pid) - start[pid]
                threads = {
                    name: value - start_threads[pid].get(name, 0)
                    for name, value in thread_ticks(pid).items()
                }
            except OSError:
                continue
            cpu[f"{role}:{pid}"] = {
                "cpu_pct": round(100.0 * ticks / HZ / elapsed, 2),
                "threads_pct": {
                    name: round(100.0 * value / HZ / elapsed, 2)
                    for name, value in sorted(threads.items(), key=lambda kv: -kv[1])
                    if value > 0
                },
            }
        report["cpu"] = cpu
        report["after_cpu_memory"] = {
            f"{role}:{pid}": memory(pid) for pid, role in procs.items() if Path(f"/proc/{pid}").exists()
        }
    finally:
        # Each benchmark client owns a new process group. Never signal arbitrary
        # /proc descendants: those PIDs may have been reused by unrelated apps.
        try:
            processes = ([compositor] if compositor is not None else []) + opened
            for proc in processes:
                try:
                    os.killpg(proc.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
            for proc in processes:
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    try:
                        os.killpg(proc.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    proc.wait()
        finally:
            # Only this mkdtemp-created directory is automatically destroyed.
            shutil.rmtree(home, ignore_errors=True)

    (out / "report.json").write_text(json.dumps(report, indent=2))
    # `heap + anon` is what the process allocated; PSS also counts its executable and libraries.
    print(f"{'process':<22}{'cpu %':>8}{'pss kB':>10}{'uss kB':>10}{'heap kB':>10}{'anon kB':>10}")
    for name, mem in sorted(report["open"].items(), key=lambda kv: -kv[1]["pss"]):
        load = report["cpu"].get(name, {}).get("cpu_pct", 0.0)
        print(
            f"{name:<22}{load:>8.2f}{mem['pss']:>10}{mem['uss']:>10}"
            f"{mem['heap_pss']:>10}{mem['anon_pss']:>10}"
        )
    allocated = sum(mem["heap_pss"] + mem["anon_pss"] for mem in report["open"].values())
    print(f"total heap+anon kB {allocated}")
    print(f"total pss kB {sum(mem['pss'] for mem in report['open'].values())}")
    print(f"total cpu % {sum(entry['cpu_pct'] for entry in report['cpu'].values()):.2f}")
    print(f"report: {out / 'report.json'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

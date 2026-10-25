#!/usr/bin/env python3
"""FlameWM session compatibility companion; native services stay authoritative.

Never acts as WM, compositor, XDG MIME resolver, policy daemon, or wallet manager.
The caller owns this process lifetime and sends SIGTERM at logout.
"""
from __future__ import annotations

import argparse
import fcntl
import json
import os
from pathlib import Path
import shutil
import signal
import re
import time
import subprocess
import sys
import threading
from typing import Callable

POLKIT_CANDIDATES = (
    "lxqt-policykit-agent",
    "/usr/libexec/polkit-kde-authentication-agent-1",
    "/usr/lib/polkit-kde-authentication-agent-1",
    "/usr/libexec/polkit-gnome-authentication-agent-1",
    "/usr/lib/policykit-1-gnome/polkit-gnome-authentication-agent-1",
)
NOTIFIER_CANDIDATES = ("dunst", "xfce4-notifyd", "lxqt-notificationd")
CLIPBOARD_CANDIDATES = ("xfce4-clipman", "qlipper", "copyq")
BLUETOOTH_CANDIDATES = ("blueman-applet",)
NOTIFIER_SERVICE_NAMES = (
    "org.freedesktop.Notifications.service",
    "org.xfce.xfce4-notifyd.Notifications.service",
)
SERVICE_ROOTS = ("/usr/share/dbus-1/services", "/usr/local/share/dbus-1/services")


def session_service_roots(env: dict[str, str]) -> tuple[str, ...]:
    """Honor the XDG user directory before system D-Bus activation paths."""
    home = Path(env.get("HOME", str(Path.home())))
    data_home = Path(env.get("XDG_DATA_HOME") or home / ".local/share")
    data_dirs = (env.get("XDG_DATA_DIRS") or "/usr/local/share:/usr/share").split(":")
    return tuple(str(path / "dbus-1/services") for path in (data_home, *(Path(d) for d in data_dirs if d)))


def flamewm_autostart_environment(env: dict[str, str]) -> dict[str, str]:
    """Mirror only Pass 30's session-scoped MIME/Brave overlay for dex children.

    flamewm-session sets these paths in WM/Desktop/Shell child plans, not in
    its own environment. This companion is a sibling process, so without this
    the autostarted applications would silently miss the same XDG overlays.
    No user settings are written and the KDE desktop identity is untouched.
    """
    result = dict(env)
    result["XDG_MENU_PREFIX"] = "flamewm-"
    config_home = Path(env.get("XDG_CONFIG_HOME") or Path(env.get("HOME", str(Path.home()))) / ".config")
    owned = config_home / "flamewm/session-compat"
    config, data = owned / "config", owned / "data"
    if config.is_dir() and data.is_dir():
        for key, overlay, fallback in (
            ("XDG_CONFIG_DIRS", config, "/etc/xdg"),
            ("XDG_DATA_DIRS", data, "/usr/local/share:/usr/share"),
        ):
            current = env.get(key) or fallback
            if str(overlay) not in current.split(":"):
                result[key] = str(overlay) + ":" + current
    return result


def is_flamewm(env: dict[str, str]) -> bool:
    return "flamewm" in {part.casefold() for part in env.get("XDG_CURRENT_DESKTOP", "").split(":")}


def executable(candidate: str, find: Callable[[str], str | None] = shutil.which) -> str | None:
    if candidate.startswith("/"):
        return candidate if Path(candidate).is_file() and os.access(candidate, os.X_OK) else None
    return find(candidate)


def active_process(names: tuple[str, ...], proc: Path = Path("/proc")) -> bool:
    """Avoid starting a second known policy agent, without inspecting process arguments/secrets."""
    if not proc.is_dir():
        return False
    aliases = set(names)
    aliases.update(Path(name).name[:15] for name in names)
    for pid in proc.iterdir():
        if not pid.name.isdigit() or int(pid.name) == os.getpid():
            continue
        try:
            if pid.stat().st_uid == os.getuid():
                if (pid / "comm").read_text(encoding="utf-8").strip() in aliases:
                    return True
        except (OSError, UnicodeError):
            continue
    return False


def notification_activatable(roots: tuple[str, ...] | None = None) -> bool:
    if roots is None:
        roots = session_service_roots(dict(os.environ))
    for root in roots:
        directory = Path(root)
        if any((directory / name).is_file() for name in NOTIFIER_SERVICE_NAMES):
            return True
        # D-Bus service filenames need not match the name they provide.
        if directory.is_dir():
            for service in directory.glob("*.service"):
                try:
                    if any(line.strip() == "Name=org.freedesktop.Notifications"
                           for line in service.read_text(encoding="utf-8").splitlines()):
                        return True
                except (OSError, UnicodeError):
                    continue
    return False


def notification_owned() -> bool:
    busctl = shutil.which("busctl")
    if not busctl:
        return False
    try:
        result = subprocess.run(
            [busctl, "--user", "--no-pager", "--no-legend", "list"],
            capture_output=True, text=True, timeout=3, check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return False
    return result.returncode == 0 and any(
        line.split()[:1] == ["org.freedesktop.Notifications"]
        for line in result.stdout.splitlines()
    )


def activate_notification_service() -> bool:
    """Try the XDG D-Bus owner once, not just a possibly stale .service file.

    A KDE/other provider that successfully owns the well-known name wins.
    A missing/broken activator must not suppress FlameWM's fallback forever.
    """
    busctl = shutil.which("busctl")
    if busctl is None:
        return False
    try:
        result = subprocess.run(
            [busctl, "--user", "call", "org.freedesktop.DBus", "/org/freedesktop/DBus",
             "org.freedesktop.DBus", "StartServiceByName", "su", "org.freedesktop.Notifications", "0"],
            capture_output=True, text=True, timeout=4, check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return False
    # D-Bus only reports activation success; verify the actual owner.
    return result.returncode == 0 and notification_owned()


def notification_fallback_argv(candidate: str, env: dict[str, str]) -> list[str]:
    """Keep the bundled bottom-right appearance scoped to a FlameWM-started dunst."""
    if Path(candidate).name == "dunst":
        package_config = Path("/usr/share/flamewm/dunstrc")
        source_config = Path(__file__).resolve().parents[1] / "packaging/notifications/dunstrc"
        config = package_config if package_config.is_file() else source_config
        if config.is_file():
            return [candidate, "--config", str(config)]
    return [candidate]


def choose_programs(
    env: dict[str, str],
    find: Callable[[str], str | None] = shutil.which,
    process_active: Callable[[tuple[str, ...]], bool] = active_process,
    notifier_available: Callable[[], bool] | None = None,
    notifier_owned: Callable[[], bool] = notification_owned,
) -> dict[str, list[str] | None]:
    if not is_flamewm(env):
        return {"autostart": None, "polkit": None, "notifications": None,
                "clipboard": None, "bluetooth": None}
    if notifier_available is None:
        notifier_available = lambda: notification_activatable(session_service_roots(env))
    dex = executable("dex", find)
    # Use the upstream parser for OnlyShowIn/Hidden/TryExec; do not implement
    # a second, security-sensitive Desktop Entry parser in FlameWM.
    autostart = [dex, "--autostart", "--environment", "FlameWM"] if dex else None
    polkit = None
    polkit_names = tuple(Path(x).name for x in POLKIT_CANDIDATES)
    if not process_active(polkit_names):
        for candidate in POLKIT_CANDIDATES:
            if path := executable(candidate, find):
                polkit = [path]
                break
    notifier = None
    # DBus activation owns an installed notifier; explicitly spawning a second
    # copy would compete for org.freedesktop.Notifications and waste RAM.
    if not notifier_available() and not notifier_owned():
        notifier_names = tuple(Path(x).name for x in NOTIFIER_CANDIDATES)
        if not process_active(notifier_names):
            for candidate in NOTIFIER_CANDIDATES:
                if path := executable(candidate, find):
                    notifier = notification_fallback_argv(path, env)
                    break
    optional: dict[str, list[str] | None] = {"clipboard": None, "bluetooth": None}
    # Clipboard history can retain secrets; never enable persistent capture by default.
    for role, flag, candidates in (
        ("clipboard", "FLAMEWM_ENABLE_CLIPBOARD_HISTORY", CLIPBOARD_CANDIDATES),
        ("bluetooth", "FLAMEWM_ENABLE_BLUETOOTH_APPLET", BLUETOOTH_CANDIDATES),
    ):
        if env.get(flag) == "1" and not process_active(tuple(Path(p).name for p in candidates)):
            for candidate in candidates:
                if path := executable(candidate, find):
                    optional[role] = [path]
                    break
    return {"autostart": autostart, "polkit": polkit,
            "notifications": notifier, **optional}


def session_lock(env: dict[str, str]):
    runtime = Path(env.get("XDG_RUNTIME_DIR", ""))
    # Don't create a fallback lock in /tmp: /run/user/$UID is private per user.
    if not runtime.is_absolute() or not runtime.is_dir() or runtime.stat().st_uid != os.getuid():
        raise RuntimeError("XDG_RUNTIME_DIR must be an existing, user-owned absolute directory")
    fd = os.open(runtime / "flamewm-desktop-services.lock", os.O_CREAT | os.O_RDWR | os.O_CLOEXEC, 0o600)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        os.close(fd)
        return None
    return fd


def wait_for_wm(env: dict[str, str], stop: threading.Event, timeout: float = 25.0) -> bool:
    """One-time bootstrap barrier so XDG apps never race WM/KWallet init."""
    tool = shutil.which("xprop")
    if not tool:
        print("flamewm-services: xprop missing; deferred autostart is unavailable", file=sys.stderr)
        return False
    deadline = time.monotonic() + timeout
    while not stop.is_set() and time.monotonic() < deadline:
        try:
            reply = subprocess.run(
                [tool, "-root", "_NET_SUPPORTING_WM_CHECK"],
                env=env, capture_output=True, text=True, timeout=2, check=False,
            )
            if reply.returncode == 0 and re.search(r"window id # 0x[1-9a-fA-F][0-9a-fA-F]*", reply.stdout):
                return True
            # flamewm-wayland does not publish _NET_SUPPORTING_WM_CHECK on the XWayland root; its
            # readiness is the XWayland server answering the root query, which xprop reports with exit 0.
            if env.get("FLAMEWM_WAYLAND_SESSION") == "1" and reply.returncode == 0:
                return True
        except (OSError, subprocess.TimeoutExpired):
            pass
        stop.wait(0.2)
    return False

def launch_program(role: str, argv: list[str], env: dict[str, str]) -> subprocess.Popen | None:
    try:
        return subprocess.Popen(argv, env=env, close_fds=True, start_new_session=True)
    except OSError as error:
        print(f"flamewm-services: {role} unavailable: {error.strerror}", file=sys.stderr)
        return None


def run_services(env: dict[str, str], stop: threading.Event) -> int:
    if not is_flamewm(env):
        return 0
    lock = session_lock(env)
    if lock is None:
        print("flamewm-services: companion already running; skipping", file=sys.stderr)
        return 0
    children: list[subprocess.Popen] = []
    try:
        if not wait_for_wm(env, stop):
            print("flamewm-services: WM was not ready; skipped autostart/providers", file=sys.stderr)
            return 1 if not stop.is_set() else 0
        # The native session creates these before WM readiness. Import their
        # per-session XDG overlay, not a global KDE desktop identity.
        env = flamewm_autostart_environment(env)
        autostart = choose_programs(env)["autostart"]
        if autostart and not stop.is_set():
            child = launch_program("autostart", autostart, env)
            if child is not None:
                try:
                    status = child.wait(timeout=8)
                    if status:
                        print(f"flamewm-services: dex returned {status}", file=sys.stderr)
                except subprocess.TimeoutExpired:
                    print("flamewm-services: dex startup exceeded 8s; continuing", file=sys.stderr)
            # Some autostart executables fork their real agent. Give them one
            # bounded bootstrap window before deciding whether to start another.
            stop.wait(0.35)
        if not stop.is_set():
            # Recheck after dex: its generic XDG entries may have launched the
            # user's preferred polkit or notification provider already.
            # A .service file does not prove a working notification provider.
            # Prefer an already running owner, explicitly activate an installed
            # provider once, then use dunst as the FlameWM session fallback.
            owned = notification_owned()
            # Prefer a packaged, FlameWM-styled dunst for this session unless
            # the user already has a running notification server. Otherwise
            # explicitly probe an existing D-Bus activator before fallback.
            active = owned or (
                executable("dunst") is None
                and notification_activatable(session_service_roots(env))
                and activate_notification_service()
            )
            plan = choose_programs(env, notifier_available=lambda: active,
                                   notifier_owned=lambda: active or notification_owned())
            for role in ("polkit", "notifications", "clipboard", "bluetooth"):
                argv = plan[role]
                if argv and not stop.is_set():
                    child = launch_program(role, argv, env)
                    if child is not None:
                        children.append(child)
        stop.wait()
    finally:
        for child in children:
            if child.poll() is None:
                child.terminate()
        for child in children:
            try:
                child.wait(timeout=2)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
        os.close(lock)
    return 0


def doctor(env: dict[str, str]) -> dict:
    plan = choose_programs(env)
    return {
        "desktop": env.get("XDG_CURRENT_DESKTOP", ""),
        "flamewm_session": is_flamewm(env),
        "xdg_autostart_runner": bool(plan["autostart"]),
        "polkit_agent_candidate": (plan["polkit"] or [None])[0],
        "notifications_dbus_activatable": notification_activatable(session_service_roots(env)),
        "notifications_dbus_owned": notification_owned(),
        "notification_fallback_candidate": (plan["notifications"] or [None])[0],
        "portal_config_present": Path("/usr/share/xdg-desktop-portal/flamewm-portals.conf").is_file(),
        "session_xdg_overlay_available": (Path(env.get("XDG_CONFIG_HOME") or Path(env.get("HOME", str(Path.home()))) / ".config") / "flamewm/session-compat/config").is_dir(),
        "existing_clipboard_manager": active_process(("xfce4-clipman", "qlipper", "copyq", "clipit")),
        "udisks2_available": shutil.which("udisksctl") is not None,
        "gvfs_available": shutil.which("gio") is not None,
        "security_note": "Does not inspect wallet secrets or browser profiles",
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--run", action="store_true", help="run companion until logout signal")
    group.add_argument("--plan", action="store_true", help="show selected process argv, no changes")
    group.add_argument("--doctor", action="store_true", help="read-only JSON integration diagnostics")
    args = parser.parse_args(argv)
    env = dict(os.environ)
    if args.plan:
        print(json.dumps(choose_programs(env), indent=2))
        return 0
    if args.doctor:
        print(json.dumps(doctor(env), indent=2, sort_keys=True))
        return 0
    stop = threading.Event()
    def handle_stop(_signal, _frame):
        stop.set()
    signal.signal(signal.SIGTERM, handle_stop)
    signal.signal(signal.SIGINT, handle_stop)
    signal.signal(signal.SIGHUP, handle_stop)
    try:
        return run_services(env, stop)
    except (OSError, RuntimeError) as error:
        print(f"flamewm-services: integration degraded: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())

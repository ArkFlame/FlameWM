#!/usr/bin/env python3
"""Read-only FlameWM installed-session diagnostic (no secrets, no mutations).

Use when reporting monitor swapping, file associations or KDE Wallet access.
The output is safe to attach after reviewing arbitrary command stderr.
"""
from __future__ import annotations
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys


def run(args: list[str], timeout: int = 5) -> dict:
    if shutil.which(args[0]) is None:
        return {"status": "UNAVAILABLE", "command": args[0]}
    try:
        r = subprocess.run(args, text=True, stdout=subprocess.PIPE,
                           stderr=subprocess.PIPE, timeout=timeout, check=False)
        return {"status": "PASS" if r.returncode == 0 else "FAIL",
                "code": r.returncode, "output": r.stdout[:12000]}
    except (subprocess.TimeoutExpired, OSError):
        return {"status": "ERROR", "command": args[0]}


def mime_default_with_desktop(desktop: str, kind: str) -> dict:
    """Read the default application ID only; never launch a program or read a file."""
    if shutil.which("xdg-mime") is None:
        return {"status": "UNAVAILABLE"}
    try:
        result = subprocess.run(["xdg-mime", "query", "default", kind],
                                capture_output=True, text=True, timeout=4, check=False,
                                env={**os.environ, "XDG_CURRENT_DESKTOP": desktop})
        return {"status": "PASS" if result.returncode == 0 else "FAIL",
                "application_id": result.stdout.strip()[:256]}
    except (subprocess.TimeoutExpired, OSError):
        return {"status": "ERROR"}


def outputs_from_xrandr(text: str) -> list[dict]:
    monitors = []
    for line in text.splitlines():
        match = re.match(r"^(\S+) connected(?: primary)?(?: (\d+)x(\d+)\+([\d-]+)\+([\d-]+))?", line)
        if match:
            name, w, h, x, y = match.groups()
            monitors.append({"name": name, "primary": " primary" in line.split('(')[0],
                             "geometry": [int(x), int(y), int(w), int(h)] if all((w, h, x, y)) else None})
    return monitors


def report() -> dict:
    env = {key: os.environ.get(key, "") for key in (
        "XDG_SESSION_TYPE", "XDG_CURRENT_DESKTOP", "DESKTOP_SESSION", "XDG_SESSION_DESKTOP",
        "XDG_MENU_PREFIX", "QT_QPA_PLATFORMTHEME", "GTK_THEME")}
    displays = run(["xrandr", "--query"])
    connected = outputs_from_xrandr(displays.get("output", ""))
    # Query only the names of DBus services. Never introspect wallet contents,
    # browser profiles, usernames, keys or saved credentials.
    services = run(["busctl", "--user", "list", "--no-pager"])
    names = services.get("output", "")
    session = run(["loginctl", "show-session", "self", "-p", "Type", "-p", "Desktop"], 3)
    versions = {"xdg_open": run(["xdg-mime", "query", "default", "text/plain"]),
                "directory": run(["xdg-mime", "query", "default", "inode/directory"]),
                "pdf": run(["xdg-mime", "query", "default", "application/pdf"])}
    for record in versions.values():
        if record.get("output"):
            record["output"] = record["output"].strip()
    assets = Path("/usr/share/flamewm/assets/web/breeze")
    return {
        "environment": env,
        "session": session,
        "randr": {"status": displays["status"], "connected": connected,
                  "exactly_one_primary": sum(bool(x["primary"]) for x in connected) == 1},
        "titlebar_icons": {icon: (assets / f"window-{icon}.svg").is_file()
                            for icon in ("minimize", "maximize", "restore", "close")},
        "mime_defaults": versions,
        "mime_desktop_comparison": {
            desktop: mime_default_with_desktop(desktop, "text/plain")
            for desktop in ("FlameWM", "KDE", "FlameWM:KDE")
        },
        "kscreen_or_kded_active": run(["pgrep", "-f", "kscreen_backend_launcher|kded6|kscreen_daemon"], 2).get("status") == "PASS",
        "kde_wallet": {"bus_status": services["status"],
                       "kwallet_service_visible": "org.kde.kwalletd" in names,
                       "secret_service_visible": "org.freedesktop.secrets" in names},
        "available_launchers": {program: shutil.which(program) is not None
                                for program in ("dolphin", "konsole", "xdg-open", "gio", "kdialog", "x-terminal-emulator")},
        "note": "Read-only probes. A present service does not prove wallet unlocked, password availability, or X11 correctness."
    }

if __name__ == "__main__":
    json.dump(report(), sys.stdout, indent=2, ensure_ascii=False)
    print()

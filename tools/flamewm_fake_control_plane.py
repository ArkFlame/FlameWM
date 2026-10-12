#!/usr/bin/env python3
"""Fake FlameWM control plane for the lock acceptance runner (LK13).

Owns com.arkflame.FlameWM1 on the runner's private session bus, so the lock's
ControlClient reaches this service instead of a WM. It reports the session
capabilities given by --caps, answers Settings.GetSnapshot with an empty
snapshot (the lock then uses its defaults), and appends every Session action
call to --report as one JSON object per line. Only the Session methods that the
lock calls are implemented. It must run inside a private dbus-run-session and
is never installed as a desktop service.

Exit status: 0 after SIGTERM, 3 when dbus-python or GLib bindings are missing.
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

BUS_NAME = "com.arkflame.FlameWM1"
OBJECT_PATH = "/com/arkflame/FlameWM1"
IFACE_SESSION = "com.arkflame.FlameWM1.Session"
IFACE_SETTINGS = "com.arkflame.FlameWM1.Settings"
SESSION_ACTIONS = ("Lock", "Logout", "Suspend", "Reboot", "Shutdown")
ALL_CAPS = ("lock", "logout", "suspend", "reboot", "shutdown")


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("--report", required=True, help="private JSON-lines file of session calls")
    result.add_argument("--caps", default=",".join(ALL_CAPS),
                        help="comma-separated capability names reported by Session.GetCapabilities")
    result.add_argument("--ready", required=True, help="file written once the bus name is owned")
    return result


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        import dbus
        import dbus.service
        from dbus.mainloop.glib import DBusGMainLoop
        from gi.repository import GLib
    except Exception as error:  # pragma: no cover - exact host capability
        print(f"CONTROL_FAKE_UNAVAILABLE: {error}", file=sys.stderr)
        return 3

    DBusGMainLoop(set_as_default=True)
    caps = [name for name in args.caps.split(",") if name]
    report = Path(args.report)

    def record(method: str) -> None:
        with report.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps({"method": method, "monotonic": time.monotonic()}) + "\n")
            handle.flush()

    class ControlPlane(dbus.service.Object):
        @dbus.service.method(IFACE_SESSION, in_signature="", out_signature="as")
        def GetCapabilities(self):
            return dbus.Array(caps, signature="s")

        @dbus.service.method(IFACE_SETTINGS, in_signature="", out_signature="a{sv}")
        def GetSnapshot(self):
            return dbus.Dictionary({}, signature="sv")

    def action(name: str):
        def handler(self):
            record(name)

        handler.__name__ = name
        return dbus.service.method(IFACE_SESSION, in_signature="", out_signature="")(handler)

    for name in SESSION_ACTIONS:
        setattr(ControlPlane, name, action(name))

    bus = dbus.SessionBus()
    bus_name = dbus.service.BusName(BUS_NAME, bus)  # noqa: F841 - keeps the name owned
    ControlPlane(bus, OBJECT_PATH)
    Path(args.ready).write_text("ready\n", encoding="ascii")
    GLib.MainLoop().run()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

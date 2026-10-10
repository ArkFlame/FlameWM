#!/usr/bin/env python3
"""Owned XDG FileChooser portal fixture for isolated FlameWM acceptance runs.

The service deliberately implements only the FileChooser.OpenFile and
Request.Response exchange needed by FlameWM's wallpaper chooser. It must run
inside a private dbus-run-session and is never installed as a desktop service.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path


BUS_NAME = "org.freedesktop.portal.Desktop"
PORTAL_PATH = "/org/freedesktop/portal/desktop"
FILE_CHOOSER = "org.freedesktop.portal.FileChooser"
REQUEST = "org.freedesktop.portal.Request"


def dependencies() -> tuple[object, object, object]:
    try:
        import dbus
        import dbus.service
        from dbus.mainloop.glib import DBusGMainLoop
        from gi.repository import GLib
    except Exception as error:  # pragma: no cover - exact host capability
        raise RuntimeError(f"dbus-python/GLib service bindings unavailable: {error}") from error
    return dbus, DBusGMainLoop, GLib


def atomic_report(path: Path, value: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(json.dumps(value, sort_keys=True) + "\n", encoding="utf-8")
    os.replace(temporary, path)


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("--probe", action="store_true", help="report whether D-Bus service bindings load")
    result.add_argument("--mode", choices=("success", "cancel", "error"))
    result.add_argument("--image", help="absolute fixture image path returned in success mode")
    result.add_argument("--report", help="private JSON invocation report path")
    return result


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        dbus, DBusGMainLoop, GLib = dependencies()
    except RuntimeError as error:
        print(f"PORTAL_FIXTURE_UNAVAILABLE: {error}", file=sys.stderr)
        return 3
    if args.probe:
        print(json.dumps({"dbus_python": True, "glib_main_loop": True}))
        return 0
    if args.mode is None or args.report is None:
        parser().error("--mode and --report are required unless --probe is used")
    if args.mode == "success":
        if not args.image:
            parser().error("--image is required in success mode")
        image = Path(args.image)
        if not image.is_absolute() or not image.is_file():
            parser().error("--image must name an existing absolute fixture path")
    else:
        image = None

    DBusGMainLoop(set_as_default=True)
    report_path = Path(args.report)
    state: dict = {"ready": False, "mode": args.mode, "invocations": [], "error": None}

    class RequestObject(dbus.service.Object):
        def __init__(self, bus, object_path: str) -> None:
            super().__init__(bus, object_path)

        @dbus.service.signal(REQUEST, signature="ua{sv}")
        def Response(self, code, results) -> None:  # noqa: N802 - portal protocol member
            return None

    class PortalObject(dbus.service.Object):
        def __init__(self, bus) -> None:
            super().__init__(bus, PORTAL_PATH)
            self.bus = bus
            self.requests: list[RequestObject] = []

        @dbus.service.method(FILE_CHOOSER, in_signature="ssa{sv}", out_signature="o",
                             sender_keyword="sender")
        def OpenFile(self, parent_window, title, options, sender=None):  # noqa: N802
            if not isinstance(sender, str) or not sender.startswith(":"):
                raise dbus.DBusException("OpenFile did not include a unique caller name")
            token = options.get("handle_token")
            if not isinstance(token, str) or not token:
                raise dbus.DBusException("OpenFile omitted handle_token")
            caller_component = sender[1:].replace(".", "_")
            request_path = f"{PORTAL_PATH}/request/{caller_component}/{token}"
            invocation = {
                "sender": sender,
                "parent_window": str(parent_window),
                "title": str(title),
                "handle_token": token,
                "request_path": request_path,
                "mode": args.mode,
            }
            state["invocations"].append(invocation)
            request = RequestObject(self.bus, request_path)
            self.requests.append(request)
            atomic_report(report_path, state)

            if args.mode == "success":
                results = dbus.Dictionary({"uris": dbus.Array([image.as_uri()], signature="s")},
                                          signature="sv")
                request.Response(dbus.UInt32(0), results)
            elif args.mode == "cancel":
                request.Response(dbus.UInt32(1), dbus.Dictionary({}, signature="sv"))
            else:
                request.Response(dbus.UInt32(2), dbus.Dictionary({}, signature="sv"))
            return dbus.ObjectPath(request_path)

    bus = None
    try:
        bus = dbus.SessionBus()
        name = dbus.service.BusName(BUS_NAME, bus=bus, do_not_queue=True)
        portal = PortalObject(bus)
        state.update({"ready": True, "service_unique_name": bus.get_unique_name(),
                      "service_name": BUS_NAME, "portal_path": PORTAL_PATH})
        atomic_report(report_path, state)
        loop = GLib.MainLoop()
        loop.run()
        del portal, name
    except Exception as error:
        state["error"] = f"{type(error).__name__}: {error}"
        atomic_report(report_path, state)
        print(f"PORTAL_FIXTURE_FAILED: {state['error']}", file=sys.stderr)
        return 1
    finally:
        if bus is not None:
            bus.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

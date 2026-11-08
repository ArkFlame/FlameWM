#!/usr/bin/python3
"""Private freedesktop notification service for screenshot acceptance."""

import json
import sys

from gi.repository import Gio, GLib

XML = """<node><interface name="org.freedesktop.Notifications">
<method name="GetCapabilities"><arg type="as" direction="out"/></method>
<method name="GetServerInformation"><arg type="s" direction="out"/><arg type="s" direction="out"/><arg type="s" direction="out"/><arg type="s" direction="out"/></method>
<method name="Notify"><arg type="s" direction="in"/><arg type="u" direction="in"/><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="as" direction="in"/><arg type="a{sv}" direction="in"/><arg type="i" direction="in"/><arg type="u" direction="out"/></method>
</interface></node>"""

connection = Gio.bus_get_sync(Gio.BusType.SESSION, None)
reply = connection.call_sync("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "RequestName", GLib.Variant("(su)", ("org.freedesktop.Notifications", 4)), GLib.VariantType.new("(u)"), Gio.DBusCallFlags.NONE, -1, None)
if reply.unpack()[0] != 1:
    raise RuntimeError("notification service name unavailable")


def method_call(_connection, _sender, _path, _interface, method, parameters, invocation):
    if method == "Notify":
        args = parameters.unpack()
        with open(sys.argv[1], "a", encoding="utf-8") as log:
            log.write(json.dumps({"app": args[0], "summary": args[3], "body": args[4]}) + "\n")
        invocation.return_value(GLib.Variant("(u)", (1,)))
    elif method == "GetCapabilities":
        invocation.return_value(GLib.Variant("(as)", ([],)))
    elif method == "GetServerInformation":
        invocation.return_value(GLib.Variant("(ssss)", ("G3 fixture", "FlameWM", "1", "1.2")))


connection.register_object("/org/freedesktop/Notifications", Gio.DBusNodeInfo.new_for_xml(XML).interfaces[0], method_call, None, None)
print("ready", flush=True)
GLib.MainLoop().run()

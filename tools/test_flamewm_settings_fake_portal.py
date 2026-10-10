"""Protocol tests for the owned fake XDG wallpaper portal fixture."""

from __future__ import annotations

import json
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest
import zlib
from pathlib import Path

import flamewm_settings_acceptance as acceptance


ROOT = Path(__file__).resolve().parents[1]
HELPER = ROOT / "tools" / "flamewm_settings_fake_portal.py"
def png_chunk(name: bytes, data: bytes) -> bytes:
    body = name + data
    return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)


PNG = (b"\x89PNG\r\n\x1a\n"
       + png_chunk(b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 2, 0, 0, 0))
       + png_chunk(b"IDAT", zlib.compress(b"\x00\xff\x00\x00"))
       + png_chunk(b"IEND", b""))


CLIENT = r'''
import json, os, subprocess, sys, time
import dbus
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib
DBusGMainLoop(set_as_default=True)
helper, report, mode, image = sys.argv[1:]
bus = dbus.SessionBus()
signals = []
argv = [sys.executable, helper, "--mode", mode, "--report", report]
if image: argv += ["--image", image]
server = subprocess.Popen(argv, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
try:
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        try:
            state = json.load(open(report, encoding="utf-8"))
            if state.get("ready"): break
        except (OSError, ValueError): pass
        if server.poll() is not None: raise AssertionError("portal helper exited before becoming ready")
        time.sleep(.02)
    else: raise AssertionError("portal helper readiness timeout")
    loop = GLib.MainLoop()
    def stop_on_signal(code, values, **kw):
        signals.append((int(code), dict(values)))
        loop.quit()
    bus.add_signal_receiver(stop_on_signal, signal_name="Response",
                            dbus_interface="org.freedesktop.portal.Request")
    proxy = bus.get_object("org.freedesktop.portal.Desktop", "/org/freedesktop/portal/desktop")
    chooser = dbus.Interface(proxy, "org.freedesktop.portal.FileChooser")
    path = str(chooser.OpenFile("", "Choose Wallpaper", {"handle_token": "fixture_token"}))
    GLib.timeout_add(3000, lambda: (loop.quit(), False)[1])
    if not signals: loop.run()
    if len(signals) != 1: raise AssertionError("expected exactly one Request.Response signal")
    code, values = signals[0]
    state = json.load(open(report, encoding="utf-8"))
    call = state["invocations"][0]
    expected = "/org/freedesktop/portal/desktop/request/" + bus.get_unique_name()[1:].replace(".", "_") + "/fixture_token"
    assert path == expected, (path, expected)
    assert call["request_path"] == expected, call
    assert call["sender"] == bus.get_unique_name(), call
    assert call["title"] == "Choose Wallpaper" and call["handle_token"] == "fixture_token", call
    wanted = {"success": 0, "cancel": 1, "error": 2}[mode]
    assert code == wanted, (code, wanted)
    if mode == "success":
        assert str(values["uris"][0]).startswith("file://"), values
    assert server.poll() is None
finally:
    server.terminate()
    try: server.wait(timeout=3)
    except subprocess.TimeoutExpired: server.kill(); server.wait(timeout=3)
    bus.close()
'''


class FakePortalTests(unittest.TestCase):
    def test_ss05_requires_all_three_measured_effects(self) -> None:
        self.assertEqual(acceptance.assess_ss05({})[0], acceptance.STATUS_NOT_RUN)
        self.assertEqual(acceptance.assess_ss05({
            "success_applied_without_input": True,
            "cancel_preserved_without_error": True,
            "error_preserved_and_visible": None,
        })[0], acceptance.STATUS_NOT_RUN)
        status, reason = acceptance.assess_ss05({
            "success_applied_without_input": True,
            "cancel_preserved_without_error": True,
            "error_preserved_and_visible": False,
        })
        self.assertEqual(status, acceptance.STATUS_FAIL)
        self.assertIn("error_preserved_and_visible", reason)
        self.assertEqual(acceptance.assess_ss05({
            "success_applied_without_input": True,
            "cancel_preserved_without_error": True,
            "error_preserved_and_visible": True,
        })[0], acceptance.STATUS_PASS)

    def test_dbus_service_bindings_probe(self) -> None:
        result = subprocess.run([sys.executable, str(HELPER), "--probe"],
                                capture_output=True, text=True, timeout=10, check=False)
        if result.returncode == 3:
            self.skipTest(result.stderr.strip())
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout), {"dbus_python": True, "glib_main_loop": True})

    def test_openfile_request_path_and_response_modes(self) -> None:
        runner = shutil.which("dbus-run-session")
        if runner is None:
            self.skipTest("dbus-run-session unavailable")
        with tempfile.TemporaryDirectory(prefix="flamewm-fake-portal-") as temporary:
            root = Path(temporary)
            image = root / "fixture.png"
            image.write_bytes(PNG)
            for mode in ("success", "cancel", "error"):
                with self.subTest(mode=mode):
                    report = root / f"{mode}.json"
                    result = subprocess.run(
                        [runner, "--", sys.executable, "-c", CLIENT,
                         str(HELPER), str(report), mode, str(image) if mode == "success" else ""],
                        capture_output=True, text=True, timeout=15, check=False)
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()

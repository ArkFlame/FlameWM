#!/usr/bin/env python3
"""Native probe for the FlameWM tray battery (nested Xephyr, private D-Bus).

Each case runs in its own private system bus (a fresh dbus-daemon whose
address is exported as DBUS_SYSTEM_BUS_ADDRESS, so the host bus is never
touched). The UPower stand-in serves DisplayDevice properties from argv, or
nothing for the `unavailable` case. Tray crops are compared with ImageMagick
`compare -metric AE`; the click path opens the battery popup and crops it.

Usage:
  python3 -I tools/flamewm_battery_probe.py --bin-dir DIR --display :108 \
      --out DIR [--cases unavailable,87,45,15,5] [--keep]

Internal (fake UPower service, run by the probe itself):
  python3 -I tools/flamewm_battery_probe.py --serve-upower PERCENT STATE
"""

from __future__ import annotations

import argparse
import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TRAY_CROP = (1176, 594, 40, 34)  # x, y, w, h of the tray battery button
TRAY_CLICK = (1196, 611)
UPOWER_NAME = "org.freedesktop.UPower"
UPOWER_PATH = "/org/freedesktop/UPower/devices/DisplayDevice"

# name -> (percent, UPower State) or None for no UPower service at all.
CASES: dict[str, tuple[int, int] | None] = {
    "unavailable": None,
    "87": (87, 1),
    "45": (45, 2),
    "15": (15, 2),
    "5": (5, 2),
    "45s5": (45, 5),
    # Host system bus, no fake UPower: reads the real BAT0 through the host UPower.
    "host": None,
}


def serve_upower(percent: int, state: int) -> int:
    """Fake UPower DisplayDevice on DBUS_SYSTEM_BUS_ADDRESS (private bus)."""
    import dbus
    import dbus.service
    from dbus.mainloop.glib import DBusGMainLoop
    from gi.repository import GLib

    DBusGMainLoop(set_as_default=True)
    props = {
        "Type": dbus.UInt32(2),
        "IsPresent": dbus.Boolean(True),
        "State": dbus.UInt32(state),
        "Percentage": dbus.Double(float(percent)),
        "IsRechargeable": dbus.Boolean(True),
        "Energy": dbus.Double(0.0),
        "TimeToEmpty": dbus.Int64(0),
        "TimeToFull": dbus.Int64(0),
        "Capacity": dbus.Double(100.0),
        "Online": dbus.Boolean(False),
        "Technology": dbus.UInt32(0),
        "Vendor": dbus.String(""),
        "Model": dbus.String(""),
        "Serial": dbus.String(""),
        "Voltage": dbus.Double(0.0),
        "EnergyRate": dbus.Double(0.0),
        "Temperature": dbus.Double(0.0),
        "WarningLevel": dbus.UInt32(0),
        "Luminosity": dbus.Double(0.0),
        "HasHistory": dbus.Boolean(False),
        "HasStatistics": dbus.Boolean(False),
        "NativePath": dbus.String(""),
        "UpdateTime": dbus.UInt64(0),
        "EnergyEmpty": dbus.Double(0.0),
        "EnergyFull": dbus.Double(0.0),
        "EnergyFullDesign": dbus.Double(0.0),
        "ChargeCycles": dbus.Int32(0),
        "EnergyWarning": dbus.Double(0.0),
    }

    class Device(dbus.service.Object):
        @dbus.service.method("org.freedesktop.DBus.Properties", in_signature="ss", out_signature="v")
        def Get(self, iface, name):
            return props[name]

        @dbus.service.method("org.freedesktop.DBus.Properties", in_signature="s", out_signature="a{sv}")
        def GetAll(self, iface):
            return dict(props)

    bus = dbus.SystemBus()
    _name = dbus.service.BusName(UPOWER_NAME, bus)
    Device(bus, UPOWER_PATH)
    print("FAKE_UPOWER_READY", flush=True)
    GLib.MainLoop().run()
    return 0


def start_private_bus() -> tuple[str, int]:
    out = subprocess.run(
        ["dbus-daemon", "--session", "--fork", "--print-address=1", "--print-pid=1", "--nopidfile"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.split()
    return out[0], int(out[1])


def kill_group(pid: int) -> None:
    for sig in (signal.SIGTERM, signal.SIGKILL):
        try:
            os.killpg(pid, sig)
        except ProcessLookupError:
            return
        time.sleep(1)


def wait_display(display: str, timeout: float = 30.0) -> None:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if subprocess.run(["xdpyinfo"], env={**os.environ, "DISPLAY": display},
                          capture_output=True).returncode == 0:
            return
        time.sleep(0.5)
    raise RuntimeError(f"display {display} never became ready")


def screenshot(display: str, out: Path, window: str = "root") -> Path:
    subprocess.run(["import", "-window", window, str(out)],
                   env={**os.environ, "DISPLAY": display}, check=True)
    return out


def crop(src: Path, dst: Path, box: tuple[int, int, int, int]) -> Path:
    x, y, w, h = box
    subprocess.run(["convert", str(src), "-crop", f"{w}x{h}+{x}+{y}", "+repage", str(dst)], check=True)
    return dst


def ae(a: Path, b: Path) -> int:
    """ImageMagick AE pixel count; 0 means identical."""
    proc = subprocess.run(["compare", "-metric", "AE", str(a), str(b), "null:"],
                          capture_output=True, text=True)
    fields = proc.stderr.split()  # ImageMagick prints "N (normalized)"
    return int(float(fields[0])) if fields else 0


def find_popup(display: str) -> str | None:
    from Xlib import display as xdisplay

    d = xdisplay.Display(display)
    for window in d.screen().root.query_tree().children:
        try:
            if window.get_wm_name() == "FlameWM battery" and window.get_attributes().map_state == 2:
                return hex(window.id)
        except Exception:
            continue
    return None


def click(display: str, x: int, y: int) -> None:
    from Xlib import X, display as xdisplay
    from Xlib.ext import xtest

    d = xdisplay.Display(display)
    xtest.fake_input(d, X.MotionNotify, x=x, y=y)
    d.sync()
    time.sleep(0.2)
    xtest.fake_input(d, X.ButtonPress, 1)
    xtest.fake_input(d, X.ButtonRelease, 1)
    d.sync()


def escape(display: str) -> None:
    from Xlib import X, XK, display as xdisplay
    from Xlib.ext import xtest

    d = xdisplay.Display(display)
    code = d.keysym_to_keycode(XK.string_to_keysym("Escape"))
    xtest.fake_input(d, X.KeyPress, code)
    xtest.fake_input(d, X.KeyRelease, code)
    d.sync()


def run_case(name: str, args: argparse.Namespace, out: Path) -> dict:
    case_dir = out / name
    case_dir.mkdir(parents=True, exist_ok=True)
    bus_address, bus_pid = start_private_bus()
    env = {
        **os.environ,
        "DBUS_SYSTEM_BUS_ADDRESS": bus_address,
        "PATH": f"{args.bin_dir}:{os.environ['PATH']}",
        "FLAMEWM_DEBUG": "0",
        "FLAMEWM_XEPHYR_DISPLAY": args.display,
        "FLAMEWM_XEPHYR_BUILD": "0",
        "FLAMEWM_XEPHYR_WM_BIN": str(Path(args.bin_dir) / "flamewm"),
        "FLAMEWM_XEPHYR_DESKTOP_BIN": str(Path(args.bin_dir) / "flamewm-desktop"),
        "FLAMEWM_XEPHYR_SHELL_BIN": str(Path(args.bin_dir) / "flamewm-shell"),
        "FLAMEWM_XEPHYR_OUTPUT_DIR": str(case_dir / "out"),
        "FLAMEWM_XEPHYR_EVIDENCE_ROOT": str(case_dir / "evidence"),
        "FLAMEWM_XEPHYR_DEBUG_ROOT": str(case_dir / "debug"),
    }
    if name == "host":
        env.pop("DBUS_SYSTEM_BUS_ADDRESS")
    upower = None
    if CASES[name] is not None:
        percent, state = CASES[name]
        upower = subprocess.Popen(
            [sys.executable, "-I", str(Path(__file__).resolve()), "--serve-upower", str(percent), str(state)],
            env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
            start_new_session=True,
        )
        assert upower.stdout is not None
        upower.stdout.readline()  # FAKE_UPOWER_READY
    stack = subprocess.Popen(
        [str(ROOT / "scripts" / "xephyr")], cwd=ROOT, env=env,
        stdout=open(case_dir / "xephyr.out", "w"), stderr=subprocess.STDOUT, start_new_session=True,
    )
    result: dict = {"case": name}
    try:
        wait_display(args.display)
        time.sleep(args.settle)
        full = screenshot(args.display, case_dir / "root.png")
        tray = crop(full, case_dir / "tray.png", TRAY_CROP)
        result["tray_png"] = str(tray)
        if name in ("45", "45s5"):
            click(args.display, *TRAY_CLICK)
            time.sleep(1.0)
            popup_id = find_popup(args.display)
            result["popup_mapped"] = popup_id is not None
            if popup_id:
                screenshot(args.display, case_dir / "popup.png", window=popup_id)
                result["popup_png"] = str(case_dir / "popup.png")
            escape(args.display)
    finally:
        kill_group(stack.pid)
        if upower is not None:
            kill_group(upower.pid)
        try:
            os.kill(bus_pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        subprocess.run(["pkill", "-f", f"Xephyr {args.display}"], capture_output=True)
    return result


def main() -> int:
    if len(sys.argv) > 1 and sys.argv[1] == "--serve-upower":
        return serve_upower(int(sys.argv[2]), int(sys.argv[3]))
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--bin-dir", required=True)
    parser.add_argument("--display", default=":108")
    parser.add_argument("--out", required=True)
    parser.add_argument("--cases", default="unavailable,87,45,15,5")
    parser.add_argument("--settle", type=float, default=10.0)
    args = parser.parse_args()
    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    results = []
    for name in args.cases.split(","):
        if name not in CASES:
            parser.error(f"unknown case {name}")
        results.append(run_case(name, args, out))
    # AE between tray crops: 0 means pixel-identical.
    tray = {r["case"]: Path(r["tray_png"]) for r in results}
    pairs = {}
    names = list(tray)
    for i, a in enumerate(names):
        for b in names[i + 1:]:
            pairs[f"{a}-vs-{b}"] = ae(tray[a], tray[b])
    report = {"results": results, "tray_ae": pairs}
    (out / "report.json").write_text(json.dumps(report, indent=2))
    print(json.dumps(report, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

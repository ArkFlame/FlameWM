#!/usr/bin/env python3
"""G10 probe: GTK3 appearance end-to-end through the real flamewm WM.

For each case a fresh Xephyr display is started, the real flamewm binary is
run under a private dbus-run-session with an isolated HOME/XDG tree and a
settings.toml carrying `appearance=<mode>` (the product persistence path), the
_XSETTINGS_S0 owner and its _XSETTINGS_SETTINGS blob are read and decoded, and
a real GTK3 window (this file, re-executed with --gtk-child) is sampled via
`import -window`. The control case runs with no WM and therefore no owner.

Usage: flamewm_gtk3_e2e_probe.py --wm-bin <flamewm> --work <dir> [--display :115]
"""

import argparse
import json
import os
import re
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import time
from pathlib import Path

WINDOW_LINE = re.compile(r"(0x[0-9a-f]+) \"([^\"]*)\":.*?(\d+)x(\d+)\+")
TITLE = "FlameWM-G10-GTK3"
CHILD_FLAG = "--gtk-child"
SELECTION = "_XSETTINGS_S0"
SETTINGS_PROPERTY = "_XSETTINGS_SETTINGS"
KDEGLOBALS_REL = Path("flamewm/appearance/kdeglobals")
SAMPLE_SECONDS = 4.0
SAMPLE_INTERVAL = 0.01
# (name, persisted settings appearance or None for no WM, extra app env, extra WM env,
#  claim-time sampling). Sampling polls _XSETTINGS_S0 and the owned kdeglobals from WM start.
CASES = [
    ("control", None, {}, {}, False),
    ("dark", "dark", {}, {}, True),
    ("light", "light", {}, {}, False),
    ("dark-explicit-GTK_THEME", "dark", {"GTK_THEME": "Breeze"}, {}, False),
    ("control-explicit-GTK_THEME", None, {"GTK_THEME": "Breeze"}, {}, False),
    # Persisted light; FLAMEWM_APPEARANCE explicitly absent, then explicitly dark.
    ("light-sample-env-unset", "light", {}, {}, True),
    ("light-sample-env-dark", "light", {}, {"FLAMEWM_APPEARANCE": "dark"}, True),
]


def gtk_child():
    """GTK3 test window: label, button, and an empty bottom strip for sampling."""
    import gi

    gi.require_version("Gtk", "3.0")
    from gi.repository import GLib, Gtk

    settings = Gtk.Settings.get_default()
    win = Gtk.Window(title=TITLE)
    win.set_default_size(320, 200)
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
    box.set_margin_top(12)
    button = Gtk.Button(label="G10 button")
    button.set_halign(Gtk.Align.CENTER)
    box.pack_start(Gtk.Label(label="G10 label"), False, False, 0)
    box.pack_start(button, False, False, 0)
    win.add(box)
    win.connect("destroy", Gtk.main_quit)
    win.show_all()

    def report():
        print(
            json.dumps(
                {
                    "gtk_theme_name": settings.get_property("gtk-theme-name"),
                    "gtk_prefer_dark": settings.get_property(
                        "gtk-application-prefer-dark-theme"
                    ),
                }
            ),
            flush=True,
        )
        return False

    GLib.timeout_add(1500, report)
    Gtk.main()


def decode_xsettings(data: bytes) -> dict:
    """Decode the XSETTINGS blob (spec: byte order, serial, count, entries)."""
    order = "<" if data[0] == 0 else ">"
    _serial, count = struct.unpack_from(order + "II", data, 4)
    off = 12
    out = {}
    for _ in range(count):
        kind = data[off]
        (name_len,) = struct.unpack_from(order + "H", data, off + 2)
        off += 4
        name = data[off : off + name_len].decode()
        off += (name_len + 3) & ~3
        off += 4  # last-change serial
        if kind == 0:
            (value,) = struct.unpack_from(order + "i", data, off)
            off += 4
        elif kind == 1:
            (text_len,) = struct.unpack_from(order + "I", data, off)
            off += 4
            value = data[off : off + text_len].decode()
            off += (text_len + 3) & ~3
        else:
            raise ValueError(f"unsupported XSETTINGS type {kind} for {name}")
        out[name] = value
    return out


def selection_owner(display: str):
    from Xlib import display as xdisplay

    d = xdisplay.Display(display)
    try:
        owner = d.get_selection_owner(d.intern_atom(SELECTION))
        if not owner:
            return None
        prop = owner.get_full_property(d.intern_atom(SETTINGS_PROPERTY), 0)
        blob = bytes(prop.value) if prop is not None else b""
        return owner.id, blob
    finally:
        d.close()


def wait_for_owner(display: str, wm, seconds: float):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if wm.poll() is not None:
            return None
        found = selection_owner(display)
        if found is not None:
            return found
        time.sleep(0.3)
    return None


def wait_for_display(display: str, xephyr, seconds: float = 20):
    from Xlib import display as xdisplay

    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if xephyr.poll() is not None:
            raise RuntimeError("Xephyr exited during startup")
        try:
            xdisplay.Display(display).close()
            return
        except Exception:  # noqa: BLE001 - display not accepting yet
            time.sleep(0.2)
    raise RuntimeError(f"Xephyr {display} never accepted connections")


def wait_for_window(display: str, title: str, seconds: float):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        tree = subprocess.run(
            ["xwininfo", "-display", display, "-root", "-tree"],
            capture_output=True,
            text=True,
        ).stdout
        for window_id, name, width, height in WINDOW_LINE.findall(tree):
            if title == name and int(width) >= 100 and int(height) >= 100:
                return window_id
        time.sleep(0.5)
    return None


def kill_group(proc):
    if proc.poll() is not None:
        return
    try:
        os.killpg(proc.pid, signal.SIGTERM)
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        os.killpg(proc.pid, signal.SIGKILL)
        proc.wait()
    except ProcessLookupError:
        pass


def sample_claim(display: str, t0: float, owned_kdeglobals: Path, seconds: float):
    """Poll the _XSETTINGS_S0 owner blob and the owned kdeglobals every 10 ms from WM start.

    Records only changes as {t_ms, field, value} (t_ms since t0, the WM Popen time).
    Returns (timeline, last_found) where last_found is (owner_id, blob) or None.
    """
    from Xlib import display as xdisplay

    timeline, last, found = [], {}, None
    d = xdisplay.Display(display)
    selection = d.intern_atom(SELECTION)
    settings_atom = d.intern_atom(SETTINGS_PROPERTY)
    end = t0 + seconds
    try:
        while time.monotonic() < end:
            state = {}
            try:
                owner = d.get_selection_owner(selection)
                if owner:
                    prop = owner.get_full_property(settings_atom, 0)
                    blob = bytes(prop.value) if prop is not None else b""
                    net = decode_xsettings(blob).get("Net/ThemeName") if blob else None
                    state["xsettings_net_theme"] = {"owner": hex(owner.id), "net_theme": net}
                    found = (owner.id, blob)
                else:
                    state["xsettings_net_theme"] = None
            except Exception as error:  # noqa: BLE001 - owner may vanish mid-poll
                state["xsettings_net_theme"] = f"error: {error}"
            try:
                state["kdeglobals"] = owned_kdeglobals.read_text()
            except FileNotFoundError:
                state["kdeglobals"] = None
            now = round((time.monotonic() - t0) * 1000, 1)
            for field, value in state.items():
                if field not in last or last[field] != value:
                    last[field] = value
                    timeline.append({"t_ms": now, "field": field, "value": value})
            time.sleep(SAMPLE_INTERVAL)
    finally:
        d.close()
    return timeline, found


def summarize_timeline(timeline):
    """Collapse the sampled changes into first publish, sequence, and corrections."""
    xs = [e for e in timeline if e["field"] == "xsettings_net_theme" and e["value"] is not None]
    qt = [e for e in timeline if e["field"] == "kdeglobals"]
    xs_sequence = []
    for event in xs:
        net = event["value"].get("net_theme")
        if not xs_sequence or xs_sequence[-1]["net_theme"] != net:
            xs_sequence.append({"t_ms": event["t_ms"], "net_theme": net})
    qt_sequence = []
    for event in qt:
        content = event["value"]
        scheme = None
        if content:
            for line in content.splitlines():
                if line.startswith("ColorScheme="):
                    scheme = line.split("=", 1)[1]
        qt_sequence.append({"t_ms": event["t_ms"], "color_scheme": scheme})
    return {
        "xsettings_first_publish_ms": xs[0]["t_ms"] if xs else None,
        "xsettings_net_theme_sequence": xs_sequence,
        "kdeglobals_color_scheme_sequence": qt_sequence,
    }


def last_json_line(path: Path):
    lines = [line for line in path.read_text().splitlines() if line.startswith("{")]
    return json.loads(lines[-1]) if lines else None


def run_case(args, name, appearance, extra_env, extra_wm_env, sample, work: Path, scratch: Path):
    from PIL import Image

    logs = work / name
    logs.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix="g10-", dir=scratch))
    home, runtime = root / "home", root / "runtime"
    config, data = home / ".config", home / ".local/share"
    for path in (config, data, runtime):
        path.mkdir(parents=True)
    runtime.chmod(0o700)
    base = {
        "PATH": os.environ["PATH"],
        "LANG": "C.UTF-8",
        "HOME": str(home),
        "XDG_CONFIG_HOME": str(config),
        "XDG_DATA_HOME": str(data),
        "XDG_CACHE_HOME": str(root / "cache"),
        "XDG_RUNTIME_DIR": str(runtime),
    }
    display = args.display
    procs = []
    result = {"case": name, "persisted_appearance": appearance}
    try:
        xephyr = subprocess.Popen(
            ["Xephyr", display, "-screen", args.screen, "-dpi", "96", "-ac", "-br",
             "-noreset", "-nolisten", "tcp"],
            stdout=(logs / "xephyr.log").open("w"),
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        procs.append(xephyr)
        wait_for_display(display, xephyr)
        if appearance is not None:
            (config / "flamewm").mkdir()
            (config / "flamewm" / "settings.toml").write_text(
                f"revision=1\nappearance={appearance}\n"
            )
            wm_env = {
                **base,
                "DISPLAY": display,
                "XDG_SESSION_TYPE": "x11",
                "XDG_CURRENT_DESKTOP": "FlameWM",
                **extra_wm_env,
            }
            result["wm_env_appearance"] = extra_wm_env.get("FLAMEWM_APPEARANCE", "<unset>")
            t0 = time.monotonic()
            wm = subprocess.Popen(
                ["dbus-run-session", "--", args.wm_bin],
                env=wm_env,
                stdout=(logs / "wm.log").open("w"),
                stderr=subprocess.STDOUT,
                start_new_session=True,
            )
            procs.append(wm)
            if sample:
                owned = config / KDEGLOBALS_REL
                timeline, found = sample_claim(display, t0, owned, SAMPLE_SECONDS)
                result["claim_timeline"] = timeline
                result.update(summarize_timeline(timeline))
            else:
                found = wait_for_owner(display, wm, 20)
            if found is None:
                result["error"] = "no _XSETTINGS_S0 owner (WM exited or never claimed)"
                return result
            owner, blob = found
            settings = decode_xsettings(blob)
            result["xsettings_owner_window"] = hex(owner)
            result["xsettings_net_theme_name"] = settings.get("Net/ThemeName")
            result["xsettings_prefer_dark"] = settings.get("Gtk/ApplicationPreferDarkTheme")
            result["xsettings_gtk_prefer_dark_theme_key"] = settings.get("Gtk/PreferDarkTheme")
            result["xsettings_serial_blob_keys"] = sorted(settings)
        app_env = {
            **base,
            "DISPLAY": display,
            "GDK_BACKEND": "x11",
            "NO_AT_BRIDGE": "1",
            **extra_env,
        }
        app = subprocess.Popen(
            ["dbus-run-session", "--", sys.executable, "-I",
             str(Path(__file__).resolve()), CHILD_FLAG],
            env=app_env,
            stdout=(logs / "gtk-app.log").open("w"),
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        procs.append(app)
        window = wait_for_window(display, TITLE, 30)
        if window is None:
            result["error"] = "GTK3 window never mapped"
            return result
        time.sleep(3)
        png = logs / "window.png"
        subprocess.run(["import", "-display", display, "-window", window, str(png)],
                       check=True, env=app_env)
        image = Image.open(png).convert("RGB")
        width, height = image.size
        result["window"] = window
        result["size"] = [width, height]
        result["pixel_bottom_left"] = list(image.getpixel((6, height - 6)))
        result["pixel_bottom_right"] = list(image.getpixel((width - 6, height - 6)))
        result["gtk_reported"] = last_json_line(logs / "gtk-app.log")
        result["explicit_env"] = extra_env
    finally:
        for proc in reversed(procs):
            kill_group(proc)
        shutil.rmtree(root, ignore_errors=True)
    return result


def verdict(cases):
    by = {case["case"]: case for case in cases}
    dark, light = by.get("dark", {}), by.get("light", {})
    explicit = by.get("dark-explicit-GTK_THEME", {})
    dark_pixel = dark.get("pixel_bottom_left")
    light_pixel = light.get("pixel_bottom_left")
    return {
        "owner_claimed_all_wm_cases": all(
            by.get(n, {}).get("xsettings_owner_window") for n in ("dark", "light", "dark-explicit-GTK_THEME")
        ),
        "dark_net_theme_breeze_dark": dark.get("xsettings_net_theme_name") == "Breeze-Dark",
        "light_net_theme_breeze": light.get("xsettings_net_theme_name") == "Breeze",
        "dark_gtk_selected": (dark.get("gtk_reported") or {}).get("gtk_theme_name"),
        "light_gtk_selected": (light.get("gtk_reported") or {}).get("gtk_theme_name"),
        "dark_pixel_differs_from_light": dark_pixel is not None and dark_pixel != light_pixel,
        # Gtk.Settings reports the XSETTINGS name even under explicit GTK_THEME (and Yaru with no
        # WM), so the rendered pixel is the authority: explicit wins when it matches the no-WM
        # explicit case and is not the dark pixel.
        "explicit_gtk_theme_wins": explicit.get("pixel_bottom_left") is not None
        and explicit.get("pixel_bottom_left") == by.get("control-explicit-GTK_THEME", {}).get("pixel_bottom_left")
        and explicit.get("pixel_bottom_left") != dark_pixel,
        "explicit_xsettings_still_published": explicit.get("xsettings_net_theme_name") == "Breeze-Dark",
        "explicit_rendered_pixel": explicit.get("pixel_bottom_left"),
        "explicit_pixel_equals_light_breeze": explicit.get("pixel_bottom_left") == light_pixel,
        "control_explicit_pixel": by.get("control-explicit-GTK_THEME", {}).get("pixel_bottom_left"),
    }


def main():
    if CHILD_FLAG in sys.argv:
        gtk_child()
        return 0
    parser = argparse.ArgumentParser()
    parser.add_argument("--wm-bin", required=True)
    parser.add_argument("--work", required=True)
    parser.add_argument("--display", default=":115")
    parser.add_argument("--screen", default="1024x768")
    args = parser.parse_args()
    number = args.display.lstrip(":")
    if Path(f"/tmp/.X11-unix/X{number}").exists() or Path(f"/tmp/.X{number}-lock").exists():
        print(f"display {args.display} already in use; refusing", file=sys.stderr)
        return 2
    work = Path(args.work)
    work.mkdir(parents=True, exist_ok=True)
    scratch = Path(tempfile.mkdtemp(prefix="flamewm-g10-root-"))
    cases = []
    try:
        for name, appearance, extra, wm_extra, sample in CASES:
            cases.append(
                run_case(args, name, appearance, extra, wm_extra, sample, work, scratch)
            )
    finally:
        shutil.rmtree(scratch, ignore_errors=True)
    out = {"cases": cases, "verdict": verdict(cases)}
    (work / "g10-results.json").write_text(json.dumps(out, indent=2) + "\n")
    print(json.dumps(out, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())

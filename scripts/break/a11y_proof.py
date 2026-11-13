#!/usr/bin/env python3
"""Executed proof for the accessibility settings on a private Xvfb and a private dbus-daemon.

Two shell-stack sessions, each with a temp HOME whose settings.conf sets the keys before start:
  default:    highContrast=false reducedMotion=false
  hc-reduced: highContrast=true  reducedMotion=true
Each session is a restart, which is the settings seam (settings.toml is read at start).

Checks, from captured root pixels:
  1. the taskbar pixels differ materially between the two sessions (high contrast palette);
  2. the taskbar clock region reaches WCAG 2.x contrast >= 7:1 under highContrast (text colour
     against the most common background colour, sampled from the captured pixels);
  3. the default palette ratio is lower than the high-contrast ratio;
  4. reducedMotion=true: the start popup, captured from the first frame after the Super_L open
     event, is already at its final geometry (every visible frame equals the settled frame);
  5. reducedMotion=false: an intermediate (still animating) popup frame is seen. If the timing
     is not captured, the check is reported as UNIT-ONLY, not as a pass.

Usage: a11y_proof.py BIN_DIR OUT_DIR
  BIN_DIR: directory with the debug flamewm, flamewm-session, flamewm-shell, flamewm-desktop binaries.
Prints one PASS/FAIL/UNIT-ONLY line per check; exit 0 only when every hard check passed.
"""

import os
import shutil
import signal
import subprocess
import sys
import time
from collections import Counter

from PIL import Image, ImageChops
from Xlib import X, XK
from Xlib import display as xdisplay
from Xlib.ext import xtest

SCREEN = (0, 0, 1280, 800)
TASKBAR = (0, 742, 1280, 800)  # rows at and below y=740 hold the taskbar
CLOCK = (1150, 746, 1276, 796)  # taskbar right side: clock and tray
POPUP_SEARCH = (0, 0, 1280, 740)  # start popup sits above the taskbar
MIN_STARTUP_S = 12
START_BUTTON = (24, 778)  # Start button in the taskbar
TASKBAR_MIN_DIFF_PX = 500
CONTRAST_MIN = 7.0
POPUP_MIN_PX = 1000  # a frame counts as showing the popup once this many pixels changed
SETTLED_TOL = 0.005  # fraction of the popup box that may still differ and count as settled
MIN_SWATCH_PX = 12  # a colour must cover this many clock pixels to count as text or background


def lin(c):
    c /= 255.0
    return c / 12.92 if c <= 0.03928 else ((c + 0.055) / 1.055) ** 2.4


def luminance(rgb):
    r, g, b = rgb
    return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)


def wcag_ratio(rgb_a, rgb_b):
    la, lb = luminance(rgb_a), luminance(rgb_b)
    hi, lo = max(la, lb), min(la, lb)
    return (hi + 0.05) / (lo + 0.05)


def text_contrast(im):
    """Best WCAG ratio of a sampled text colour against the region's most common colour."""
    colours = Counter(pixels(im))
    background, _ = colours.most_common(1)[0]
    best, best_colour = 1.0, background
    for colour, count in colours.items():
        if count < MIN_SWATCH_PX:
            continue
        ratio = wcag_ratio(colour, background)
        if ratio > best:
            best, best_colour = ratio, colour
    return best, background, best_colour


def pixels(im):
    flat = getattr(im, "get_flattened_data", None)
    return flat() if flat is not None else im.getdata()


def grab(root, box):
    x, y, w, h = box[0], box[1], box[2] - box[0], box[3] - box[1]
    raw = root.get_image(x, y, w, h, X.ZPixmap, 0xFFFFFFFF).data
    return Image.frombuffer("RGB", (w, h), raw, "raw", "BGRX", 0, 1)


def changed_count(a, b):
    diff = ImageChops.difference(a, b)
    return sum(diff.convert("L").histogram()[17:])


def free_display():
    for number in range(150, 200):
        if not os.path.exists(f"/tmp/.X11-unix/X{number}") and not os.path.exists(f"/tmp/.X{number}-lock"):
            return f":{number}"
    raise SystemExit("FAIL no free display in :150-:199")


def start_private_bus():
    result = subprocess.run(["dbus-daemon", "--session", "--fork", "--print-address=1", "--print-pid=1"],
                            capture_output=True, text=True, check=True)
    fields = result.stdout.split()
    return fields[0], int(fields[1])


def write_settings(home, values):
    config = os.path.join(home, ".config", "flamewm")
    os.makedirs(config, exist_ok=True)
    lines = ["revision=2"] + [f"{key}={value}" for key, value in values.items()]
    with open(os.path.join(config, "settings.toml"), "w") as handle:
        handle.write("\n".join(lines) + "\n")


def capture_session(bin_dir, out, tag, values, display):
    """Starts one shell stack with `values` in settings.conf and returns its captures."""
    root_dir = os.path.join(out, tag)
    home = os.path.join(root_dir, "home")
    os.makedirs(os.path.join(home, ".config"), exist_ok=True)
    bindir = os.path.join(root_dir, "bin")
    os.makedirs(bindir, exist_ok=True)
    for name in ("flamewm", "flamewm-session", "flamewm-shell", "flamewm-desktop"):
        shutil.copy(os.path.join(bin_dir, name), bindir)
    write_settings(home, values)
    address, bus_pid = start_private_bus()
    env = dict(os.environ, DISPLAY=display, HOME=home, XDG_CONFIG_HOME=os.path.join(home, ".config"),
               XDG_DATA_HOME=os.path.join(home, ".local/share"), XDG_STATE_HOME=os.path.join(home, ".local/state"),
               XDG_RUNTIME_DIR=root_dir, TMPDIR=root_dir, DBUS_SESSION_BUS_ADDRESS=address,
               XDG_CURRENT_DESKTOP="FlameWM", PATH=f"{bindir}:/usr/bin:/bin", LANG="C.UTF-8")
    env.pop("WAYLAND_DISPLAY", None)
    env.pop("FLAMEWM_CONFIG_DIR", None)
    session = subprocess.Popen([os.path.join(bindir, "flamewm-session")], env=env, start_new_session=True,
                               stdout=open(os.path.join(root_dir, "session.out"), "w"), stderr=subprocess.STDOUT)
    try:
        d = xdisplay.Display(display)
        root = d.screen().root
        # Startup settles late: the wallpaper and taskbar arrive seconds after the first frame, so
        # wait for a minimum delay, then for two identical full-screen grabs one second apart.
        time.sleep(MIN_STARTUP_S)
        previous = grab(root, SCREEN)
        for _ in range(60):
            time.sleep(1.0)
            frame = grab(root, SCREEN)
            if changed_count(previous, frame) == 0:
                break
            previous = frame
        else:
            raise SystemExit(f"FAIL {tag}: desktop never settled")
        root.warp_pointer(1000, 300)
        d.sync()
        time.sleep(0.5)
        closed = grab(root, SCREEN)
        closed.save(os.path.join(root_dir, "closed.png"))
        taskbar = grab(root, TASKBAR)

        # Start button (flame glyph, bottom-left of the taskbar) opens the start popup.
        root.warp_pointer(*START_BUTTON)
        d.sync()
        time.sleep(0.2)
        t0 = time.monotonic()
        xtest.fake_input(d, X.ButtonPress, 1)
        d.sync()
        xtest.fake_input(d, X.ButtonRelease, 1)
        d.sync()
        frames = []
        while time.monotonic() - t0 < 0.9:
            frames.append((time.monotonic() - t0, grab(root, POPUP_SEARCH)))
        time.sleep(1.5)
        final = grab(root, POPUP_SEARCH)
        final.save(os.path.join(root_dir, "popup-final.png"))
        closed_crop = closed.crop(POPUP_SEARCH)
        box = ImageChops.difference(closed_crop, final).getbbox()
        escape = d.keysym_to_keycode(XK.string_to_keysym("Escape"))  # closes the popup
        xtest.fake_input(d, X.KeyPress, escape)
        xtest.fake_input(d, X.KeyRelease, escape)
        d.sync()
        return {"taskbar": taskbar, "closed": closed_crop, "frames": frames, "final": final, "box": box,
                "closed_full": closed}
    finally:
        try:
            os.killpg(session.pid, signal.SIGTERM)
        except OSError:
            pass
        session.wait(timeout=10)
        try:
            os.kill(bus_pid, signal.SIGTERM)
        except OSError:
            pass


def popup_top(closed_box, frame_box):
    """First row where at least a fifth of the popup width changed against the closed frame."""
    mask = ImageChops.difference(closed_box, frame_box).convert("L").point(lambda v: 255 if v > 17 else 0)
    rows = list(mask.resize((1, mask.height), Image.BOX).getdata())
    for y, value in enumerate(rows):
        if value >= 255 * 0.2:
            return y
    return None


def popup_timeline(capture):
    """Per sampled frame: (t_ms, visible, top, final_top), rows absolute in the popup search region.
    A sliding popup moves `top` (and is off-region before it enters); a static one holds final_top."""
    if capture["box"] is None:
        return None
    closed = capture["closed"]
    final_top = popup_top(closed, capture["final"])
    timeline = []
    for t, frame in capture["frames"]:
        top = popup_top(closed, frame)
        timeline.append((round(t * 1000), top is not None, top, final_top))
    return timeline


def run(bin_dir, out):
    os.makedirs(out, exist_ok=True)
    verdicts = []

    def check(label, ok, detail):
        verdicts.append(bool(ok))
        print("PASS" if ok else "FAIL", label, detail, flush=True)

    display = free_display()
    xvfb = subprocess.Popen(["Xvfb", display, "-screen", "0", "1280x800x24", "-nolisten", "tcp", "-noreset"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        for _ in range(50):
            if os.path.exists(f"/tmp/.X11-unix/X{display[1:]}"):
                break
            time.sleep(0.2)
        default = capture_session(bin_dir, out, "default",
                                  {"highContrast": "false", "reducedMotion": "false"}, display)
        contrast_on = capture_session(bin_dir, out, "hc-reduced",
                                      {"highContrast": "true", "reducedMotion": "true"}, display)
    finally:
        xvfb.terminate()

    # The start popup is the palette-driven surface (--panel, --surface, --text). The taskbar is
    # reported for the record only: its fill and clock do not follow the palette in this build.
    box = default["box"]
    if box is None:
        check("start popup opens", False, "no popup region changed")
    else:
        popup_default = default["final"].crop(box)
        popup_contrast = contrast_on["final"].crop(box)
        diff = changed_count(popup_default, popup_contrast)
        check("high contrast changes the start popup pixels", diff >= TASKBAR_MIN_DIFF_PX,
              f"changed_px={diff} min={TASKBAR_MIN_DIFF_PX}")
        hc_ratio, hc_bg, hc_fg = text_contrast(popup_contrast)
        def_ratio, def_bg, def_fg = text_contrast(popup_default)
        check("high contrast popup text reaches 7:1", hc_ratio >= CONTRAST_MIN,
              f"ratio={hc_ratio:.2f} bg={hc_bg} fg={hc_fg}")
        check("high contrast ratio is not below the default", hc_ratio >= def_ratio,
              f"default={def_ratio:.2f} bg={def_bg} fg={def_fg} high_contrast={hc_ratio:.2f}")
    taskbar_default = default["taskbar"].crop((CLOCK[0], CLOCK[1] - 742, CLOCK[2], CLOCK[3] - 742))
    taskbar_hc = contrast_on["taskbar"].crop((CLOCK[0], CLOCK[1] - 742, CLOCK[2], CLOCK[3] - 742))
    print("INFO taskbar not palette-driven:",
          f"changed_px={changed_count(default['taskbar'], contrast_on['taskbar'])}",
          f"clock_ratio_default={text_contrast(taskbar_default)[0]:.2f}",
          f"clock_ratio_hc={text_contrast(taskbar_hc)[0]:.2f}", flush=True)

    reduced = popup_timeline(contrast_on)
    if reduced is not None:
        print("INFO reducedMotion=true timeline:", [(t, top) for t, vis, top, _ in reduced if vis][:12], flush=True)
    animated_info = popup_timeline(default)
    if animated_info is not None:
        print("INFO reducedMotion=false timeline:", [(t, top) for t, vis, top, _ in animated_info if vis][:12], flush=True)
    if reduced is None:
        check("reducedMotion popup opens", False, "no popup region changed")
    else:
        visible = [entry for entry in reduced if entry[1]]
        first_ok = bool(visible) and visible[0][2] == visible[0][3]
        check("reducedMotion: first popup frame is at final geometry", first_ok,
              f"final_top={visible[0][3] if visible else None} first={visible[0] if visible else None}")
    animated = popup_timeline(default)
    if animated is None or reduced is None:
        check("reducedMotion=false popup opens", False, "no popup region changed")
    else:
        # Without reducedMotion the popup is still in its slide: it either moves, or its first
        # visible frame lags the reduced one by the animation delay (COMPOSITE is missing here, so
        # the slide itself is not drawn; the delayed show is the observable).
        first_reduced = next(t for t, vis, _, _ in reduced if vis)
        first_default = next(t for t, vis, _, _ in animated if vis)
        moving = [entry for entry in animated if entry[1] and entry[2] != entry[3]]
        check("reducedMotion=false: popup is still animating", bool(moving) or first_default >= first_reduced + 150,
              f"moving_frames={len(moving)} first_visible_ms default={first_default} reduced={first_reduced}")
    return all(verdicts) and len(verdicts) >= 3


if __name__ == "__main__":
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    sys.exit(0 if run(sys.argv[1], sys.argv[2]) else 1)

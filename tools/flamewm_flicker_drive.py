#!/usr/bin/env python3
"""Live flicker driver for window transitions on an isolated Xephyr display.

Runs one WM per animation style (none, fade, magic-lamp) on a fresh HOME, drives a slow solid
client through open, minimize, restore, maximize and unmaximize, and samples the root at
about 2 ms. Each phase reports the flicker count from flamewm_flicker_acceptance.count_flicker
and the number of samples with white pixels.

Never point this at a user's display. Pass the display of a stack you started yourself.

  python3 -I tools/flamewm_flicker_drive.py --display :123 --wm-bin .target/release/flamewm \
      --home /tmp/claude-1000/flicker-home-N --out-dir /tmp/claude-1000/flicker-N
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import threading
import time

import numpy as np
from Xlib import X, Xatom, display as xdisplay
from Xlib.protocol import event as xevent

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import flamewm_flicker_acceptance as flicker  # noqa: E402

CLIENT_POS = (300, 200)  # requested root position of the solid client
CLIENT_SIZE = (320, 240)  # w, h of the solid client
INK = (30, 160, 200)  # client fill, R,G,B (non-white)
WHITE_MIN_PIXELS = 50
REGION = (0, 0, 1000, 700)  # sampled root region (whole nested screen)
PHASE_SECONDS = 0.9
SLOW_CLIENT_MS = 60

STYLES = {
    # config name: (minimize/maximize style, open style)
    "none": ("none", "none"),
    "fade": ("fade", "fade"),
    "magic-lamp": ("magic-lamp", "scale"),
}


def write_settings(home: str, style: str) -> None:
    move, open_style = STYLES[style]
    cfg_dir = os.path.join(home, ".config", "flamewm")
    os.makedirs(cfg_dir, exist_ok=True)
    text = (
        "revision=2\n"
        "animationsEnabled=true\n"
        "animationSpeed=100\n"
        "minimizeAnimationEnabled=true\n"
        f"minimizeAnimation={move}\n"
        "maximizeAnimationEnabled=true\n"
        f"maximizeAnimation={move}\n"
        "windowOpenAnimationEnabled=true\n"
        f"windowOpenAnimation={open_style}\n"
    )
    with open(os.path.join(cfg_dir, "settings.toml"), "w", encoding="utf-8") as handle:
        handle.write(text)


def classify(frame: np.ndarray, ink, client_size) -> tuple[str, int]:
    """Label one BGRX root region and count white pixels.

    The label depends only on the ink bounding box, so it does not care where the WM places the
    client. CLIENT: the box has the client's size and the ink fills at least 90% of it (the frame
    rounds the corners). OVERLAY: other ink. BACKGROUND: no ink. White pixels are counted inside
    the whole region in both cases. The titlebar text is white, so the driver subtracts a warm-up
    baseline before judging a white flash.
    """
    blue, green, red = frame[..., 0], frame[..., 1], frame[..., 2]
    ink_mask = (blue == ink[2]) & (green == ink[1]) & (red == ink[0])
    white_mask = (red >= 240) & (green >= 240) & (blue >= 240)
    total = int(ink_mask.sum())
    if total == 0:
        return flicker.BACKGROUND, int(white_mask.sum())  # whole region
    ys, xs = np.nonzero(ink_mask)
    x0, x1, y0, y1 = int(xs.min()), int(xs.max()), int(ys.min()), int(ys.max())
    box_w, box_h = x1 - x0 + 1, y1 - y0 + 1
    cw, ch = client_size
    white = int(white_mask.sum())
    if abs(box_w - cw) <= 2 and abs(box_h - ch) <= 2 and total >= 0.9 * box_w * box_h:
        return flicker.CLIENT, white
    return flicker.OVERLAY, white


def white_count(display_name: str) -> int:
    root = xdisplay.Display(display_name).screen().root
    x, y, w, h = REGION
    image = root.get_image(x, y, w, h, X.ZPixmap, 0xFFFFFFFF)
    frame = np.frombuffer(image.data, dtype=np.uint8).reshape(h, w, 4)
    _, white = classify(frame, INK, CLIENT_SIZE)
    return white


class Sampler(threading.Thread):
    def __init__(self, display_name: str, client_size, base_white: int) -> None:
        super().__init__(daemon=True)
        self.base_white = base_white
        self.disp = xdisplay.Display(display_name)
        self.root = self.disp.screen().root
        self.client_size = client_size
        self.stop_flag = threading.Event()
        self.labels: list[str] = []
        self.whites: list[int] = []

    def run(self) -> None:
        x, y, w, h = REGION
        while not self.stop_flag.is_set():
            image = self.root.get_image(x, y, w, h, X.ZPixmap, 0xFFFFFFFF)
            frame = np.frombuffer(image.data, dtype=np.uint8).reshape(h, w, 4)
            label, white = classify(frame, INK, self.client_size)
            self.labels.append(label)
            self.whites.append(white - self.base_white)
            time.sleep(0.002)

    def finish(self) -> tuple[list[str], list[int]]:
        self.stop_flag.set()
        self.join()
        return self.labels, self.whites


class Client(threading.Thread):
    """Solid client that repaints `SLOW_CLIENT_MS` after each Expose, like a slow toolkit."""

    def __init__(self, display_name: str) -> None:
        super().__init__(daemon=True)
        self.disp = xdisplay.Display(display_name)
        self.root = self.disp.screen().root
        x, y = CLIENT_POS
        w, h = CLIENT_SIZE
        color = (INK[0] << 16) | (INK[1] << 8) | INK[2]
        self.win = self.root.create_window(
            x, y, w, h, 0, self.disp.screen().root_depth, X.InputOutput, X.CopyFromParent,
            background_pixel=color,
            event_mask=X.ExposureMask | X.StructureNotifyMask,
        )
        self.win.set_wm_name("flicker-client")
        self.color = color
        self.stop_flag = threading.Event()

    def map(self) -> None:
        self.win.map()
        self.disp.flush()

    def unmap(self) -> None:
        self.win.unmap()
        self.disp.flush()

    def send(self, atom_name: str, data: list[int]) -> None:
        atom = self.disp.intern_atom(atom_name)
        ev = xevent.ClientMessage(window=self.win, client_type=atom, data=(32, data + [0] * (5 - len(data))))
        self.root.send_event(ev, event_mask=X.SubstructureRedirectMask | X.SubstructureNotifyMask)
        self.disp.flush()

    def iconify(self) -> None:
        self.send("WM_CHANGE_STATE", [3])

    def normal(self) -> None:
        self.send("WM_CHANGE_STATE", [1])

    def set_maximized(self, add: bool) -> None:
        vert = self.disp.intern_atom("_NET_WM_STATE_MAXIMIZED_VERT")
        horz = self.disp.intern_atom("_NET_WM_STATE_MAXIMIZED_HORZ")
        self.send("_NET_WM_STATE", [1 if add else 0, vert, horz, 1])


def run_phase(name: str, action, display_name: str, results: dict, style: str, rect, base_white: int) -> None:
    sampler = Sampler(display_name, rect, base_white)
    sampler.start()
    time.sleep(0.05)
    action()
    time.sleep(PHASE_SECONDS)
    labels, whites = sampler.finish()
    counts = {
        "samples": len(labels),
        "flicker": flicker.count_flicker(labels),
        "white_samples": sum(1 for w in whites if w >= WHITE_MIN_PIXELS),
        "white_excess_max": max(whites) if whites else 0,
        "labels": "".join(label[0] for label in labels),
    }
    results.setdefault(style, {})[name] = counts


def run_style(style: str, args, results: dict) -> None:
    write_settings(args.home, style)
    env = dict(os.environ, DISPLAY=args.display, HOME=args.home, FLAMEWM_LINGER_LOG="1")
    env.pop("XDG_CONFIG_HOME", None)
    log = open(os.path.join(args.out_dir, f"wm-{style}.log"), "w", encoding="utf-8")
    wm = subprocess.Popen([args.wm_bin], env=env, stdout=subprocess.DEVNULL, stderr=log)
    try:
        time.sleep(2.0)
        client = Client(args.display)
        client.start()
        # Warm-up map measures the client's root rect; unmap so the timed open phase maps again.
        client.map()
        time.sleep(1.2)
        rect = CLIENT_SIZE
        base_white = white_count(args.display)
        client.unmap()
        time.sleep(0.5)
        run_phase("open", client.map, args.display, results, style, rect, base_white)
        time.sleep(0.3)
        run_phase("minimize", client.iconify, args.display, results, style, rect, base_white)
        time.sleep(0.3)
        run_phase("restore", client.normal, args.display, results, style, rect, base_white)
        time.sleep(0.3)
        run_phase("maximize", lambda: client.set_maximized(True), args.display, results, style, rect, base_white)
        time.sleep(0.3)
        run_phase("unmaximize", lambda: client.set_maximized(False), args.display, results, style, rect, base_white)
        client.stop_flag.set()
    finally:
        wm.terminate()
        try:
            wm.wait(timeout=5)
        except subprocess.TimeoutExpired:
            wm.kill()
        log.close()


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--display", required=True)
    parser.add_argument("--wm-bin", required=True)
    parser.add_argument("--home", required=True)
    parser.add_argument("--out-dir", required=True)
    parser.add_argument("--styles", default="none,fade,magic-lamp")
    args = parser.parse_args(argv)
    os.makedirs(args.out_dir, exist_ok=True)
    results: dict = {}
    for style in args.styles.split(","):
        run_style(style, args, results)
    path = os.path.join(args.out_dir, "results.json")
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(results, handle, indent=2)
    for style, phases in results.items():
        for phase, counts in phases.items():
            print(f"{style:11s} {phase:11s} samples={counts['samples']:4d} "
                  f"flicker={counts['flicker']:3d} white={counts['white_samples']:3d}")
    print(f"wrote {path}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

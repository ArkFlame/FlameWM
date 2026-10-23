#!/usr/bin/env python3
"""Moved-window flicker driver: samples the root for 0-1000 ms after a move release.

A01 ("no disappear/reappear flicker after moving windows"): after a window is moved and the motion
stops, the WM must not hide the frame. The pre-fix settle emptied the window's bounding shape (and
its frame ancestor's), synced and read the whole root, so the window vanished and came back about
250 ms after the release.

This driver moves one solid client with a sweep of `ConfigureWindow` requests, stops (that is the
release), then samples the root at ~2 ms for one second and reports, from the ink bounding box of
each frame (`flamewm_flicker_acceptance.classify_move`):

  samples            frames sampled after the release
  hidden_frames      frames with no ink at all: the window was off the root (the A01 defect)
  hidden_span_ms     first-to-last hidden frame in ms after the release
  afterimage_frames  frames with the window only at its old rect (a leftover finish overlay)
  client_frames      frames with the client exactly at its target rect
  moving_frames      frames with the window somewhere else (still travelling)

Per style (no animation / fade / magic-lamp) one run is reported, because the finish overlay only
exists when transitions are enabled.

Never point this at a user's display. Pass the display of a stack you started yourself, exactly
like `flamewm_flicker_drive.py`:

  python3 -I tools/flamewm_move_flicker_drive.py --display :123 \
      --wm-bin .target/release/flamewm --home /tmp/.../move-home-1 --out-dir /tmp/.../move-1
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
from Xlib import X, display as xdisplay

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import flamewm_flicker_acceptance as flicker  # noqa: E402
import flamewm_flicker_drive as drive  # noqa: E402

INK = drive.INK  # client fill, R,G,B (non-white)
CLIENT_SIZE = drive.CLIENT_SIZE  # w, h of the solid client
START_POS = drive.CLIENT_POS  # root position before the move
MOVE_DELTA = (180, 120)  # root delta of the move (crosses the titlebar blur cell)
MARGIN = 80  # extra pixels sampled around both rects (frame chrome, blur spill)
SAMPLE_INTERVAL_S = 0.002
SAMPLES_MS = 1000  # sampling window after the release
SWEEP_STEPS = 20  # configure steps of the move
SWEEP_STEP_S = 0.01  # ~200 ms of motion, like a short drag
SETTLE_S = 0.6  # quiet time before the move, so no transition is in flight
STYLES = ("none", "fade", "magic-lamp")


def region_rect() -> tuple[int, int, int, int]:
    """Root rect covering both the start and the target position, plus `MARGIN`."""
    w, h = CLIENT_SIZE
    (sx, sy) = START_POS
    (tx, ty) = (sx + MOVE_DELTA[0], sy + MOVE_DELTA[1])
    x0 = min(sx, tx) - MARGIN
    y0 = min(sy, ty) - MARGIN
    x1 = max(sx, tx) + w + MARGIN
    y1 = max(sy, ty) + h + MARGIN
    return (x0, y0, x1 - x0, y1 - y0)


def ink_box(frame: np.ndarray):
    """Ink bounding box `(x0, y0, x1, y1)` of one BGRX region capture, `None` when there is none."""
    blue, green, red = frame[..., 0], frame[..., 1], frame[..., 2]
    mask = (blue == INK[2]) & (green == INK[1]) & (red == INK[0])
    if not mask.any():
        return None
    ys, xs = np.nonzero(mask)
    return (int(xs.min()), int(ys.min()), int(xs.max()), int(ys.max()))


class MoveSampler(threading.Thread):
    """Samples the root region and labels every frame against the old and target client rects."""

    def __init__(self, display_name: str, origin: tuple[int, int], old_rect, target_rect) -> None:
        super().__init__(daemon=True)
        self.disp = xdisplay.Display(display_name)
        self.root = self.disp.screen().root
        self.origin = origin
        self.old_rect = old_rect
        self.target_rect = target_rect
        self.stop_flag = threading.Event()
        self.started = 0.0
        self.samples: list[tuple[float, str]] = []

    def run(self) -> None:
        x, y, w, h = region_rect()
        self.started = time.monotonic()
        while not self.stop_flag.is_set():
            image = self.root.get_image(x, y, w, h, X.ZPixmap, 0xFFFFFFFF)
            frame = np.frombuffer(image.data, dtype=np.uint8).reshape(h, w, 4)
            label = flicker.classify_move(
                ink_box(frame), CLIENT_SIZE, self.old_rect, self.target_rect
            )
            self.samples.append((time.monotonic() - self.started, label))
            time.sleep(SAMPLE_INTERVAL_S)

    def finish(self) -> tuple[float, list[tuple[float, str]]]:
        self.stop_flag.set()
        self.join()
        return self.started, self.samples


def run_style(style: str, args, results: dict) -> None:
    drive.write_settings(args.home, style)
    env = dict(os.environ, DISPLAY=args.display, HOME=args.home, FLAMEWM_LINGER_LOG="1")
    env.pop("XDG_CONFIG_HOME", None)
    log_path = os.path.join(args.out_dir, f"move-wm-{style}.log")
    log = open(log_path, "w", encoding="utf-8")
    wm = subprocess.Popen([args.wm_bin], env=env, stdout=subprocess.DEVNULL, stderr=log)
    try:
        time.sleep(2.0)
        client = drive.Client(args.display)
        client.start()
        client.map()
        time.sleep(1.2)  # first paint, then the WM frame is in place
        region = region_rect()
        origin = (region[0], region[1])
        (sx, sy) = START_POS
        w, h = CLIENT_SIZE
        old_rect = (sx - origin[0], sy - origin[1], w, h)
        target_rect = (
            sx + MOVE_DELTA[0] - origin[0],
            sy + MOVE_DELTA[1] - origin[1],
            w,
            h,
        )
        sampler = MoveSampler(args.display, origin, old_rect, target_rect)
        sampler.start()
        time.sleep(SETTLE_S)  # baseline: the client sitting still at its start position
        # Move: a sweep of configure requests, then stop. The stop is the release.
        for step in range(1, SWEEP_STEPS + 1):
            client.win.configure(
                x=sx + MOVE_DELTA[0] * step // SWEEP_STEPS,
                y=sy + MOVE_DELTA[1] * step // SWEEP_STEPS,
            )
            client.disp.flush()
            time.sleep(SWEEP_STEP_S)
        release = time.monotonic()
        time.sleep(args.samples_ms / 1000.0)
        started, samples = sampler.finish()
        client.stop_flag.set()

        # Samples are offsets from the sampler start; the release is an offset too.
        release_at = release - started
        base_labels = [label for (t, label) in samples if t < release_at]
        moved = [(t - release_at, label) for (t, label) in samples if t >= release_at]
        moved_labels = [label for (_, label) in moved]
        hidden_ms = [t * 1000.0 for (t, label) in moved if label == flicker.HIDDEN]
        results.setdefault(style, {})["move"] = {
            "samples": len(moved_labels),
            "baseline_samples": len(base_labels),
            "baseline_not_client": sum(1 for label in base_labels if label != flicker.CLIENT),
            "hidden_frames": flicker.count_hidden_frames(moved_labels),
            "hidden_span_ms": (
                [round(min(hidden_ms), 1), round(max(hidden_ms), 1)] if hidden_ms else []
            ),
            "afterimage_frames": flicker.count_afterimage_frames(moved_labels),
            "client_frames": sum(1 for label in moved_labels if label == flicker.CLIENT),
            "moving_frames": sum(1 for label in moved_labels if label == flicker.MOVING),
            "labels": "".join(label[0] for label in moved_labels),
        }
    finally:
        wm.terminate()
        try:
            wm.wait(timeout=5)
        except subprocess.TimeoutExpired:
            wm.kill()
        log.close()


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--display", required=True, help="isolated display name, e.g. :123")
    parser.add_argument("--wm-bin", required=True)
    parser.add_argument("--home", required=True)
    parser.add_argument("--out-dir", required=True)
    parser.add_argument("--samples-ms", type=int, default=SAMPLES_MS)
    parser.add_argument("--styles", default=",".join(STYLES))
    args = parser.parse_args(argv)
    os.makedirs(args.out_dir, exist_ok=True)
    results: dict = {}
    for style in args.styles.split(","):
        run_style(style, args, results)
    path = os.path.join(args.out_dir, "move-results.json")
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(results, handle, indent=2)
    for style, phases in results.items():
        for phase, counts in phases.items():
            print(
                f"{style:11s} {phase:6s} samples={counts['samples']:4d} "
                f"hidden={counts['hidden_frames']:3d} span_ms={counts['hidden_span_ms']} "
                f"afterimage={counts['afterimage_frames']:3d} "
                f"client={counts['client_frames']:4d} moving={counts['moving_frames']:3d}"
            )
    print(f"wrote {path}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

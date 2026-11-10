#!/usr/bin/env python3
"""Self-test: root-relative window origin from python-xlib must match X ground truth.

python-xlib's `win.translate_coords(root, 0, 0)` returns the NEGATED root origin of
`win` (its x/y are the root point expressed in win's coords sign-flipped); the
correct root-relative origin is `root.translate_coords(win, 0, 0)`. This test
starts a private Xvfb (display >= 340, -noreset), places windows at a known
negative and positive offset, and compares both calls against `xwininfo`
"Absolute upper-left" values. Run:
    python3 -I -m unittest tools/test_flamewm_frame_origin.py
"""
from __future__ import annotations

import os
import re
import shutil
import site
import subprocess
import sys
import time
import unittest

try:
    from Xlib import X, display
except ImportError:  # python3 -I drops the user site-packages where python-xlib lives
    sys.path.append(site.getusersitepackages())
    try:
        from Xlib import X, display
    except ImportError:
        X = display = None

WINDOWS = {"neg": (-40, -25), "pos": (120, 80)}
ABS_RE = re.compile(r"Absolute upper-left ([XY]):\s*(-?\d+)")


def display_free(number: int) -> bool:
    return not os.path.exists(f"/tmp/.X11-unix/X{number}") and not os.path.exists(f"/tmp/.X{number}-lock")


@unittest.skipUnless(display is not None and shutil.which("Xvfb") and shutil.which("xwininfo"),
                     "needs python-xlib, Xvfb and xwininfo")
class RootOriginGroundTruth(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        number = next(n for n in range(347, 400) if display_free(n))
        cls.name = f":{number}"
        cls.xvfb = subprocess.Popen(["Xvfb", cls.name, "-screen", "0", "1024x768x24", "-noreset", "-nolisten", "tcp"],
                                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        cls.d = None
        for _ in range(100):
            try:
                cls.d = display.Display(cls.name)
                break
            except Exception:
                time.sleep(0.1)
        if cls.d is None:
            cls.xvfb.kill()
            raise RuntimeError(f"Xvfb {cls.name} did not accept connections")
        cls.root = cls.d.screen().root
        cls.windows = {}
        for label, (x, y) in WINDOWS.items():
            w = cls.root.create_window(x, y, 50, 40, 0, cls.d.screen().root_depth, X.InputOutput,
                                       X.CopyFromParent, background_pixel=cls.d.screen().white_pixel)
            w.set_wm_name(f"origin-{label}")
            w.map()
            cls.windows[label] = w
        cls.d.sync()
        time.sleep(0.2)

    @classmethod
    def tearDownClass(cls):
        if cls.d is not None:
            cls.d.close()
        cls.xvfb.terminate()
        cls.xvfb.wait(timeout=10)

    def xwininfo_origin(self, win) -> tuple[int, int]:
        out = subprocess.run(["xwininfo", "-display", self.name, "-id", str(win.id)],
                             capture_output=True, text=True, check=True).stdout
        vals = {axis: int(v) for axis, v in ABS_RE.findall(out)}
        return vals["X"], vals["Y"]

    def test_root_relative_call_matches_ground_truth(self):
        for label, win in self.windows.items():
            with self.subTest(window=label):
                t = self.root.translate_coords(win, 0, 0)
                self.assertEqual((t.x, t.y), self.xwininfo_origin(win))
                self.assertEqual((t.x, t.y), WINDOWS[label])

    def test_window_call_is_negated_and_must_not_be_used_for_origin(self):
        for label, win in self.windows.items():
            with self.subTest(window=label):
                t = win.translate_coords(self.root, 0, 0)
                self.assertEqual((t.x, t.y), (-WINDOWS[label][0], -WINDOWS[label][1]))

    def test_negative_offset_is_negative_not_clamped(self):
        t = self.root.translate_coords(self.windows["neg"], 0, 0)
        self.assertLess(t.x, 0)
        self.assertLess(t.y, 0)


if __name__ == "__main__":
    unittest.main()

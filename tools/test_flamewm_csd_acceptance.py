#!/usr/bin/env python3
"""Unit tests for the pure CSD acceptance checks (no X server needed)."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import flamewm_csd_acceptance as harness  # noqa: E402


CLIENT = (100, 100, 320, 200)
NO_FRAME = (100, 100, 320, 200)
TITLED_FRAME = (100, 78, 320, 222)  # 22px titlebar above the client


class FrameDeltaTests(unittest.TestCase):
    def test_no_frame_has_zero_delta(self):
        delta = harness.frame_delta(CLIENT, NO_FRAME)
        self.assertEqual(delta, {"offset_x": 0, "offset_y": 0, "dw": 0, "dh": 0})

    def test_titled_frame_reports_offset_and_height(self):
        delta = harness.frame_delta(CLIENT, TITLED_FRAME)
        self.assertEqual(delta["offset_y"], 22)
        self.assertEqual(delta["dh"], 22)


class VerdictTests(unittest.TestCase):
    def test_motif_without_frame_and_zero_extents_passes(self):
        delta = harness.frame_delta(CLIENT, NO_FRAME)
        self.assertEqual(harness.verdict_for_role("motif", delta, [0, 0, 0, 0]), [])

    def test_gtk_without_frame_and_zero_extents_passes(self):
        delta = harness.frame_delta(CLIENT, NO_FRAME)
        self.assertEqual(harness.verdict_for_role("gtk", delta, [0, 0, 0, 0]), [])

    def test_negative_control_motif_with_titlebar_fails(self):
        # A wrong fix that keeps the server titlebar must be rejected.
        delta = harness.frame_delta(CLIENT, TITLED_FRAME)
        failures = harness.verdict_for_role("motif", delta, [0, 0, 0, 0])
        self.assertTrue(failures)

    def test_negative_control_gtk_publishing_extents_fails(self):
        # Publishing the GTK extents as _NET_FRAME_EXTENTS is the wrong contract.
        delta = harness.frame_delta(CLIENT, NO_FRAME)
        failures = harness.verdict_for_role("gtk", delta, [20, 20, 20, 20])
        self.assertTrue(failures)

    def test_negative_control_missing_extents_fails(self):
        delta = harness.frame_delta(CLIENT, NO_FRAME)
        self.assertTrue(harness.verdict_for_role("motif", delta, None))

    def test_plain_with_titlebar_and_top_extent_passes(self):
        delta = harness.frame_delta(CLIENT, TITLED_FRAME)
        self.assertEqual(harness.verdict_for_role("plain", delta, [1, 1, 22, 1]), [])

    def test_negative_control_plain_without_frame_fails(self):
        # Plain windows must keep the server frame; stripping it is a regression.
        delta = harness.frame_delta(CLIENT, NO_FRAME)
        self.assertTrue(harness.verdict_for_role("plain", delta, [0, 0, 0, 0]))

    def test_unknown_role_fails(self):
        self.assertTrue(harness.verdict_for_role("bogus", {}, None))


class MoveResizeTests(unittest.TestCase):
    def test_exact_injected_delta_passes(self):
        self.assertTrue(harness.moveresize_ok((100, 100), (220, 160), (120, 60)))

    def test_negative_control_no_movement_fails(self):
        self.assertFalse(harness.moveresize_ok((100, 100), (100, 100), (120, 60)))

    def test_negative_control_wrong_delta_fails(self):
        self.assertFalse(harness.moveresize_ok((100, 100), (100, 160), (120, 60)))

    def test_negative_control_half_the_drag_fails(self):
        # Moving only half the injected distance is not a move request honoured.
        self.assertFalse(harness.moveresize_ok((100, 100), (160, 130), (120, 60)))

    def test_one_drag_step_of_slack_is_accepted(self):
        self.assertTrue(harness.moveresize_ok((860, 80), (960, 140), (120, 60)))

    def test_zero_expected_delta_is_rejected(self):
        self.assertFalse(harness.moveresize_ok((100, 100), (100, 100), (0, 0)))


class MainArgumentTests(unittest.TestCase):
    def test_reserved_display_is_refused(self):
        self.assertEqual(harness.main(["--display", ":97"]), 1)

    def test_display_is_required(self):
        self.assertEqual(harness.main([]), 1)


if __name__ == "__main__":
    unittest.main()

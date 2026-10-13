"""Unit tests for the click-focus acceptance decision helpers (no X server needed)."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import flamewm_click_focus_acceptance as acc  # noqa: E402

A, B = 0x100000C, 0x120000C


class StackingTests(unittest.TestCase):
    def test_click_on_back_window_raises_it_to_top(self):
        self.assertEqual(acc.stacking_violations([A, B], [B, A], A, control=False), [])

    def test_back_window_still_below_is_rejected(self):
        # Wrong fix: focus without raise leaves the target under the other window.
        self.assertTrue(acc.stacking_violations([A, B], [A, B], A, control=False))

    def test_control_click_on_active_top_window_keeps_stacking(self):
        self.assertEqual(acc.stacking_violations([B, A], [B, A], A, control=True), [])

    def test_control_click_that_restacks_is_rejected(self):
        self.assertTrue(acc.stacking_violations([B, A], [A, B], A, control=True))


class ActiveTests(unittest.TestCase):
    def test_active_matches_target(self):
        self.assertEqual(acc.active_violations(A, A), [])

    def test_stale_active_is_rejected(self):
        self.assertTrue(acc.active_violations(B, A))
        self.assertTrue(acc.active_violations(None, A))


class DeliveryTests(unittest.TestCase):
    def test_one_new_mouse_report_is_delivered(self):
        self.assertEqual(acc.delivery_violations("x^[[Mabc", 0, 1), [])

    def test_no_report_means_press_was_swallowed(self):
        # Wrong fix: a sync grab that never replays would freeze the client.
        self.assertTrue(acc.delivery_violations("", 0, 0))

    def test_stale_reports_do_not_count(self):
        self.assertTrue(acc.delivery_violations("^[[M", 1, 1))


class ParseTests(unittest.TestCase):
    def test_parses_stacking_list(self):
        text = "_NET_CLIENT_LIST_STACKING(WINDOW): window id # 0x100000c, 0x120000c\n"
        self.assertEqual(acc.parse_window_list(text), [A, B])

    def test_empty_list(self):
        self.assertEqual(acc.parse_window_list("_NET_CLIENT_LIST_STACKING(WINDOW): window id # \n"), [])


class DisplayGuardTests(unittest.TestCase):
    def test_user_display_is_refused(self):
        with self.assertRaises(SystemExit):
            acc.check_display(":97")

    def test_explicit_nested_display_is_accepted(self):
        self.assertEqual(acc.check_display(":132"), ":132")

    def test_non_display_is_refused(self):
        with self.assertRaises(SystemExit):
            acc.check_display("localhost:132; rm -rf")


if __name__ == "__main__":
    unittest.main()

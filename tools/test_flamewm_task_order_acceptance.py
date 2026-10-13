from __future__ import annotations

import sys
import unittest
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))

import flamewm_task_order_acceptance as to  # noqa: E402


class OrderModelTests(unittest.TestCase):
    def test_new_window_is_appended_right(self):
        self.assertEqual(to.append_new(["A", "B", "C"], "D"), ["A", "B", "C", "D"])

    def test_focus_does_not_reorder(self):
        # Focus is not an input to the model; negative control: an MRU rule would reorder.
        order = ["A", "B", "C"]
        self.assertEqual(to.reconcile(order, ["A", "B", "C"]), ["A", "B", "C"])

    def test_drag_c_before_a_then_close_a(self):
        order = to.move(["A", "B", "C"], 2, 0)
        self.assertEqual(order, ["C", "A", "B"])
        order = to.append_new(order, "D")
        self.assertEqual(order, ["C", "A", "B", "D"])
        self.assertEqual(to.remove_closed(order, "A"), ["C", "B", "D"])

    def test_reconcile_keeps_relative_order_and_appends_new(self):
        self.assertEqual(to.reconcile(["C", "A", "B"], ["A", "B", "C", "D"]),
                         ["C", "A", "B", "D"])

    def test_negative_control_alphabetical_resort_is_rejected(self):
        # A wrong fix that sorts by name would yield A,B,C,D here.
        self.assertNotEqual(to.reconcile(["C", "A", "B"], ["A", "B", "C"]),
                            sorted(["C", "A", "B"]))

    def test_move_out_of_range_is_noop(self):
        self.assertEqual(to.move(["A", "B"], 0, 5), ["A", "B"])


class BackgroundTests(unittest.TestCase):
    def test_matching_pixels_are_clear(self):
        px = [(20, 21, 22)] * 4
        self.assertTrue(to.background_is_clear(px, px))

    def test_negative_control_filled_box_is_rejected(self):
        # Red-tinted fill (239,64,72,56 blended) must fail the clear check.
        surround = [(20, 21, 22)] * 4
        filled = [(60, 30, 32)] * 4
        self.assertFalse(to.background_is_clear(filled, surround))

    def test_length_mismatch_raises(self):
        with self.assertRaises(ValueError):
            to.background_is_clear([(0, 0, 0)], [])


class CliTests(unittest.TestCase):
    def test_rejects_non_display(self):
        self.assertEqual(
            to.main(["--display", "133", "--screenshot", "/tmp/x.png"]), 2)


if __name__ == "__main__":
    unittest.main()

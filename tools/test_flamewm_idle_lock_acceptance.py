from __future__ import annotations

import sys
import unittest
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))

import flamewm_idle_lock_acceptance as il  # noqa: E402

XDPYINFO_WITH_SCREENSAVER = """number of extensions:    3
    MIT-SCREEN-SAVER  (opcode: 144, base event: 92)
    RANDR  (opcode: 140, base event: 89, base error: 147)
    XKEYBOARD  (opcode: 135, base event: 85, base error: 137)
"""
XDPYINFO_WITHOUT = "number of extensions:    2\n    RANDR\n    XKEYBOARD\n"
DISABLED_LOG = (
    "flamewm: XSettings wake not registered\n"
    "flamewm: idle lock disabled, MIT-SCREEN-SAVER query failed: unsupported extension\n"
)


class CapabilityTests(unittest.TestCase):
    def test_listed_extension_is_capable(self):
        self.assertIsNone(il.capability_reason(XDPYINFO_WITH_SCREENSAVER, ":112"))

    def test_missing_extension_is_blocked_with_probe_reason(self):
        reason = il.capability_reason(XDPYINFO_WITHOUT, ":112")
        self.assertIn("MIT-SCREEN-SAVER not listed", reason)
        self.assertIn(":112", reason)

    def test_substring_match_does_not_count(self):
        self.assertIsNotNone(il.capability_reason("    MIT-SCREEN-SAVER-EXTRA  (opcode: 1)\n", ":112"))

    def test_wm_disabled_line_is_extracted(self):
        self.assertIn("idle lock disabled", il.wm_disabled_line(DISABLED_LOG))
        self.assertIsNone(il.wm_disabled_line("flamewm: XSettings wake not registered\n"))


class LockAppearanceTests(unittest.TestCase):
    def test_boundaries_are_inclusive(self):
        self.assertEqual(il.classify_lock_appearance(55.0, None)[0], il.PASS)
        self.assertEqual(il.classify_lock_appearance(90.0, None)[0], il.PASS)

    def test_early_lock_fails(self):
        self.assertEqual(il.classify_lock_appearance(54.9, None)[0], il.FAIL)

    def test_late_lock_fails(self):
        self.assertEqual(il.classify_lock_appearance(90.5, None)[0], il.FAIL)

    def test_no_lock_fails_when_counter_works(self):
        self.assertEqual(il.classify_lock_appearance(None, None)[0], il.FAIL)

    def test_no_lock_with_disabled_counter_is_blocked_never_pass(self):
        status, reason = il.classify_lock_appearance(None, il.wm_disabled_line(DISABLED_LOG))
        self.assertEqual(status, il.ENV_BLOCKED)
        self.assertIn("idle lock disabled", reason)


class NegativeTests(unittest.TestCase):
    def test_no_lock_observed_passes(self):
        self.assertEqual(il.classify_no_lock([], None, "x")[0], il.PASS)

    def test_observed_lock_fails(self):
        self.assertEqual(il.classify_no_lock([12.0], None, "x")[0], il.FAIL)

    def test_disabled_counter_never_passes_negative_case(self):
        status, _ = il.classify_no_lock([], "flamewm: idle lock disabled", "x")
        self.assertEqual(status, il.ENV_BLOCKED)

    def test_motion_schedule_is_below_idle_threshold(self):
        # Input every 20 s must keep idle time below the 60 s threshold of a 1-minute timeout.
        self.assertLess(il.MOTION_EVERY_S, 60.0)


class DocumentTests(unittest.TestCase):
    def test_all_ids_present_and_fail_dominates(self):
        results = {"IL01": il.entry("IL01", il.PASS), "IL02": il.entry("IL02", il.FAIL, "x")}
        doc = il.compose_document(results)
        self.assertEqual([r["id"] for r in doc["results"]], list(il.IDS))
        self.assertEqual(doc["verdict"], "FAIL")
        self.assertEqual(doc["exit_code"], 1)
        self.assertEqual(doc["results"][2]["status"], il.NOT_RUN)

    def test_all_blocked_is_environment_blocked(self):
        results = {sid: il.entry(sid, il.ENV_BLOCKED, "no xdpyinfo") for sid in il.IDS}
        self.assertEqual(il.compose_document(results)["exit_code"], 3)

    def test_all_pass_is_success(self):
        results = {sid: il.entry(sid, il.PASS) for sid in il.IDS}
        doc = il.compose_document(results)
        self.assertEqual((doc["verdict"], doc["exit_code"]), ("ALL_SCENARIOS_PASS", 0))


if __name__ == "__main__":
    unittest.main()

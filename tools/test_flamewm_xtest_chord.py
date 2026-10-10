"""Pure unit tests for flamewm_xtest.parse_chord. No X server is required."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from flamewm_xtest import parse_chord  # noqa: E402


class ParseChordTests(unittest.TestCase):
    def test_modifiers_and_target(self) -> None:
        self.assertEqual(parse_chord("ctrl+super+Right"), (["ctrl", "super"], "Right"))

    def test_bare_keysym(self) -> None:
        self.assertEqual(parse_chord("Super_L"), ([], "Super_L"))

    def test_trailing_separator_rejected(self) -> None:
        with self.assertRaises(ValueError):
            parse_chord("alt+")

    def test_unknown_modifier_rejected(self) -> None:
        with self.assertRaises(ValueError):
            parse_chord("bogus+a")

    def test_case_insensitive_modifiers(self) -> None:
        self.assertEqual(parse_chord("CTRL+A"), (["ctrl"], "A"))

    def test_modifier_aliases_canonicalised(self) -> None:
        self.assertEqual(parse_chord("control+meta+x"), (["ctrl", "super"], "x"))

    def test_empty_text_rejected(self) -> None:
        with self.assertRaises(ValueError):
            parse_chord("")

    def test_empty_middle_token_rejected(self) -> None:
        with self.assertRaises(ValueError):
            parse_chord("ctrl++a")

    def test_missing_target_when_last_token_is_modifier(self) -> None:
        with self.assertRaises(ValueError):
            parse_chord("ctrl+shift")

    def test_duplicate_modifier_rejected(self) -> None:
        with self.assertRaises(ValueError):
            parse_chord("ctrl+control+a")


if __name__ == "__main__":
    unittest.main()

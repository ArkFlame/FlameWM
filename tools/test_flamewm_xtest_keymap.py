"""Unit tests for the XQueryKeymap modifier helpers in flamewm_xtest. No X server is required."""

from __future__ import annotations

import ctypes
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from flamewm_xtest import (  # noqa: E402
    HELD_MODIFIER_KEYSYMS,
    X11Backend,
    modifiers_from_keymap,
    release_stuck_modifiers,
)

# Fake keycode table: each modifier keysym maps to one keycode (Shift_L=50 as on a stock keymap).
KEYCODES = {"Shift_L": 50, "Shift_R": 62, "Control_L": 37, "Control_R": 105, "Alt_L": 64, "Alt_R": 108,
            "Super_L": 133, "Super_R": 134, "Meta": 135}


def keymap_with(*keycodes: int) -> bytes:
    """Build a 32-byte XQueryKeymap bitmask with the given keycodes set."""
    mask = bytearray(32)
    for code in keycodes:
        mask[code // 8] |= 1 << (code % 8)
    return bytes(mask)


class FakeBackend:
    """Stands in for X11Backend: fixed keycodes and keymap, records XTEST key events."""

    def __init__(self, keymap: bytes, reject: set[str] | None = None) -> None:
        self.keymap = keymap
        self.reject = reject or set()
        self.events: list[tuple[int, bool]] = []
        self._by_code = {code: name for name, code in KEYCODES.items()}

    def keycode(self, keysym: str) -> int:
        return KEYCODES.get(keysym, 0)

    def query_keymap(self) -> bytes:
        return self.keymap

    def fake_key(self, code: int, down: bool) -> bool:
        self.events.append((code, down))
        return self._by_code.get(code) not in self.reject


class FakeX11:
    """ctypes-shaped fake: XQueryKeymap writes the bitmask into the caller buffer."""

    def __init__(self, keymap: bytes) -> None:
        self.keymap = keymap

    def XQueryKeymap(self, display, buffer) -> None:
        buffer[:] = self.keymap

    def XStringToKeysym(self, name: bytes) -> int:
        names = list(KEYCODES)
        return names.index(name.decode()) + 1 if name.decode() in KEYCODES else 0

    def XKeysymToKeycode(self, display, symbol) -> int:
        return KEYCODES[list(KEYCODES)[symbol - 1]] if symbol else 0


class ModifiersFromKeymapTests(unittest.TestCase):
    def test_clean_keymap_reports_no_modifiers(self) -> None:
        self.assertEqual(modifiers_from_keymap(FakeBackend(keymap_with()), keymap_with()), [])

    def test_single_shift_bit_reports_shift_l_only(self) -> None:
        keymap = keymap_with(KEYCODES["Shift_L"])
        self.assertEqual(modifiers_from_keymap(FakeBackend(keymap), keymap), ["Shift_L"])

    def test_non_modifier_bit_is_ignored(self) -> None:
        keymap = keymap_with(24)  # 'q' on a stock keymap: not a modifier keycode in the fake table
        self.assertEqual(modifiers_from_keymap(FakeBackend(keymap), keymap), [])

    def test_several_modifiers_reported_in_canonical_order(self) -> None:
        keymap = keymap_with(KEYCODES["Super_R"], KEYCODES["Control_L"], KEYCODES["Shift_L"])
        self.assertEqual(modifiers_from_keymap(FakeBackend(keymap), keymap),
                         ["Shift_L", "Control_L", "Super_R"])

    def test_every_listed_modifier_is_detected(self) -> None:
        keymap = keymap_with(*(KEYCODES[name] for name in HELD_MODIFIER_KEYSYMS))
        self.assertEqual(modifiers_from_keymap(FakeBackend(keymap), keymap), list(HELD_MODIFIER_KEYSYMS))

    def test_unmapped_modifier_keysym_is_skipped(self) -> None:
        backend = FakeBackend(keymap_with(KEYCODES["Shift_L"]))
        backend.keycode = lambda keysym: 0  # no keycode on this keymap for any name
        self.assertEqual(modifiers_from_keymap(backend, keymap_with(KEYCODES["Shift_L"])), [])


class ReleaseStuckModifiersTests(unittest.TestCase):
    def test_clean_keymap_sends_no_events(self) -> None:
        backend = FakeBackend(keymap_with())
        self.assertEqual(release_stuck_modifiers(backend), [])
        self.assertEqual(backend.events, [])

    def test_releases_only_modifiers_reported_down(self) -> None:
        backend = FakeBackend(keymap_with(KEYCODES["Shift_L"], KEYCODES["Super_L"]))
        self.assertEqual(release_stuck_modifiers(backend), ["Shift_L", "Super_L"])
        self.assertEqual(backend.events, [(KEYCODES["Shift_L"], False), (KEYCODES["Super_L"], False)])

    def test_rejected_release_raises(self) -> None:
        backend = FakeBackend(keymap_with(KEYCODES["Alt_L"]), reject={"Alt_L"})
        with self.assertRaises(RuntimeError):
            release_stuck_modifiers(backend)


class X11BackendKeymapTests(unittest.TestCase):
    def test_query_keymap_returns_the_32_byte_snapshot(self) -> None:
        keymap = keymap_with(KEYCODES["Shift_R"], 9)
        backend = X11Backend(FakeX11(keymap), None, ctypes.c_void_p(1))
        snapshot = backend.query_keymap()
        self.assertEqual(len(snapshot), 32)
        self.assertEqual(snapshot, keymap)

    def test_keycode_resolves_keysym_name_through_x11(self) -> None:
        backend = X11Backend(FakeX11(keymap_with()), None, ctypes.c_void_p(1))
        self.assertEqual(backend.keycode("Shift_L"), KEYCODES["Shift_L"])
        self.assertEqual(backend.keycode("Bogus_Key"), 0)

    def test_fixture_bitmask_detects_shift_through_x11_backend(self) -> None:
        keymap = keymap_with(KEYCODES["Shift_L"])
        backend = X11Backend(FakeX11(keymap), None, ctypes.c_void_p(1))
        self.assertEqual(modifiers_from_keymap(backend, backend.query_keymap()), ["Shift_L"])


if __name__ == "__main__":
    unittest.main()

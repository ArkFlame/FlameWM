"""Regression for E0061 after an old desktop overlay overwrote the scale-aware caller.

The Rust compiler remains the authoritative proof. This guard detects the specific
cross-version mismatch before an expensive workspace build.
"""

from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]


def project_item_call(source: str) -> list[str]:
    match = re.search(r"let key\s*=\s*projection::project_item\s*\((.*?)\)\s*\?;", source, re.S)
    if match is None:
        raise AssertionError("desktop sync_document must call projection::project_item")
    return [part.strip() for part in match.group(1).split(",") if part.strip()]


class DesktopScaleCallContract(unittest.TestCase):
    def test_sync_document_passes_the_actual_output_scale(self):
        # sync_document's project_item call moved from main.rs into slots.rs; the
        # scale now comes from the per-output grid captured in the slot inputs.
        source = (ROOT / "crates/flamewm-desktop/src/slots.rs").read_text()
        call = project_item_call(source)
        self.assertEqual(len(call), 8, f"project_item needs 8 arguments, got {call!r}")
        self.assertEqual(call[6], "inputs.icon.as_deref()")
        self.assertEqual(call[7], "inputs.scale")
        self.assertIn("let output_grid = grid_for_output(state, output);", source)
        self.assertIn("let scale = output_grid.visual_scale_percent();", source)
        self.assertIn("state.icon_slots[index] = Some(key);", source)

    def test_callee_and_icon_cache_receive_the_same_scale(self):
        source = (ROOT / "crates/flamewm-desktop/src/projection.rs").read_text()
        start = source.index("pub fn project_item(")
        end = source.index("\npub fn clear_item(", start)
        body = source[start:end]
        self.assertRegex(body, r"icon_override:\s*Option<&str>,\s*scale:\s*u16,")
        self.assertIn("icons.project_row(document, index, kind, label, point, icon_override, scale)", body)


if __name__ == "__main__":
    unittest.main()

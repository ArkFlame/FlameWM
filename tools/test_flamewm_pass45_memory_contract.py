"""Focused source regression guards; actual Rust unit tests and RSS need native execution."""
import os
import re
import unittest
from pathlib import Path

ROOT = Path(os.getenv('FLAMEWM_SOURCE_ROOT', Path(__file__).resolve().parents[1]))


def src(path):
    return (ROOT / path).read_text()


class MemoryContract(unittest.TestCase):
    def test_screenshot_editor_stages_only_bounded_pixel_rows(self):
        text = src('crates/flamewm-screenshot/src/ui.rs')
        self.assertIn('use x11rb::connection::{Connection, RequestConnection};', text)
        self.assertIn('fn pack_rows(', text)
        self.assertIn('max_request.min(64 * 1024).max(stride)', text)
        self.assertIn('let mut buffer = vec![0u8; stride.checked_mul(rows_per_request)', text)
        self.assertIn('pack_rows(image, &mut buffer[..bytes], start, rows', text)
        self.assertNotIn('vec![0;row_bytes * image.height as usize]', text)
        self.assertNotIn('let (data,depth) = xpixel(conn, screen, image)?', text)
        self.assertIn('bounded_row_packing_preserves_pixels_endianness_and_padding', text)
        self.assertIn('small_rgb565_batches_round_trip_in_pixel_order', text)

    def test_region_crop_reuses_capture_allocation(self):
        text = src('crates/flamewm-screenshot/src/capture.rs')
        self.assertIn('pub fn crop(mut self, rect: Rect)', text)
        self.assertIn('self.rgba.copy_within(start..start + row_bytes, row * row_bytes)', text)
        self.assertIn('self.rgba.truncate(bytes)', text)
        self.assertIn('self.rgba.shrink_to_fit()', text)
        self.assertIn('crop_reuses_full_frame_storage_and_works_with_overlapping_rows', text)
        self.assertNotIn('let mut rgba = Vec::with_capacity(bytes);', text)
        self.assertIn('full.crop(rect)?', src('crates/flamewm-screenshot/src/main.rs'))

    def test_oversized_raster_cannot_exceed_icon_cache_byte_budget(self):
        text = src('crates/flamewm-integrations-linux/src/icons.rs')
        insert = text.split('fn insert(&mut self, key: RasterKey', 1)[1].split('\n    fn memory_estimate_bytes', 1)[0]
        self.assertRegex(insert, r'if cost > self\.budget_bytes\s*\{\s*return;\s*\}')
        self.assertLess(insert.index('if cost > self.budget_bytes'), insert.index('while !self.entries.is_empty()'))
        self.assertIn('oversized_raster_does_not_evict_or_escape_byte_budget', text)

    def test_existing_shell_and_desktop_build_fixes_survive(self):
        runtime = src('crates/flamewm-shell/src/runtime.rs')
        self.assertRegex(runtime, r'pub\s+fn\s+select_panel_output\(')
        self.assertRegex(runtime, r'pub\s+fn\s+select_input_output\(')
        self.assertIn('pointer_on_output', src('crates/flamewm-desktop/src/sticky.rs'))
        self.assertIn('grid_for_output(state, &output)', src('crates/flamewm-desktop/src/main.rs'))
        clipboard = src('crates/flamewm-screenshot/src/clipboard.rs')
        self.assertIn('use x11rb::connection::{Connection, RequestConnection};', clipboard)

    def test_installer_and_shortcuts_still_include_screenshot(self):
        self.assertIn('flamewm-screenshot', src('packaging/install-manifest.txt'))
        self.assertIn('flamewm-screenshot', src('packaging/source-manifest.txt') + src('scripts/package'))
        self.assertIn('ACTION_SCREENSHOT_FULL', src('crates/flamewm-api/src/shortcuts.rs'))
        self.assertTrue((ROOT / 'packaging/flamewm-session.desktop').is_file())


if __name__ == '__main__':
    unittest.main()

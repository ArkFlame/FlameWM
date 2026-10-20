"""Source contracts for the Pass 47 wallpaper allocation fast paths.

Rust pixel-equivalence and buffer-ownership checks live in wallpaper_loader.rs.
These guards are deliberately source-level and do not substitute for cargo test.
"""

import os
from pathlib import Path
import re
import unittest

ROOT = Path(os.environ.get('FLAMEWM_TEST_ROOT', Path(__file__).resolve().parents[1]))
SOURCE = ROOT / 'crates/flamewm-desktop/src/wallpaper_loader.rs'


class WallpaperIdentityFastPath(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.source = SOURCE.read_text()

    def test_full_root_monitor_bypasses_second_frame(self):
        text = self.source
        start = text.index('fn compose_decoded(')
        section = text[start:text.index('fn compose_per_monitor(', start)]
        self.assertIn('single_full_root_output(monitors, target)', section)
        self.assertIn('if full_root {\n        wallpaper_fit::compose(&natural, target.0, target.1, fit)', section)
        self.assertIn('compose_per_monitor(&natural, target, monitors, fit)', section)
        # The only root-sized canvas is allocated in the bypassed function.
        self.assertNotIn('let mut canvas', section)

    def test_exact_size_decoded_buffer_moves_without_clone(self):
        text = self.source
        start = text.index('fn compose_decoded(')
        section = text[start:text.index('fn compose_per_monitor(', start)]
        self.assertRegex(section, r'if full_root && target\.0 != 0 && target\.1 != 0\s*&& \(natural\.width, natural\.height\) == target\s*\{\s*return Some\(natural\);')
        self.assertNotIn('natural.clone()', section)

    def test_multi_monitor_independent_composition_is_retained(self):
        text = self.source
        self.assertIn('for monitor in monitors {', text)
        self.assertIn('let image = wallpaper_fit::compose(natural, rect.width as u32, rect.height as u32, fit)?;', text)
        self.assertIn('fn independent_outputs_fit_the_wallpaper_separately_from_shared_canvas()', text)
        self.assertIn('fn one_full_output_matches_shared_composition_for_all_fits()', text)
        self.assertIn('fn matching_root_wallpaper_reuses_decoded_buffer_without_copy()', text)

    def test_worker_mailbox_and_stale_result_guards_unchanged(self):
        text = self.source
        for needle in ['latest_generation', 'state.pending = Some(request)', 'if request.generation != state.latest_generation', 'PollOutcome::Stale', 'WallpaperPresentation::Solid']:
            self.assertIn(needle, text)


if __name__ == '__main__':
    unittest.main()

"""Read-only, self-contained tests for the smaps ownership report and source contract."""
import importlib.util
from pathlib import Path
import os
import tempfile
import unittest

FILE = Path(__file__).with_name('bench-flamewm-memory-owners.py')
spec = importlib.util.spec_from_file_location('flamewm_memory_owners', FILE)
bench = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench)

class OwnersTests(unittest.TestCase):
    def test_smaps_groups_pss_and_private_fields(self):
        sample = '''1234-2345 rw-p 00000000 00:00 0 [heap]\nPss: 100 kB\nRss: 110 kB\nPrivate_Clean: 0 kB\nPrivate_Dirty: 100 kB\nAnonymous: 100 kB\n2345-3456 r-xp 00000000 08:01 10 /usr/lib/libc.so.6\nPss: 50 kB\nRss: 100 kB\nPrivate_Dirty: 0 kB\nPrivate_Clean: 0 kB\n3456-4567 r-xp 00000000 08:01 10 /usr/bin/flamewm-shell\nPss: 20 kB\nRss: 30 kB\nPrivate_Clean: 5 kB\nPrivate_Dirty: 0 kB\n'''
        got = bench.parse_smaps(sample, '/usr/bin/flamewm-shell')
        self.assertEqual(got['heap | [heap]']['Pss'], 100)
        self.assertEqual(got['shared-libraries | /usr/lib/libc.so.6']['Pss'], 50)
        self.assertEqual(got['executable | /usr/bin/flamewm-shell']['Pss'], 20)

    def test_readonly_discovery_with_exact_executable(self):
        with tempfile.TemporaryDirectory() as t:
            root = Path(t)
            fake = root / '1234'; fake.mkdir()
            (fake/'comm').write_text('flamewm-shell\n')
            (fake/'exe').symlink_to('/usr/bin/flamewm-shell')
            self.assertEqual(len(bench.discover(root, None)), 1)
            (fake/'comm').write_text('flamewm-task-ma\n')
            self.assertEqual(len(bench.discover(root, None)), 1,
                'truncated Linux comm must not suppress exact executable match')
            (fake/'exe').unlink()
            (fake/'exe').symlink_to('/usr/bin/other-tool')
            self.assertEqual(bench.discover(root, None), [])
            self.assertEqual(len(bench.discover(root, {1234})), 1)

    def test_source_uses_shared_writer_instead_of_monitor_raster(self):
        base = Path(os.getenv('FLAMEWM_SOURCE_ROOT', FILE.parents[1]))
        wallpaper = (base/'crates/flamewm-image-core/src/wallpaper.rs').read_text()
        desktop = (base/'crates/flamewm-desktop/src/wallpaper_loader.rs').read_text()
        self.assertIn('fn emit_region(', wallpaper)
        self.assertIn('pub fn compose_region_into(', wallpaper)
        self.assertIn('compose_region_into(\n            natural, ow, oh', wallpaper)
        fragment = desktop.split('fn compose_per_monitor(', 1)[1].split('\n#[cfg(test)]', 1)[0]
        self.assertIn('flamewm_image_core::wallpaper::compose_region_into(', fragment)
        self.assertNotIn('let image = wallpaper_fit::compose(natural, rect.width', fragment)

if __name__ == '__main__':
    unittest.main()

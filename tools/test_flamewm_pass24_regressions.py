#!/usr/bin/env python3
"""Offline source-contract gates for Pass 24. Native Rust/X11 remains separate."""
import unittest
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]

class Pass24Contracts(unittest.TestCase):
    def read(self, path):
        return (ROOT / path).read_text()

    def test_randr_does_not_make_disconnected_crtc_root_sized(self):
        src = self.read('crates/flamewm-wm-x11/src/lib.rs')
        self.assertIn('single_output_root_geometry(single_randr_output, root_size, &modes)', src)
        self.assertIn('if !single_output || !root_size.is_valid()', src)
        self.assertIn('output.primary && output.geometry.is_valid()', src)
        self.assertNotIn('if !geometry.is_valid() && root_size.is_valid()', src)

    def test_disabled_output_cannot_create_a_panel(self):
        src = self.read('crates/flamewm-platform/src/services/panels.rs')
        self.assertIn('.filter(|output| output.connected && output.geometry.is_valid())', src)
        self.assertIn('disconnected_or_geometryless_modes_never_create_panels', src)
        self.assertIn('legacy_xephyr_root_without_randr_mode_still_has_a_panel', src)
        self.assertIn('attached_but_disabled_crtc_cannot_create_phantom_panel', src)

    def test_drag_preview_pixels_are_actually_semialpha(self):
        src = self.read('crates/flamewm-desktop/src/projection.rs')
        self.assertIn('fade_ghost_pixels(&mut raster.pixels)', src)
        self.assertIn('fn ghost_alpha_fades_actual_pixels_without_colorkey()', src)
        css = self.read('ui/desktop/desktop.css')
        self.assertIn('desktop-drag-ghost-label-1', css)
        self.assertIn('color:#ffffff80', css)

    def test_file_opening_uses_shared_mime_provider(self):
        apps = self.read('crates/flamewm-applications/src/lib.rs')
        self.assertIn('pub fn open_target', apps)
        self.assertIn('"kioclient6", &["exec"]', apps)
        # File opening shares the canonical startup-notified, child-reaping launcher.
        self.assertIn('spawn_with_startup(&mut command, &name, &icon, &wm_class, true)', apps)
        self.assertIn('spawn_reaped_with_id(command, if sent { id.clone() } else { None })', apps)
        self.assertIn('open_target(path.as_os_str())', self.read('crates/flamewm-desktop/src/main.rs'))
        self.assertIn('open_target(std::ffi::OsStr::new(uri))', self.read('crates/flamewm-x11-desktop/src/lib.rs'))

    def test_no_automatic_password_migration_or_wallet_wipe(self):
        changed_files = ['crates/flamewm-applications/src/lib.rs', 'crates/flamewm-desktop/src/main.rs']
        for path in changed_files:
            src = self.read(path)
            self.assertNotIn('kwallet_delete', src)
            self.assertNotIn('password_store_migrate', src)

if __name__ == '__main__': unittest.main()

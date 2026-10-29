#!/usr/bin/env python3
"""Offline source-contract checks for Pass 15. Native visual checks are separate.

FLAMEWM_CHECK_SOURCE=/path/to/other/checkout python3 -I tools/test_flamewm_surface_icon_slide_regressions.py
"""
import os
import unittest
from pathlib import Path

ROOT = Path(os.environ.get('FLAMEWM_CHECK_SOURCE', Path(__file__).resolve().parents[1]))

def src(path):
    return (ROOT / path).read_text(encoding='utf-8')

class PerOutputOpacity(unittest.TestCase):
    def test_surface_backdrop_belongs_to_each_panel(self):
        rs = src('crates/flamewm-shell/src/runtime.rs')
        record = rs.split('pub struct PanelSurfaceRecord {', 1)[1].split('}', 1)[0]
        self.assertIn('snapshot: Option<RgbaImage>', record)
        self.assertIn('capture: Option<PanelCapture>', record)
        self.assertNotIn('panel_snapshot: Option<RgbaImage>', rs.split('pub struct ShellSurfaces {', 1)[1].split('}', 1)[0])
        self.assertIn('for record in &mut self.panels', rs)
        self.assertIn('for record in &self.panels', rs)
        self.assertIn('runtime.set_surface_backdrop(panel,', rs)

    def test_settings_projected_and_redrawn_for_every_output(self):
        app = src('crates/flamewm-shell/src/app.rs')
        project = app.split('fn project_settings(', 1)[1].split('fn quick_kind_shown(', 1)[0]
        self.assertIn('for panel in self.surfaces.panel_handles()', project)
        self.assertIn('runtime.with_document(panel', project)
        self.assertIn('runtime.redraw(panel)', project)
        self.assertIn('self.surfaces.refresh_panel_backdrops(', app)

    def test_new_panel_gets_settings_projection(self):
        app = src('crates/flamewm-shell/src/app.rs')
        resync = app.split('pub(crate) fn resync_panel_geometry(',1)[1].split('fn ',1)[0]
        self.assertIn('self.settings_projection_pending = true;', resync)

class OnlyTwoIconThemes(unittest.TestCase):
    def test_bundled_default_is_mactahoe_and_cursor_registry_intact(self):
        picker = src('crates/flamewm-settings/src/theme_picker.rs')
        self.assertIn('BUNDLED_ICON_THEMES: [&str; 1] = ["MacTahoe"]', picker)
        self.assertIn('registry.cursor.push', picker)
        self.assertNotIn('PlagueSur', picker)

    def test_no_old_icon_packs_in_binary_package(self):
        pkg = src('scripts/package')
        install = pkg.split('# Shipped default icon theme (MacTahoe)',1)[1]
        self.assertNotIn('candy-icons', install)
        self.assertNotIn('McMojave-circle', install)
        self.assertIn('assets/themes/icons/MacTahoe"', install)
        manifest = src('packaging/install-manifest.txt')
        self.assertNotIn('assets/themes/icons/candy-icons ',manifest)
        self.assertNotIn('assets/themes/icons/McMojave-circle ',manifest)

class ReverseSlideReliability(unittest.TestCase):
    def test_full_and_clipped_paths_share_shape(self):
        code = src('crates/flamewm-render-x11/src/app/render.rs')
        self.assertIn('fn base_shape_spans(', code)
        self.assertIn('self.base_shape_spans(commands, scale, backdrop_radius)', code)
        self.assertIn('self.base_shape_spans(&cached.commands, 1.0, backdrop_radius)', code)
        self.assertIn('self.dock_corner_radius', code)

    def test_close_cannot_restore_shape_before_unmap(self):
        code = src('crates/flamewm-render-x11/src/surface_controller.rs')
        branch = code.split('// Never restore the full shape while still mapped:',1)[1]
        branch = branch.split('if landing_releases_grab',1)[0]
        self.assertLess(branch.index('XUnmapWindow(display, window)'), branch.index('refresh_shape_mask(&instance.document)'))
        slide = src('crates/flamewm-render-x11/src/surface_controller/slide.rs')
        tests = src('crates/flamewm-render-x11/src/surface_controller/tests.rs')
        self.assertIn('SLIDE_OPEN_BASE_MS', slide)
        self.assertIn('Easing::EaseInCubic,', tests)
        self.assertIn('(1.0, 0.0)', tests)
        self.assertNotIn('Slide-out starts above application windows', code)

    def test_mirror_easing_is_registered(self):
        code = src('crates/flamewm-reactor/src/timeline.rs')
        self.assertIn('EaseInCubic,', code)
        self.assertIn('Self::EaseInCubic => t.powi(3)', code)

if __name__ == '__main__':
    unittest.main()

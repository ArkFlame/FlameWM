"""Pass 17 static source contracts and independent rounded contour sampling."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]
def read(p): return (ROOT / p).read_text()

class Pass17Contracts(unittest.TestCase):
    def test_panel_root_clears_only_in_separate_mode(self):
        projection=read('crates/flamewm-shell/src/settings_projection.rs')
        self.assertIn('if separate {\n                document.border("taskbar", UiColor { a: 0, ..fill })?', projection)
        self.assertIn('project_surface_border(document, root, snapshot)?;', projection)
        self.assertIn('assert!(unified.borders["taskbar"].a > 0', projection)
    def test_workspace_is_shared_capsule_and_child_buttons_remain_square(self):
        panel=read('crates/flamewm-shell/src/projection.rs')
        css=read('ui/shell/flamewm.css')
        for setter in ('background','border','corner_radius','node_backdrop'):
            self.assertIn(f'document.{setter}("workspace-pager",',panel)
        self.assertIn('.tray-group, .workspace-pager {',css)
        self.assertIn('.workspace-button {',css)
        self.assertIn('border-radius:1px;',css)
    def test_lower_contour_reuses_shared_owner(self):
        painter=read('crates/flamewm-wm-x11/src/frame/chrome.rs')
        self.assertIn('fn bottom_contour_strip(',painter)
        self.assertIn('flamewm_ui_core::surface::inner_contour_coverage(',painter)
        self.assertIn('renderer.blit_rgba(',painter)
        self.assertIn('bottom_result?;',painter)
    def test_terminal_pool_preserves_opencode_colored_rows(self):
        pool=read('crates/flamewm-terminal/src/pool.rs')
        build=read('crates/flamewm-terminal/build.rs')
        ui=read('crates/flamewm-terminal/src/ui.rs')
        self.assertIn('pub const POOL_RUNS: usize = 128;',pool)
        self.assertIn('include!("src/pool.rs")',build)
        self.assertIn('0..POOL_RUNS',ui)
        self.assertIn('dense_tui_row_preserves_per_cell_colors_within_supported_width',ui)
    def test_bottom_pixels_have_rounded_corner_and_continuous_edge(self):
        def within(px, py, w, h, r):
            if px<0 or py<0 or px>=w or py>=h: return False
            if r<=0: return True
            cx=max(r,min(px,w-r));cy=max(r,min(py,h-r))
            return (px-cx)**2+(py-cy)**2<=r*r
        def cov(x,y,w=120,h=100,r=12):
            return sum(within(x+(i+0.5)/4,y+(j+0.5)/4,w,h,r) and not within(x+(i+0.5)/4-1,y+(j+0.5)/4-1,w-2,h-2,r-1) for i in range(4) for j in range(4))
        for x,y in [(0,99),(119,99),(60,87)]: self.assertEqual(cov(x,y),0)
        for x,y in [(11,99),(108,99),(60,99),(0,87)]:self.assertGreater(cov(x,y),0)

if __name__=='__main__': unittest.main()

#!/usr/bin/env python3
"""Source contracts plus independent corner-geometry checks. Native Rust tests are separate."""
from pathlib import Path
import math
import unittest

ROOT = Path(__file__).resolve().parents[1]
def read(path): return (ROOT/path).read_text()

def inset(w, h, r, y):
    if r <= 1: return 0
    y = min(y, h - 1 - y)
    if y >= r: return 0
    dy = r - (y + 0.5)
    return int(max(0, min(r, math.floor(r - math.sqrt(max(0, r*r-dy*dy))))))

class SourceContracts(unittest.TestCase):
    def test_agent_fix_removes_duplicates_and_borrow_error(self):
        model=read('crates/flamewm-task-manager/src/model.rs')
        app=read('crates/flamewm-task-manager/src/lib.rs')
        self.assertEqual(model.count('pub priority: i64'),1)
        self.assertEqual(model.count('priority: fields[15].parse().ok()?'),1)
        self.assertIn('visible_rows(*height)',app)
    def test_shared_contour_and_node_border_width(self):
        core=read('crates/flamewm-ui-core/src/surface.rs')
        panel=read('crates/flamewm-shell/src/projection.rs')
        style=read('ui/shell/flamewm.css')
        self.assertIn('pub fn inner_contour_coverage(',core)
        self.assertIn('set_taskbar_border(',panel)
        for node in ['start-capsule','tray-group','clock-capsule','task-slots']:
            self.assertIn(f'document.border("{node}"',panel)
        self.assertIn('.capsule, .task-slots, .tray-group',style)
        self.assertIn('border:1px solid',style)
    def test_shadow_radius_flows_through_mask_shading_and_rubber(self):
        pure=read('crates/flamewm-wm-x11/src/frame/shadow.rs')
        wm=read('crates/flamewm-wm-x11/src/wm/shadow.rs')
        self.assertIn('rounded_shadow_mask_rectangles(',pure)
        self.assertIn('shade_visible_shadow_rounded(',pure)
        self.assertIn('held.radius',wm)
        self.assertIn('ShadowBands::capture_rounded(',wm)
        self.assertIn('visible_motion_damage_regions_rounded(',wm)
    def test_clipped_ring_keeps_arc(self):
        painter=read('crates/flamewm-render-x11/src/xrender_aa.rs')
        caller=read('crates/flamewm-render-x11/src/app/render.rs')
        self.assertIn('stroke_rounded_rect_clipped(',painter)
        self.assertIn('stroke_rounded_rect_clipped(',caller)
        self.assertIn('stroke_row_runs(',painter)
    def test_rounded_notch_geometry_with_randomized_outputs(self):
        import random
        rng=random.Random(20261027)
        checked=0
        for _ in range(5000):
            w=rng.randrange(60,2000)
            h=rng.randrange(60,1600)
            r=rng.randrange(0,24)
            for y in (0,1,h//2,max(0,h-r),h-1):
                i=inset(w,h,r,y)
                self.assertTrue(0<=i<=r)
                self.assertEqual(i,inset(w,h,r,h-1-y))
                self.assertGreaterEqual(w-i,w//2)
                checked+=1
        self.assertEqual(checked,25000)

if __name__=='__main__': unittest.main()

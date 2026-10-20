"""Source contract guards for native animation regressions (not an X11 runtime substitute).

The native Rust tests for shape geometry, frame ordering, content capture and
maximize defaults live beside their canonical owners. This probe protects the
producer->consumer seams when an agent partially merges only some files.
"""
from pathlib import Path
import re
import unittest

ROOT=Path(__file__).resolve().parents[1]
def src(name): return (ROOT/name).read_text()

class GraphicalFlickerSourceContracts(unittest.TestCase):
    def test_slide_shape_reapplied_after_redraw_and_during_every_frame(self):
        present=src('crates/flamewm-render-x11/src/surface_controller/present.rs')
        controller=src('crates/flamewm-render-x11/src/surface_controller.rs')
        self.assertIn('Self::refresh_moving_mask(instance, dock, active.edge)',present)
        self.assertGreaterEqual(controller.count('Self::refresh_moving_mask(instance, dock, active.edge)'),1)
        self.assertIn('unsafe { XSync(display, 0) };',controller)
        self.assertLess(controller.index('unsafe { XUnmapWindow(display, window) };'),
                        controller.index('unsafe { XSync(display, 0) };'))

    def test_overlay_shape_is_recomputed_for_all_styles(self):
        overlay=src('crates/flamewm-render-x11/src/overlay.rs')
        self.assertIn('shape: Option<XShapeBridge>',overlay)
        for method in ('paint_warp','paint_stretch','paint_fade','paint_crossfade'):
            start=overlay.index('fn '+method+'(')
            end=overlay.find('\n    fn ',start+4)
            if end == -1: end=len(overlay)
            self.assertIn('apply_content_mask',overlay[start:end],method)
        self.assertIn('shape.apply_clipped_mask(self.window, &[], (0,0,0,0))',overlay)

    def test_shape_required_and_open_fade_has_same_rounded_radius(self):
        overlay=src('crates/flamewm-render-x11/src/overlay.rs')
        wm=src('crates/flamewm-wm-x11/src/wm/overlay.rs')
        self.assertIn('self.shape = Some(unsafe { XShapeBridge::new(display)? });',overlay)
        self.assertIn('overlay.shape = Some(unsafe { XShapeBridge::new(display)? });',overlay)
        self.assertIn('fn transition_corner_radius(&self, client: Window)',wm)
        self.assertEqual(wm.count('overlay.set_corner_radius(self.transition_corner_radius(client))'),2)
        self.assertIn('overlay.set_linear_stretch(true)',wm)

    def test_scale_is_not_magic_lamp_and_default_maximize_off(self):
        wm=src('crates/flamewm-wm-x11/src/wm/overlay.rs')
        self.assertRegex(wm,r'Effect::Open, animation::Style::Scale')
        self.assertIn('TransitionStyle::Stretch',wm)
        settings=src('crates/flamewm-platform/src/settings.rs')
        self.assertIn('maximize_animation: AnimationStyle::None',settings)
        self.assertIn('maximize_animation_enabled: false',settings)
        self.assertIn('"maximizeAnimation" => &[Self::None, Self::Fade, Self::Scale]',settings)
        html=src('ui/settings.html')
        self.assertIn('id="maximize-style-scale"',html)
        self.assertNotIn('id="maximize-style-magic-lamp"',html)

    def test_frame_geometry_commits_before_paint_in_resize(self):
        controller=src('crates/flamewm-wm-x11/src/frame/controller.rs')
        block=controller.split('fn full_configure(',1)[1].split('fn move_effects',1)[0]
        self.assertLess(block.index('configure_effects(frame, extents)'),block.index('FrameEffect::PaintChrome'))
        block=controller.split('fn resize_step(',1)[1].split('pub fn reduce(',1)[0]
        self.assertIn('FrameEffect::PaintChrome, FrameEffect::ApplyShape',block)
        self.assertIn('fn resize_motion_paints_after_native_resize_not_at_the_old_size',controller)

    def test_window_capture_gates_unpainted_client_interior(self):
        animation=src('crates/flamewm-wm-x11/src/animation.rs')
        consumer=src('crates/flamewm-wm-x11/src/wm/overlay.rs')
        self.assertIn('pub fn is_unpainted_client_capture(',animation)
        self.assertIn('animation::is_unpainted_client_capture(',consumer)

if __name__=='__main__': unittest.main()

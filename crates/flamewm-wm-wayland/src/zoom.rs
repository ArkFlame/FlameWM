//! Magnifier geometry for the composite pass: the source rectangle sampled from the offscreen
//! frame, centred on the pointer and clamped inside the output. Pure, no Smithay.
//!
//! Precedent: `KWin`'s zoom effect (zoom about the pointer, clamped to the screen edges) and COSMIC's
//! magnifier (level steps driven by shortcuts). Choice only; re-implemented here, nothing copied.
//! The DRM path does not apply the magnifier yet (not covered).

use flamewm_api::settings::{ZOOM_LEVEL_MAX, ZOOM_LEVEL_MIN};
use flamewm_api::shortcuts::{ACTION_ZOOM_IN, ACTION_ZOOM_OUT, ACTION_ZOOM_RESET};

/// Zoom change per shortcut press.
const STEP: f64 = 0.5;

/// The sampled part of the frame, in physical pixels with a top-left origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// The zoom level after a zoom shortcut, or `None` when `action` is not a zoom action.
pub fn after_action(zoom: f64, action: &str) -> Option<f64> {
    match action {
        ACTION_ZOOM_IN => Some((zoom + STEP).min(ZOOM_LEVEL_MAX)),
        ACTION_ZOOM_OUT => Some((zoom - STEP).max(ZOOM_LEVEL_MIN)),
        ACTION_ZOOM_RESET => Some(ZOOM_LEVEL_MIN),
        _ => None,
    }
}

/// The source rectangle for `zoom` centred on `pointer`, in physical pixels of a `size` output.
/// At zoom 1 it is the whole output, and it never leaves the output.
pub fn viewport(size: (f64, f64), zoom: f64, pointer: (f64, f64)) -> Viewport {
    let zoom = zoom.clamp(ZOOM_LEVEL_MIN, ZOOM_LEVEL_MAX);
    let (w, h) = (size.0 / zoom, size.1 / zoom);
    Viewport {
        x: (pointer.0 - w / 2.0).clamp(0.0, size.0 - w),
        y: (pointer.1 - h / 2.0).clamp(0.0, size.1 - h),
        w,
        h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUTPUT: (f64, f64) = (800.0, 600.0);

    #[test]
    fn unit_zoom_samples_the_whole_output() {
        assert_eq!(
            viewport(OUTPUT, 1.0, (100.0, 100.0)),
            Viewport {
                x: 0.0,
                y: 0.0,
                w: 800.0,
                h: 600.0
            }
        );
    }

    #[test]
    fn zoom_crops_around_the_pointer() {
        assert_eq!(
            viewport(OUTPUT, 2.0, (400.0, 300.0)),
            Viewport {
                x: 200.0,
                y: 150.0,
                w: 400.0,
                h: 300.0
            }
        );
    }

    #[test]
    fn viewport_clamps_inside_the_output_at_each_edge() {
        let top_left = viewport(OUTPUT, 4.0, (0.0, 0.0));
        assert_eq!((top_left.x, top_left.y), (0.0, 0.0));
        let bottom_right = viewport(OUTPUT, 4.0, (800.0, 600.0));
        assert_eq!((bottom_right.x, bottom_right.y), (600.0, 450.0));
        assert_eq!((bottom_right.w, bottom_right.h), (200.0, 150.0));
    }

    #[test]
    fn zoom_outside_the_range_is_clamped_to_it() {
        assert_eq!(viewport(OUTPUT, 99.0, (400.0, 300.0)).w, 100.0);
        assert_eq!(viewport(OUTPUT, 0.1, (400.0, 300.0)).w, 800.0);
    }

    #[test]
    fn zoom_shortcuts_step_clamp_and_reset() {
        assert_eq!(after_action(1.0, ACTION_ZOOM_IN), Some(1.5));
        assert_eq!(after_action(7.8, ACTION_ZOOM_IN), Some(8.0));
        assert_eq!(after_action(1.2, ACTION_ZOOM_OUT), Some(1.0));
        assert_eq!(after_action(3.0, ACTION_ZOOM_RESET), Some(1.0));
        assert_eq!(after_action(3.0, "WindowClose"), None);
    }
}

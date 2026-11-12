//! Magnifier geometry for the composite pass: the source rectangle sampled from the offscreen
//! frame, placed by a follow mode and clamped inside the output. Pure, no Smithay.
//!
//! Precedent: `KWin`'s zoom effect (pointer tracking modes proportional, centred and push) and
//! COSMIC's magnifier (level steps driven by shortcuts). Choice only; re-implemented here, nothing
//! copied. The DRM path does not apply the magnifier yet (not covered).

use flamewm_api::settings::{ZOOM_LEVEL_MAX, ZOOM_LEVEL_MIN};
use flamewm_api::shortcuts::{ACTION_ZOOM_IN, ACTION_ZOOM_OUT, ACTION_ZOOM_RESET};

/// Zoom change per shortcut press.
const STEP: f64 = 0.5;

/// How the magnified viewport tracks the pointer (`zoomFollowMode`), named as in `KWin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FollowMode {
    /// The pointer's place across the output maps to the viewport's place across the frame.
    #[default]
    Proportional,
    /// The pointer stays at the viewport centre, clamped at the output edges.
    Centred,
    /// The viewport holds still until the pointer reaches its edge, then moves just far enough.
    Push,
}

impl FollowMode {
    /// Parses the `zoomFollowMode` value. Unknown text is `None`, so the caller keeps the default.
    pub fn from_setting(text: &str) -> Option<Self> {
        match text {
            "proportional" => Some(Self::Proportional),
            "centred" => Some(Self::Centred),
            "push" => Some(Self::Push),
            _ => None,
        }
    }
}

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

/// The source rectangle for `zoom` following `pointer`, in physical pixels of a `size` output.
/// `previous` is the last origin: `Push` holds it, and `None` (first frame or after a zoom change)
/// centres on the pointer. At zoom 1 it is the whole output, and it never leaves the output.
pub fn viewport(
    mode: FollowMode,
    size: (f64, f64),
    zoom: f64,
    pointer: (f64, f64),
    previous: Option<(f64, f64)>,
) -> Viewport {
    let zoom = zoom.clamp(ZOOM_LEVEL_MIN, ZOOM_LEVEL_MAX);
    let (w, h) = (size.0 / zoom, size.1 / zoom);
    let centred = (pointer.0 - w / 2.0, pointer.1 - h / 2.0);
    let (x, y) = match (mode, previous) {
        (FollowMode::Proportional, _) => (
            pointer.0 / size.0 * (size.0 - w),
            pointer.1 / size.1 * (size.1 - h),
        ),
        (FollowMode::Push, Some((px, py))) => (push(px, pointer.0, w), push(py, pointer.1, h)),
        (FollowMode::Centred | FollowMode::Push, _) => centred,
    };
    Viewport {
        x: x.clamp(0.0, size.0 - w),
        y: y.clamp(0.0, size.1 - h),
        w,
        h,
    }
}

/// The least move of `origin` that keeps `at` inside `[origin, origin + len]`.
fn push(origin: f64, at: f64, len: f64) -> f64 {
    if at < origin {
        at
    } else if at > origin + len {
        at - len
    } else {
        origin
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUTPUT: (f64, f64) = (800.0, 600.0);

    fn view(mode: FollowMode, zoom: f64, pointer: (f64, f64)) -> Viewport {
        viewport(mode, OUTPUT, zoom, pointer, None)
    }

    #[test]
    fn unit_zoom_samples_the_whole_output() {
        for mode in [FollowMode::Proportional, FollowMode::Centred] {
            assert_eq!(
                view(mode, 1.0, (100.0, 100.0)),
                Viewport {
                    x: 0.0,
                    y: 0.0,
                    w: 800.0,
                    h: 600.0
                }
            );
        }
    }

    #[test]
    fn centred_crops_around_the_pointer() {
        assert_eq!(
            view(FollowMode::Centred, 2.0, (400.0, 300.0)),
            Viewport {
                x: 200.0,
                y: 150.0,
                w: 400.0,
                h: 300.0
            }
        );
    }

    #[test]
    fn proportional_maps_the_pointer_across_the_whole_frame() {
        // Pointer at the output edges reaches the frame edges; the middle stays in the middle.
        let left = view(FollowMode::Proportional, 2.0, (0.0, 0.0));
        assert_eq!((left.x, left.y), (0.0, 0.0));
        let right = view(FollowMode::Proportional, 2.0, (800.0, 600.0));
        assert_eq!((right.x, right.y), (400.0, 300.0));
        // A quarter of the way across moves the viewport a quarter of the slack, not to centre.
        let quarter = view(FollowMode::Proportional, 2.0, (200.0, 150.0));
        assert_eq!((quarter.x, quarter.y), (100.0, 75.0));
    }

    #[test]
    fn push_holds_until_the_pointer_reaches_the_edge() {
        let held = (200.0, 150.0);
        let inside = viewport(FollowMode::Push, OUTPUT, 2.0, (300.0, 250.0), Some(held));
        assert_eq!((inside.x, inside.y), held);
        let right = viewport(FollowMode::Push, OUTPUT, 2.0, (650.0, 250.0), Some(held));
        assert_eq!((right.x, right.y), (250.0, 150.0));
        let left = viewport(FollowMode::Push, OUTPUT, 2.0, (100.0, 250.0), Some(held));
        assert_eq!((left.x, left.y), (100.0, 150.0));
    }

    #[test]
    fn push_without_a_previous_origin_centres_on_the_pointer() {
        let first = view(FollowMode::Push, 2.0, (400.0, 300.0));
        assert_eq!((first.x, first.y), (200.0, 150.0));
    }

    #[test]
    fn viewport_clamps_inside_the_output_at_each_edge() {
        let top_left = view(FollowMode::Centred, 4.0, (0.0, 0.0));
        assert_eq!((top_left.x, top_left.y), (0.0, 0.0));
        let bottom_right = view(FollowMode::Centred, 4.0, (800.0, 600.0));
        assert_eq!((bottom_right.x, bottom_right.y), (600.0, 450.0));
        assert_eq!((bottom_right.w, bottom_right.h), (200.0, 150.0));
    }

    #[test]
    fn zoom_outside_the_range_is_clamped_to_it() {
        assert_eq!(view(FollowMode::Centred, 99.0, (400.0, 300.0)).w, 100.0);
        assert_eq!(view(FollowMode::Centred, 0.1, (400.0, 300.0)).w, 800.0);
    }

    #[test]
    fn follow_mode_setting_parses_and_rejects_unknown_text() {
        assert_eq!(FollowMode::from_setting("push"), Some(FollowMode::Push));
        assert_eq!(
            FollowMode::from_setting("centred"),
            Some(FollowMode::Centred)
        );
        assert_eq!(FollowMode::from_setting("bogus"), None);
        assert_eq!(FollowMode::default(), FollowMode::Proportional);
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

//! Pure region-selection rules for the X11 overlay: clamp to the captured output,
//! confirm a normalised drag above the minimum size, and keyboard nudging. No X11
//! state lives here, so the rules are unit-tested without a display.
//! Cancel is not a rule: the caller simply never calls `confirm`.
//! Crop of the RGBA frame is `Image::crop` in `capture.rs`.

use crate::capture::Rect;

/// Smallest accepted side in pixels; a narrower drag is treated as cancellation.
pub const MIN_SIDE: u32 = 3;

/// Pixels moved per arrow-key press.
// shortcut: keyboard nudge is model-only (no key binding in ui.rs yet); revisit when the overlay binds arrow keys.
#[cfg(test)]
pub const NUDGE_STEP: i32 = 1;

/// Clamps `point` inside `bounds`; empty bounds collapse to their origin.
pub fn clamp_point(point: (i32, i32), bounds: Rect) -> (i32, i32) {
    (
        clamp_axis(point.0, bounds.x, bounds.w),
        clamp_axis(point.1, bounds.y, bounds.h),
    )
}

fn clamp_axis(value: i32, start: i32, len: u32) -> i32 {
    let first = i64::from(start);
    let last = (first + i64::from(len)).max(first + 1) - 1;
    i64::from(value).clamp(first, last.min(i64::from(i32::MAX))) as i32
}

/// Moves `point` by `(dx, dy)` steps of `NUDGE_STEP`, staying inside `bounds`.
#[cfg(test)]
pub fn nudge(point: (i32, i32), dx: i32, dy: i32, bounds: Rect) -> (i32, i32) {
    let moved = (
        point.0.saturating_add(dx.saturating_mul(NUDGE_STEP)),
        point.1.saturating_add(dy.saturating_mul(NUDGE_STEP)),
    );
    clamp_point(moved, bounds)
}

/// Confirms a drag from `anchor` to `cursor`, both clamped into `bounds`.
/// Returns the normalised rectangle, or None when either side is below `MIN_SIDE`.
pub fn confirm(anchor: (i32, i32), cursor: (i32, i32), bounds: Rect) -> Option<Rect> {
    let a = clamp_point(anchor, bounds);
    let c = clamp_point(cursor, bounds);
    let r = Rect::from_points(a.0, a.1, c.0, c.1);
    (r.w >= MIN_SIDE && r.h >= MIN_SIDE).then_some(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect {
        x: 0,
        y: 0,
        w: 100,
        h: 50,
    };

    #[test]
    fn reverse_drag_normalises() {
        let r = confirm((30, 40), (10, 20), SCREEN).expect("valid");
        assert_eq!((r.x, r.y, r.w, r.h), (10, 20, 21, 21));
    }

    #[test]
    fn degenerate_drags_are_cancelled() {
        assert_eq!(confirm((5, 5), (5, 5), SCREEN), None);
        assert_eq!(confirm((5, 5), (5, 200), SCREEN), None);
        assert_eq!(confirm((5, 5), (6, 200), SCREEN), None);
        assert_eq!(
            confirm((0, 0), (2, 2), SCREEN).map(|r| (r.w, r.h)),
            Some((3, 3))
        );
    }

    #[test]
    fn drag_past_the_output_is_clamped() {
        let r = confirm((-20, -5), (500, 500), SCREEN).expect("valid");
        assert_eq!((r.x, r.y, r.w, r.h), (0, 0, 100, 50));
    }

    #[test]
    fn nudge_steps_and_stops_at_edges() {
        assert_eq!(nudge((10, 10), 1, -1, SCREEN), (11, 9));
        assert_eq!(nudge((99, 49), 5, 5, SCREEN), (99, 49));
        assert_eq!(nudge((0, 0), -1, -1, SCREEN), (0, 0));
    }

    #[test]
    fn empty_bounds_collapse_to_origin() {
        let empty = Rect {
            x: 10,
            y: 10,
            w: 0,
            h: 0,
        };
        assert_eq!(clamp_point((5, 500), empty), (10, 10));
    }
}

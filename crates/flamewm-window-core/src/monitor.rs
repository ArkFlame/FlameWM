//! Pure monitor-to-monitor placement policy, shared by the X11 and Wayland backends.
//!
//! A backend supplies the source and target work areas; this module only computes geometry.
//! The rule follows mango's `tagmon`: keep the window where it sits relative to the monitor it
//! came from, then make sure it lands fully inside the target work area.

use flamewm_api::Rect;

/// Moves `rect` from `source` into `target`, preserving its offset from the work-area origin and
/// its size, then clamps the result fully inside `target`.
///
/// A degenerate source (zero width or height) has no meaningful relative position, so the rect is
/// simply fitted into `target`. Oversized rects are capped to the target work area by [`fit_into`].
#[must_use]
pub fn relocate_to_work_area(rect: Rect, source: Rect, target: Rect) -> Rect {
    if source.width <= 0 || source.height <= 0 {
        return crate::fit_into(rect, target, 1);
    }
    let moved = Rect::new(
        target.x.saturating_add(rect.x.saturating_sub(source.x)),
        target.y.saturating_add(rect.y.saturating_sub(source.y)),
        rect.width,
        rect.height,
    );
    crate::fit_into(moved, target, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(x: i32, y: i32, width: i32, height: i32) -> Rect {
        Rect::new(x, y, width, height)
    }

    #[test]
    fn keeps_relative_offset_and_size_when_target_is_shifted() {
        let source = area(0, 0, 1280, 800);
        let target = area(1280, 0, 1280, 800);
        let rect = area(100, 50, 400, 300);
        assert_eq!(
            relocate_to_work_area(rect, source, target),
            area(1380, 50, 400, 300)
        );
    }

    #[test]
    fn clamps_fully_inside_a_smaller_target() {
        let source = area(0, 0, 1280, 800);
        let target = area(1280, 100, 800, 600);
        let rect = area(1100, 700, 300, 200);
        // Offset (1100, 700) lands at (2380, 800); fit_into slides it back to (1780, 500).
        assert_eq!(
            relocate_to_work_area(rect, source, target),
            area(1780, 500, 300, 200)
        );
    }

    #[test]
    fn negative_origins_preserve_the_offset() {
        let source = area(-1920, 0, 1920, 1080);
        let target = area(0, 0, 1280, 800);
        let rect = area(-1900, 20, 300, 200);
        assert_eq!(
            relocate_to_work_area(rect, source, target),
            area(20, 20, 300, 200)
        );
    }

    #[test]
    fn oversized_rect_is_capped_to_the_target_work_area() {
        let source = area(0, 0, 800, 600);
        let target = area(800, 0, 800, 600);
        assert_eq!(
            relocate_to_work_area(area(0, 0, 1000, 900), source, target),
            area(800, 0, 800, 600)
        );
    }

    #[test]
    fn degenerate_source_falls_back_to_clamping() {
        assert_eq!(
            relocate_to_work_area(
                area(50, 60, 100, 100),
                area(0, 0, 0, 0),
                area(1000, 0, 500, 400)
            ),
            area(1000, 60, 100, 100)
        );
    }
}

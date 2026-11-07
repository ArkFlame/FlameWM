//! Centring of a transient window over its `WM_TRANSIENT_FOR` parent, clamped to the work area.

use crate::frame::coords::RootRect;

/// Frame of `w` x `h` centred over `parent`, kept inside `work`. A frame larger than the work
/// area anchors to its top-left edge so its controls stay reachable.
pub(super) fn centre_over_parent(parent: RootRect, w: i32, h: i32, work: RootRect) -> RootRect {
    let x = parent.x.saturating_add((parent.w - w) / 2);
    let y = parent.y.saturating_add((parent.h - h) / 2);
    let x = x.clamp(work.x, work.x.saturating_add((work.w - w).max(0)));
    let y = y.clamp(work.y, work.y.saturating_add((work.h - h).max(0)));
    RootRect::new(x, y, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: RootRect = RootRect::new(0, 40, 1600, 860);

    #[test]
    fn dialog_is_centred_over_parent() {
        let parent = RootRect::new(400, 300, 400, 300);
        assert_eq!(
            centre_over_parent(parent, 200, 100, WORK),
            RootRect::new(500, 400, 200, 100)
        );
    }

    #[test]
    fn dialog_near_edge_is_clamped_into_work_area() {
        let parent = RootRect::new(0, 0, 100, 100);
        // Centred would be (-50, -25): clamped to the work-area origin.
        assert_eq!(
            centre_over_parent(parent, 200, 150, WORK),
            RootRect::new(0, 40, 200, 150)
        );
        let corner = RootRect::new(1500, 800, 100, 100);
        assert_eq!(
            centre_over_parent(corner, 300, 200, WORK),
            RootRect::new(1300, 700, 300, 200)
        );
    }

    #[test]
    fn oversize_dialog_anchors_to_work_area_origin() {
        let parent = RootRect::new(100, 100, 50, 50);
        assert_eq!(
            centre_over_parent(parent, 2000, 1000, WORK),
            RootRect::new(0, 40, 2000, 1000)
        );
    }
}

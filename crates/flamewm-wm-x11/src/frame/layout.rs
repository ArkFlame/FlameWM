//! Frame child layout: pure partition of the frame outer into 12 children.
//!
//! Pure value objects. No X calls, no x11rb.
//!
//! J06 freeze: titlebar 31, buttons 3x38x31, controls_width 114,
//! resize thickness 6, min 160x96.
//!
//! Children: 8 resize zones + 1 title-drag body + 3 control buttons.
//! Non-overlap partition (half-open rects):
//! corners 6x6 at outer extremes; top/bottom strips exclude corners
//! (top additionally excludes the controls band); left/right strips start
//! at `titlebar_h` and exclude corners; title-drag is the titlebar body
//! below the top-6px strip excluding control rects; controls follow the
//! skin formula (`x = W - cw + i*38`, `w = 38`, `h = titlebar_h`) clipped
//! below the top strip when touching it.

use super::coords::FrameRect;
#[cfg(test)]
use super::model::ResizeEdges;

/// Frozen defaults (J06).
pub const TITLEBAR_H: i32 = 31;
/// Three buttons of 38px.
pub const CONTROLS_WIDTH: i32 = 114;
/// Single button width.
pub const BUTTON_WIDTH: i32 = 38;
/// Resize hit thickness.
pub const RESIZE_T: i32 = 6;

/// Twelve frame children: 8 resize + 1 title-drag + 3 controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameChildLayout {
    pub top_left: FrameRect,
    pub top_right: FrameRect,
    pub bottom_left: FrameRect,
    pub bottom_right: FrameRect,
    pub top: FrameRect,
    pub bottom: FrameRect,
    pub left: FrameRect,
    pub right: FrameRect,
    pub title_drag: FrameRect,
    /// Left-to-right: minimize, maximize/restore, close.
    pub controls: [FrameRect; 3],
}

impl FrameChildLayout {
    /// Resize zones paired with their edges, corner-first order.
    #[cfg(test)]
    #[must_use]
    pub const fn resize_pairs() -> [ResizeEdges; 8] {
        crate::frame::resources::RESIZE_ORDER
    }

    /// Resize rects in the same corner-first order as [`Self::resize_pairs`].
    #[must_use]
    pub fn resize_rects(self) -> [FrameRect; 8] {
        [
            self.top_left,
            self.top_right,
            self.bottom_left,
            self.bottom_right,
            self.top,
            self.bottom,
            self.left,
            self.right,
        ]
    }
}

/// Pure partition of the frame outer (frame-local origin) into 12 children.
#[must_use]
pub fn layout_frame_children(
    outer_w: i32,
    outer_h: i32,
    titlebar_h: i32,
    controls_width: i32,
    resize_t: i32,
) -> FrameChildLayout {
    let w = outer_w.max(0);
    let h = outer_h.max(0);
    let t = resize_t.max(1);
    let tb = titlebar_h.max(t + 1);
    let cw = controls_width.clamp(0, (w - 2 * t).max(0));
    let bw = if cw == controls_width {
        BUTTON_WIDTH
    } else {
        (cw / 3).max(1)
    };

    let strip_w = (w - 2 * t).max(0);
    let mid_w = (strip_w - cw).max(0);
    let side_h = (h - tb - t).max(0);

    // Controls from the skin formula, then clipped below the top strip
    // when touching it (control top lives at y=0, strip owns y=0..t).
    let controls = std::array::from_fn(|i| {
        let raw = FrameRect::new(w - cw + bw * i as i32, 0, bw, tb);
        if raw.y < t {
            FrameRect::new(raw.x, t, raw.w, (raw.h - (t - raw.y)).max(0))
        } else {
            raw
        }
    });

    FrameChildLayout {
        top_left: FrameRect::new(0, 0, t, t),
        top_right: FrameRect::new(w - t, 0, t, t),
        bottom_left: FrameRect::new(0, h - t, t, t),
        bottom_right: FrameRect::new(w - t, h - t, t, t),
        top: FrameRect::new(t, 0, mid_w, t),
        bottom: FrameRect::new(t, h - t, strip_w, t),
        left: FrameRect::new(0, tb, t, side_h),
        right: FrameRect::new(w - t, tb, t, side_h),
        title_drag: FrameRect::new(t, t, mid_w, (tb - t).max(0)),
        controls,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::coords::{RootPoint, RootRect};

    const W: i32 = 400;
    const H: i32 = 300;

    fn layout() -> FrameChildLayout {
        layout_frame_children(W, H, TITLEBAR_H, CONTROLS_WIDTH, RESIZE_T)
    }

    fn overlaps(a: FrameRect, b: FrameRect) -> bool {
        a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
    }

    /// All twelve children: 8 resize, title-drag, 3 controls.
    fn all_children(l: FrameChildLayout) -> Vec<FrameRect> {
        let mut all = l.resize_rects().to_vec();
        all.push(l.title_drag);
        all.extend(l.controls);
        all
    }

    #[test]
    fn j06_zone_pins_400x300() {
        let l = layout();
        assert_eq!(l.top_left, FrameRect::new(0, 0, 6, 6));
        assert_eq!(l.top_right, FrameRect::new(394, 0, 6, 6));
        assert_eq!(l.bottom_left, FrameRect::new(0, 294, 6, 6));
        assert_eq!(l.bottom_right, FrameRect::new(394, 294, 6, 6));
        assert_eq!(l.top, FrameRect::new(6, 0, 274, 6));
        assert_eq!(l.bottom, FrameRect::new(6, 294, 388, 6));
        assert_eq!(l.left, FrameRect::new(0, 31, 6, 263));
        assert_eq!(l.right, FrameRect::new(394, 31, 6, 263));
        assert_eq!(l.title_drag, FrameRect::new(6, 6, 274, 25));
        assert_eq!(
            l.controls,
            [
                FrameRect::new(286, 6, 38, 25),
                FrameRect::new(324, 6, 38, 25),
                FrameRect::new(362, 6, 38, 25),
            ]
        );
    }

    #[test]
    fn eight_resize_zones_unique() {
        let l = layout();
        let rects = l.resize_rects();
        let mut seen = std::collections::HashSet::new();
        for rect in rects {
            assert!(rect.w > 0 && rect.h > 0, "{rect:?}");
            assert!(
                seen.insert((rect.x, rect.y, rect.w, rect.h)),
                "{rect:?} dup"
            );
        }
        assert_eq!(seen.len(), 8);
        // Edges pair one-to-one in corner-first order.
        let edges = FrameChildLayout::resize_pairs();
        assert_eq!(
            edges,
            [
                ResizeEdges::top_left(),
                ResizeEdges::top_right(),
                ResizeEdges::bottom_left(),
                ResizeEdges::bottom_right(),
                ResizeEdges::top(),
                ResizeEdges::bottom(),
                ResizeEdges::left(),
                ResizeEdges::right(),
            ]
        );
    }

    #[test]
    fn twelve_children_nonoverlapping() {
        let all = all_children(layout());
        assert_eq!(all.len(), 12);
        for i in 0..all.len() {
            assert!(all[i].w > 0 && all[i].h > 0, "{:?}", all[i]);
            for j in (i + 1)..all.len() {
                assert!(!overlaps(all[i], all[j]), "{:?} vs {:?}", all[i], all[j]);
            }
        }
    }

    #[test]
    fn title_body_excludes_controls_and_top_resize() {
        let l = layout();
        // Below the top-6px strip, above the titlebar bottom.
        assert!(l.title_drag.y >= RESIZE_T);
        assert_eq!(l.title_drag.y + l.title_drag.h, TITLEBAR_H);
        // Excludes control rects (ends at or before the controls band).
        assert!(l.title_drag.x + l.title_drag.w <= W - CONTROLS_WIDTH);
        assert!(l.title_drag.x + l.title_drag.w <= l.controls[0].x);
        for control in l.controls {
            assert!(!overlaps(l.title_drag, control));
            // Clipped below the top strip.
            assert!(control.y >= RESIZE_T);
            assert!(!overlaps(l.top, control));
        }
        assert!(!overlaps(l.title_drag, l.top));
    }

    #[test]
    fn controls_tile_and_nonoverlap() {
        let l = layout();
        assert_eq!(l.controls[0].w, 38);
        assert_eq!(l.controls[2].x + l.controls[2].w, W);
        assert_eq!(l.controls[0].x + 38, l.controls[1].x);
        assert_eq!(l.controls[1].x + 38, l.controls[2].x);
        assert!(!overlaps(l.controls[0], l.controls[1]));
        assert!(!overlaps(l.controls[1], l.controls[2]));
    }

    #[test]
    fn root_origin_does_not_change_local_layout() {
        // Layout is frame-local; crossing the (500,200) root origin and back
        // is the identity on every child rect.
        let origin = RootPoint::new(500, 200);
        for rect in all_children(layout()) {
            let root: RootRect = rect.to_root(origin);
            assert_eq!(root.x, rect.x + 500);
            assert_eq!(root.y, rect.y + 200);
            assert_eq!(root.to_frame(origin), rect);
        }
    }
}

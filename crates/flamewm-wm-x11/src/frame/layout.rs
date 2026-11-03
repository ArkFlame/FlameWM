//! Frame child layout: pure partition of the frame outer into 12 children.
//!
//! Pure value objects. No X calls, no x11rb.
//!
//! J06 freeze: titlebar 31, buttons 3x38x31, `controls_width` 114,
//! resize thickness 6, min 160x96.
//!
//! Children: 8 resize zones + 1 title-drag body + 3 control buttons.
//! Partition of the frame chrome (titlebar band + the `resize_t` border; the client owns the
//! interior):
//! corner grips are `grip x grip` at the outer extremes, sized by
//! [`corner_grip_for_titlebar`] so the arc is grabbable through the frame's rounded bounding
//! shape; top/bottom strips exclude the grips; left/right strips run from the grip down to the
//! opposite grip; title-drag is the titlebar body below the top strip, between the left grip and
//! the controls band; controls follow the skin formula (`x = W - cw + i*38`, `w = 38`,
//! `h = titlebar_h`) clipped below the top strip when touching it.
//!
//! The top corner grips deliberately overlap the title-drag body and the control column: the wm
//! raises the 8 resize children above the other input children, so the corner wins inside its
//! square (Wayland resolves the same overlap by checking the corner first).

use super::coords::FrameRect;
#[cfg(test)]
use super::model::ResizeEdges;

/// Frozen defaults (J06).
pub const TITLEBAR_H: i32 = 31;
/// Three buttons of 38px.
#[cfg(test)]
pub const CONTROLS_WIDTH: i32 = 114;
/// Single button width.
pub const BUTTON_WIDTH: i32 = 38;
/// Resize hit thickness.
pub const RESIZE_T: i32 = 6;
/// Corner grip side in logical pixels: `flamewm-wm-wayland`'s unclipped `CORNER_GRIP` square.
const CORNER_GRIP: i32 = 16;

/// Convert authoritative logical control width to the physical titlebar size.
/// The titlebar height is supplied by the frame-extents authority, so hit,
/// glyph, fill and input children derive their scale from the same metric.
#[must_use]
pub fn button_width_for_titlebar(titlebar_h: i32) -> i32 {
    if titlebar_h <= 0 {
        return 0;
    }
    (i64::from(BUTTON_WIDTH) * i64::from(titlebar_h.max(1)) + i64::from(TITLEBAR_H / 2))
        .checked_div(i64::from(TITLEBAR_H))
        .unwrap_or(1)
        .clamp(1, i64::from(i32::MAX)) as i32
}

/// Keep the invisible resize targets proportional to the same titlebar scale.
/// A hidden titlebar (0) keeps the base thickness so edges stay grabbable.
#[must_use]
pub fn resize_thickness_for_titlebar(titlebar_h: i32) -> i32 {
    if titlebar_h <= 0 {
        return RESIZE_T;
    }
    (RESIZE_T * titlebar_h.max(1) + TITLEBAR_H / 2) / TITLEBAR_H
}

#[must_use]
pub fn controls_width_for_titlebar(titlebar_h: i32) -> i32 {
    button_width_for_titlebar(titlebar_h).saturating_mul(3)
}

/// Corner resize grip side for the physical titlebar height.
///
/// Measured on Xvfb with XTest: a parent's Shape Bounding region clips a child's input region, so
/// an `InputOnly` grip only receives presses where the rounded mask still covers the pixel. A
/// 16 px square at a radius-24 corner still covers the visible arc triangle (~133 px² of grab area
/// over rows 2..15), so it feels like Wayland's unclipped 16 px `CORNER_GRIP` square.
///
/// 16 is also the largest size that keeps the Close button usable: its glyph is 16 px centred in
/// the 38 px button, so its centre sits at `w - 19`, which a 24 px grip (`w - 24 .. w`) would
/// swallow. Only the logical 16 is scaled, never the radius.
#[must_use]
pub fn corner_grip_for_titlebar(titlebar_h: i32) -> i32 {
    if titlebar_h <= 0 {
        return CORNER_GRIP;
    }
    let scaled = (CORNER_GRIP * titlebar_h.max(1) + TITLEBAR_H / 2) / TITLEBAR_H;
    scaled.max(8)
}

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
    // A hidden titlebar is a real zero band: no title-drag or control row, and the
    // side strips start at the frame top instead of below a phantom titlebar.
    let tb = if titlebar_h <= 0 {
        0
    } else {
        titlebar_h.max(t + 1)
    };
    let cw = controls_width.clamp(0, (w - 2 * t).max(0));
    // Divide the clipped band by thirds without dropping remainder pixels:
    // even the narrowest frame must keep the last Close hit cell flush right.
    let cell_edge = |index: i32| cw.saturating_mul(index) / 3;

    // Corner grips cover the decoration arc; tiny frames keep half the frame.
    let grip = corner_grip_for_titlebar(titlebar_h).min(w / 2).min(h / 2);
    let strip_w = (w - 2 * grip).max(0);
    let drag_w = (w - cw - t).max(0);
    let side_h = (h - 2 * grip).max(0);

    // Controls from the skin formula, then clipped below the top strip
    // when touching it (control top lives at y=0, strip owns y=0..t).
    let controls = std::array::from_fn(|i| {
        let begin = cell_edge(i as i32);
        let end = cell_edge(i as i32 + 1);
        let raw = FrameRect::new(w - cw + begin, 0, end - begin, tb);
        if raw.y < t {
            FrameRect::new(raw.x, t, raw.w, (raw.h - (t - raw.y)).max(0))
        } else {
            raw
        }
    });

    FrameChildLayout {
        top_left: FrameRect::new(0, 0, grip, grip),
        top_right: FrameRect::new(w - grip, 0, grip, grip),
        bottom_left: FrameRect::new(0, h - grip, grip, grip),
        bottom_right: FrameRect::new(w - grip, h - grip, grip, grip),
        top: FrameRect::new(grip, 0, strip_w, t),
        bottom: FrameRect::new(grip, h - t, strip_w, t),
        left: FrameRect::new(0, grip, t, side_h),
        right: FrameRect::new(w - t, grip, t, side_h),
        title_drag: FrameRect::new(t, t, drag_w, (tb - t).max(0)),
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
        // 31 px titlebar: the scaled logical 16 px square, never the 24 px skin radius.
        assert_eq!(corner_grip_for_titlebar(TITLEBAR_H), 16);
        assert_eq!(l.top_left, FrameRect::new(0, 0, 16, 16));
        assert_eq!(l.top_right, FrameRect::new(384, 0, 16, 16));
        assert_eq!(l.bottom_left, FrameRect::new(0, 284, 16, 16));
        assert_eq!(l.bottom_right, FrameRect::new(384, 284, 16, 16));
        assert_eq!(l.top, FrameRect::new(16, 0, 368, 6));
        assert_eq!(l.bottom, FrameRect::new(16, 294, 368, 6));
        assert_eq!(l.left, FrameRect::new(0, 16, 6, 268));
        assert_eq!(l.right, FrameRect::new(394, 16, 6, 268));
        assert_eq!(l.title_drag, FrameRect::new(6, 6, 280, 25));
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
    fn sixteen_px_grip_keeps_the_arc_grabbable_and_the_close_glyph_clickable() {
        // The parent's rounded mask clips the child's input: only the arc triangle is clickable.
        let (w, h, radius) = (W as u32, H as u32, 24_u32);
        let grip = layout().top_right;
        let clickable: i32 = crate::frame::geometry_shape::rounded_mask_spans(w, h, radius)
            .into_iter()
            .filter(|(row, _, _)| {
                let y = *row as i32;
                y >= grip.y && y < grip.y + grip.h
            })
            .map(|(_, start, end)| {
                ((end as i32).min(grip.x + grip.w) - (start as i32).max(grip.x)).max(0)
            })
            .sum();
        // Measured 133 px² over rows 2..15: still a full corner handle inside radius 24.
        assert_eq!(clickable, 133);
        // Close glyph centre of the 38 px button stays outside the grip, so it closes.
        assert!(W - 19 < grip.x, "grip must not swallow the Close glyph");
    }

    #[test]
    fn top_resize_strip_covers_every_pixel_above_controls() {
        for (w, height) in [(160, 31), (400, 31), (400, 42), (800, 62)] {
            let t = resize_thickness_for_titlebar(height);
            let layout =
                layout_frame_children(w, 300, height, controls_width_for_titlebar(height), t);
            for x in 0..w {
                for y in 0..t {
                    let count = [layout.top_left, layout.top_right, layout.top]
                        .into_iter()
                        .filter(|r| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h)
                        .count();
                    assert_eq!(count, 1, "x={x} y={y} w={w} height={height}");
                }
            }
        }
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

    /// Frame chrome = the titlebar band plus the `resize_t` border; the client owns the interior.
    fn in_chrome(x: i32, y: i32, w: i32, h: i32, tb: i32, t: i32) -> bool {
        y < tb || x < t || y >= h - t || x >= w - t
    }

    #[test]
    fn twelve_children_partition_the_frame_chrome() {
        for (w, h, titlebar) in [
            (400, 300, 31),
            (160, 96, 31),
            (400, 300, 42),
            (800, 600, 62),
        ] {
            let t = resize_thickness_for_titlebar(titlebar);
            let tb = if titlebar <= 0 {
                0
            } else {
                titlebar.max(t + 1)
            };
            let l = layout_frame_children(w, h, titlebar, controls_width_for_titlebar(titlebar), t);
            let all = all_children(l);
            assert_eq!(all.len(), 12);
            // (a) No dead zones: every chrome pixel belongs to at least one input child.
            for y in 0..h {
                for x in 0..w {
                    if !in_chrome(x, y, w, h, tb, t) {
                        continue;
                    }
                    let hit = all
                        .iter()
                        .any(|r| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h);
                    assert!(hit, "dead zone at {x},{y} for {w}x{h} titlebar {titlebar}");
                }
            }
            // (b) Outer overlaps only: grips may meet the drag body / control column, and only
            // inside the titlebar band. Below it the eight resize rects never double up.
            for (i, a) in all.iter().enumerate() {
                for b in all.iter().skip(i + 1) {
                    if overlaps(*a, *b) {
                        let grip = l.resize_rects().contains(a);
                        assert!(grip || l.resize_rects().contains(b), "{a:?} vs {b:?}");
                        assert!(
                            a.y.max(b.y) < tb,
                            "{a:?} vs {b:?} overlap out of the titlebar band"
                        );
                    }
                }
            }
            // (c) Every corner grip is a grip x grip square at an outer corner.
            let grip = corner_grip_for_titlebar(titlebar).min(w / 2).min(h / 2);
            for (rect, corner) in l.resize_rects().into_iter().take(4).zip([
                (0, 0),
                (w - grip, 0),
                (0, h - grip),
                (w - grip, h - grip),
            ]) {
                assert_eq!(rect, FrameRect::new(corner.0, corner.1, grip, grip));
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

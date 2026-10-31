//! Frame shape: canonical rounded-mask spans for frame windows.
//!
//! Single formula owner is render-x11 (`external_rounded_row_inset`, float
//! rounded `floor(r - dx)` at pixel-row centers): full-width rows when the
//! radius is 0/1, or mid-span; corner insets otherwise. The radius policy
//! is owned by skin (`corner_radius`). Bounding-region assembly lives here
//! over safe `x11rb` rectangles; the live mask apply lives in `render-x11`'s
//! `XShape` bridge.

use flamewm_render_x11::external_rounded_row_inset;
use flamewm_skin::recipes::decoration::{DecorationStyle, corner_radius};

use x11rb::protocol::xproto::Rectangle;

use crate::geometry::Rect;

/// Resolve decoration radius from the same titlebar metric used by the
/// managed frame/client extents. Source policy stays in flamewm-skin.
#[must_use]
pub fn scaled_corner_radius(floating: bool, style: DecorationStyle, titlebar_h: u16) -> u16 {
    const LOGICAL_TITLEBAR: u32 = flamewm_skin::recipes::window_chrome::WINDOW_CHROME
        .metrics
        .titlebar as u32;
    let logical_radius = u32::from(corner_radius(floating, style));
    let value = (logical_radius * u32::from(titlebar_h) + LOGICAL_TITLEBAR / 2) / LOGICAL_TITLEBAR;
    value.min(u32::from(u16::MAX)) as u16
}

/// Client area of a frame: it sits at `(0, titlebar)` and spans the full frame width and the
/// rest of the height. The border never inflates the client (see `FrameExtents::delta`).
#[must_use]
pub fn client_area_size(outer: Rect, titlebar_h: u16) -> (u32, u32) {
    (
        outer.width,
        outer.height.saturating_sub(u32::from(titlebar_h)),
    )
}

/// Corner rows lose one more pixel. The 1-bit XShape edge then sits inside the 1px AA outline
/// ring that chrome paints on the frame, so the ring, not the stair-stepped mask edge, is what
/// the eye sees. Straight rows (inset 0) keep their full width.
#[must_use]
pub fn corner_edge_inset(inset: u32) -> u32 {
    if inset > 0 { inset + 1 } else { 0 }
}

/// Canonical row spans (y, `x_start`, `x_end_exclusive`) for a rounded rect:
/// float rounded mask, single full-width rows when rectangular.
#[must_use]
pub fn rounded_mask_spans(width: u32, height: u32, radius: u32) -> Vec<(u32, u32, u32)> {
    let _guard = flamewm_profiler::start("wm.decoration.shape");
    if width == 0 || height == 0 {
        return Vec::new();
    }
    if radius <= 1 {
        return (0..height).map(|row| (row, 0, width)).collect();
    }
    let mut spans = Vec::with_capacity(height as usize);
    for row in 0..height {
        let inset = corner_edge_inset(external_rounded_row_inset(width, height, radius, row));
        let start = inset.min(width);
        let end = width.saturating_sub(inset).max(start);
        spans.push((row, start, end));
    }
    spans
}

/// Bounding rectangles for a frame outer: a single rect unless `floating`
/// (skin radius), otherwise the canonical rounded row spans.
#[must_use]
#[cfg(test)]
pub fn bounding_rectangles(outer: Rect, floating: bool, style: DecorationStyle) -> Vec<Rectangle> {
    bounding_rectangles_with_titlebar(
        outer,
        floating,
        style,
        flamewm_skin::recipes::window_chrome::WINDOW_CHROME
            .metrics
            .titlebar,
    )
}

#[must_use]
pub fn bounding_rectangles_with_titlebar(
    outer: Rect,
    floating: bool,
    style: DecorationStyle,
    titlebar_h: u16,
) -> Vec<Rectangle> {
    let width = outer.width.min(u32::from(u16::MAX));
    let height = outer.height.min(u32::from(u16::MAX));
    let radius = scaled_corner_radius(floating, style, titlebar_h);
    if radius == 0 {
        return vec![Rectangle {
            x: 0,
            y: 0,
            width: width as u16,
            height: height as u16,
        }];
    }
    rounded_mask_spans(width, height, u32::from(radius))
        .into_iter()
        .map(|(y, start, end)| Rectangle {
            x: i16::try_from(start.min(u32::from(u16::MAX))).unwrap_or(i16::MAX),
            y: i16::try_from(y.min(u32::from(u16::MAX))).unwrap_or(i16::MAX),
            width: end.saturating_sub(start).min(u32::from(u16::MAX)) as u16,
            height: 1,
        })
        .collect()
}

/// Client shape inside the frame's 1px outline ring. The client is inset by `ring` pixels on
/// both sides and at the bottom; the ring itself is painted on the frame there (see
/// `content_contour_blits`). Bottom corners follow the rounded frame, while the titlebar above
/// keeps its square top corners. `radius = 0` and `ring = 0` is one full rectangle.
#[must_use]
pub fn client_bottom_rounded_rectangles(
    width: u32,
    height: u32,
    radius: u32,
    ring: u32,
) -> Vec<Rectangle> {
    let rows = height.saturating_sub(ring);
    let inner_w = width.saturating_sub(ring.saturating_mul(2));
    if inner_w == 0 || rows == 0 {
        return Vec::new();
    }
    let radius = radius.min(width / 2).min(height);
    if radius == 0 {
        return vec![Rectangle {
            x: i16::try_from(ring).unwrap_or(i16::MAX),
            y: 0,
            width: inner_w.min(u32::from(u16::MAX)) as u16,
            height: rows.min(u32::from(u16::MAX)) as u16,
        }];
    }
    (0..rows)
        .map(|row| {
            let inset = if row >= height - radius {
                ring.max(corner_edge_inset(external_rounded_row_inset(
                    width, height, radius, row,
                )))
            } else {
                ring
            };
            let start = inset.min(width);
            let end = width.saturating_sub(inset).max(start);
            Rectangle {
                x: i16::try_from(start.min(u32::from(u16::MAX))).unwrap_or(i16::MAX),
                y: i16::try_from(row.min(u32::from(u16::MAX))).unwrap_or(i16::MAX),
                width: end.saturating_sub(start).min(u32::from(u16::MAX)) as u16,
                height: 1,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn covers(rects: &[Rectangle], x: i32, y: i32) -> bool {
        rects.iter().any(|r| {
            x >= i32::from(r.x)
                && x < i32::from(r.x) + i32::from(r.width)
                && y >= i32::from(r.y)
                && y < i32::from(r.y) + i32::from(r.height)
        })
    }

    #[test]
    fn client_sits_one_pixel_inside_the_outline_ring() {
        // The ring occupies the frame's outermost pixel on both sides and at the bottom; the
        // client must not cover it, or the outline disappears behind the window contents.
        let outer = Rect {
            x: 0,
            y: 0,
            width: 484,
            height: 347,
        };
        assert_eq!(client_area_size(outer, 31), (484, 316));
        let (cw, ch) = client_area_size(outer, 31);
        let full = client_bottom_rounded_rectangles(cw, ch, 0, 1);
        assert_eq!(full.len(), 1);
        assert_eq!((full[0].x, full[0].width), (1, 482));
        assert_eq!(full[0].height, 315);
        assert!(client_bottom_rounded_rectangles(cw, ch, 0, 0)[0].width == 484);
    }

    #[test]
    fn client_bottom_corners_round_while_the_top_stays_square() {
        let rects = client_bottom_rounded_rectangles(200, 120, 12, 1);
        assert_eq!(rects.len(), 119);
        assert_eq!(rects[0].x, 1);
        assert_eq!(rects[0].width, 198);
        let last = rects.last().expect("rows");
        assert!(last.x > 1, "bottom-left corner is not cut: {last:?}");
        assert!(last.width < 198, "bottom-right corner is not cut");
        let squared = client_bottom_rounded_rectangles(200, 120, 0, 1);
        assert_eq!(squared.len(), 1);
        assert_eq!((squared[0].x, squared[0].width), (1, 198));
    }

    #[test]
    fn corner_radius_matches_physical_titlebar_scale() {
        for (height, expected) in [(31, 24), (42, 33), (62, 48)] {
            assert_eq!(
                scaled_corner_radius(true, DecorationStyle::Rounded, height),
                expected
            );
            assert_eq!(
                scaled_corner_radius(true, DecorationStyle::Squared, height),
                0
            );
            assert_eq!(
                scaled_corner_radius(false, DecorationStyle::Rounded, height),
                0
            );
            let rects = bounding_rectangles_with_titlebar(
                Rect {
                    x: -400,
                    y: 200,
                    width: 500,
                    height: 300,
                },
                true,
                DecorationStyle::Rounded,
                height,
            );
            assert!(!rects.is_empty());
            assert!(!covers(&rects, 499, 0));
        }
    }

    #[test]
    fn rounded_mask_is_mirror_symmetric_left_to_right() {
        // mask(x, y) == mask(W-1-x, y): the right corners must cut exactly like the left ones.
        for (w, h, r) in [(120_u32, 80_u32, 24_u32), (121, 81, 24), (97, 53, 7)] {
            let spans = rounded_mask_spans(w, h, r);
            for (row, start, end) in spans {
                assert_eq!(
                    w - end,
                    start,
                    "row {row} of {w}x{h} r={r}: right inset {} != left inset {start}",
                    w - end
                );
            }
        }
    }

    #[test]
    fn rounded_floating_shape_has_no_corner_hit_leak() {
        let outer = Rect {
            x: 0,
            y: 0,
            width: 400,
            height: 300,
        };
        let rects = bounding_rectangles(outer, true, DecorationStyle::Rounded);
        for (x, y) in [(0, 0), (399, 0), (0, 299), (399, 299)] {
            assert!(!covers(&rects, x, y), "corner ({x},{y}) must be clipped");
        }
        assert!(covers(&rects, 200, 0), "top edge middle stays");
        assert!(covers(&rects, 0, 150), "left edge middle stays");
    }

    #[test]
    fn squared_and_unfloating_shapes_are_full_rectangles() {
        let outer = Rect {
            x: 0,
            y: 0,
            width: 400,
            height: 300,
        };
        for (floating, style) in [
            (true, DecorationStyle::Squared),
            (false, DecorationStyle::Rounded),
            (true, DecorationStyle::Squared),
        ] {
            let rects = bounding_rectangles(outer, floating, style);
            assert_eq!(rects.len(), 1);
            assert!(covers(&rects, 0, 0) && covers(&rects, 399, 299));
        }
    }

    #[test]
    fn corner_rows_inset_one_extra_pixel_and_straight_rows_do_not() {
        assert_eq!(corner_edge_inset(0), 0);
        assert_eq!(corner_edge_inset(1), 2);
        assert_eq!(corner_edge_inset(5), 6);
        let spans = rounded_mask_spans(100, 60, 12);
        let (_, start_mid, end_mid) = spans[30];
        assert_eq!(
            (start_mid, end_mid),
            (0, 100),
            "straight edge must keep full width"
        );
        let (_, start_top, _) = spans[0];
        assert!(start_top > external_rounded_row_inset(100, 60, 12, 0));
    }

    #[test]
    fn t26_shape_spans_match_canonical_mask() {
        assert_eq!(
            rounded_mask_spans(8, 8, 4),
            vec![
                (0, 3, 5),
                (1, 0, 8),
                (2, 0, 8),
                (3, 0, 8),
                (4, 0, 8),
                (5, 0, 8),
                (6, 0, 8),
                (7, 3, 5),
            ]
        );
        assert_eq!(rounded_mask_spans(4, 2, 0), vec![(0, 0, 4), (1, 0, 4)]);
        assert!(rounded_mask_spans(0, 8, 4).is_empty());
    }
}

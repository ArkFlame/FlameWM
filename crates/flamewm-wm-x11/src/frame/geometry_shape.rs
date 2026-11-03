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

/// Corner rows lose one more pixel. The 1-bit `XShape` edge then sits inside the 1px AA outline
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
    rounded_mask_spans_with_inset(width, height, radius, corner_edge_inset)
}

/// Exact row spans for a rounded rect: the float rounded mask without the extra corner pixel
/// [`corner_edge_inset`] adds for the frame-band fallback. The ARGB overlay path shapes the client
/// with these, so the antialiased ring blends over the client's own corner pixels instead of the
/// frame's dark background.
#[must_use]
pub fn rounded_mask_spans_exact(width: u32, height: u32, radius: u32) -> Vec<(u32, u32, u32)> {
    rounded_mask_spans_with_inset(width, height, radius, std::convert::identity)
}

fn rounded_mask_spans_with_inset(
    width: u32,
    height: u32,
    radius: u32,
    inset_policy: fn(u32) -> u32,
) -> Vec<(u32, u32, u32)> {
    let _guard = flamewm_profiler::start("wm.decoration.shape");
    if width == 0 || height == 0 {
        return Vec::new();
    }
    if radius <= 1 {
        return (0..height).map(|row| (row, 0, width)).collect();
    }
    let mut spans = Vec::with_capacity(height as usize);
    for row in 0..height {
        let inset = inset_policy(external_rounded_row_inset(width, height, radius, row));
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
        false,
    )
}

/// Frame bounding/clip rectangles. `ring_inset` mirrors [`client_rounded_rectangles`]: `false`
/// keeps the frame-band fallback's extra corner pixel, `true` uses [`rounded_mask_spans_exact`] so
/// an ARGB overlay child (and the exact-arc client) is clipped to the same arc the ring covers.
#[must_use]
pub fn bounding_rectangles_with_titlebar(
    outer: Rect,
    floating: bool,
    style: DecorationStyle,
    titlebar_h: u16,
    ring_inset: bool,
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
    let spans = if ring_inset {
        rounded_mask_spans_exact(width, height, u32::from(radius))
    } else {
        rounded_mask_spans(width, height, u32::from(radius))
    };
    spans
        .into_iter()
        .map(|(y, start, end)| Rectangle {
            x: i16::try_from(start.min(u32::from(u16::MAX))).unwrap_or(i16::MAX),
            y: i16::try_from(y.min(u32::from(u16::MAX))).unwrap_or(i16::MAX),
            width: end.saturating_sub(start).min(u32::from(u16::MAX)) as u16,
            height: 1,
        })
        .collect()
}

/// Client shape inside the frame's 1px outline ring, sampled from the frame's own mask.
///
/// The client is the square child that actually shows pixels at the bottom edge, so its mask has to
/// follow the same arc rows the frame uses; deriving the arc from the client's own height samples
/// the curve one row too high and leaves the bottom-left/bottom-right corner visibly square (or a
/// pixel wide) next to the antialiased top corners. Rows are taken from the frame's spans for the
/// *frame* and mapped into client coordinates by subtracting the titlebar band, so nothing can
/// drift between the two masks.
///
/// `titlebar_h == 0` means the client covers the whole frame, so it also carries the top arc. On a
/// square frame the result is one full rectangle inset by `ring`, or the full client rect when
/// `ring == 0`.
///
/// `ring_inset` picks the frame's span owner: `false` uses [`rounded_mask_spans`] (the frame-band
/// fallback keeps its extra corner pixel), `true` uses [`rounded_mask_spans_exact`] (the ARGB
/// overlay draws the ring over the client, so the client starts on the exact arc). `ring` still
/// reserves the outer pixel in both modes.
#[must_use]
pub fn client_rounded_rectangles(
    frame_width: u32,
    frame_height: u32,
    titlebar_h: u16,
    radius: u32,
    ring: u32,
    ring_inset: bool,
) -> Vec<Rectangle> {
    let titlebar = u32::from(titlebar_h).min(frame_height);
    let rows = frame_height.saturating_sub(titlebar);
    let inner_w = frame_width.saturating_sub(ring.saturating_mul(2));
    if inner_w == 0 || rows == 0 {
        return Vec::new();
    }
    let radius = radius.min(frame_width / 2).min(frame_height);
    if radius == 0 {
        let first = titlebar;
        let last = frame_height.saturating_sub(ring);
        if last <= first {
            return Vec::new();
        }
        return vec![Rectangle {
            x: i16::try_from(ring).unwrap_or(i16::MAX),
            y: 0,
            width: inner_w.min(u32::from(u16::MAX)) as u16,
            height: (last - first).min(u32::from(u16::MAX)) as u16,
        }];
    }
    // The ring occupies the frame's outermost pixel. The frame-band fallback paints it, so the
    // client never covers it; the ARGB overlay instead composites over the client's own pixels.
    let last = frame_height.saturating_sub(ring).max(titlebar);
    let spans = if ring_inset {
        rounded_mask_spans_exact(frame_width, frame_height, radius)
    } else {
        rounded_mask_spans(frame_width, frame_height, radius)
    };
    spans
        .into_iter()
        .filter(|(row, _, _)| *row >= titlebar && *row < last)
        .map(|(row, start, end)| {
            let inset = start.max(ring);
            let end = end.saturating_sub(ring).max(inset);
            Rectangle {
                x: i16::try_from(inset.min(u32::from(u16::MAX))).unwrap_or(i16::MAX),
                y: i16::try_from((row - titlebar).min(u32::from(u16::MAX))).unwrap_or(i16::MAX),
                width: end.saturating_sub(inset).min(u32::from(u16::MAX)) as u16,
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
        let full = client_rounded_rectangles(outer.width, outer.height, 31, 0, 1, false);
        assert_eq!(full.len(), 1);
        assert_eq!((full[0].x, full[0].width), (1, 482));
        assert_eq!(full[0].height, 315);
        assert_eq!(
            client_rounded_rectangles(outer.width, outer.height, 31, 0, 0, false)[0].width,
            484
        );
    }

    /// The client mask must be the frame mask shifted by the titlebar band: any drift here is
    /// exactly what makes the bottom corners look square while the top corners are rounded.
    #[test]
    fn client_mask_rows_match_the_frame_mask_rows() {
        let (frame_w, frame_h, titlebar, radius, ring) = (200_u32, 151_u32, 31_u16, 12_u32, 1_u32);
        let frame = rounded_mask_spans(frame_w, frame_h, radius);
        let client = client_rounded_rectangles(frame_w, frame_h, titlebar, radius, ring, false);
        assert_eq!(
            client.len(),
            usize::try_from(frame_h - u32::from(titlebar) - ring).unwrap()
        );
        for (index, rect) in client.iter().enumerate() {
            let (_, frame_start, frame_end) =
                frame[usize::try_from(u32::from(titlebar)).unwrap() + index];
            assert_eq!(
                i32::from(rect.y),
                index as i32,
                "client rows stay contiguous"
            );
            assert_eq!(
                i32::from(rect.x),
                i32::try_from(frame_start.max(ring)).unwrap(),
                "row {index}: left inset must follow the frame arc"
            );
            assert_eq!(
                i32::from(rect.x) + i32::from(rect.width),
                i32::try_from(frame_end.saturating_sub(ring)).unwrap(),
                "row {index}: right inset must follow the frame arc"
            );
        }
        let last = client.last().expect("client rows");
        assert!(last.x > 1, "bottom-left corner is not cut: {last:?}");
        assert!(last.width < 198, "bottom-right corner is not cut");
    }

    /// Without a titlebar band the client covers the frame, so it carries both arcs.
    #[test]
    fn titlebarless_client_rounds_all_four_corners() {
        let rects = client_rounded_rectangles(200, 120, 0, 12, 1, false);
        assert_eq!(rects.len(), 119);
        for rect in [rects[0], *rects.last().expect("rows")] {
            assert!(
                rect.x > 1 && rect.width < 198,
                "both top and bottom corners must be cut: {rect:?}"
            );
        }
        let middle = rects[60];
        assert_eq!(
            (middle.x, middle.width),
            (1, 198),
            "the straight middle rows keep only the ring inset"
        );
        let squared = client_rounded_rectangles(200, 120, 0, 0, 1, false);
        assert_eq!(squared.len(), 1);
        assert_eq!((squared[0].x, squared[0].width), (1, 198));
    }

    /// The exact spans are the float mask alone: no `corner_edge_inset` extra pixel anywhere.
    #[test]
    fn exact_spans_are_the_float_mask_without_the_corner_edge_inset() {
        let (w, h, r) = (100u32, 60u32, 12u32);
        let exact = rounded_mask_spans_exact(w, h, r);
        let band = rounded_mask_spans(w, h, r);
        assert_eq!(exact.len(), usize::try_from(h).unwrap());
        for row in 0..h {
            let start = external_rounded_row_inset(w, h, r, row);
            let index = usize::try_from(row).unwrap();
            let (_, exact_start, exact_end) = exact[index];
            assert_eq!((exact_start, exact_end), (start, w - start), "row {row}");
            let (_, band_start, band_end) = band[index];
            assert_eq!(band_start, corner_edge_inset(start), "row {row}");
            assert_eq!(band_end, w - corner_edge_inset(start), "row {row}");
        }
        // Straight rows are identical; only corner rows carry the extra pixel.
        assert_eq!(exact[30], band[30]);
        assert_ne!(exact[0], band[0]);
        assert_eq!(
            rounded_mask_spans_exact(4, 2, 0),
            vec![(0, 0, 4), (1, 0, 4)]
        );
        assert!(rounded_mask_spans_exact(0, 8, 4).is_empty());
    }

    /// The frame-band fallback keeps the existing +1 corner rows: `ring_inset == false` is the
    /// pre-overlay geometry and must not drift.
    #[test]
    fn frame_band_client_keeps_the_extra_corner_pixel() {
        let (fw, fh, titlebar, radius, ring) = (200u32, 151u32, 31u16, 12u32, 1u32);
        let band = rounded_mask_spans(fw, fh, radius);
        let client = client_rounded_rectangles(fw, fh, titlebar, radius, ring, false);
        for (index, rect) in client.iter().enumerate() {
            let (_, start, end) = band[usize::try_from(u32::from(titlebar)).unwrap() + index];
            assert_eq!(
                i32::from(rect.x),
                i32::try_from(start.max(ring)).unwrap(),
                "row {index}: fallback keeps the frame-band inset"
            );
            assert_eq!(
                i32::from(rect.x) + i32::from(rect.width),
                i32::try_from(end.saturating_sub(ring)).unwrap(),
                "row {index}"
            );
        }
    }

    /// The overlay path shapes the client to the exact arc while still reserving `ring` pixels.
    /// Corner rows therefore start one pixel earlier than the frame-band fallback, so the ring's
    /// antialiased coverage lands on client pixels rather than the frame background.
    #[test]
    fn overlay_client_rows_start_at_the_exact_arc_and_keep_the_outer_pixel() {
        let (fw, fh, titlebar, radius, ring) = (200u32, 151u32, 31u16, 12u32, 1u32);
        let exact = rounded_mask_spans_exact(fw, fh, radius);
        let client = client_rounded_rectangles(fw, fh, titlebar, radius, ring, true);
        let first = usize::try_from(u32::from(titlebar)).unwrap();
        for (index, rect) in client.iter().enumerate() {
            let (_, start, end) = exact[first + index];
            assert_eq!(
                i32::from(rect.x),
                i32::try_from(start.max(ring)).unwrap(),
                "row {index}: overlay keeps only the ring pixel"
            );
            assert_eq!(
                i32::from(rect.x) + i32::from(rect.width),
                i32::try_from(end.saturating_sub(ring)).unwrap(),
                "row {index}"
            );
        }
        let band = client_rounded_rectangles(fw, fh, titlebar, radius, ring, false);
        let last = client.len() - 1;
        assert!(
            client[last].x < band[last].x,
            "the bottom corner starts on the exact arc, not one pixel inside it"
        );
    }

    /// Documented overlay rule: only corner rows change. A straight row keeps exactly the `ring`
    /// inset on both sides and matches the frame-band fallback.
    #[test]
    fn overlay_path_does_not_inset_straight_rows() {
        let (fw, fh, titlebar, radius, ring) = (200u32, 151u32, 31u16, 12u32, 1u32);
        let overlay = client_rounded_rectangles(fw, fh, titlebar, radius, ring, true);
        let band = client_rounded_rectangles(fw, fh, titlebar, radius, ring, false);
        // Row 75 is outside both 12px corner bands.
        let index =
            usize::try_from(fh / 2).unwrap() - usize::try_from(u32::from(titlebar)).unwrap();
        assert_eq!(overlay[index].x, band[index].x);
        assert_eq!(overlay[index].width, band[index].width);
        assert_eq!(i32::from(overlay[index].x), 1);
        assert_eq!(i32::from(overlay[index].width), 198);
    }

    /// The overlay path also needs the frame's bounding/clip regions on the exact arc, otherwise
    /// the +1 mask clips its own child overlay and the exact-arc client rows.
    #[test]
    fn overlay_frame_shape_uses_the_exact_arc() {
        let outer = Rect {
            x: 0,
            y: 0,
            width: 500,
            height: 300,
        };
        let build = |ring_inset| {
            bounding_rectangles_with_titlebar(outer, true, DecorationStyle::Rounded, 31, ring_inset)
        };
        let exact = build(true);
        let band = build(false);
        let left = |rects: &[Rectangle], row: i16| {
            rects
                .iter()
                .filter(|r| r.y == row)
                .map(|r| i32::from(r.x))
                .min()
                .expect("one rect per row")
        };
        for row in [0i16, 1, 2, 297, 298, 299] {
            assert_eq!(
                left(&exact, row) + 1,
                left(&band, row),
                "row {row}: exact frame shape starts one pixel earlier"
            );
        }
        assert_eq!(left(&exact, 150), left(&band, 150), "straight row");
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
                false,
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

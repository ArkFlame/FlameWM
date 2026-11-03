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
    let value = value.min(u32::from(u16::MAX)) as u16;
    // INTERNAL: dev-only live outline tuning; remove or keep behind the env gate. One owner for
    // the radius, so the frame shape, the client mask, the frame band and the overlay agree.
    match crate::outline_tune::current().radius {
        Some(radius) => u16::try_from(radius).unwrap_or(u16::MAX),
        None => value,
    }
}

/// Client area of a frame: the client window sits at `(inset, titlebar)` and spans the frame width
/// minus the outline band on both sides, and the height minus the titlebar band and the bottom
/// outline band. The band is therefore geometrically outside the client, so no client shape can
/// cover the ring. The X border never inflates the client (see `FrameExtents::delta`).
#[must_use]
pub fn client_area_size(outer: Rect, titlebar_h: u16, inset: u16) -> (u32, u32) {
    let inset = u32::from(inset);
    (
        outer.width.saturating_sub(inset.saturating_mul(2)),
        outer
            .height
            .saturating_sub(u32::from(titlebar_h))
            .saturating_sub(inset),
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

/// Frame bounding/clip rectangles. `outline` mirrors [`client_rounded_rectangles`]: `false` keeps
/// the frame-band fallback's extra corner pixel, `true` uses [`rounded_mask_spans_exact`] so the
/// frame region, the client and the antialiased ring all follow one exact arc.
#[must_use]
pub fn bounding_rectangles_with_titlebar(
    outer: Rect,
    floating: bool,
    style: DecorationStyle,
    titlebar_h: u16,
    outline: bool,
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
    let spans = if outline {
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

/// Client shape inside the frame's outline band, sampled from the frame's own mask.
///
/// The client is the square child that actually shows pixels at the bottom edge, so its mask has to
/// follow the same arc rows the frame uses; deriving the arc from the client's own height samples
/// the curve one row too high and leaves the bottom-left/bottom-right corner visibly square (or a
/// pixel wide) next to the antialiased top corners. Rows are taken from the frame's spans for the
/// *frame* and mapped into client coordinates by subtracting the titlebar band, so nothing can
/// drift between the two masks.
///
/// `inset` is the outline band the client window is placed inside (frame-band path): the band lies
/// outside the client window, so the window itself keeps the ring uncovered whatever shape the
/// application installs later. `ring` is the band that is still *inside* the client window (a
/// client-side frame whose frame rect equals the client rect), and is 0 for every server frame.
///
/// `titlebar_h == 0` means the client covers the whole frame height, so it also carries the top
/// arc. On a square frame the result is one full rectangle inset by `ring`, or the full client
/// rect when `ring == 0`.
///
/// `exact_arc` picks the frame's span owner: `false` uses [`rounded_mask_spans`] (the frame-band
/// fallback keeps its extra corner pixel while no outline is drawn), `true` uses
/// [`rounded_mask_spans_exact`] (an outline is drawn, so the client starts on the exact arc the
/// ring covers).
#[must_use]
pub fn client_rounded_rectangles(
    frame_width: u32,
    frame_height: u32,
    titlebar_h: u16,
    radius: u32,
    inset: u32,
    ring: u32,
    exact_arc: bool,
) -> Vec<Rectangle> {
    let titlebar = u32::from(titlebar_h).min(frame_height);
    let inset = inset.min(frame_width / 2).min(frame_height - titlebar);
    // The client window: `inset` inside the frame on the left/right and above the bottom edge.
    let client_w = frame_width.saturating_sub(inset.saturating_mul(2));
    let client_h = frame_height.saturating_sub(titlebar).saturating_sub(inset);
    // Rows the mask spans: the client window minus the band still inside it at the bottom.
    let rows = client_h.saturating_sub(ring);
    if client_w.saturating_sub(ring.saturating_mul(2)) == 0 || rows == 0 {
        return Vec::new();
    }
    let radius = radius.min(frame_width / 2).min(frame_height);
    if radius == 0 {
        return vec![Rectangle {
            x: i16::try_from(ring).unwrap_or(i16::MAX),
            y: 0,
            width: client_w
                .saturating_sub(ring.saturating_mul(2))
                .min(u32::from(u16::MAX)) as u16,
            height: rows.min(u32::from(u16::MAX)) as u16,
        }];
    }
    let spans = if exact_arc {
        rounded_mask_spans_exact(frame_width, frame_height, radius)
    } else {
        rounded_mask_spans(frame_width, frame_height, radius)
    };
    spans
        .into_iter()
        .filter(|(row, _, _)| *row >= titlebar && *row < titlebar.saturating_add(rows))
        .map(|(row, start, end)| {
            // The band is measured inward from the arc: `inset` of it is already outside the
            // client window, `ring` of it is still inside. `max` would put the client on the arc
            // itself and its own pixels would cover the ring's outermost corner pixel.
            let x = start.saturating_add(ring).min(client_w);
            let end = end
                .saturating_sub(inset.saturating_mul(2))
                .saturating_sub(ring)
                .max(x)
                .min(client_w);
            Rectangle {
                x: i16::try_from(x.min(u32::from(u16::MAX))).unwrap_or(i16::MAX),
                y: i16::try_from((row - titlebar).min(u32::from(u16::MAX))).unwrap_or(i16::MAX),
                width: end.saturating_sub(x).min(u32::from(u16::MAX)) as u16,
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
        // client window is placed inside that band, so it can never cover the ring.
        let outer = Rect {
            x: 0,
            y: 0,
            width: 484,
            height: 347,
        };
        assert_eq!(client_area_size(outer, 31, 0), (484, 316));
        assert_eq!(client_area_size(outer, 31, 1), (482, 315));
        let full = client_rounded_rectangles(outer.width, outer.height, 31, 0, 1, 0, false);
        assert_eq!(full.len(), 1);
        assert_eq!((full[0].x, full[0].width), (0, 482));
        assert_eq!(full[0].height, 315);
        assert_eq!(
            client_rounded_rectangles(outer.width, outer.height, 31, 0, 0, 0, false)[0].width,
            484
        );
    }

    /// Frame-band path: the client window is placed `t` px inside the frame, so its mask (mapped
    /// back to frame coordinates) never reaches the ring band on any row or column. This is what
    /// makes the ring immune to a client that installs its own full-size shape.
    #[test]
    fn frame_band_client_mask_never_covers_the_ring() {
        let (fw, fh, titlebar, radius, t) = (200u32, 151u32, 31u16, 12u32, 2u32);
        let outer = Rect {
            x: 0,
            y: 0,
            width: fw,
            height: fh,
        };
        let (cw, ch) = client_area_size(outer, titlebar, t as u16);
        assert_eq!((cw, ch), (fw - 2 * t, fh - u32::from(titlebar) - t));
        let rects = client_rounded_rectangles(fw, fh, titlebar, radius, t, 0, true);
        assert_eq!(
            rects.len(),
            usize::try_from(ch).unwrap(),
            "one rect per row"
        );
        for rect in &rects {
            let (x0, x1) = (
                i32::from(rect.x) + t as i32,
                i32::from(rect.x) + i32::from(rect.width) + t as i32,
            );
            let (y0, y1) = (
                i32::from(rect.y) + i32::from(titlebar),
                i32::from(rect.y) + i32::from(rect.height) + i32::from(titlebar),
            );
            assert!(
                x0 >= t as i32 && x1 <= (fw - t) as i32,
                "row {} covers the left/right band: {rect:?}",
                rect.y
            );
            assert!(
                y0 >= i32::from(titlebar) && y1 <= (fh - t) as i32,
                "row {} covers the bottom band: {rect:?}",
                rect.y
            );
        }
        // The first and last rows of the client leave the `t`-wide bottom band uncovered, and the
        // outermost columns leave the side bands uncovered.
        let last = rects.last().expect("client rows");
        assert_eq!(
            i32::from(last.y) + i32::from(titlebar) + 1,
            (fh - t) as i32,
            "the client stops exactly above the bottom band"
        );
        // A straight row covers the whole client width: no dark gap between the band and the
        // client on the left, right or bottom edge.
        let middle = rects[usize::try_from(ch / 2).unwrap()];
        assert_eq!((middle.x, middle.width), (0, cw as u16));
    }

    /// The client mask must be the frame mask shifted by the titlebar band: any drift here is
    /// exactly what makes the bottom corners look square while the top corners are rounded. The
    /// client window sits `t` px inside the frame, so its local rows are the frame rows minus the
    /// titlebar band, and its local columns are the frame arc minus the band on both sides.
    #[test]
    fn client_mask_rows_match_the_frame_mask_rows() {
        let (frame_w, frame_h, titlebar, radius, t) = (200_u32, 151_u32, 31_u16, 12_u32, 1_u32);
        let frame = rounded_mask_spans_exact(frame_w, frame_h, radius);
        let client = client_rounded_rectangles(frame_w, frame_h, titlebar, radius, t, 0, true);
        assert_eq!(
            client.len(),
            usize::try_from(frame_h - u32::from(titlebar) - t).unwrap()
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
                i32::try_from(frame_start).unwrap(),
                "row {index}: the client starts on the frame arc, one band inside the frame edge"
            );
            assert_eq!(
                i32::from(rect.x) + i32::from(rect.width),
                i32::try_from(frame_end.saturating_sub(t * 2)).unwrap(),
                "row {index}: the right edge stops one band inside the frame arc"
            );
        }
        let last = client.last().expect("client rows");
        assert!(last.x > 1, "bottom-left corner is not cut: {last:?}");
        assert!(last.width < 198, "bottom-right corner is not cut");
    }

    /// Without a titlebar band the client covers the frame, so it carries both arcs.
    #[test]
    fn titlebarless_client_rounds_all_four_corners() {
        let rects = client_rounded_rectangles(200, 120, 0, 12, 1, 0, false);
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
            (0, 198),
            "the straight middle rows cover the whole client window"
        );
        let squared = client_rounded_rectangles(200, 120, 0, 0, 1, 0, false);
        assert_eq!(squared.len(), 1);
        assert_eq!((squared[0].x, squared[0].width), (0, 198));
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

    /// With no outline drawn nothing reserves a band (`inset == 0`, `ring == 0`) and the
    /// frame-band fallback keeps its +1 corner rows: the pre-outline geometry must not drift.
    #[test]
    fn no_outline_client_keeps_the_extra_corner_pixel() {
        let (fw, fh, titlebar, radius) = (200u32, 151u32, 31u16, 12u32);
        let band = rounded_mask_spans(fw, fh, radius);
        let client = client_rounded_rectangles(fw, fh, titlebar, radius, 0, 0, false);
        for (index, rect) in client.iter().enumerate() {
            let (_, start, end) = band[usize::try_from(u32::from(titlebar)).unwrap() + index];
            assert_eq!(
                i32::from(rect.x),
                i32::try_from(start).unwrap(),
                "row {index}: fallback keeps the frame-band inset"
            );
            assert_eq!(
                i32::from(rect.x) + i32::from(rect.width),
                i32::try_from(end).unwrap(),
                "row {index}"
            );
        }
    }

    /// A client-side frame *is* the client rect, so its band cannot sit outside the window: the
    /// mask reserves the ring inside its own rect, on the exact arc the ring rasterises on.
    #[test]
    fn client_side_frame_reserves_the_ring_inside_its_own_rect() {
        let (fw, fh, titlebar, radius, ring) = (200u32, 151u32, 31u16, 12u32, 1u32);
        let exact = rounded_mask_spans_exact(fw, fh, radius);
        let client = client_rounded_rectangles(fw, fh, titlebar, radius, 0, ring, true);
        let first = usize::try_from(u32::from(titlebar)).unwrap();
        for (index, rect) in client.iter().enumerate() {
            let (_, start, end) = exact[first + index];
            assert_eq!(
                i32::from(rect.x),
                i32::try_from(start + ring).unwrap(),
                "row {index}: the band is reserved inside the arc"
            );
            assert_eq!(
                i32::from(rect.x) + i32::from(rect.width),
                i32::try_from(end.saturating_sub(ring)).unwrap(),
                "row {index}"
            );
        }
        let band = client_rounded_rectangles(fw, fh, titlebar, radius, 0, ring, false);
        let last = client.len() - 1;
        assert!(
            client[last].x < band[last].x,
            "the bottom corner starts on the exact arc, not one pixel inside it"
        );
    }

    /// The compositing-manager overlay keeps the client at its full frame rect: nothing is
    /// reserved outside the window, so a straight row runs edge to edge and only the corner rows
    /// follow the exact arc. The frame-band path instead loses the band on both sides.
    #[test]
    fn overlay_path_keeps_the_client_full_width() {
        let (fw, fh, titlebar, radius) = (200u32, 151u32, 31u16, 12u32);
        // Row 75 is outside both 12px corner bands.
        let index =
            usize::try_from(fh / 2).unwrap() - usize::try_from(u32::from(titlebar)).unwrap();
        let overlay = client_rounded_rectangles(fw, fh, titlebar, radius, 0, 0, true);
        assert_eq!(i32::from(overlay[index].x), 0);
        assert_eq!(i32::from(overlay[index].width), 200);
        let band = client_rounded_rectangles(fw, fh, titlebar, radius, 1, 0, true);
        assert_eq!(i32::from(band[index].x), 0);
        assert_eq!(i32::from(band[index].width), 198);
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
        for (height, expected) in [(31, 12), (42, 16), (62, 24)] {
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

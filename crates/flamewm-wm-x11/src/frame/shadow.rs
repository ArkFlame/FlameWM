//! Simple 45-degree cast shadow: one square with a small radius, offset to the
//! bottom-right of the frame.
//!
//! No compositor exists, so a shadow cannot be alpha-blended by the server.
//! One override-redirect window per frame holds a root snapshot darkened in
//! place; the frame covers all but the right/bottom band. The silhouette is a
//! single rectangle with a fixed small radius, so there is no per-window
//! corner-radius bookkeeping, no multi-strip tiling and no upward fill.
//! `plan_shadow` still keeps readbacks off the motion path: a drag only slides
//! the window, and one re-shade happens after everything settles.

use crate::geometry::Rect;

/// Cast offset from the frame's top-left, in pixels (45 degrees).
pub const OFFSET: i32 = 6;
/// How far the shadow extends beyond the offset at the right and bottom.
pub const SPREAD: i32 = 10;
/// Fixed corner radius of the cast square.
pub const RADIUS: u32 = 6;
/// Peak darkening (0..=1) at the frame edge for a focused frame.
pub const PEAK_FOCUSED: f32 = 0.52;
/// Peak darkening (0..=1) at the frame edge for an unfocused frame.
pub const PEAK_UNFOCUSED: f32 = 0.30;

/// What the WM must do to one frame's shadow window this turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShadowAction {
    /// The window already holds the right pixels at the right place.
    Noop,
    /// Slide the existing window with the frame; no root readback at all.
    Move,
    /// Re-shade the window from a fresh root snapshot.
    Reshade,
}

/// What one frame's shadow currently holds, as last written by the WM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShadowState {
    /// Frame outer rect the pixels were shaded from.
    pub shaded: Rect,
    /// Focus (shadow alpha) the pixels were shaded with.
    pub focused: bool,
    /// Root generation at that shading; a newer generation means the root behind
    /// the shadow may have changed (another window settled, wallpaper or layout).
    pub generation: u64,
    /// Frame outer rect the window currently sits at. Differs from `shaded`
    /// while a live move has dragged it off its snapshot.
    pub placed: Rect,
}

/// Decide this turn's work for one frame. Pure: the caller owns every X effect.
///
/// `gesture_active` means frames are still moving this turn — a live move/resize
/// (this frame's *or any other frame's*) or a running open/close flight. A
/// re-shade needs a server grab and one `get_image` round-trip, so none may
/// overlap one, whoever is moving: motion therefore never re-shades. The window
/// simply follows the frame and is re-shaded once, when everything has settled
/// and the root generation has advanced.
pub fn plan_shadow(
    prev: Option<ShadowState>,
    current: Rect,
    focused: bool,
    gesture_active: bool,
    root_generation: u64,
) -> ShadowAction {
    let Some(prev) = prev else {
        // First sight of this frame, or the feature was just switched on.
        return ShadowAction::Reshade;
    };
    if gesture_active {
        return if current == prev.placed {
            ShadowAction::Noop
        } else {
            ShadowAction::Move
        };
    }
    if prev.shaded != current || prev.focused != focused || prev.generation != root_generation {
        return ShadowAction::Reshade;
    }
    if prev.placed != current {
        // Nothing behind the frame changed, but the window is parked elsewhere.
        return ShadowAction::Move;
    }
    ShadowAction::Noop
}

/// Root-space rect of the shadow for a frame whose outer rect is `frame`.
/// It starts inside the frame's top-left and covers the frame plus a right and
/// bottom band; it never extends left of or above the frame.
#[must_use]
pub fn shadow_rect(frame: Rect) -> Rect {
    Rect {
        x: frame.x + OFFSET,
        y: frame.y + OFFSET,
        width: frame.width + SPREAD as u32,
        height: frame.height + SPREAD as u32,
    }
}

/// Circular horizontal inset of a rounded corner at `row` (0 = the extreme
/// row). Used only for the visible bottom-right corner.
fn corner_inset(row: u32, radius: u32) -> u32 {
    if radius == 0 {
        return 0;
    }
    let r = radius as f32;
    let dy = r - (row as f32 + 0.5);
    if dy <= 0.0 {
        return 0;
    }
    (r - (r * r - dy * dy).max(0.0).sqrt()).ceil().max(0.0) as u32
}

/// Visible row spans `(row, x_start, x_end_exclusive)` of the shadow window.
///
/// The cast is a 45-degree shadow: the right band tapers from zero at the
/// frame's top-right corner to its full width over [`OFFSET`] + [`SPREAD`]
/// rows, and the bottom band tapers the same way from the bottom-left corner.
/// Those two diagonal cuts make the shadow meet the window smoothly instead of
/// ending in a hard square cut. The visible bottom-right corner keeps the small
/// fixed radius.
// Row generation is shared by the owned compatibility API and the hot paths below.
// Streaming it avoids allocating a frame-height vector only to clip and discard it.
fn shadow_spans_iter(rect: Rect) -> impl Iterator<Item = (u32, u32, u32)> {
    let (w, h) = (rect.width, rect.height);
    let band = (OFFSET + SPREAD) as u32;
    let chamfer = band.min(w).min(h);
    let radius = RADIUS.min(w).min(h);
    let rows = if w == 0 { 0 } else { h };
    (0..rows).map(move |row| {
        let mut start = 0;
        let mut end = w;
        if row < chamfer {
            end = w.saturating_sub(chamfer - row);
        }
        if row + chamfer >= h {
            start = (row + chamfer).saturating_sub(h);
        }
        if radius > 0 && row + radius >= h {
            let inset = corner_inset(row + radius - h, radius);
            end = end.min(w.saturating_sub(inset));
        }
        (row, start.min(end), end.max(start))
    })
}

#[must_use]
#[cfg(test)]
pub fn shadow_spans(rect: Rect) -> Vec<(u32, u32, u32)> {
    shadow_spans_iter(rect).collect()
}

// Masks and visible shading consume rows once. Keep the original owned
// clip_spans_outside_frame API available for callers and equivalence tests.
fn clipped_shadow_rows(area: Rect, frame: Rect) -> impl Iterator<Item = (u32, u32, u32)> {
    let frame_bottom = frame.y + frame.height as i32;
    let frame_right = frame.x + frame.width as i32;
    let cut = (frame_right - area.x).max(0) as u32;
    shadow_spans_iter(area).map(move |(row, start, end)| {
        let y = area.y + row as i32;
        if y >= frame.y && y < frame_bottom {
            (row, start.max(cut).min(end), end)
        } else {
            (row, start, end)
        }
    })
}

/// Removes the part of `spans` (rows of `rect`) that lies inside `frame`, so the
/// shadow window only shows outside the window. Rows beside the frame keep the
/// full band; rows below it keep their full width, since that is the cast.
#[must_use]
#[cfg(test)]
pub fn clip_spans_outside_frame(
    spans: Vec<(u32, u32, u32)>,
    rect: Rect,
    frame: Rect,
) -> Vec<(u32, u32, u32)> {
    let frame_bottom = frame.y + frame.height as i32;
    let frame_right = frame.x + frame.width as i32;
    let cut = (frame_right - rect.x).max(0) as u32;
    spans
        .into_iter()
        .map(|(row, start, end)| {
            let y = rect.y + row as i32;
            if y >= frame.y && y < frame_bottom {
                let start = start.max(cut).min(end);
                (row, start, end)
            } else {
                (row, start, end)
            }
        })
        .collect()
}

/// The shadow must also occupy the small outside-of-frame arc exposed by the
/// rounded bounding mask. The historical rectangular cut dropped these pixels,
/// leaving a transparent square where the cast should continue behind the frame.
fn clipped_shadow_rows_with_radius(
    area: Rect, frame: Rect, radius: u32,
) -> impl Iterator<Item = (u32, u32, u32)> {
    let frame_bottom = frame.y + frame.height as i32;
    let frame_right = frame.x + frame.width as i32;
    let r = radius.min(frame.width.min(frame.height) / 2);
    shadow_spans_iter(area).map(move |(row, start, end)| {
        let y = area.y + row as i32;
        if y >= frame.y && y < frame_bottom {
            let inset = if r == 0 { 0 } else {
                flamewm_render_x11::external_rounded_row_inset(
                    frame.width, frame.height, r, (y - frame.y) as u32,
                )
            };
            let right_edge = frame_right - inset as i32;
            let cut = (right_edge - area.x).max(0) as u32;
            (row, start.max(cut).min(end), end)
        } else {
            (row, start, end)
        }
    })
}

/// Shape rows that include a rounded frame's exposed corner notches.
#[must_use]
pub fn rounded_shadow_mask_rectangles(area: Rect, frame: Rect, radius: u32) -> Vec<(u32, u32, u32, u32)> {
    let mut rectangles: Vec<(u32, u32, u32, u32)> = Vec::new();
    for (row, start, end) in clipped_shadow_rows_with_radius(area, frame, radius) {
        if start == end { continue; }
        let width = end - start;
        if let Some(last) = rectangles.last_mut() {
            if last.0 == start && last.2 == width && last.1 + last.3 == row {
                last.3 += 1;
                continue;
            }
        }
        rectangles.push((start, row, width, 1));
    }
    rectangles
}

/// Pack adjacent identical X Shape rows into taller rectangles. The shape remains the
/// exact union of `clip_spans_outside_frame` (including its taper and rounded corner), but
/// a long right-hand strip needs one rectangle instead of one protocol item per pixel row.
/// Returned tuples are `(x, y, width, height)` relative to the shadow window.
#[must_use]
pub fn shadow_mask_rectangles(area: Rect, frame: Rect) -> Vec<(u32, u32, u32, u32)> {
    let mut rectangles: Vec<(u32, u32, u32, u32)> = Vec::new();
    for (row, start, end) in clipped_shadow_rows(area, frame) {
        if start == end {
            continue;
        }
        let width = end - start;
        if let Some(last) = rectangles.last_mut() {
            if last.0 == start && last.2 == width && last.1 + last.3 == row {
                last.3 += 1;
                continue;
            }
        }
        rectangles.push((start, row, width, 1));
    }
    rectangles
}

/// Keep factor (0..=256) for the shadow at root pixel `(x, y)` of `frame`'s shadow area: a
/// channel is scaled by `keep / 256`. Pointwise, so a sub-region can be shaded on its own.
#[must_use]
pub fn keep_at(frame: Rect, focused: bool, x: i32, y: i32) -> u32 {
    let peak = if focused {
        PEAK_FOCUSED
    } else {
        PEAK_UNFOCUSED
    };
    let right_edge = frame.x + frame.width as i32;
    let bottom_edge = frame.y + frame.height as i32;
    let reach = (SPREAD + OFFSET).max(1) as f32;
    let dx = (x - right_edge + 1).max(0);
    let dy = (y - bottom_edge + 1).max(0);
    let distance = dx.max(dy) as f32;
    let t = (1.0 - distance / reach).max(0.0);
    let alpha = peak * t * t;
    (256.0 * (1.0 - alpha)) as u32
}

/// The shadow profile has a fixed 16-pixel reach. Computing its floating-point falloff
/// for every uploaded pixel is unnecessary: the same distance always has the same keep factor.
/// These entries come from `keep_at` itself to preserve its rounding and appearance exactly.
const KEEP_TABLE_LEN: usize = (OFFSET + SPREAD + 1) as usize;

#[must_use]
pub(crate) fn shadow_keep_table(focused: bool) -> [u32; KEEP_TABLE_LEN] {
    let reference = Rect::new(0, 0, 1, 1);
    std::array::from_fn(|distance| keep_at(reference, focused, distance as i32, 0))
}

/// Pure integer lookup. Pixels beyond the fade radius retain their original colour (keep=256).
#[must_use]
pub(crate) fn shadow_keep_from_table(
    table: &[u32; KEEP_TABLE_LEN],
    frame: Rect,
    x: i32,
    y: i32,
) -> u32 {
    let right_edge = frame.x + frame.width as i32;
    let bottom_edge = frame.y + frame.height as i32;
    let dx = (x - right_edge + 1).max(0);
    let dy = (y - bottom_edge + 1).max(0);
    table[(dx.max(dy) as usize).min(KEEP_TABLE_LEN - 1)]
}

/// Shade the visible cast and the pixels revealed by the frame's round corners.
pub fn shade_visible_shadow_rounded(frame: Rect, focused: bool, radius: u32, pixels: &mut [u8]) {
    let area = shadow_rect(frame);
    let stride = area.width as usize * 4;
    if pixels.len() != stride * area.height as usize { return; }
    let keep_table = shadow_keep_table(focused);
    for (row, start, end) in clipped_shadow_rows_with_radius(area, frame, radius) {
        let y = area.y + row as i32;
        for col in start..end {
            let x = area.x + col as i32;
            let at = row as usize * stride + col as usize * 4;
            let keep = shadow_keep_from_table(&keep_table, frame, x, y);
            for channel in &mut pixels[at..at + 3] {
                *channel = ((u32::from(*channel) * keep) >> 8) as u8;
            }
        }
    }
}

/// Darken the snapshot pixels (BGRX rows of `shadow_rect(frame)`) in place.
/// Alpha is highest at the frame's right/bottom edge and falls off with the
/// 45-degree cast distance, so the band reads as a simple drop shadow rather
/// than a hard border.
#[cfg(test)]
pub fn shade_shadow(frame: Rect, focused: bool, pixels: &mut [u8]) {
    let area = shadow_rect(frame);
    let (w, h) = (area.width as i32, area.height as i32);
    if w == 0 || h == 0 || pixels.len() != (w * h * 4) as usize {
        return;
    }
    for row in 0..h {
        for col in 0..w {
            let keep = keep_at(frame, focused, area.x + col, area.y + row);
            let at = ((row * w + col) * 4) as usize;
            for channel in &mut pixels[at..at + 3] {
                *channel = ((u32::from(*channel) * keep) >> 8) as u8;
            }
        }
    }
}

/// Shade only the right/bottom bands admitted by the X Shape mask. Pixels under the frame are
/// never presented; the original full-frame shader remains available for pure equivalence tests.
pub fn shade_visible_shadow(frame: Rect, focused: bool, pixels: &mut [u8]) {
    let area = shadow_rect(frame);
    let stride = area.width as usize * 4;
    if pixels.len() != stride * area.height as usize {
        return;
    }
    let keep_table = shadow_keep_table(focused);
    for (row, start, end) in clipped_shadow_rows(area, frame) {
        let y = area.y + row as i32;
        for col in start..end {
            let x = area.x + col as i32;
            let at = row as usize * stride + col as usize * 4;
            let keep = shadow_keep_from_table(&keep_table, frame, x, y);
            for channel in &mut pixels[at..at + 3] {
                *channel = ((u32::from(*channel) * keep) >> 8) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: Rect = Rect {
        x: 100,
        y: 100,
        width: 200,
        height: 150,
    };

    const MOVED: Rect = Rect {
        x: 160,
        y: 140,
        width: 200,
        height: 150,
    };

    const OTHER: Rect = Rect {
        x: 700,
        y: 500,
        width: 320,
        height: 240,
    };

    fn shaded(rect: Rect, focused: bool, generation: u64) -> ShadowState {
        ShadowState {
            shaded: rect,
            focused,
            generation,
            placed: rect,
        }
    }

    fn overlaps(a: Rect, b: Rect) -> bool {
        let (ax2, ay2) = (a.x + a.width as i32, a.y + a.height as i32);
        let (bx2, by2) = (b.x + b.width as i32, b.y + b.height as i32);
        a.x < bx2 && b.x < ax2 && a.y < by2 && b.y < ay2
    }

    #[test]
    fn packed_xshape_mask_has_exact_original_pixel_coverage() {
        let cases = [
            Rect::new(100, 100, 200, 150),
            Rect::new(-1920, -1080, 1280, 720),
            Rect::new(0, 0, 1, 1),
            Rect::new(300, -200, 11, 24),
            Rect::new(-200, 100, 8, 3),
        ];
        for frame in cases {
            let area = shadow_rect(frame);
            let original = clip_spans_outside_frame(shadow_spans(area), area, frame);
            let packed = shadow_mask_rectangles(area, frame);
            let mut expected = vec![false; area.width as usize * area.height as usize];
            let mut actual = expected.clone();
            for (row, start, end) in &original {
                for x in *start..*end {
                    expected[*row as usize * area.width as usize + x as usize] = true;
                }
            }
            for &(x, y, width, height) in &packed {
                assert!(x + width <= area.width && y + height <= area.height);
                for row in y..y + height {
                    for col in x..x + width {
                        let at = row as usize * area.width as usize + col as usize;
                        assert!(!actual[at], "packed rectangles must not overlap");
                        actual[at] = true;
                    }
                }
            }
            assert_eq!(actual, expected, "frame={frame:?}");
            assert!(packed.len() <= original.len());
        }
    }

    #[test]
    fn streamed_rows_match_owned_clipped_spans_including_edge_cases() {
        // Compare against the unchanged owned public path, not merely two calls to the iterator.
        for width in [0, 1, 2, 3, 6, 11, 16, 20, 320, 1280] {
            for height in [0, 1, 2, 3, 6, 11, 16, 20, 200, 720] {
                let frame = Rect::new(-1920, -75, width, height);
                let area = shadow_rect(frame);
                let expected = clip_spans_outside_frame(shadow_spans(area), area, frame);
                let actual: Vec<_> = clipped_shadow_rows(area, frame).collect();
                assert_eq!(actual, expected, "frame={frame:?}");
            }
        }
    }

    #[test]
    fn streaming_preserves_mask_rectangles_and_shaded_pixels() {
        for frame in [
            FRAME,
            Rect::new(-1920, 0, 1, 1),
            Rect::new(0, -1080, 16, 9),
            Rect::new(50, 50, 1280, 720),
        ] {
            let area = shadow_rect(frame);
            let mut expected_rectangles: Vec<(u32, u32, u32, u32)> = Vec::new();
            for (row, start, end) in clip_spans_outside_frame(shadow_spans(area), area, frame) {
                if start == end {
                    continue;
                }
                let width = end - start;
                if let Some(last) = expected_rectangles.last_mut() {
                    if last.0 == start && last.2 == width && last.1 + last.3 == row {
                        last.3 += 1;
                        continue;
                    }
                }
                expected_rectangles.push((start, row, width, 1));
            }
            assert_eq!(shadow_mask_rectangles(area, frame), expected_rectangles);
            for focused in [false, true] {
                let mut expected = vec![160; area.width as usize * area.height as usize * 4];
                let mut actual = expected.clone();
                let keep_table = shadow_keep_table(focused);
                for (row, start, end) in clip_spans_outside_frame(shadow_spans(area), area, frame) {
                    for col in start..end {
                        let at = (row as usize * area.width as usize + col as usize) * 4;
                        let keep = shadow_keep_from_table(
                            &keep_table,
                            frame,
                            area.x + col as i32,
                            area.y + row as i32,
                        );
                        for channel in &mut expected[at..at + 3] {
                            *channel = ((u32::from(*channel) * keep) >> 8) as u8;
                        }
                    }
                }
                shade_visible_shadow(frame, focused, &mut actual);
                assert_eq!(actual, expected, "frame={frame:?}, focused={focused}");
            }
        }
    }

    #[test]
    fn large_shadow_shape_collapses_repeated_rows() {
        let frame = Rect::new(0, 0, 1280, 720);
        let area = shadow_rect(frame);
        let packed = shadow_mask_rectangles(area, frame);
        assert!(
            packed.len() < 50,
            "{} rectangles instead of {} rows",
            packed.len(),
            area.height
        );
    }

    #[test]
    fn integer_keep_table_matches_original_float_function_for_every_shadow_distance() {
        for frame in [FRAME, MOVED, OTHER, Rect::new(-1920, -1080, 200, 150)] {
            let area = shadow_rect(frame);
            for focused in [false, true] {
                let table = shadow_keep_table(focused);
                for y in (area.y..area.bottom()).step_by(3) {
                    for x in (area.x..area.right()).step_by(3) {
                        assert_eq!(
                            shadow_keep_from_table(&table, frame, x, y),
                            keep_at(frame, focused, x, y),
                            "frame={frame:?}, focused={focused}, x={x}, y={y}",
                        );
                    }
                }
                assert_eq!(table[KEEP_TABLE_LEN - 1], 256);
            }
        }
    }

    #[test]
    fn shadow_is_a_square_offset_down_right() {
        let area = shadow_rect(FRAME);
        assert_eq!(area.x, FRAME.x + OFFSET);
        assert_eq!(area.y, FRAME.y + OFFSET);
        assert_eq!(area.width, FRAME.width + SPREAD as u32);
        assert_eq!(area.height, FRAME.height + SPREAD as u32);
        assert!(overlaps(area, FRAME), "shadow sits under the frame");
        assert!(area.x >= FRAME.x, "nothing left of the frame");
        assert!(area.y >= FRAME.y, "nothing above the frame");
    }

    #[test]
    fn shadow_band_is_visible_only_right_and_below() {
        let area = shadow_rect(FRAME);
        let right = area.x + area.width as i32;
        let bottom = area.y + area.height as i32;
        assert_eq!(right, FRAME.x + FRAME.width as i32 + OFFSET + SPREAD);
        assert_eq!(bottom, FRAME.y + FRAME.height as i32 + OFFSET + SPREAD);
    }

    #[test]
    fn smooth_45_degree_taper_at_top_right_and_bottom_left() {
        let area = shadow_rect(FRAME);
        let spans = shadow_spans(area);
        let band = (OFFSET + SPREAD) as u32;
        assert_eq!(spans.len(), area.height as usize);
        // Top-right: the right edge tapers from the frame edge to full width.
        assert_eq!(spans[0].2, area.width - band);
        assert_eq!(spans[band as usize].2, area.width);
        // Bottom-left: the left edge tapers from the frame edge to the right.
        assert_eq!(spans[area.height as usize - band as usize].1, 0);
        assert_eq!(
            spans.last().unwrap().1,
            band - 1,
            "bottom-left ends on a 45-degree diagonal"
        );
        // Bottom-right keeps the small radius.
        assert!(spans.last().unwrap().2 < area.width);
        // The middle is the full square.
        assert_eq!(
            spans[area.height as usize / 2],
            (area.height / 2, 0, area.width)
        );
    }

    #[test]
    fn visible_shading_matches_full_shading_on_the_x_shape_mask() {
        let area = shadow_rect(FRAME);
        for focused in [false, true] {
            let mut full = vec![200u8; area.width as usize * area.height as usize * 4];
            let mut visible = full.clone();
            shade_shadow(FRAME, focused, &mut full);
            shade_visible_shadow(FRAME, focused, &mut visible);
            for (row, start, end) in clip_spans_outside_frame(shadow_spans(area), area, FRAME) {
                for col in start..end {
                    let at = (row as usize * area.width as usize + col as usize) * 4;
                    assert_eq!(&visible[at..at + 4], &full[at..at + 4]);
                }
            }
        }
    }

    #[test]
    fn shade_is_darkest_at_the_edge_and_fades_outward() {
        let area = shadow_rect(FRAME);
        let (w, h) = (area.width as usize, area.height as usize);
        let mut pixels = vec![255u8; w * h * 4];
        shade_shadow(FRAME, true, &mut pixels);
        let at = |col: usize, row: usize| pixels[(row * w + col) * 4];
        // Column/row just outside the frame edge versus the outer edge.
        let edge_col = (FRAME.width as usize - OFFSET as usize).min(w - 1);
        let far_col = w - 1;
        assert!(at(edge_col, 10) < 255, "edge band is shaded");
        assert!(
            at(edge_col, 10) < at(far_col, 10),
            "shadow fades toward the right"
        );
        let edge_row = (FRAME.height as usize - OFFSET as usize).min(h - 1);
        assert!(
            at(10, edge_row) < at(10, h - 1),
            "shadow fades toward the bottom"
        );
    }

    #[test]
    fn focus_is_darker_and_mismatched_buffer_is_untouched() {
        let area = shadow_rect(FRAME);
        let (w, h) = (area.width as usize, area.height as usize);
        let sample = |focused| {
            let mut px = vec![255u8; w * h * 4];
            shade_shadow(FRAME, focused, &mut px);
            px[(30 * w + (FRAME.width as usize / 2)) * 4]
        };
        assert!(sample(true) < sample(false));
        let mut wrong = vec![200u8; 8];
        shade_shadow(FRAME, true, &mut wrong);
        assert!(wrong.iter().all(|&b| b == 200));
    }

    #[test]
    fn clip_removes_only_the_part_under_the_frame() {
        let area = shadow_rect(FRAME);
        let spans = clip_spans_outside_frame(shadow_spans(area), area, FRAME);
        let frame_right_in_area = (FRAME.x + FRAME.width as i32 - area.x) as u32;
        let frame_bottom = FRAME.y + FRAME.height as i32;
        for (row, start, end) in spans {
            let y = area.y + row as i32;
            if y >= FRAME.y && y < frame_bottom {
                assert!(
                    start >= frame_right_in_area,
                    "row {row} starts inside frame"
                );
            }
            assert!(start <= end);
        }
    }

    #[test]
    fn plan_first_sight_reshades() {
        assert_eq!(
            plan_shadow(None, FRAME, true, false, 0),
            ShadowAction::Reshade
        );
        assert_eq!(
            plan_shadow(None, FRAME, true, true, 0),
            ShadowAction::Reshade
        );
    }

    #[test]
    fn plan_motion_never_reshades() {
        let prev = shaded(FRAME, false, 5);
        let cases = [
            (prev, FRAME, false, 5),
            (prev, FRAME, true, 5),
            (prev, FRAME, false, 6),
            (prev, MOVED, false, 5),
            (prev, MOVED, true, 9),
        ];
        for (prev, current, focused, generation) in cases {
            assert_ne!(
                plan_shadow(Some(prev), current, focused, true, generation),
                ShadowAction::Reshade
            );
            if prev.shaded != current || prev.focused != focused || prev.generation != generation {
                assert_eq!(
                    plan_shadow(Some(prev), current, focused, false, generation),
                    ShadowAction::Reshade
                );
            }
        }
    }

    #[test]
    fn plan_settle_moves_then_reshades_once() {
        let parked = ShadowState {
            placed: MOVED,
            ..shaded(FRAME, true, 4)
        };
        assert_eq!(
            plan_shadow(Some(parked), MOVED, true, false, 5),
            ShadowAction::Reshade
        );
        assert_eq!(
            plan_shadow(Some(shaded(MOVED, true, 5)), MOVED, true, false, 5),
            ShadowAction::Noop
        );
        assert_eq!(
            plan_shadow(Some(shaded(FRAME, true, 5)), MOVED, true, false, 5),
            ShadowAction::Reshade
        );
        assert_eq!(
            plan_shadow(Some(shaded(FRAME, true, 5)), MOVED, true, true, 5),
            ShadowAction::Move
        );
    }

    #[test]
    fn plan_focus_change_reshades_only_affected_frames() {
        let generation = 9;
        assert_eq!(
            plan_shadow(
                Some(shaded(FRAME, true, generation)),
                FRAME,
                false,
                false,
                generation
            ),
            ShadowAction::Reshade
        );
        assert_eq!(
            plan_shadow(
                Some(shaded(OTHER, false, generation)),
                OTHER,
                true,
                false,
                generation
            ),
            ShadowAction::Reshade
        );
        assert_eq!(
            plan_shadow(
                Some(shaded(OTHER, false, generation)),
                OTHER,
                false,
                false,
                generation
            ),
            ShadowAction::Noop
        );
    }
}

#[cfg(test)]
mod rounded_shadow_corner_tests {
    use super::*;
    fn contains(rects: &[(u32,u32,u32,u32)], x: u32, y: u32) -> bool {
        rects.iter().any(|&(rx,ry,rw,rh)| x>=rx && x<rx+rw && y>=ry && y<ry+rh)
    }
    #[test]
    fn rounded_frame_notch_has_cast_without_leaking_under_the_client() {
        let frame = Rect::new(100,100,200,150);
        let area = shadow_rect(frame);
        let rectangular = shadow_mask_rectangles(area,frame);
        let round = rounded_shadow_mask_rectangles(area,frame,12);
        let y = frame.bottom()-1;
        let x = frame.right()-3;
        let col = (x-area.x) as u32;
        let row = (y-area.y) as u32;
        assert!(!contains(&rectangular,col,row));
        assert!(contains(&round,col,row));
        let inside = ((frame.right()-15-area.x) as u32, (frame.bottom()-15-area.y) as u32);
        assert!(!contains(&round,inside.0,inside.1));
        assert_eq!(rounded_shadow_mask_rectangles(area,frame,0),rectangular);
    }
}

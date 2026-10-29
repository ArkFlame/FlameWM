//! Centred cast shadow: one rounded silhouette that extends [`SPREAD`] pixels beyond
//! every edge of the frame, with no offset.
//!
//! No compositor exists, so a shadow cannot be alpha-blended by the server.
//! One override-redirect window per frame holds a root snapshot darkened in
//! place; the frame covers the interior, so only the ring shows. The silhouette's
//! corners are concentric with the frame's corner arc, so the notches a rounded
//! frame exposes lie inside it.
//! `plan_shadow` still keeps readbacks off the motion path: a drag only slides
//! the window, and one re-shade happens after everything settles.

use crate::geometry::Rect;

/// Cast offset from the frame centre, in pixels. Zero: the shadow is centred on all four sides.
pub const OFFSET: i32 = 0;
/// How far the shadow extends beyond every edge of the frame.
pub const SPREAD: i32 = 14;
/// Fixed corner radius of the cast square.
pub const RADIUS: u32 = 6;
/// Peak darkening (0..=1) at the frame edge for a focused frame. Soft: quadratic falloff to 0.
pub const PEAK_FOCUSED: f32 = 0.20;
/// Peak darkening (0..=1) at the frame edge for an unfocused frame.
pub const PEAK_UNFOCUSED: f32 = 0.12;

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
/// It is centred on the frame: `SPREAD` pixels beyond every edge, so the window
/// covers the frame plus a ring of that width. The frame covers the interior.
#[must_use]
pub fn shadow_rect(frame: Rect) -> Rect {
    Rect {
        x: frame.x - OFFSET - SPREAD,
        y: frame.y - OFFSET - SPREAD,
        width: frame.width + 2 * (OFFSET + SPREAD) as u32,
        height: frame.height + 2 * (OFFSET + SPREAD) as u32,
    }
}

/// Opaque desktop colour (BGRX, Z_PIXMAP 24-bit) a shadow snapshot starts from. Any region the
/// desktop read does not cover keeps this colour instead of 0x000000, so it never shows black.
pub const SNAPSHOT_BASE_BGRX: [u8; 4] = [0x20, 0x1e, 0x1b, 0x00];

/// Local `(x, y, w, h)` of the desktop read for a shadow snapshot of `area`: the whole shadow rect.
/// A cast-only read leaves the frame's rounded-corner notches (inside the frame rect) unfilled.
#[must_use]
pub fn snapshot_read_rect(area: Rect) -> (i32, i32, i32, i32) {
    (0, 0, area.width.max(1) as i32, area.height.max(1) as i32)
}

/// A `width` x `height` BGRX snapshot pre-filled with the desktop colour.
#[must_use]
pub fn snapshot_base(width: usize, height: usize) -> Vec<u8> {
    SNAPSHOT_BASE_BGRX.repeat(width * height)
}

/// Circular horizontal inset of a rounded corner at `row` (0 = the extreme
/// row). Used for the rounded silhouette corners.
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

/// Corner radius of the shadow silhouette for a frame with corner radius `radius`.
/// It is concentric with the frame's corner arc, so the notches a rounded frame
/// exposes at its corners lie inside the silhouette, at every corner.
#[must_use]
pub fn silhouette_radius(radius: u32) -> u32 {
    if radius == 0 {
        0
    } else {
        radius + SPREAD as u32
    }
}

/// Visible row spans `(row, x_start, x_end_exclusive)` of the shadow window.
///
/// The silhouette is the shadow rect with rounded corners of `silhouette_radius(radius)`.
/// The frame covers its interior, so no span is cut out there.
fn shadow_spans_iter(rect: Rect, radius: u32) -> impl Iterator<Item = (u32, u32, u32)> {
    let (w, h) = (rect.width, rect.height);
    let radius = silhouette_radius(radius).min(w / 2).min(h / 2);
    let rows = if w == 0 { 0 } else { h };
    (0..rows).map(move |row| {
        let from_extreme = row.min(h - 1 - row);
        let inset = if radius > 0 && from_extreme < radius {
            corner_inset(from_extreme, radius)
        } else {
            0
        };
        (row, inset.min(w), w.saturating_sub(inset))
    })
}

#[must_use]
pub fn shadow_spans(rect: Rect) -> Vec<(u32, u32, u32)> {
    shadow_spans_iter(rect, RADIUS).collect()
}

/// Shape rectangles of the ring of `area`: the silhouette rows of the shadow minus the frame's
/// rounded interior (`frame` in root space, `radius` its corner radius). The frame covers that
/// interior, so the shadow window never holds pixels under the window. Runs that repeat on the
/// next row are packed. Returned tuples are `(x, y, width, height)` relative to the shadow window.
#[must_use]
pub fn rounded_shadow_mask_rectangles(
    area: Rect,
    frame: Rect,
    radius: u32,
) -> Vec<(u32, u32, u32, u32)> {
    let mut rectangles: Vec<(u32, u32, u32, u32)> = Vec::new();
    let mut push = |start: u32, row: u32, width: u32| {
        if let Some(last) = rectangles.last_mut() {
            if last.0 == start && last.2 == width && last.1 + last.3 == row {
                last.3 += 1;
                return;
            }
        }
        rectangles.push((start, row, width, 1));
    };
    let frame_top = frame.y - area.y;
    let frame_bottom = frame_top + frame.height as i32;
    for (row, start, end) in shadow_spans_iter(area, radius) {
        if start == end {
            continue;
        }
        let y = area.y + row as i32;
        // Rows the frame does not cover have no interior: the whole silhouette span is ring.
        if (row as i32) < frame_top || (row as i32) >= frame_bottom {
            push(start, row, end - start);
            continue;
        }
        let mut run: Option<u32> = None;
        for x in start..end {
            let outside = rounded_signed_distance(frame, radius, area.x + x as i32, y) > 0.0;
            match (outside, run) {
                (true, None) => run = Some(x),
                (false, Some(from)) => {
                    push(from, row, x - from);
                    run = None;
                }
                _ => {}
            }
        }
        if let Some(from) = run {
            push(from, row, end - from);
        }
    }
    rectangles
}

/// Outward distance in pixels from `frame`'s edge to root pixel `(x, y)`: 0 inside the frame,
/// 1 in the first pixel beyond an edge. Symmetric on all four sides.
#[must_use]
fn outward_distance(frame: Rect, x: i32, y: i32) -> i32 {
    let dx = (frame.x - x).max(x - (frame.x + frame.width as i32) + 1).max(0);
    let dy = (frame.y - y).max(y - (frame.y + frame.height as i32) + 1).max(0);
    dx.max(dy)
}

/// Keep factor (0..=256) for the shadow at root pixel `(x, y)` of `frame`'s shadow area: a
/// channel is scaled by `keep / 256`. Pointwise, so a sub-region can be shaded on its own.
#[must_use]
pub fn keep_at(frame: Rect, focused: bool, x: i32, y: i32) -> u32 {
    keep_from_distance(focused, outward_distance(frame, x, y) as f32)
}

/// Keep factor for a signed or outward distance in pixels: alpha is `peak * t^2`, where `t` falls
/// linearly from 1 at the frame edge to 0 at `SPREAD`.
#[must_use]
fn keep_from_distance(focused: bool, distance: f32) -> u32 {
    let peak = if focused {
        PEAK_FOCUSED
    } else {
        PEAK_UNFOCUSED
    };
    let reach = SPREAD.max(1) as f32;
    let t = (1.0 - distance / reach).clamp(0.0, 1.0);
    let alpha = peak * t * t;
    (256.0 * (1.0 - alpha)) as u32
}

/// Euclidean signed distance in pixels from the centre of root pixel `(x, y)` to the boundary of
/// `frame`'s rounded rectangle with corner `radius`: negative inside, 0 on the edge, positive
/// outside. The corner arcs are the frame's own, so isolines are concentric with the outline.
#[must_use]
fn rounded_signed_distance(frame: Rect, radius: u32, x: i32, y: i32) -> f32 {
    let half_w = frame.width as f32 * 0.5;
    let half_h = frame.height as f32 * 0.5;
    let r = (radius as f32).min(half_w).min(half_h);
    let qx = (x as f32 + 0.5 - (frame.x as f32 + half_w)).abs() - (half_w - r);
    let qy = (y as f32 + 0.5 - (frame.y as f32 + half_h)).abs() - (half_h - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

/// Keep factor for the shadow at root pixel `(x, y)` of a frame whose corner radius is `radius`.
/// Square frames (radius 0) use the square ring of `keep_at`. Rounded frames fade along the
/// signed distance to their rounded outline, so the shadow has no square corner leak.
#[must_use]
pub fn keep_at_shape(frame: Rect, radius: u32, focused: bool, x: i32, y: i32) -> u32 {
    if radius == 0 {
        return keep_at(frame, focused, x, y);
    }
    keep_from_distance(focused, rounded_signed_distance(frame, radius, x, y))
}

/// The shadow profile has a fixed `SPREAD`-pixel reach. Computing its floating-point falloff
/// for every uploaded pixel is unnecessary: the same distance always has the same keep factor.
/// These entries come from `keep_at` itself to preserve its rounding and appearance exactly.
const KEEP_TABLE_LEN: usize = (SPREAD + 1) as usize;

#[must_use]
pub(crate) fn shadow_keep_table(focused: bool) -> [u32; KEEP_TABLE_LEN] {
    // A reference frame at the origin: root pixel x = distance lies `distance` pixels beyond its edge.
    let reference = Rect::new(0, 0, 1, 1);
    std::array::from_fn(|distance| keep_at(reference, focused, distance as i32, 0))
}

/// Keep factor from the square-ring table for radius 0, or the rounded model otherwise. Pixels
/// beyond the fade radius retain their original colour (keep=256).
#[must_use]
pub(crate) fn shadow_keep_from_table(
    table: &[u32; KEEP_TABLE_LEN],
    frame: Rect,
    radius: u32,
    focused: bool,
    x: i32,
    y: i32,
) -> u32 {
    if radius > 0 {
        return keep_at_shape(frame, radius, focused, x, y);
    }
    let distance = outward_distance(frame, x, y);
    if distance as usize >= KEEP_TABLE_LEN {
        return 256;
    }
    table[distance.max(0) as usize]
}

/// Shade the visible shadow silhouette, including the pixels revealed by the frame's round corners.
pub fn shade_visible_shadow_rounded(frame: Rect, focused: bool, radius: u32, pixels: &mut [u8]) {
    let area = shadow_rect(frame);
    let stride = area.width as usize * 4;
    if pixels.len() != stride * area.height as usize {
        return;
    }
    let keep_table = shadow_keep_table(focused);
    for (row, start, end) in shadow_spans_iter(area, radius) {
        let y = area.y + row as i32;
        for col in start..end {
            let x = area.x + col as i32;
            let at = row as usize * stride + col as usize * 4;
            let keep = shadow_keep_from_table(&keep_table, frame, radius, focused, x, y);
            for channel in &mut pixels[at..at + 3] {
                *channel = ((u32::from(*channel) * keep) >> 8) as u8;
            }
        }
    }
}

/// Darken the snapshot pixels (BGRX rows of `shadow_rect(frame)`) in place, with the full
/// frame shader. Alpha is highest at the frame's edge and falls off outward on all four sides.
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

    /// Shaded BGRX pixel of the visible silhouette at root point `(x, y)` (`255` = untouched).
    fn shaded_at(frame: Rect, focused: bool, radius: u32, x: i32, y: i32) -> u8 {
        let area = shadow_rect(frame);
        let mut pixels = vec![255u8; area.width as usize * area.height as usize * 4];
        shade_visible_shadow_rounded(frame, focused, radius, &mut pixels);
        let at = ((y - area.y) as usize * area.width as usize + (x - area.x) as usize) * 4;
        pixels[at]
    }

    #[test]
    fn shadow_rect_is_centred_and_grows_spread_on_every_side() {
        let area = shadow_rect(FRAME);
        assert_eq!(area.x, FRAME.x - SPREAD);
        assert_eq!(area.y, FRAME.y - SPREAD);
        assert_eq!(area.width, FRAME.width + 2 * SPREAD as u32);
        assert_eq!(area.height, FRAME.height + 2 * SPREAD as u32);
        assert_eq!(OFFSET, 0, "the cast is centred: no offset");
    }

    #[test]
    fn keep_falls_off_identically_on_all_four_sides() {
        for focused in [true, false] {
            for d in 0..=SPREAD {
                let mid_x = FRAME.x + FRAME.width as i32 / 2;
                let mid_y = FRAME.y + FRAME.height as i32 / 2;
                let right = keep_at(FRAME, focused, FRAME.right() + d - 1, mid_y);
                let left = keep_at(FRAME, focused, FRAME.x - d, mid_y);
                let bottom = keep_at(FRAME, focused, mid_x, FRAME.bottom() + d - 1);
                let top = keep_at(FRAME, focused, mid_x, FRAME.y - d);
                assert_eq!(right, left, "left/right differ at distance {d}");
                assert_eq!(top, bottom, "top/bottom differ at distance {d}");
                assert_eq!(right, top, "sides differ at distance {d}");
            }
        }
    }

    #[test]
    fn keep_table_matches_keep_at_on_every_side_and_distance() {
        for focused in [true, false] {
            let table = shadow_keep_table(focused);
            for d in 0..=SPREAD {
                let x = FRAME.right() + d - 1;
                let y = FRAME.y + 10;
                assert_eq!(
                    shadow_keep_from_table(&table, FRAME, 0, focused, x, y),
                    keep_at(FRAME, focused, x, y),
                    "table/float mismatch at distance {d}"
                );
            }
            assert_eq!(
                shadow_keep_from_table(&table, FRAME, 0, focused, FRAME.x - SPREAD - 1, FRAME.y + 10),
                256
            );
        }
    }

    const ROUND: u32 = 24;

    #[test]
    fn rounded_corner_pixel_beyond_silhouette_is_fully_transparent() {
        // Outer silhouette radius is ROUND + SPREAD, concentric with the corner arc centre
        // (FRAME.x + ROUND, FRAME.y + ROUND). The shadow-rect corner pixel lies outside it.
        for focused in [true, false] {
            let corner = (FRAME.x - SPREAD, FRAME.y - SPREAD);
            assert_eq!(keep_at_shape(FRAME, ROUND, focused, corner.0, corner.1), 256);
            let beyond = (FRAME.x - SPREAD + 1, FRAME.y - SPREAD + 1);
            assert_eq!(keep_at_shape(FRAME, ROUND, focused, beyond.0, beyond.1), 256);
        }
    }

    #[test]
    fn rounded_and_squared_corners_differ_where_the_square_leaks() {
        // One pixel inside the square corner: the square ring still shades it, the rounded
        // outline (outside radius + SPREAD from the arc centre) does not.
        let (x, y) = (FRAME.x - SPREAD + 1, FRAME.y - SPREAD + 1);
        assert!(keep_at(FRAME, true, x, y) < 256, "squared ring shades the corner");
        assert_eq!(keep_at_shape(FRAME, ROUND, true, x, y), 256);
        assert_ne!(
            keep_at_shape(FRAME, 0, true, x, y),
            keep_at_shape(FRAME, ROUND, true, x, y)
        );
    }

    #[test]
    fn rounded_corner_profile_is_radially_smooth_and_monotone() {
        // Walk outward along the 45 degree diagonal from the arc centre. Keep rises
        // monotonically from the peak to 256 with no step larger than the falloff allows.
        let centre = (FRAME.x + ROUND as i32, FRAME.y + ROUND as i32);
        let mut previous = None;
        for step in 0..=(SPREAD + ROUND as i32) {
            let x = centre.0 - ROUND as i32 - step;
            let y = centre.1 - ROUND as i32 - step;
            let keep = keep_at_shape(FRAME, ROUND, true, x, y);
            if let Some(prev) = previous {
                assert!(keep >= prev, "keep fell moving outward at step {step}");
                assert!(keep - prev <= 12, "keep jumped {} at step {step}", keep - prev);
            }
            previous = Some(keep);
        }
        assert_eq!(previous, Some(256));
    }

    #[test]
    fn rounded_frame_keeps_square_ring_on_straight_edges() {
        // Away from the corners the rounded model matches the square ring at the edge midpoint,
        // within the half-pixel offset of sampling pixel centres (max falloff slope ~7 per pixel).
        let (x, y) = (FRAME.right() + 3, FRAME.y + FRAME.height as i32 / 2);
        let rounded = keep_at_shape(FRAME, ROUND, true, x, y) as i32;
        let square = keep_at(FRAME, true, x, y) as i32;
        assert!((rounded - square).abs() <= 4, "rounded {rounded} vs square {square}");
    }

    #[test]
    fn keep_peaks_are_unchanged_and_focus_is_darker() {
        // Peaks are the same subtle values as before: 0.20 focused, 0.12 unfocused at the edge.
        let edge = |focused| keep_at(FRAME, focused, FRAME.right(), FRAME.y + 10) as f32 / 256.0;
        let t: f32 = 1.0 - 1.0 / SPREAD as f32;
        assert!((edge(true) - (1.0 - PEAK_FOCUSED * t * t)).abs() < 0.01);
        assert!((edge(false) - (1.0 - PEAK_UNFOCUSED * t * t)).abs() < 0.01);
        assert!(edge(true) < edge(false));
    }

    #[test]
    fn silhouette_is_symmetric_top_to_bottom_and_full_width_in_the_middle() {
        let area = shadow_rect(FRAME);
        let spans = shadow_spans_iter(area, 24).collect::<Vec<_>>();
        for (row, start, end) in &spans {
            let mirror = spans[(area.height - 1 - row) as usize];
            assert_eq!((start, end), (&mirror.1, &mirror.2), "row {row} not mirrored");
        }
        let mid = spans[area.height as usize / 2];
        assert_eq!((mid.1, mid.2), (0, area.width), "middle rows span the full width");
        assert!(spans[0].1 > 0, "top-left silhouette corner is rounded");
        assert_eq!(silhouette_radius(24), 24 + SPREAD as u32);
        assert_eq!(silhouette_radius(0), 0);
    }

    #[test]
    fn visible_shading_is_symmetric_on_all_four_sides() {
        let mid_x = FRAME.x + FRAME.width as i32 / 2;
        let mid_y = FRAME.y + FRAME.height as i32 / 2;
        for focused in [true, false] {
            for d in 1..=SPREAD {
                let right = shaded_at(FRAME, focused, 0, FRAME.right() + d - 1, mid_y);
                let left = shaded_at(FRAME, focused, 0, FRAME.x - d, mid_y);
                let bottom = shaded_at(FRAME, focused, 0, mid_x, FRAME.bottom() + d - 1);
                let top = shaded_at(FRAME, focused, 0, mid_x, FRAME.y - d);
                assert_eq!((right, left, bottom, top), (right, right, right, right), "distance {d}");
            }
        }
    }

    #[test]
    fn shade_is_darkest_at_the_edge_and_fades_outward() {
        let mut far = vec![200u8; shadow_rect(FRAME).width as usize * shadow_rect(FRAME).height as usize * 4];
        shade_shadow(FRAME, true, &mut far);
        let area = shadow_rect(FRAME);
        let at = |x: i32| ((FRAME.y + 5 - area.y) as usize * area.width as usize + (x - area.x) as usize) * 4;
        assert!(far[at(FRAME.right())] < far[at(FRAME.right() + SPREAD - 1)]);
        // The last ring column is SPREAD pixels out: the fade has reached zero there.
        assert_eq!(far[at(FRAME.right() + SPREAD - 1)], 200, "the outermost ring column keeps its colour");
    }

    #[test]
    fn focus_is_darker_and_mismatched_buffer_is_untouched() {
        let area = shadow_rect(FRAME);
        let mut focused = vec![200u8; area.width as usize * area.height as usize * 4];
        let mut unfocused = focused.clone();
        shade_shadow(FRAME, true, &mut focused);
        shade_shadow(FRAME, false, &mut unfocused);
        assert!(focused.iter().zip(&unfocused).all(|(f, u)| f <= u));
        let mut wrong = vec![7u8; 16];
        shade_shadow(FRAME, true, &mut wrong);
        assert!(wrong.iter().all(|&b| b == 7));
    }

    #[test]
    fn rounded_frame_notches_are_covered_by_the_silhouette() {
        let frame = Rect::new(100, 100, 200, 150);
        let area = shadow_rect(frame);
        let round = rounded_shadow_mask_rectangles(area, frame, 24);
        let contains = |x: i32, y: i32| {
            let (cx, cy) = ((x - area.x) as u32, (y - area.y) as u32);
            round.iter().any(|&(rx, ry, rw, rh)| cx >= rx && cx < rx + rw && cy >= ry && cy < ry + rh)
        };
        // Notch pixels: inside the frame rect, outside its 24px arc, at all four corners.
        for (x, y) in [
            (frame.x + 1, frame.y + 1),
            (frame.right() - 2, frame.y + 1),
            (frame.x + 1, frame.bottom() - 2),
            (frame.right() - 2, frame.bottom() - 2),
        ] {
            assert!(contains(x, y), "corner notch ({x},{y}) must show the cast");
        }
        // Far outside the rounded silhouette corner is not part of the cast.
        assert!(!contains(area.x, area.y));
        assert!(contains(frame.x - SPREAD, frame.y + 60), "the ring is present on the left");
        assert!(contains(frame.right() + SPREAD - 1, frame.y + 60), "the ring is present on the right");
    }

    #[test]
    fn rounded_interior_is_trimmed_out_of_the_shadow_window() {
        let frame = Rect::new(100, 100, 200, 150);
        let area = shadow_rect(frame);
        let round = rounded_shadow_mask_rectangles(area, frame, 24);
        let contains = |x: i32, y: i32| {
            let (cx, cy) = ((x - area.x) as u32, (y - area.y) as u32);
            round.iter().any(|&(rx, ry, rw, rh)| cx >= rx && cx < rx + rw && cy >= ry && cy < ry + rh)
        };
        // Every pixel whose centre lies inside the frame's rounded outline is outside the mask.
        let mut interior = 0;
        for y in frame.y..frame.bottom() {
            for x in frame.x..frame.right() {
                if rounded_signed_distance(frame, 24, x, y) <= 0.0 {
                    interior += 1;
                    assert!(!contains(x, y), "interior pixel ({x},{y}) must not be shaped");
                }
            }
        }
        assert!(interior > 0);
        // The ring just outside the outline stays shaped.
        assert!(contains(frame.x - 1, frame.y + 60));
        assert!(contains(frame.x + 60, frame.y - 1));
    }

    #[test]
    fn square_frame_casts_a_square_corner() {
        let frame = Rect::new(100, 100, 200, 150);
        let area = shadow_rect(frame);
        let spans = shadow_spans_iter(area, 0).collect::<Vec<_>>();
        assert!(spans.iter().all(|&(_, s, e)| s == 0 && e == area.width));
    }

    #[test]
    fn clip_is_gone_so_the_cast_is_a_single_silhouette() {
        // The silhouette keeps the interior rows: the frame covers them, and that keeps the mask
        // a single rounded rectangle rather than a ring with a cut-out.
        let area = shadow_rect(FRAME);
        let spans = shadow_spans_iter(area, 24).collect::<Vec<_>>();
        let inner = &spans[(SPREAD + 40) as usize];
        assert_eq!((inner.1, inner.2), (0, area.width));
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
    fn contains(rects: &[(u32, u32, u32, u32)], x: u32, y: u32) -> bool {
        rects
            .iter()
            .any(|&(rx, ry, rw, rh)| x >= rx && x < rx + rw && y >= ry && y < ry + rh)
    }
    #[test]
    fn cast_corner_follows_the_frame_radius() {
        let rect = Rect { x: 0, y: 0, width: 200, height: 120 };
        let area = |radius| shadow_spans_iter(rect, radius).map(|(_, s, e)| e - s).sum::<u32>();
        assert!(area(24) < area(RADIUS), "a 24px frame rounds its cast more than the legacy 6px");
        assert_eq!(shadow_spans(rect), shadow_spans_iter(rect, RADIUS).collect::<Vec<_>>());
    }
}

#[cfg(test)]
mod snapshot_base_tests {
    use super::*;

    #[test]
    fn desktop_read_covers_the_rounded_corner_notches() {
        // Regression: a read limited to the right/bottom cast left the top-right notch (inside the
        // frame rect) on the base colour, which shaded to a flat #151719 sliver.
        let frame = Rect { x: 90, y: 59, width: 484, height: 347 };
        let area = shadow_rect(frame);
        let (x, y, w, h) = snapshot_read_rect(area);
        // A notch pixel inside the frame rect and inside the shadow rect (rows start at frame.y + OFFSET).
        let notch = (frame.right() - 2 - area.x, frame.y + 8 - area.y);
        assert!(notch.0 >= x && notch.0 < x + w && notch.1 >= y && notch.1 < y + h);
        assert_eq!((x, y, w, h), (0, 0, area.width as i32, area.height as i32));
    }

    #[test]
    fn uncovered_snapshot_pixels_are_never_black() {
        let pixels = snapshot_base(7, 5);
        assert_eq!(pixels.len(), 7 * 5 * 4);
        for px in pixels.chunks_exact(4) {
            assert_ne!(&px[..3], &[0, 0, 0], "uncovered shadow pixel must not be black");
        }
    }
}

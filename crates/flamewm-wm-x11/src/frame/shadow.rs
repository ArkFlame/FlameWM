//! Win7-style frame shadow: pure strip geometry and pixel shading.
//!
//! No compositor exists, so a shadow cannot be alpha-blended by the server.
//! Instead each strip is shaded once from a root snapshot (`shade_strip`) and
//! held by the server as a window background. Strips are three thin windows
//! per frame: left, right and bottom. They sit under the frame and do not
//! overlap each other, so no pixel is darkened twice by the same shadow.
//! The X side lives in `wm/shadow.rs`.

use crate::geometry::Rect;

/// Side spread in pixels.
pub const SPREAD: i32 = 12;
/// Extra spread at the bottom, so the shadow is wider below than at the sides.
pub const BOTTOM_EXTRA: i32 = 4;
/// Downward offset of the shadow body.
pub const OFFSET_Y: i32 = 4;
/// Peak darkening (0..=1) at the frame edge for a focused frame.
pub const PEAK_FOCUSED: f32 = 0.62;
/// Peak darkening (0..=1) at the frame edge for an unfocused frame.
pub const PEAK_UNFOCUSED: f32 = 0.32;
/// Rows over which the side shadows fade in below the offset top edge.
const TOP_FADE_ROWS: i32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strip {
    Left,
    Right,
    Bottom,
}

pub const STRIPS: [Strip; 3] = [Strip::Left, Strip::Right, Strip::Bottom];

/// Root-space rect of one strip for a frame whose outer rect is `frame`.
/// Strips lie outside the frame and never overlap each other.
#[must_use]
pub fn strip_rect(strip: Strip, frame: Rect) -> Rect {
    let fx = frame.x;
    let fy = frame.y;
    let fw = frame.width as i32;
    let fh = frame.height as i32;
    let side_h = (fh - OFFSET_Y).max(0) as u32;
    let bottom_h = (OFFSET_Y + SPREAD + BOTTOM_EXTRA) as u32;
    match strip {
        Strip::Left => Rect {
            x: fx - SPREAD,
            y: fy + OFFSET_Y,
            width: SPREAD as u32,
            height: side_h,
        },
        Strip::Right => Rect {
            x: fx + fw,
            y: fy + OFFSET_Y,
            width: SPREAD as u32,
            height: side_h,
        },
        Strip::Bottom => Rect {
            x: fx - SPREAD,
            y: fy + fh,
            width: (fw + 2 * SPREAD) as u32,
            height: bottom_h,
        },
    }
}

/// Smooth falloff: 1 at distance 0, 0 at distance `reach`.
fn falloff(distance: i32, reach: i32) -> f32 {
    let t = 1.0 - distance as f32 / reach as f32;
    if t <= 0.0 { 0.0 } else { t * t }
}

/// Darkening factor (0..=1) for one pixel of `strip`. `col`/`row` are local to
/// the strip rect; `frame_w` is the frame width for the bottom strip's span.
fn alpha_at(strip: Strip, frame_w: i32, col: i32, row: i32) -> f32 {
    match strip {
        Strip::Left => falloff(SPREAD - col, SPREAD) * top_fade(row),
        Strip::Right => falloff(col + 1, SPREAD) * top_fade(row),
        Strip::Bottom => {
            // Body is offset down, so the first OFFSET_Y rows are inside it.
            let dy = if row < OFFSET_Y { 0 } else { row - OFFSET_Y + 1 };
            let dy_factor = if dy == 0 {
                1.0
            } else {
                falloff(dy, SPREAD + BOTTOM_EXTRA)
            };
            // Horizontal distance outside the frame span (0 under the frame).
            let dx = if col < SPREAD {
                SPREAD - col
            } else if col >= SPREAD + frame_w {
                col - (SPREAD + frame_w) + 1
            } else {
                0
            };
            let dx_factor = if dx == 0 { 1.0 } else { falloff(dx, SPREAD) };
            dy_factor * dx_factor
        }
    }
}

fn top_fade(row: i32) -> f32 {
    ((row + 1) as f32 / TOP_FADE_ROWS as f32).min(1.0)
}

/// Darken `pixels` (BGRX rows of `strip_rect(strip, frame)`) in place: each
/// pixel keeps `1 - alpha` of the snapshot colour. Pure, no allocation.
pub fn shade_strip(strip: Strip, frame: Rect, focused: bool, pixels: &mut [u8]) {
    let area = strip_rect(strip, frame);
    let (w, h) = (area.width as i32, area.height as i32);
    if w == 0 || h == 0 || pixels.len() != (w * h * 4) as usize {
        return;
    }
    let peak = if focused {
        PEAK_FOCUSED
    } else {
        PEAK_UNFOCUSED
    };
    let frame_w = frame.width as i32;
    for row in 0..h {
        for col in 0..w {
            let alpha = peak * alpha_at(strip, frame_w, col, row);
            let keep = (256.0 * (1.0 - alpha)) as u32;
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

    fn overlaps(a: Rect, b: Rect) -> bool {
        let (ax2, ay2) = (a.x + a.width as i32, a.y + a.height as i32);
        let (bx2, by2) = (b.x + b.width as i32, b.y + b.height as i32);
        a.x < bx2 && b.x < ax2 && a.y < by2 && b.y < ay2
    }

    #[test]
    fn strips_sit_outside_frame_and_are_disjoint() {
        let rects = STRIPS.map(|s| strip_rect(s, FRAME));
        for (i, a) in rects.iter().enumerate() {
            let frame_overlap = overlaps(*a, FRAME);
            assert!(!frame_overlap, "strip {i} covers the frame");
            for b in rects.iter().skip(i + 1) {
                assert!(!overlaps(*a, *b), "strips overlap");
            }
        }
    }

    #[test]
    fn side_strips_flank_frame_and_bottom_strip_sits_below() {
        let left = strip_rect(Strip::Left, FRAME);
        let right = strip_rect(Strip::Right, FRAME);
        let bottom = strip_rect(Strip::Bottom, FRAME);
        assert_eq!((left.x, left.width), (FRAME.x - SPREAD, SPREAD as u32));
        assert_eq!(right.x, FRAME.x + FRAME.width as i32);
        assert_eq!(bottom.y, FRAME.y + FRAME.height as i32);
        assert_eq!(bottom.x, FRAME.x - SPREAD);
        assert_eq!(bottom.width, FRAME.width + 2 * SPREAD as u32);
    }

    #[test]
    fn bottom_is_wider_than_sides() {
        let left = strip_rect(Strip::Left, FRAME);
        let bottom = strip_rect(Strip::Bottom, FRAME);
        assert!(bottom.height > left.width, "shadow must reach further down");
        assert_eq!(bottom.height as i32, OFFSET_Y + SPREAD + BOTTOM_EXTRA);
    }

    #[test]
    fn shade_darkens_near_edge_less_at_far_edge_and_focus_is_darker() {
        let area = strip_rect(Strip::Left, FRAME);
        let (w, h) = (area.width as usize, area.height as usize);
        let white = vec![255u8; w * h * 4];
        let shade = |focused| {
            let mut px = white.clone();
            shade_strip(Strip::Left, FRAME, focused, &mut px);
            px
        };
        let focused = shade(true);
        let unfocused = shade(false);
        // Row 20 is past the top fade. Column w-1 is at the frame edge.
        let at = |px: &[u8], col: usize| px[(20 * w + col) * 4];
        assert!(at(&focused, w - 1) < at(&white, w - 1));
        assert!(at(&focused, w - 1) < at(&unfocused, w - 1));
        assert!(at(&focused, 0) >= at(&focused, w - 1), "far edge is lighter");
        assert_eq!(at(&focused, 0), 255, "far edge is untouched");
    }

    #[test]
    fn bottom_body_under_offset_is_uniform_and_fades_out() {
        let area = strip_rect(Strip::Bottom, FRAME);
        let (w, h) = (area.width as usize, area.height as usize);
        let mut px = vec![255u8; w * h * 4];
        shade_strip(Strip::Bottom, FRAME, true, &mut px);
        let at = |row: usize, col: usize| px[(row * w + col) * 4];
        let mid = w / 2;
        assert_eq!(at(0, mid), at(OFFSET_Y as usize - 1, mid));
        assert!(at(0, mid) < 255, "body under the frame is shaded");
        assert_eq!(at(h - 1, mid), 255, "far bottom edge is untouched");
    }

    #[test]
    fn mismatched_buffer_is_left_untouched() {
        let mut px = vec![200u8; 8];
        shade_strip(Strip::Right, FRAME, true, &mut px);
        assert!(px.iter().all(|&b| b == 200));
    }
}

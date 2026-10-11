//! Frame shape: canonical rounded-mask spans for frame windows.
//!
//! Single formula owner is render-x11 (`external_rounded_row_inset`, float
//! rounded `floor(r - dx)` at pixel-row centers): full-width rows when the
//! radius is 0/1, or mid-span; corner insets otherwise. The radius policy
//! is owned by skin (`corner_radius`). Bounding-region assembly lives here
//! over safe `x11rb` rectangles; the live mask apply lives in `render-x11`'s
//! XShape bridge.

use flamewm_render_x11::external_rounded_row_inset;
use flamewm_skin::recipes::decoration::{DecorationStyle, corner_radius};

use x11rb::protocol::xproto::Rectangle;

use crate::geometry::Rect;

/// Canonical row spans (y, x_start, x_end_exclusive) for a rounded rect:
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
        let inset = external_rounded_row_inset(width, height, radius, row);
        let start = inset.min(width);
        let end = width.saturating_sub(inset).max(start);
        spans.push((row, start, end));
    }
    spans
}

/// Bounding rectangles for a frame outer: a single rect unless `floating`
/// (skin radius), otherwise the canonical rounded row spans.
#[must_use]
pub fn bounding_rectangles(outer: Rect, floating: bool, style: DecorationStyle) -> Vec<Rectangle> {
    let width = outer.width.min(u32::from(u16::MAX));
    let height = outer.height.min(u32::from(u16::MAX));
    let radius = corner_radius(floating, style);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t26_shape_spans_match_canonical_mask() {
        assert_eq!(
            rounded_mask_spans(8, 8, 4),
            vec![
                (0, 2, 6),
                (1, 0, 8),
                (2, 0, 8),
                (3, 0, 8),
                (4, 0, 8),
                (5, 0, 8),
                (6, 0, 8),
                (7, 2, 6),
            ]
        );
        assert_eq!(rounded_mask_spans(4, 2, 0), vec![(0, 0, 4), (1, 0, 4)]);
        assert!(rounded_mask_spans(0, 8, 4).is_empty());
    }
}

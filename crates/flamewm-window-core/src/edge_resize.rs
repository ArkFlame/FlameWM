//! Shared-edge resize between two adjacent tiles (edge-tile resize). Pure: given two tiles that
//! touch along `axis` and span the same cross extent, moves their shared edge and returns both new
//! rects. Returns `None` when the tiles are not adjacent, do not match on the cross axis, or when
//! either tile would shrink below `min_extent`.

use flamewm_api::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// The shared edge is vertical: `first` is left of `second`.
    Vertical,
    /// The shared edge is horizontal: `first` is above `second`.
    Horizontal,
}

/// Moves the edge shared by `first` (left or top) and `second` (right or bottom) to `edge`.
#[must_use]
pub fn move_shared_edge(
    first: Rect,
    second: Rect,
    axis: Axis,
    edge: i32,
    min_extent: i32,
) -> Option<(Rect, Rect)> {
    let min = min_extent.max(1);
    let (a_start, a_len, b_start, b_len, a_cross, b_cross) = match axis {
        Axis::Vertical => (
            first.x,
            first.width,
            second.x,
            second.width,
            (first.y, first.height),
            (second.y, second.height),
        ),
        Axis::Horizontal => (
            first.y,
            first.height,
            second.y,
            second.height,
            (first.x, first.width),
            (second.x, second.width),
        ),
    };
    if a_start + a_len != b_start || a_cross != b_cross {
        return None;
    }
    let new_a_len = edge - a_start;
    let new_b_len = b_start + b_len - edge;
    if new_a_len < min || new_b_len < min {
        return None;
    }
    let (mut a, mut b) = (first, second);
    match axis {
        Axis::Vertical => {
            a.width = new_a_len;
            b.x = edge;
            b.width = new_b_len;
        }
        Axis::Horizontal => {
            a.height = new_a_len;
            b.y = edge;
            b.height = new_b_len;
        }
    }
    Some((a, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, width: i32, height: i32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn vertical_edge_moves_both_halves_and_conserves_width() {
        let left = rect(0, 0, 500, 800);
        let right = rect(500, 0, 500, 800);
        assert_eq!(
            move_shared_edge(left, right, Axis::Vertical, 600, 100),
            Some((rect(0, 0, 600, 800), rect(600, 0, 400, 800)))
        );
    }

    #[test]
    fn horizontal_edge_moves_top_and_bottom_and_conserves_height() {
        let top = rect(0, 0, 800, 400);
        let bottom = rect(0, 400, 800, 400);
        assert_eq!(
            move_shared_edge(top, bottom, Axis::Horizontal, 300, 100),
            Some((rect(0, 0, 800, 300), rect(0, 300, 800, 500)))
        );
    }

    #[test]
    fn odd_extents_keep_the_total_span_exact() {
        let left = rect(10, 0, 333, 101);
        let right = rect(343, 0, 334, 101);
        let (a, b) = move_shared_edge(left, right, Axis::Vertical, 401, 1).unwrap();
        assert_eq!(a.x + a.width, b.x);
        assert_eq!(a.width + b.width, 333 + 334);
        assert_eq!(b.x + b.width, 343 + 334);
    }

    #[test]
    fn either_tile_below_minimum_is_refused() {
        let left = rect(0, 0, 500, 800);
        let right = rect(500, 0, 500, 800);
        assert_eq!(move_shared_edge(left, right, Axis::Vertical, 0, 100), None);
        assert_eq!(
            move_shared_edge(left, right, Axis::Vertical, 950, 100),
            None
        );
    }

    #[test]
    fn non_adjacent_or_mismatched_cross_extent_is_refused() {
        let left = rect(0, 0, 500, 800);
        let gapped = rect(520, 0, 480, 800);
        assert_eq!(move_shared_edge(left, gapped, Axis::Vertical, 600, 1), None);
        let quarter = rect(500, 0, 500, 400);
        assert_eq!(
            move_shared_edge(left, quarter, Axis::Vertical, 600, 1),
            None
        );
    }
}

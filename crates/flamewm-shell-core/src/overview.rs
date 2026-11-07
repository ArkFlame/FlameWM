//! Overview workspace grid geometry. The grid topology is owned by `flamewm_api::workspace`;
//! this module only divides a screen area into one thumbnail cell per workspace.

use flamewm_api::Rect;
use flamewm_api::workspace::{WORKSPACE_ROWS, grid_position, workspace_columns};

/// Thumbnail cells for `count` workspaces laid out in the two-row topology inside `area`.
///
/// Cell `index` is the rectangle at `grid_position(index, count)`. Cells are separated by `gap`
/// pixels and share `area` exactly; leftover pixels from integer division go to the last
/// column and row so the grid never overflows `area`.
#[must_use]
pub fn thumbnail_rects(count: usize, area: Rect, gap: i32) -> Vec<Rect> {
    let columns = workspace_columns(count) as i32;
    let rows = WORKSPACE_ROWS as i32;
    let gap = gap.max(0);
    let cell_width = ((area.width - gap * (columns - 1)) / columns).max(0);
    let cell_height = ((area.height - gap * (rows - 1)) / rows).max(0);
    (0..count)
        .filter_map(|index| grid_position(index, count))
        .map(|position| {
            let column = position.column as i32;
            let row = position.row as i32;
            Rect::new(
                area.x + column * (cell_width + gap),
                area.y + row * (cell_height + gap),
                cell_width,
                cell_height,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect::new(0, 0, 1000, 500);

    #[test]
    fn one_row_pair_of_cells_for_two_workspaces() {
        // Two workspaces -> one column, two rows.
        let cells = thumbnail_rects(2, AREA, 10);
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[0], Rect::new(0, 0, 1000, 245));
        assert_eq!(cells[1], Rect::new(0, 255, 1000, 245));
    }

    #[test]
    fn four_workspaces_read_as_two_by_two() {
        let cells = thumbnail_rects(4, AREA, 10);
        assert_eq!(cells[0], Rect::new(0, 0, 495, 245));
        assert_eq!(cells[1], Rect::new(505, 0, 495, 245));
        assert_eq!(cells[2], Rect::new(0, 255, 495, 245));
        assert_eq!(cells[3], Rect::new(505, 255, 495, 245));
    }

    #[test]
    fn odd_count_wraps_last_workspace_to_lower_row() {
        // Three workspaces: two columns; index 2 wraps to row 1, column 0.
        let cells = thumbnail_rects(3, AREA, 0);
        assert_eq!(cells.len(), 3);
        assert_eq!(cells[2], Rect::new(0, 250, 500, 250));
    }

    #[test]
    fn cells_stay_inside_area_for_max_workspaces() {
        let area = Rect::new(7, 9, 1001, 503);
        let cells = thumbnail_rects(flamewm_api::workspace::MAX_WORKSPACES, area, 6);
        assert_eq!(cells.len(), flamewm_api::workspace::MAX_WORKSPACES);
        for cell in &cells {
            assert!(cell.is_valid());
            assert!(cell.x >= area.x && cell.y >= area.y);
            assert!(cell.x + cell.width <= area.x + area.width);
            assert!(cell.y + cell.height <= area.y + area.height);
        }
    }

    #[test]
    fn zero_workspaces_yield_no_cells() {
        assert!(thumbnail_rects(0, AREA, 10).is_empty());
    }
}

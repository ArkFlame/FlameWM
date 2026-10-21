use std::collections::{BTreeMap, BTreeSet};

use flamewm_api::{Point, Rect};

pub const BASE_CELL_LOGICAL: i32 = 96;
pub const MIN_CELL_PHYSICAL: i32 = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridConfig {
    pub work_area: Rect,
    /// Output UI scale from the display configuration.
    pub scale_percent: u16,
    /// User preference for desktop icon/label size. Always 50..=200.
    pub entry_scale_percent: u16,
    pub cell_width: i32,
    pub cell_height: i32,
    pub columns: i32,
    pub rows: i32,
}

impl GridConfig {
    #[must_use]
    pub fn compute(work_area: Rect, scale_percent: u16) -> Self {
        Self::compute_with_entry_scale(work_area, scale_percent, 100)
    }

    /// Output scale and the desktop entry preference multiply: cell spacing grows
    /// with the icons and labels so neighbouring entries keep their own space.
    #[must_use]
    pub fn compute_with_entry_scale(
        work_area: Rect,
        scale_percent: u16,
        entry_scale_percent: u16,
    ) -> Self {
        let scale = if (100..=200).contains(&scale_percent) && scale_percent % 5 == 0 {
            scale_percent
        } else {
            100
        };
        let entry_scale = if (50..=200).contains(&entry_scale_percent) {
            entry_scale_percent
        } else {
            100
        };
        let numerator = i64::from(BASE_CELL_LOGICAL) * i64::from(scale) * i64::from(entry_scale);
        let cell = ((numerator + 5_000) / 10_000) as i32;
        let cell = cell.max(MIN_CELL_PHYSICAL);
        Self {
            work_area,
            scale_percent: scale,
            entry_scale_percent: entry_scale,
            cell_width: cell,
            cell_height: cell,
            columns: (work_area.width / cell).max(1),
            rows: (work_area.height / cell).max(1),
        }
    }

    /// Combined output × entry scale used for glyph, label, item box, and previews.
    #[must_use]
    pub const fn visual_scale_percent(self) -> u16 {
        ((self.scale_percent as u32 * self.entry_scale_percent as u32) / 100) as u16
    }

    #[must_use]
    pub const fn cell_to_pixel(self, cell: Cell) -> Point {
        Point::new(
            self.work_area.x + cell.column * self.cell_width,
            self.work_area.y + cell.row * self.cell_height,
        )
    }

    #[must_use]
    pub fn pixel_to_cell(self, point: Point) -> Cell {
        self.clamp_cell(Cell::new(
            (point.x - self.work_area.x).div_euclid(self.cell_width),
            (point.y - self.work_area.y).div_euclid(self.cell_height),
        ))
    }

    #[must_use]
    pub const fn clamp_cell(self, cell: Cell) -> Cell {
        Cell::new(
            if cell.column < 0 {
                0
            } else if cell.column >= self.columns {
                self.columns - 1
            } else {
                cell.column
            },
            if cell.row < 0 {
                0
            } else if cell.row >= self.rows {
                self.rows - 1
            } else {
                cell.row
            },
        )
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cell {
    pub column: i32,
    pub row: i32,
}

impl Cell {
    #[must_use]
    pub const fn new(column: i32, row: i32) -> Self {
        Self { column, row }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DragItem {
    pub id: String,
    pub cell: Cell,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupDragError {
    EmptySelection,
    InvalidGrid,
    InternalCollision,
    Occupied(Cell),
}

/// Move a selected group as one transaction. Relative offsets are preserved, the *whole* bounding
/// rectangle is clamped into the grid, collision with unselected cells rejects the entire move.
pub fn group_drag_transaction(
    selected: &[DragItem],
    unselected_occupied: &BTreeSet<Cell>,
    delta: Cell,
    grid: GridConfig,
) -> Result<BTreeMap<String, Cell>, GroupDragError> {
    if selected.is_empty() {
        return Err(GroupDragError::EmptySelection);
    }
    if grid.columns <= 0 || grid.rows <= 0 {
        return Err(GroupDragError::InvalidGrid);
    }

    let first = &selected[0];
    let mut min_column = first.cell.column;
    let mut max_column = first.cell.column;
    let mut min_row = first.cell.row;
    let mut max_row = first.cell.row;
    for item in &selected[1..] {
        min_column = min_column.min(item.cell.column);
        max_column = max_column.max(item.cell.column);
        min_row = min_row.min(item.cell.row);
        max_row = max_row.max(item.cell.row);
    }

    let mut d_column = delta.column;
    let mut d_row = delta.row;
    if min_column + d_column < 0 {
        d_column = -min_column;
    }
    if max_column + d_column >= grid.columns {
        d_column = grid.columns - 1 - max_column;
    }
    if min_row + d_row < 0 {
        d_row = -min_row;
    }
    if max_row + d_row >= grid.rows {
        d_row = grid.rows - 1 - max_row;
    }

    let mut result = BTreeMap::new();
    let mut result_cells = BTreeSet::new();
    for item in selected {
        let cell = Cell::new(item.cell.column + d_column, item.cell.row + d_row);
        if !result_cells.insert(cell) {
            return Err(GroupDragError::InternalCollision);
        }
        if unselected_occupied.contains(&cell) {
            return Err(GroupDragError::Occupied(cell));
        }
        result.insert(item.id.clone(), cell);
    }
    Ok(result)
}

/// Pixel drag threshold before a pointer motion becomes a cell drag.
pub const DRAG_THRESHOLD_PX: f32 = 5.0;

/// Renderer-neutral pointer displacement in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointerDelta {
    pub dx: f32,
    pub dy: f32,
}

impl PointerDelta {
    #[must_use]
    pub const fn new(dx: f32, dy: f32) -> Self {
        Self { dx, dy }
    }
}

/// Convert a pixel displacement into whole-cell drag steps (nearest-cell rounding).
#[must_use]
pub fn drag_cell_delta(pixel_dx: f32, pixel_dy: f32, cell_w: i32, cell_h: i32) -> Cell {
    let columns = if cell_w > 0 {
        (pixel_dx / cell_w as f32).round() as i32
    } else {
        0
    };
    let rows = if cell_h > 0 {
        (pixel_dy / cell_h as f32).round() as i32
    } else {
        0
    };
    Cell::new(columns, rows)
}

/// True once pointer motion reaches the drag threshold.
#[must_use]
pub fn threshold_passed(dx: f32, dy: f32) -> bool {
    dx.hypot(dy) >= DRAG_THRESHOLD_PX
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_changes_grid_cell_not_authority() {
        let grid = GridConfig::compute(Rect::new(0, 0, 1920, 1080), 150);
        assert_eq!(grid.cell_width, 144);
        assert_eq!(grid.columns, 13);
    }

    #[test]
    fn entry_scale_changes_cell_spacing_and_visual_scale_not_output_scale() {
        let area = Rect::new(0, 0, 1920, 1080);
        let grid = GridConfig::compute_with_entry_scale(area, 150, 120);
        assert_eq!(grid.scale_percent, 150);
        assert_eq!(grid.entry_scale_percent, 120);
        assert_eq!(grid.visual_scale_percent(), 180);
        assert_eq!(grid.cell_width, 173);
        assert_eq!(grid.columns, 11);
    }

    #[test]
    fn invalid_or_out_of_range_entry_scale_falls_back_to_default() {
        let area = Rect::new(0, 0, 1920, 1080);
        for entry in [0, 49, 201, u16::MAX] {
            let grid = GridConfig::compute_with_entry_scale(area, 100, entry);
            assert_eq!(grid.entry_scale_percent, 100);
            assert_eq!(grid.cell_width, 96);
            assert_eq!(grid.visual_scale_percent(), 100);
        }
    }

    #[test]
    fn five_percent_scale_steps_preserve_coordinate_roundtrip() {
        let area = Rect::new(-1920, 24, 1920, 1056);
        for scale in (100..=200).step_by(5) {
            let grid = GridConfig::compute(area, scale);
            assert_eq!(grid.scale_percent, scale);
            assert_eq!(grid.cell_width, ((96 * u32::from(scale) + 50) / 100) as i32);
            let cell = grid.clamp_cell(Cell::new(2, 3));
            assert_eq!(grid.pixel_to_cell(grid.cell_to_pixel(cell)), cell);
        }
        for invalid in [0, 99, 101, 199, 201, u16::MAX] {
            assert_eq!(GridConfig::compute(area, invalid).scale_percent, 100);
        }
    }

    #[test]
    fn pixel_to_cell_handles_negative_monitor_origin() {
        let grid = GridConfig::compute(Rect::new(-1920, 0, 1920, 1080), 100);
        assert_eq!(grid.pixel_to_cell(Point::new(-1910, 10)), Cell::new(0, 0));
    }

    #[test]
    fn group_drag_clamps_whole_group_and_preserves_offsets() {
        let grid = GridConfig::compute(Rect::new(0, 0, 480, 480), 100);
        let selected = vec![
            DragItem {
                id: "a".to_owned(),
                cell: Cell::new(3, 2),
            },
            DragItem {
                id: "b".to_owned(),
                cell: Cell::new(4, 2),
            },
        ];
        let moved = group_drag_transaction(&selected, &BTreeSet::new(), Cell::new(7, 0), grid)
            .expect("group clamps into bounds");
        assert_eq!(moved["a"], Cell::new(3, 2));
        assert_eq!(moved["b"], Cell::new(4, 2));
    }

    #[test]
    fn group_collision_rejects_entire_transaction() {
        let grid = GridConfig::compute(Rect::new(0, 0, 960, 960), 100);
        let selected = vec![DragItem {
            id: "a".to_owned(),
            cell: Cell::new(0, 0),
        }];
        let occupied = BTreeSet::from([Cell::new(1, 0)]);
        assert_eq!(
            group_drag_transaction(&selected, &occupied, Cell::new(1, 0), grid),
            Err(GroupDragError::Occupied(Cell::new(1, 0)))
        );
    }

    #[test]
    fn drag_cell_delta_rounds_to_nearest_cell() {
        assert_eq!(drag_cell_delta(96.0, 48.0, 96, 96), Cell::new(1, 1));
        assert_eq!(drag_cell_delta(50.0, -140.0, 96, 96), Cell::new(1, -1));
        assert_eq!(drag_cell_delta(0.0, 0.0, 96, 96), Cell::new(0, 0));
    }

    #[test]
    fn drag_threshold_gates_pointer_motion() {
        assert_eq!(DRAG_THRESHOLD_PX, 5.0);
        assert!(!threshold_passed(3.0, 4.0 - 0.1));
        assert!(threshold_passed(3.0, 4.0));
        assert!(threshold_passed(-6.0, 0.0));
    }
}

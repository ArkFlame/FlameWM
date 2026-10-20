use crate::{ErrorCode, FlameError, FlameResult};

/// Product maximum for workspace count; shared by planner, WM clamp and env override.
pub const MAX_WORKSPACES: usize = 18;

/// Key of the persisted workspace count, written by the platform workspace service.
pub const WORKSPACE_COUNT_KEY: &str = "workspaces.count";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSnapshot {
    pub revision: u64,
    pub count: usize,
    pub active_index: usize,
    pub last_index: Option<usize>,
    pub names: Vec<String>,
}

impl WorkspaceSnapshot {
    pub fn validate(&self) -> FlameResult<()> {
        if self.count == 0 {
            return Err(FlameError::invalid("workspace count must be at least one"));
        }
        if self.active_index >= self.count {
            return Err(FlameError::invalid(
                "active workspace is outside the topology",
            ));
        }
        if self.last_index.is_some_and(|index| index >= self.count) {
            return Err(FlameError::invalid(
                "last workspace is outside the topology",
            ));
        }
        if !self.names.is_empty() && self.names.len() != self.count {
            return Err(FlameError::invalid(
                "workspace names must be empty or match count",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspacePlanKind {
    Activate,
    Insert,
    Remove,
}

/// Complete engine-neutral workspace mutation plan.
///
/// `old_to_new[old_index]` gives the destination index for every old workspace. For removal,
/// windows from the removed workspace migrate through this same mapping. For insertion, all old
/// indices remain represented and the inserted workspace has no old source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspacePlan {
    pub kind: WorkspacePlanKind,
    pub expected_revision: u64,
    pub new_revision: u64,
    pub old_count: usize,
    pub new_count: usize,
    pub requested_index: usize,
    pub old_to_new: Vec<usize>,
    pub new_active: usize,
    pub new_last: Option<usize>,
    pub new_names: Vec<String>,
}

impl WorkspacePlan {
    pub fn validate(&self) -> FlameResult<()> {
        if self.old_count == 0 || self.new_count == 0 {
            return Err(FlameError::invalid(
                "workspace plan cannot contain zero workspaces",
            ));
        }
        if self.old_to_new.len() != self.old_count {
            return Err(FlameError::invalid(
                "workspace mapping length must equal old count",
            ));
        }
        if self.old_to_new.iter().any(|&index| index >= self.new_count) {
            return Err(FlameError::invalid(
                "workspace mapping points outside new topology",
            ));
        }
        if self.new_active >= self.new_count {
            return Err(FlameError::invalid("planned active workspace is invalid"));
        }
        if self.new_last.is_some_and(|index| index >= self.new_count) {
            return Err(FlameError::invalid("planned last workspace is invalid"));
        }
        if self.new_names.len() != self.new_count {
            return Err(FlameError::invalid("planned names must match new count"));
        }
        if self.new_revision <= self.expected_revision {
            return Err(FlameError::new(
                ErrorCode::StaleRevision,
                "new workspace revision must advance",
            ));
        }
        Ok(())
    }
}

/// Rows in the product workspace topology.
///
/// Every workspace count is laid out on exactly two rows; the column count follows the live
/// workspace count, so four workspaces read `1 2 / 3 4` and six read `1 2 3 / 4 5 6`. Left/right
/// stay inside the current row, up/down switch rows. This module is the single owner of that
/// grid: the WM resolves arrow shortcuts with it, the shell pager renders with it, and the
/// `_NET_DESKTOP_VIEWPORT` publication exposes it to external EWMH pagers.
pub const WORKSPACE_ROWS: usize = 2;

/// Columns the two-row topology needs for `count` workspaces (`ceil(count / 2)`, at least 1).
#[must_use]
pub fn workspace_columns(count: usize) -> usize {
    if count == 0 {
        1
    } else {
        count.div_ceil(WORKSPACE_ROWS)
    }
}

/// Cell of the two-row topology.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridPosition {
    pub row: usize,
    pub column: usize,
}

/// Row-major cell of `index`; `None` when `index` is outside `count`.
#[must_use]
pub fn grid_position(index: usize, count: usize) -> Option<GridPosition> {
    if index >= count {
        return None;
    }
    let columns = workspace_columns(count);
    Some(GridPosition {
        row: index / columns,
        column: index % columns,
    })
}

/// Absolute index of cell `(row, column)`; `None` for a row outside the topology or an empty cell
/// (the lower row of a column that only holds an upper workspace).
#[must_use]
pub fn grid_index(row: usize, column: usize, count: usize) -> Option<usize> {
    if row >= WORKSPACE_ROWS || column >= workspace_columns(count) {
        return None;
    }
    let index = row * workspace_columns(count) + column;
    (index < count).then_some(index)
}

/// Direction of a workspace navigation shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridDirection {
    Left,
    Right,
    Up,
    Down,
}

/// Neighbour of `index` in `direction`.
///
/// Left/right move inside the current row, up/down move to the same column of the other row.
/// Moves never wrap and never cross rows horizontally, so a cell on the row edge or a lower-row
/// cell with no cell below it returns `None` (a deliberate no-op, not an error).
#[must_use]
pub fn grid_neighbor(index: usize, count: usize, direction: GridDirection) -> Option<usize> {
    let position = grid_position(index, count)?;
    let (row, column) = match direction {
        GridDirection::Left => (position.row, position.column.checked_sub(1)?),
        GridDirection::Right => (position.row, position.column + 1),
        GridDirection::Up => (position.row.checked_sub(1)?, position.column),
        GridDirection::Down => (position.row + 1, position.column),
    };
    grid_index(row, column, count)
}

/// `_NET_WM_DESKTOP` value a window must carry after `plan`, or `None` when unchanged.
/// Sticky (`0xFFFF_FFFF`) and out-of-range values are left alone.
#[must_use]
pub fn remapped_desktop(plan: &WorkspacePlan, desktop: u32) -> Option<u32> {
    let old = usize::try_from(desktop).ok()?;
    let new = u32::try_from(*plan.old_to_new.get(old)?).ok()?;
    (new != desktop).then_some(new)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(kind: WorkspacePlanKind, old_to_new: Vec<usize>, new_count: usize) -> WorkspacePlan {
        WorkspacePlan {
            kind,
            expected_revision: 1,
            new_revision: 2,
            old_count: old_to_new.len(),
            new_count,
            requested_index: 0,
            old_to_new,
            new_active: 0,
            new_last: None,
            new_names: Vec::new(),
        }
    }

    #[test]
    fn remapped_desktop_follows_insert_mapping() {
        let plan = plan(WorkspacePlanKind::Insert, vec![0, 2, 3], 4);
        assert_eq!(remapped_desktop(&plan, 0), None);
        assert_eq!(remapped_desktop(&plan, 1), Some(2));
        assert_eq!(remapped_desktop(&plan, 2), Some(3));
    }

    #[test]
    fn remapped_desktop_moves_removed_desktop_to_survivor_and_keeps_sticky() {
        let plan = plan(WorkspacePlanKind::Remove, vec![0, 1, 1, 2], 3);
        assert_eq!(remapped_desktop(&plan, 0), None);
        assert_eq!(remapped_desktop(&plan, 2), Some(1));
        assert_eq!(remapped_desktop(&plan, 3), Some(2));
        assert_eq!(remapped_desktop(&plan, u32::MAX), None);
    }

    fn cell(row: usize, column: usize) -> GridPosition {
        GridPosition { row, column }
    }

    #[test]
    fn four_workspaces_fill_two_rows_of_two_columns() {
        assert_eq!(workspace_columns(4), 2);
        assert_eq!(grid_position(0, 4), Some(cell(0, 0)));
        assert_eq!(grid_position(1, 4), Some(cell(0, 1)));
        assert_eq!(grid_position(2, 4), Some(cell(1, 0)));
        assert_eq!(grid_position(3, 4), Some(cell(1, 1)));
    }

    #[test]
    fn six_workspaces_fill_two_rows_of_three_columns() {
        assert_eq!(workspace_columns(6), 3);
        assert_eq!(grid_position(2, 6), Some(cell(0, 2)));
        assert_eq!(grid_position(5, 6), Some(cell(1, 2)));
    }

    #[test]
    fn horizontal_moves_stay_inside_the_row() {
        // Row 0 of a six-workspace grid is 1 2 3; the row edge is a no-op, never a row jump.
        assert_eq!(grid_neighbor(1, 6, GridDirection::Left), Some(0));
        assert_eq!(grid_neighbor(1, 6, GridDirection::Right), Some(2));
        assert_eq!(grid_neighbor(2, 6, GridDirection::Right), None);
        assert_eq!(grid_neighbor(0, 6, GridDirection::Left), None);
        // Row 1 is 4 5 6 and behaves the same.
        assert_eq!(grid_neighbor(4, 6, GridDirection::Left), Some(3));
        assert_eq!(grid_neighbor(4, 6, GridDirection::Right), Some(5));
        assert_eq!(grid_neighbor(3, 6, GridDirection::Left), None);
        assert_eq!(grid_neighbor(5, 6, GridDirection::Right), None);
    }

    #[test]
    fn vertical_moves_switch_rows_in_the_same_column() {
        assert_eq!(grid_neighbor(2, 6, GridDirection::Down), Some(5));
        assert_eq!(grid_neighbor(5, 6, GridDirection::Up), Some(2));
        assert_eq!(grid_neighbor(0, 6, GridDirection::Up), None);
        assert_eq!(grid_neighbor(3, 6, GridDirection::Down), None);
    }

    #[test]
    fn odd_tail_column_has_no_lower_neighbour() {
        // Five workspaces read 1 2 3 / 4 5; nothing sits below 3.
        assert_eq!(workspace_columns(5), 3);
        assert_eq!(grid_index(1, 2, 5), None);
        assert_eq!(grid_neighbor(2, 5, GridDirection::Down), None);
        assert_eq!(grid_neighbor(2, 5, GridDirection::Up), None);
        assert_eq!(grid_neighbor(2, 5, GridDirection::Left), Some(1));
    }

    #[test]
    fn single_and_pair_topologies_keep_vertical_navigation_only() {
        assert_eq!(workspace_columns(1), 1);
        assert_eq!(grid_neighbor(0, 1, GridDirection::Down), None);
        assert_eq!(grid_neighbor(0, 1, GridDirection::Right), None);
        assert_eq!(grid_neighbor(0, 2, GridDirection::Down), Some(1));
        assert_eq!(grid_neighbor(1, 2, GridDirection::Up), Some(0));
        assert_eq!(grid_neighbor(0, 2, GridDirection::Right), None);
    }

    #[test]
    fn out_of_range_indexes_have_no_neighbour() {
        assert_eq!(grid_position(4, 4), None);
        assert_eq!(grid_neighbor(4, 4, GridDirection::Left), None);
        assert_eq!(grid_index(2, 0, 18), None);
        assert_eq!(grid_index(0, 9, 18), None);
    }
}

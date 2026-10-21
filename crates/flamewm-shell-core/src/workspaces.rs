use flamewm_api::workspace::{
    GridPosition, MAX_WORKSPACES, WORKSPACE_ROWS, WorkspaceSnapshot, grid_index, grid_position,
    workspace_columns,
};

#[must_use]
pub fn workspace_labels(count: usize) -> Vec<String> {
    (1..=count).map(|index| index.to_string()).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceView {
    pub index: usize,
    /// Visible pager label. Always numeric `index + 1` per the numeric pager
    /// caller contract (shell `runtime.rs` maps pager cell `workspace-k` back to
    /// the workspace index of cell `k` through [`cell_index`]).
    /// Protocol names never leak here; they are preserved in [`WorkspaceView::name`].
    pub label: String,
    /// Preserved protocol name from the snapshot (`""` when unnamed).
    pub name: String,
    pub active: bool,
    /// Row of the two-row product topology ([`flamewm_api::workspace::grid_position`]).
    pub row: usize,
    /// Column of the two-row product topology; the column count follows the
    /// live workspace count, so four workspaces read `1 2 / 3 4`.
    pub column: usize,
}

/// Pager cell width in CSS px (matches `.workspace-button` in ui/shell/flamewm.css).
pub const PAGER_CELL_WIDTH: f32 = 22.0;
/// Pager cell height in CSS px (matches `.workspace-button`).
pub const PAGER_CELL_HEIGHT: f32 = 16.0;
/// Gap between the two stacked cells of one column and between neighbour columns.
pub const PAGER_CELL_GAP: f32 = 1.0;
/// `.workspace-pager` padding as `(top/bottom, left/right)` CSS px.
pub const PAGER_PADDING: (f32, f32) = (4.0, 3.0);
/// `.workspace-pager` height in CSS px.
pub const PAGER_HEIGHT: f32 = 44.0;
/// Pager columns at the product maximum workspace count.
pub const PAGER_COLUMNS: usize = MAX_WORKSPACES.div_ceil(WORKSPACE_ROWS);
/// Pager cells at the product maximum workspace count.
pub const PAGER_CELLS: usize = PAGER_COLUMNS * WORKSPACE_ROWS;

/// Columns the pager shows for `count` workspaces. The pager draws the same
/// two-row topology the WM navigates, so it only needs the live count.
#[must_use]
pub fn pager_columns(count: usize) -> usize {
    workspace_columns(count)
}

/// Pager box width for `columns` visible columns (51px at the compiled
/// two-column default, 212px at the product maximum).
#[must_use]
pub fn pager_width(columns: usize) -> f32 {
    2.0 * PAGER_PADDING.1
        + columns as f32 * PAGER_CELL_WIDTH
        + columns.saturating_sub(1) as f32 * PAGER_CELL_GAP
}

/// Left edge of pager column `column` (0-based) inside the pager content box.
#[must_use]
pub fn cell_left(column: usize) -> f32 {
    column as f32 * (PAGER_CELL_WIDTH + PAGER_CELL_GAP)
}

/// Top edge of row `row` (0-based) inside one pager column.
#[must_use]
pub fn cell_top(row: usize) -> f32 {
    row as f32 * (PAGER_CELL_HEIGHT + PAGER_CELL_GAP)
}

/// Pager cell of workspace `index` in a `count` workspace topology.
///
/// Cells are column-major (`cell = 1` is the upper cell of the first column,
/// `cell = 2` its lower cell), so the markup column that holds a workspace is
/// `(cell - 1) / 2 + 1`. `None` when `index` is outside `count`.
#[must_use]
pub fn pager_cell(index: usize, count: usize) -> Option<usize> {
    let position = grid_position(index, count)?;
    Some(position.column * WORKSPACE_ROWS + position.row + 1)
}

/// Topology cell of pager cell id `cell` (1-based). `None` outside `1..=PAGER_CELLS`.
#[must_use]
pub fn cell_position(cell: usize) -> Option<GridPosition> {
    let offset = cell.checked_sub(1)?;
    if cell > PAGER_CELLS {
        return None;
    }
    Some(GridPosition {
        row: offset % WORKSPACE_ROWS,
        column: offset / WORKSPACE_ROWS,
    })
}

/// Workspace index of pager cell `cell`, or `None` for an empty cell (the lower
/// cell of a column whose upper workspace is the odd tail) or a cell outside `count`.
#[must_use]
pub fn cell_index(cell: usize, count: usize) -> Option<usize> {
    let position = cell_position(cell)?;
    grid_index(position.row, position.column, count)
}

#[must_use]
pub fn project(snapshot: Option<&WorkspaceSnapshot>) -> Vec<WorkspaceView> {
    let Some(snapshot) = snapshot else {
        return Vec::new();
    };
    if snapshot.count < 2 {
        return Vec::new();
    }
    let names = if snapshot.names.is_empty() || snapshot.names.len() != snapshot.count {
        vec![String::new(); snapshot.count]
    } else {
        snapshot.names.clone()
    };
    let labels = workspace_labels(snapshot.count);
    names
        .into_iter()
        .zip(labels)
        .enumerate()
        .map(|(index, (name, label))| {
            let position =
                grid_position(index, snapshot.count).unwrap_or(GridPosition { row: 0, column: 0 });
            WorkspaceView {
                index,
                label,
                name,
                active: index == snapshot.active_index,
                row: position.row,
                column: position.column,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(count: usize, active_index: usize) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            revision: 1,
            count,
            active_index,
            last_index: None,
            names: Vec::new(),
        }
    }

    #[test]
    fn named_workspace_compact_labels_are_numeric_index_plus_one() {
        assert_eq!(workspace_labels(4), vec!["1", "2", "3", "4"]);
    }

    // REGRESSION: the pager box must reproduce the compiled `.workspace-pager`
    // width exactly, and grow one 23px column per extra column.
    #[test]
    fn pager_width_matches_the_compiled_stylesheet() {
        assert_eq!(pager_width(2), 51.0);
        assert_eq!(pager_width(3), 74.0);
        assert_eq!(pager_width(9), 212.0);
        assert_eq!(PAGER_COLUMNS, 9);
        assert_eq!(PAGER_CELLS, 18);
    }

    #[test]
    fn cell_offsets_match_the_column_box_metrics() {
        assert_eq!(cell_left(0), 0.0);
        assert_eq!(cell_left(1), 23.0);
        assert_eq!(cell_left(2), 46.0);
        assert_eq!(cell_top(0), 0.0);
        assert_eq!(cell_top(1), 17.0);
    }

    // Four workspaces read `1 2 / 3 4`: the left column holds cells 1/2, the
    // right column cells 3/4 (column-major cell ids, row-major workspaces).
    #[test]
    fn four_workspaces_fill_two_cells_per_column() {
        assert_eq!(pager_cell(0, 4), Some(1));
        assert_eq!(pager_cell(1, 4), Some(3));
        assert_eq!(pager_cell(2, 4), Some(2));
        assert_eq!(pager_cell(3, 4), Some(4));
        assert_eq!(cell_position(1), Some(GridPosition { row: 0, column: 0 }));
        assert_eq!(cell_position(2), Some(GridPosition { row: 1, column: 0 }));
        assert_eq!(cell_position(3), Some(GridPosition { row: 0, column: 1 }));
    }

    #[test]
    fn cell_and_index_round_trip_for_every_product_count() {
        for count in 1..=MAX_WORKSPACES {
            for index in 0..count {
                let cell = pager_cell(index, count).expect("index inside count has a cell");
                assert!(cell <= PAGER_CELLS, "cell {cell} outside the pager");
                assert_eq!(cell_index(cell, count), Some(index), "count {count}");
            }
            // Every cell above the count is an empty cell, never a workspace.
            for cell in count + 1..=PAGER_CELLS {
                assert_eq!(cell_index(cell, count), None, "cell {cell} count {count}");
            }
        }
        assert_eq!(pager_cell(18, 18), None);
    }

    #[test]
    fn empty_tail_cells_have_no_index() {
        assert_eq!(cell_index(18, 4), None);
        assert_eq!(cell_index(0, 4), None);
        assert_eq!(cell_index(19, 18), None);
    }

    // The view column follows the live count (row-major), so six workspaces
    // read `1 2 3 / 4 5 6` rather than one flat page of two columns.
    #[test]
    fn six_workspaces_read_two_rows_of_three() {
        let views = project(Some(&snapshot(6, 5)));
        let shape: Vec<(usize, usize)> = views.iter().map(|view| (view.row, view.column)).collect();
        assert_eq!(shape, vec![(0, 0), (0, 1), (0, 2), (1, 0), (1, 1), (1, 2)]);
        assert!(views[5].active);
        assert_eq!(pager_cell(5, 6), Some(6));
    }
}

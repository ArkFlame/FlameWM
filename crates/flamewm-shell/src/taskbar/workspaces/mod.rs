use super::intent::TaskbarIntent;
use flamewm_api::workspace::WorkspaceSnapshot;
use flamewm_control_core::ControlRequest;

pub use flamewm_shell_core::workspaces::{
    cell_index, cell_left, pager_cell, pager_columns, pager_width, project as core_project,
    workspace_labels, WorkspaceView, PAGER_CELLS, PAGER_COLUMNS, PAGER_HEIGHT,
};

/// Pager projection with the J07 static shell span (`shell.pager.project`).
/// Single owner: wraps the core workspace projection; no second pager.
#[must_use]
pub fn project(snapshot: Option<&WorkspaceSnapshot>) -> Vec<WorkspaceView> {
    let _span = crate::runtime::shell_span("shell.pager.project").start();
    core_project(snapshot)
}

#[must_use]
pub fn intent_for_click(
    snapshot: Option<&WorkspaceSnapshot>,
    index: usize,
) -> Option<TaskbarIntent> {
    snapshot
        .filter(|snapshot| index < snapshot.count)
        .map(|_| TaskbarIntent::ActivateWorkspace(index))
}

#[must_use]
pub fn control_request_for_click(
    snapshot: Option<&WorkspaceSnapshot>,
    index: usize,
) -> Option<ControlRequest> {
    snapshot
        .filter(|snapshot| index < snapshot.count)
        .map(|snapshot| ControlRequest::ActivateWorkspace {
            index,
            expected_revision: snapshot.revision,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_shell_core::workspaces as core;

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
    fn single_workspace_projects_empty() {
        let snap = snapshot(1, 0);
        assert!(project(Some(&snap)).is_empty());
    }

    // Six workspaces read `1 2 3 / 4 5 6`: the pager draws exactly the cells
    // the WM navigates, so workspace 4 sits in the lower row of column 1.
    #[test]
    fn six_workspaces_project_two_rows_of_three_columns() {
        let snap = snapshot(6, 3);
        let views = project(Some(&snap));
        assert_eq!(views.len(), 6);
        assert_eq!(views[2].column, 2);
        assert_eq!(views[3].row, 1);
        assert_eq!(views[3].column, 0);
        assert!(views[3].active);
        assert_eq!(pager_columns(snap.count), 3);
        assert_eq!(pager_width(pager_columns(snap.count)), 74.0);
        assert_eq!(pager_cell(3, 6), Some(2));
    }

    // Eighteen workspaces need the product maximum of nine columns; the last
    // workspace is the lower cell of the last column.
    #[test]
    fn eighteen_workspaces_need_nine_columns() {
        let snap = snapshot(18, 17);
        let views = project(Some(&snap));
        assert_eq!(views.iter().map(|view| view.column).max(), Some(8));
        assert_eq!(pager_columns(snap.count), PAGER_COLUMNS);
        assert_eq!(pager_width(PAGER_COLUMNS), 212.0);
        assert_eq!(pager_cell(17, 18), Some(PAGER_CELLS));
        assert_eq!(cell_index(PAGER_CELLS, 18), Some(17));
    }

    // A click target is a cell now: the live snapshot count decides which
    // workspace (if any) sits in it.
    #[test]
    fn click_cell_maps_to_workspace_index() {
        let snap = snapshot(8, 5);
        for view in project(Some(&snap)) {
            let cell = pager_cell(view.index, snap.count).expect("cell");
            assert_eq!(cell_index(cell, snap.count), Some(view.index));
            assert_eq!(
                intent_for_click(Some(&snap), view.index),
                Some(TaskbarIntent::ActivateWorkspace(view.index))
            );
        }
        assert_eq!(intent_for_click(Some(&snap), 8), None);
        assert_eq!(core::workspace_labels(2), vec!["1", "2"]);
    }

    #[test]
    fn click_request_carries_the_snapshot_revision() {
        let snap = snapshot(4, 0);
        assert_eq!(
            control_request_for_click(Some(&snap), 2),
            Some(ControlRequest::ActivateWorkspace {
                index: 2,
                expected_revision: 1,
            })
        );
        assert_eq!(control_request_for_click(Some(&snap), 4), None);
        assert_eq!(control_request_for_click(None, 0), None);
    }
}

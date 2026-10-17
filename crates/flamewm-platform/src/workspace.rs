use flamewm_api::workspace::{WorkspacePlan, WorkspacePlanKind, WorkspaceSnapshot};
use flamewm_api::{FlameError, FlameResult};

pub use flamewm_api::workspace::MAX_WORKSPACES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridPosition {
    pub row: usize,
    pub column: usize,
}

pub struct TwoRowTopology;

impl TwoRowTopology {
    #[must_use]
    pub const fn columns_for(total: usize) -> usize {
        if total == 0 { 1 } else { total.div_ceil(2) }
    }

    #[must_use]
    pub fn index_to_position(index: usize, total: usize) -> GridPosition {
        if total == 0 {
            return GridPosition { row: 0, column: 0 };
        }
        let index = index.min(total - 1);
        GridPosition {
            row: index % 2,
            column: index / 2,
        }
    }

    #[must_use]
    pub fn position_to_index(row: usize, column: usize, total: usize) -> usize {
        if total == 0 {
            return 0;
        }
        let column = column.min(Self::columns_for(total) - 1);
        let row = row.min(1);
        let candidate = column * 2 + row;
        if candidate < total {
            candidate
        } else {
            (column * 2).min(total - 1)
        }
    }

    #[must_use]
    pub fn move_left(index: usize, total: usize) -> usize {
        let pos = Self::index_to_position(index, total);
        if pos.column == 0 {
            index.min(total.saturating_sub(1))
        } else {
            Self::position_to_index(pos.row, pos.column - 1, total)
        }
    }

    #[must_use]
    pub fn move_right(index: usize, total: usize) -> usize {
        let pos = Self::index_to_position(index, total);
        if pos.column + 1 >= Self::columns_for(total) {
            index.min(total.saturating_sub(1))
        } else {
            Self::position_to_index(pos.row, pos.column + 1, total)
        }
    }

    #[must_use]
    pub fn move_up(index: usize, total: usize) -> usize {
        let pos = Self::index_to_position(index, total);
        if pos.row == 0 {
            index.min(total.saturating_sub(1))
        } else {
            Self::position_to_index(0, pos.column, total)
        }
    }

    #[must_use]
    pub fn move_down(index: usize, total: usize) -> usize {
        let pos = Self::index_to_position(index, total);
        if pos.row == 1 || (total % 2 == 1 && pos.column + 1 == Self::columns_for(total)) {
            index.min(total.saturating_sub(1))
        } else {
            Self::position_to_index(1, pos.column, total)
        }
    }
}

pub struct WorkspacePlanner;

impl WorkspacePlanner {
    pub fn activate(snapshot: &WorkspaceSnapshot, index: usize) -> FlameResult<WorkspacePlan> {
        snapshot.validate()?;
        if index >= snapshot.count {
            return Err(FlameError::invalid(
                "workspace activation index out of range",
            ));
        }
        Ok(WorkspacePlan {
            kind: WorkspacePlanKind::Activate,
            expected_revision: snapshot.revision,
            new_revision: snapshot.revision + 1,
            old_count: snapshot.count,
            new_count: snapshot.count,
            requested_index: index,
            old_to_new: (0..snapshot.count).collect(),
            new_active: index,
            new_last: Some(snapshot.active_index),
            new_names: normalized_names(snapshot),
        })
    }

    /// Product semantics: insert directly after the clicked workspace.
    pub fn insert_after(
        snapshot: &WorkspaceSnapshot,
        clicked: usize,
    ) -> FlameResult<WorkspacePlan> {
        snapshot.validate()?;
        if snapshot.count >= MAX_WORKSPACES {
            return Err(FlameError::invalid(
                "workspace count exceeds product maximum",
            ));
        }
        if clicked >= snapshot.count {
            return Err(FlameError::invalid("clicked workspace is out of range"));
        }
        let insert_index = clicked + 1;
        let new_count = snapshot.count + 1;
        let map_existing = |index: usize| {
            if index >= insert_index {
                index + 1
            } else {
                index
            }
        };
        let old_to_new = (0..snapshot.count).map(map_existing).collect::<Vec<_>>();

        let mut names = normalized_names(snapshot);
        names.insert(insert_index, format!("Workspace {}", insert_index + 1));
        let plan = WorkspacePlan {
            kind: WorkspacePlanKind::Insert,
            expected_revision: snapshot.revision,
            new_revision: snapshot.revision + 1,
            old_count: snapshot.count,
            new_count,
            requested_index: insert_index,
            old_to_new,
            new_active: map_existing(snapshot.active_index),
            new_last: snapshot.last_index.map(map_existing),
            new_names: names,
        };
        plan.validate()?;
        Ok(plan)
    }

    pub fn remove(snapshot: &WorkspaceSnapshot, index: usize) -> FlameResult<WorkspacePlan> {
        snapshot.validate()?;
        if snapshot.count <= 1 {
            return Err(FlameError::invalid("the last workspace cannot be removed"));
        }
        if index >= snapshot.count {
            return Err(FlameError::invalid("workspace removal index out of range"));
        }

        let new_count = snapshot.count - 1;
        let migrate_target = index.min(new_count - 1);
        let old_to_new = (0..snapshot.count)
            .map(|old| {
                if old == index {
                    migrate_target
                } else if old > index {
                    old - 1
                } else {
                    old
                }
            })
            .collect::<Vec<_>>();

        let mut names = normalized_names(snapshot);
        names.remove(index);
        let plan = WorkspacePlan {
            kind: WorkspacePlanKind::Remove,
            expected_revision: snapshot.revision,
            new_revision: snapshot.revision + 1,
            old_count: snapshot.count,
            new_count,
            requested_index: index,
            old_to_new: old_to_new.clone(),
            new_active: old_to_new[snapshot.active_index],
            new_last: snapshot.last_index.map(|old| old_to_new[old]),
            new_names: names,
        };
        plan.validate()?;
        Ok(plan)
    }
}

fn normalized_names(snapshot: &WorkspaceSnapshot) -> Vec<String> {
    if snapshot.names.len() == snapshot.count {
        return snapshot.names.clone();
    }
    (0..snapshot.count)
        .map(|index| format!("Workspace {}", index + 1))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_rejects_at_shared_maximum() {
        assert_eq!(MAX_WORKSPACES, 18);
        assert!(WorkspacePlanner::insert_after(&snapshot(MAX_WORKSPACES - 1, 0), 0).is_ok());
        assert!(WorkspacePlanner::insert_after(&snapshot(MAX_WORKSPACES, 0), 0).is_err());
    }

    fn snapshot(count: usize, active: usize) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            revision: 7,
            count,
            active_index: active,
            last_index: Some(0),
            names: (1..=count).map(|n| format!("Workspace {n}")).collect(),
        }
    }

    #[test]
    fn remove_middle_workspace_has_one_complete_old_to_new_mapping() {
        let plan = WorkspacePlanner::remove(&snapshot(4, 2), 1).expect("valid plan");
        assert_eq!(plan.old_to_new, vec![0, 1, 1, 2]);
        assert_eq!(plan.new_active, 1);
        assert_eq!(plan.new_count, 3);
    }

    #[test]
    fn remove_last_workspace_migrates_windows_to_new_last() {
        let plan = WorkspacePlanner::remove(&snapshot(4, 3), 3).expect("valid plan");
        assert_eq!(plan.old_to_new, vec![0, 1, 2, 2]);
        assert_eq!(plan.new_active, 2);
    }

    #[test]
    fn cannot_remove_only_workspace() {
        assert!(WorkspacePlanner::remove(&snapshot(1, 0), 0).is_err());
    }

    #[test]
    fn insert_after_shifts_existing_active_and_last_indices() {
        let mut source = snapshot(3, 2);
        source.last_index = Some(1);
        let plan = WorkspacePlanner::insert_after(&source, 0).expect("valid plan");
        assert_eq!(plan.requested_index, 1);
        assert_eq!(plan.old_to_new, vec![0, 2, 3]);
        assert_eq!(plan.new_active, 3);
        assert_eq!(plan.new_last, Some(2));
    }

    #[test]
    fn two_row_navigation_matches_pager_topology() {
        assert_eq!(
            TwoRowTopology::index_to_position(3, 5),
            GridPosition { row: 1, column: 1 }
        );
        assert_eq!(TwoRowTopology::move_right(3, 5), 4);
        assert_eq!(TwoRowTopology::move_down(4, 5), 4);
        assert_eq!(TwoRowTopology::move_up(3, 5), 2);
    }
}

use crate::{ErrorCode, FlameError, FlameResult};

/// Product maximum for workspace count; shared by planner, WM clamp and env override.
pub const MAX_WORKSPACES: usize = 18;

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
}

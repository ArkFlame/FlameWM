use crate::{DesktopAppId, OutputId, Rect, TaskEntryId, WindowRef};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum PanelEdge {
    #[default]
    Bottom,
    Top,
    Left,
    Right,
}

impl PanelEdge {
    #[must_use]
    pub const fn is_horizontal(self) -> bool {
        matches!(self, Self::Bottom | Self::Top)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelSnapshot {
    pub output: OutputId,
    pub edge: PanelEdge,
    pub logical_size: u16,
    pub visible: bool,
    pub geometry: Rect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskEntryKind {
    PinnedSlot { window: Option<WindowRef> },
    Window { window: WindowRef },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskEntry {
    pub id: TaskEntryId,
    pub app_id: DesktopAppId,
    pub kind: TaskEntryKind,
    pub order_index: usize,
}

/// A determinate task progress percentage, in the inclusive range 0–100.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskProgress(u8);

impl TaskProgress {
    #[must_use]
    pub const fn new(percent: u8) -> Option<Self> {
        if percent <= 100 {
            Some(Self(percent))
        } else {
            None
        }
    }

    #[must_use]
    pub const fn percent(self) -> u8 {
        self.0
    }
}

/// Optional progress attached to a task entry by a provider. Absence means no authoritative
/// progress is available for that entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskProgressEntry {
    pub task_id: TaskEntryId,
    pub progress: TaskProgress,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelsSnapshot {
    pub revision: u64,
    pub panels: Vec<PanelSnapshot>,
    pub tasks: Vec<TaskEntry>,
    pub pinned_apps: Vec<DesktopAppId>,
}

#[cfg(test)]
mod tests {
    use super::TaskProgress;

    #[test]
    fn task_progress_accepts_only_percentages_between_zero_and_one_hundred() {
        assert_eq!(TaskProgress::new(0).map(TaskProgress::percent), Some(0));
        assert_eq!(TaskProgress::new(100).map(TaskProgress::percent), Some(100));
        assert_eq!(TaskProgress::new(101), None);
    }
}

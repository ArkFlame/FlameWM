use flamewm_api::workspace::WorkspaceSnapshot;

#[must_use]
pub fn workspace_labels(count: usize) -> Vec<String> {
    (1..=count).map(|index| index.to_string()).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceView {
    pub index: usize,
    /// Visible pager label. Always numeric `index + 1` per the numeric pager
    /// caller contract (shell `runtime.rs` maps `workspace-N` to page slot `N - 1`).
    /// Protocol names never leak here; they are preserved in [`WorkspaceView::name`].
    pub label: String,
    /// Preserved protocol name from the snapshot (`""` when unnamed).
    pub name: String,
    pub active: bool,
    pub row: usize,
    pub column: usize,
}

/// Number of workspace slots visible in the compact taskbar page (2x2 grid).
pub const PAGE_SIZE: usize = 4;

/// Returns the visible 4-slot page containing the active workspace.
///
/// `page_base = (active_pos / PAGE_SIZE) * PAGE_SIZE` where `active_pos` is the
/// position of the active view in `views` (fallback `0` when none is active),
/// clamped to `views[page_base..min(page_base + PAGE_SIZE, len)]`.
#[must_use]
pub fn visible_page(views: &[WorkspaceView]) -> Vec<&WorkspaceView> {
    let start = active_page_base(views).min(views.len());
    let end = (start + PAGE_SIZE).min(views.len());
    views[start..end].iter().collect()
}

fn active_page_base(views: &[WorkspaceView]) -> usize {
    let active_pos = views.iter().position(|view| view.active).unwrap_or(0);
    (active_pos / PAGE_SIZE) * PAGE_SIZE
}

/// Maps a global workspace index to its `(row, column)` slot in the visible page.
///
/// Global [`WorkspaceView::row`]/[`WorkspaceView::column`] use global topology
/// (`index / 2`, `index % 2`). This function instead returns page-relative
/// coordinates: `position_in_page = global_pos - page_base` maps
/// `0 -> (0, 0)`, `1 -> (0, 1)`, `2 -> (1, 0)`, `3 -> (1, 1)`, so the left
/// column shows `visible[0]`, `visible[2]` and the right column shows
/// `visible[1]`, `visible[3]`. Returns `None` when the index is unknown or
/// outside the current visible page.
#[must_use]
pub fn slot_position(global_index: usize, views: &[WorkspaceView]) -> Option<(usize, usize)> {
    let page_base = active_page_base(views);
    let target_pos = views.iter().position(|view| view.index == global_index)?;
    let window_end = (page_base + PAGE_SIZE).min(views.len());
    if target_pos < page_base || target_pos >= window_end {
        return None;
    }
    let position_in_page = target_pos - page_base;
    Some((position_in_page / 2, position_in_page % 2))
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
        .map(|(index, (name, label))| WorkspaceView {
            index,
            label,
            name,
            active: index == snapshot.active_index,
            row: index / 2,
            column: index % 2,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::workspace_labels;

    #[test]
    fn named_workspace_compact_labels_are_numeric_index_plus_one() {
        assert_eq!(workspace_labels(4), vec!["1", "2", "3", "4"]);
    }
}

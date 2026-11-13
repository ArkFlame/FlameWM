//! Task menu rows: the task actions shown for one taskbar task entry.

use super::*;

/// Shows the task-menu action rows that apply to the task `id`; row `highlight` is the keyboard row.
pub(super) fn project_task_rows(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
    id: &flamewm_api::TaskEntryId,
    highlight: usize,
) -> Result<(), String> {
    let actions =
        crate::taskbar::context_menu::task_menu_for_entry(&snapshot.panels, &snapshot.windows, id)
            .into_iter()
            .filter_map(|entry| match entry {
                flamewm_ui_core::MenuEntry::Action { id, .. } => Some(id),
                _ => None,
            });
    for (index, row) in actions.enumerate() {
        let element = format!("context-task-{row}");
        document.visible(&element, true)?;
        super::context_menu::mark_menu_row(document, &element, index == highlight)?;
    }
    Ok(())
}

#[cfg(test)]
mod root_tests;

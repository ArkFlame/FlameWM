//! Task menu rows: the task actions shown for one taskbar task entry.

use super::*;

/// Shows the task-menu action rows that apply to the task `id`.
pub(super) fn project_task_rows(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
    id: &flamewm_api::TaskEntryId,
) -> Result<(), String> {
    for entry in
        crate::taskbar::context_menu::task_menu_for_entry(&snapshot.panels, &snapshot.windows, id)
    {
        if let flamewm_ui_core::MenuEntry::Action { id, .. } = entry {
            document.visible(&format!("context-task-{id}"), true)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod root_tests;

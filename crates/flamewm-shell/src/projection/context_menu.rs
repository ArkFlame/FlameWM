//! Context menu projection for the shell task-menu popup document.

use super::*;

pub fn project_context_menu(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
    settings: &SettingsSnapshot,
    state: Option<&crate::ContextMenuState>,
) -> Result<(), String> {
    // Shared menu surface (radius + fill) from the Surfaces settings, same owner as desktop menus.
    let surface = flamewm_ui_core::context_menu::menu_surface(settings);
    // The shell task-menu popup document holds only the `task-menu` root; desktop roots live in the desktop document.
    document.background(
        "task-menu",
        crate::settings_projection::surface_background(settings),
    )?;
    document.corner_radius("task-menu", surface.radius as f32)?;
    for id in [
        "context-task-activate",
        "context-task-close",
        "context-task-pin",
        "context-task-unpin",
        "context-workspace-activate",
        "context-workspace-insert-after",
        "context-workspace-remove",
        "context-taskbar-settings",
        "context-taskbar-task-manager",
    ] {
        document.visible(id, false)?;
    }
    let Some(state) = state else {
        return Ok(());
    };
    match &state.kind {
        crate::ContextMenuKind::Task(id) => task_menu::project_task_rows(document, snapshot, id)?,
        crate::ContextMenuKind::Workspace { index } => {
            if let Some(workspaces) = snapshot.workspaces.as_ref() {
                for (entry, _) in crate::taskbar::context_menu::workspace_menu(workspaces, *index) {
                    if let flamewm_ui_core::MenuEntry::Action { id, .. } = entry {
                        document.visible(&format!("context-workspace-{id}"), true)?;
                    }
                }
            }
        }
        crate::ContextMenuKind::Taskbar => {
            for row in crate::taskbar::context_menu::taskbar_menu() {
                document.visible(&format!("context-{}", row.id), true)?;
            }
        }
    }
    Ok(())
}

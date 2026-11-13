//! Context menu projection for the shell task-menu popup document.

use super::*;

use crate::taskbar::context_menu::TASK_OVERFLOW_MENU_ROWS;

pub fn project_context_menu(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
    settings: &SettingsSnapshot,
    state: Option<&crate::ContextMenuState>,
    start_favorites: &[String],
    overflow_highlight: usize,
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
        "context-start-add",
        "context-start-remove",
    ] {
        document.visible(id, false)?;
    }
    for index in 0..crate::tray_host::TRAY_MENU_ROWS {
        document.visible(&format!("context-tray-{index}"), false)?;
    }
    for index in 0..TASK_OVERFLOW_MENU_ROWS {
        document.visible(&format!("context-overflow-{index}"), false)?;
    }
    let Some(state) = state else {
        return Ok(());
    };
    match &state.kind {
        crate::ContextMenuKind::Task(id) => {
            task_menu::project_task_rows(document, snapshot, id, overflow_highlight)?;
        }
        crate::ContextMenuKind::Workspace { index } => {
            if let Some(workspaces) = snapshot.workspaces.as_ref() {
                let actions = crate::taskbar::context_menu::workspace_menu(workspaces, *index)
                    .into_iter()
                    .filter_map(|(entry, _)| match entry {
                        flamewm_ui_core::MenuEntry::Action { id, .. } => Some(id),
                        _ => None,
                    });
                for (row, id) in actions.enumerate() {
                    let element = format!("context-workspace-{id}");
                    document.visible(&element, true)?;
                    mark_menu_row(document, &element, row == overflow_highlight)?;
                }
            }
        }
        crate::ContextMenuKind::Taskbar => {
            for row in crate::taskbar::context_menu::taskbar_menu() {
                document.visible(&format!("context-{}", row.id), true)?;
            }
        }
        crate::ContextMenuKind::TaskOverflow { entries } => {
            for (index, state) in entries.iter().take(TASK_OVERFLOW_MENU_ROWS).enumerate() {
                let label = crate::projection::task_label(snapshot, state);
                let id = format!("context-overflow-{index}");
                document.text(&format!("{id}-label"), label.to_owned())?;
                document.visible(&id, true)?;
                // WAI-ARIA APG menu button: one active item, roving tabindex.
                mark_menu_row(document, &id, index == overflow_highlight)?;
            }
        }
        crate::ContextMenuKind::TrayItem { rows, .. } => {
            for (index, row) in rows
                .iter()
                .take(crate::tray_host::TRAY_MENU_ROWS)
                .enumerate()
            {
                document.text(&format!("context-tray-{index}-label"), row.label.clone())?;
                document.visible(&format!("context-tray-{index}"), true)?;
            }
        }
        crate::ContextMenuKind::StartApp(app) => {
            // One row shown for the current state, like the pin/unpin pair.
            let favorite = start_favorites.iter().any(|id| id == app.as_str());
            document.visible("context-start-add", !favorite)?;
            document.visible("context-start-remove", favorite)?;
        }
    }
    Ok(())
}

/// WAI-ARIA APG menu item state for one row: `aria-selected`, roving `tabindex`, and the pager's
/// active fill on the highlighted row only.
pub(super) fn mark_menu_row(
    document: &mut impl UiDocumentAccess,
    element: &str,
    highlighted: bool,
) -> Result<(), String> {
    document.attribute(
        element,
        "aria-selected",
        if highlighted { "true" } else { "false" },
    )?;
    document.attribute(element, "tabindex", if highlighted { "0" } else { "-1" })?;
    if highlighted {
        document.background(element, super::panel::pager_active_fill())
    } else {
        document.background_clear(element)
    }
}

#[cfg(test)]
mod tests {
    const TASK_MENU_HTML: &str = include_str!("../../../../ui/shell/task-menu.html");
    const SHELL_CSS: &str = include_str!("../../../../ui/shell/flamewm.css");

    #[test]
    fn favorite_context_icons_use_the_yellow_symbolic_foreground() {
        assert_eq!(
            TASK_MENU_HTML
                .matches("class=\"symbolic-icon favorite-icon\"")
                .count(),
            2
        );
        assert!(SHELL_CSS.contains(".favorite-icon { color:var(--sticky-bg); }"));
        assert!(SHELL_CSS.contains("--sticky-bg:#ffe56b"));
    }
}

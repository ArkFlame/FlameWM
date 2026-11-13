//! Keyboard operation of the open row menus (taskbar task, workspace and overflow), following the
//! WAI-ARIA menu-button pattern: Down/Up/Home/End move the highlighted row, Enter or Space activates
//! it through the same action id a mouse release uses. Escape closes through the shared route arm.
//! Precedent: WAI-ARIA APG menu button; KDE Plasma context menus (plasma-desktop, .vendor) also let
//! the arrows and Return drive the popup. Up and Down wrap at the ends (APG menu; KDE Plasma menus
//! wrap too). Every open row menu (task, workspace, overflow, taskbar, start app, tray) has rows.

use super::START_MENU_FAVORITE_ACTION;
use crate::runtime::{ContextMenuKind, ContextMenuState, ShellSnapshot};
use crate::taskbar::context_menu::{
    task_menu_for_entry, taskbar_menu, workspace_menu, TASK_OVERFLOW_MENU_ROWS,
};
use crate::tray_host::{TRAY_MENU_ACTION_PREFIX, TRAY_MENU_ROWS};

/// Action ids of the open menu's keyboard rows, in visual order. Each id is dispatched the same way
/// a mouse release on that row is (`context_menu_request_for_action`). Empty when no keyboard path.
pub(crate) fn menu_row_actions(
    menu: Option<&ContextMenuState>,
    snapshot: &ShellSnapshot,
) -> Vec<String> {
    let Some(menu) = menu else {
        return Vec::new();
    };
    match &menu.kind {
        ContextMenuKind::TaskOverflow { entries } => {
            (0..entries.len().min(TASK_OVERFLOW_MENU_ROWS))
                .map(|index| format!("task.overflow.{index}"))
                .collect()
        }
        ContextMenuKind::Task(id) => action_ids(
            "task.menu.",
            task_menu_for_entry(&snapshot.panels, &snapshot.windows, id),
        ),
        ContextMenuKind::Workspace { index } => {
            snapshot.workspaces.as_ref().map_or_else(Vec::new, |ws| {
                action_ids(
                    "workspace.menu.",
                    workspace_menu(ws, *index)
                        .into_iter()
                        .map(|(entry, _)| entry),
                )
            })
        }
        ContextMenuKind::Taskbar => taskbar_menu()
            .iter()
            .map(|row| taskbar_row_action(row.id))
            .collect(),
        // One row, shown as "Add" or "Remove" for the current favorite state.
        ContextMenuKind::StartApp(_) => vec![START_MENU_FAVORITE_ACTION.to_owned()],
        ContextMenuKind::TrayItem { rows, .. } => tray_row_actions(rows.len()),
    }
}

/// Taskbar row id (`taskbar-settings`) to its dispatched action (`taskbar.menu.settings`).
fn taskbar_row_action(id: &str) -> String {
    format!("taskbar.menu.{}", id.strip_prefix("taskbar-").unwrap_or(id))
}

/// Dbusmenu row actions (`tray.menu.<n>`) for the rows the tray projection shows.
fn tray_row_actions(rows: usize) -> Vec<String> {
    (0..rows.min(TRAY_MENU_ROWS))
        .map(|index| format!("{TRAY_MENU_ACTION_PREFIX}{index}"))
        .collect()
}

/// `prefix` + id for every action entry; separators are not keyboard rows.
fn action_ids(
    prefix: &str,
    entries: impl IntoIterator<Item = flamewm_ui_core::MenuEntry>,
) -> Vec<String> {
    entries
        .into_iter()
        .filter_map(|entry| match entry {
            flamewm_ui_core::MenuEntry::Action { id, .. } => Some(format!("{prefix}{id}")),
            _ => None,
        })
        .collect()
}

/// Highlighted row after a typed navigation key on the open overflow menu. `None` when the key is
/// not a menu navigation key or no row exists.
pub(crate) fn overflow_menu_step(payload: &str, rows: usize, highlight: usize) -> Option<usize> {
    if rows == 0 {
        return None;
    }
    let (_, key) = flamewm_shell::launcher::parse_typed_key(payload)?;
    let current = highlight.min(rows - 1);
    match key {
        "Down" => Some((current + 1) % rows),
        "Up" => Some((current + rows - 1) % rows),
        "Home" => Some(0),
        "End" => Some(rows - 1),
        _ => None,
    }
}

/// True when a typed Tab closes the open overflow menu (APG: Tab leaves the menu).
pub(crate) fn overflow_menu_closes_on(payload: &str, rows: usize) -> bool {
    rows > 0
        && flamewm_shell::launcher::parse_typed_key(payload).is_some_and(|(_, key)| key == "Tab")
}

/// Row index that a typed Enter or Space activates on the open menu. `None` when no row exists or
/// the text is another key.
pub(crate) fn overflow_activation_index(
    text: Option<&str>,
    rows: usize,
    highlight: usize,
) -> Option<usize> {
    if rows == 0 {
        return None;
    }
    match text? {
        "\n" | "\r" | " " => Some(highlight.min(rows - 1)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_and_space_activate_the_highlighted_row_while_rows_exist() {
        for text in ["\n", "\r", " "] {
            assert_eq!(
                overflow_activation_index(Some(text), 3, 0),
                Some(0),
                "key {text:?}"
            );
            assert_eq!(
                overflow_activation_index(Some(text), 3, 2),
                Some(2),
                "key {text:?}"
            );
        }
        assert_eq!(overflow_activation_index(Some("\n"), 0, 0), None);
        assert_eq!(overflow_activation_index(Some(" "), 0, 0), None);
    }

    #[test]
    fn other_keys_and_missing_text_do_not_activate() {
        assert_eq!(overflow_activation_index(Some("\u{1b}"), 3, 0), None);
        assert_eq!(overflow_activation_index(Some("a"), 3, 0), None);
        assert_eq!(overflow_activation_index(None, 3, 0), None);
    }

    #[test]
    fn action_ids_skip_separators_and_keep_row_order() {
        use flamewm_ui_core::MenuEntry;
        let rows = vec![
            MenuEntry::Action {
                id: "activate".to_owned(),
                label: "A".to_owned(),
                icon: None,
                enabled: true,
            },
            MenuEntry::Separator,
            MenuEntry::Action {
                id: "close".to_owned(),
                label: "C".to_owned(),
                icon: None,
                enabled: true,
            },
        ];
        assert_eq!(
            action_ids("task.menu.", rows),
            ["task.menu.activate", "task.menu.close"]
        );
    }

    /// Typed-key payload `modifiers US key US text`, as the shell receives `__flamewm.ui.key`.
    fn key(name: &str) -> String {
        format!("\u{1f}{name}\u{1f}")
    }

    #[test]
    fn arrows_move_and_wrap_at_the_ends() {
        assert_eq!(overflow_menu_step(&key("Down"), 3, 0), Some(1));
        assert_eq!(overflow_menu_step(&key("Down"), 3, 2), Some(0));
        assert_eq!(overflow_menu_step(&key("Up"), 3, 0), Some(2));
        assert_eq!(overflow_menu_step(&key("Up"), 3, 2), Some(1));
        assert_eq!(overflow_menu_step(&key("End"), 3, 0), Some(2));
        assert_eq!(overflow_menu_step(&key("Home"), 3, 2), Some(0));
        // A single row wraps onto itself; a stale highlight past the end is clamped first.
        assert_eq!(overflow_menu_step(&key("Down"), 1, 0), Some(0));
        assert_eq!(overflow_menu_step(&key("Up"), 3, 9), Some(1));
    }

    #[test]
    fn taskbar_rows_dispatch_the_settings_launch_actions() {
        use super::super::helpers::general_settings_launch;
        let actions: Vec<String> = taskbar_menu()
            .iter()
            .map(|row| taskbar_row_action(row.id))
            .collect();
        assert_eq!(
            actions,
            ["taskbar.menu.settings", "taskbar.menu.task-manager"]
        );
        for action in &actions {
            assert!(general_settings_launch(action).is_some(), "{action}");
        }
    }

    #[test]
    fn tray_rows_follow_the_projected_row_count() {
        assert_eq!(tray_row_actions(2), ["tray.menu.0", "tray.menu.1"]);
        assert_eq!(tray_row_actions(0), Vec::<String>::new());
        assert_eq!(tray_row_actions(TRAY_MENU_ROWS + 5).len(), TRAY_MENU_ROWS);
    }

    #[test]
    fn navigation_needs_rows_and_a_navigation_key() {
        assert_eq!(overflow_menu_step(&key("Down"), 0, 0), None);
        assert_eq!(overflow_menu_step(&key("a"), 3, 0), None);
        assert_eq!(overflow_menu_step(&key("Escape"), 3, 0), None);
    }

    #[test]
    fn tab_closes_only_an_open_overflow_menu() {
        assert!(overflow_menu_closes_on(&key("Tab"), 3));
        assert!(!overflow_menu_closes_on(&key("Tab"), 0));
        assert!(!overflow_menu_closes_on(&key("Down"), 3));
    }
}

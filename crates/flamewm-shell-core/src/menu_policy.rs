//! Shell context-menu row policy. Pure menu identity; no control or
//! runtime state here. Row identity is the stable row id, never the label.

use flamewm_api::TaskEntryId;
use flamewm_api::panels::{PanelsSnapshot, TaskEntryKind};
use flamewm_api::window::WindowSnapshot;
use flamewm_api::{Point, Rect, Size};
use flamewm_ui_core::MenuEntry;
use flamewm_ui_core::context_menu::{self, MIN_ROW_WIDTH, MenuMetrics, MenuPart};

/// Stable context-menu row ids. Rows are keyed by id, never by label.
pub const TASK_ROW_ACTIVATE: &str = "activate";
pub const TASK_ROW_CLOSE: &str = "close";
pub const TASK_ROW_PIN: &str = "pin";
pub const TASK_ROW_UNPIN: &str = "unpin";
pub const WORKSPACE_ROW_ACTIVATE: &str = "activate";
pub const WORKSPACE_ROW_INSERT_AFTER: &str = "insert-after";
pub const WORKSPACE_ROW_REMOVE: &str = "remove";

/// Pinned entries expose Unpin; running entries expose Activate, Close and Pin.
#[must_use]
pub fn task_menu_for_entry(
    snapshot: &PanelsSnapshot,
    windows: &[WindowSnapshot],
    id: &TaskEntryId,
) -> Vec<MenuEntry> {
    let Some(entry) = snapshot.tasks.iter().find(|entry| &entry.id == id) else {
        return Vec::new();
    };
    if entry.id.is_pinned() {
        return vec![action(TASK_ROW_UNPIN, "Unpin")];
    }
    let window = match entry.kind {
        TaskEntryKind::PinnedSlot { window } => window,
        TaskEntryKind::Window { window } => Some(window),
    };
    let running =
        window.is_some_and(|window| windows.iter().any(|snapshot| snapshot.reference == window));
    if !running {
        return Vec::new();
    }
    vec![
        action(TASK_ROW_ACTIVATE, "Activate"),
        action(TASK_ROW_CLOSE, "Close"),
        action(TASK_ROW_PIN, "Pin to Taskbar"),
    ]
}

fn action(id: &str, label: &str) -> MenuEntry {
    MenuEntry::Action {
        id: id.to_owned(),
        label: label.to_owned(),
        icon: None,
        enabled: true,
    }
}

/// Stable row identity for a task entry slot: entry id plus policy row id.
#[must_use]
pub fn row_identity(entry: &TaskEntryId, row: &str) -> String {
    format!("{}#{row}", entry.0)
}

/// C08 shell context-menu geometry. Height derives from the actual visible
/// part list (one `MenuPart::Row` per visible entry row); minimum width is
/// the canonical `MIN_ROW_WIDTH`. Only the returned `menu_rect` clamp
/// touches the work area/output.
#[must_use]
pub fn menu_parts_for_rows(visible_rows: usize) -> Vec<MenuPart> {
    vec![MenuPart::Row; visible_rows.max(1)]
}

#[must_use]
pub fn context_menu_size(parts: &[MenuPart]) -> Size {
    context_menu::menu_size(parts, MIN_ROW_WIDTH, MenuMetrics::canonical())
}

#[must_use]
pub fn context_menu_rect(work_area: Rect, anchor: Point, size: Size) -> Rect {
    context_menu::menu_rect(work_area, anchor, size)
}

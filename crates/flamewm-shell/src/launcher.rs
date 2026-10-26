//! Top-center application launcher: query, selection and recents state, the
//! typed-key reducer, and projection into `ui/shell/launcher.html`.
//!
//! Placement and surface ownership live in `runtime::launcher_surface`. Results
//! come from the indexed Start model, so filtering is the same search Start uses.

use std::ops::Range;

use flamewm_api::applications::DesktopApplication;
use flamewm_api::DesktopAppId;
use flamewm_shell_core::StartModel;
use flamewm_ui_x11::UiDocumentAccess;

use crate::icon_loader::{IconLoader, IconTarget};
use crate::projection::start_visible_results;
use crate::start::StartCategory;

/// Visible result rows. The list scrolls once the selection leaves this window.
pub const LAUNCHER_ROWS: usize = 8;
/// Logical row height. Must match `.launcher-row` and `.launcher-highlight` in `flamewm.css`.
pub const LAUNCHER_ROW_HEIGHT: i32 = 40;
/// Launch history kept for the empty-query view. Session-only; never persisted.
const RECENT_LIMIT: usize = 8;
const ICON_SIZE: u32 = 20;

/// Separator the typed-key payload uses between modifiers, key name and character.
const KEY_PAYLOAD_SEPARATOR: char = '\u{1f}';

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LauncherState {
    query: String,
    selected: usize,
    first: usize,
    /// Cached result count for the current query; navigation reads this, never re-filters.
    count: usize,
    recent: Vec<DesktopAppId>,
}

impl LauncherState {
    /// Opening clears the query and selection; launch history survives across opens.
    pub fn open(&mut self, model: &StartModel) {
        self.query.clear();
        self.selected = 0;
        self.first = 0;
        self.refresh(model);
    }

    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    #[must_use]
    pub const fn count(&self) -> usize {
        self.count
    }

    #[must_use]
    pub const fn selected(&self) -> usize {
        self.selected
    }

    /// Window of result indices currently shown.
    #[must_use]
    pub fn window(&self) -> Range<usize> {
        self.first..(self.first + LAUNCHER_ROWS).min(self.count)
    }

    /// Appends typed text. Returns false (no re-filter) for empty input.
    pub fn type_text(&mut self, model: &StartModel, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        self.query.push_str(text);
        self.requery(model);
        true
    }

    /// Removes the last query character. Returns false when the query is already empty.
    pub fn backspace(&mut self, model: &StartModel) -> bool {
        if self.query.pop().is_none() {
            return false;
        }
        self.requery(model);
        true
    }

    /// Moves the selection by `delta` rows, clamped to the result range; the window follows it.
    pub fn move_by(&mut self, delta: isize) {
        if self.count == 0 {
            return;
        }
        let target = self
            .selected
            .saturating_add_signed(delta)
            .min(self.count - 1);
        self.selected = target;
        if target < self.first {
            self.first = target;
        } else if target >= self.first + LAUNCHER_ROWS {
            self.first = target + 1 - LAUNCHER_ROWS;
        }
    }

    /// Moves the single selection to the visible row `slot` (pointer hover). Returns false
    /// when the selection is unchanged or the slot shows no result, so callers skip work.
    /// The window never scrolls: a hovered row is already visible.
    pub fn select_slot(&mut self, slot: usize) -> bool {
        let index = self.first + slot;
        if slot >= LAUNCHER_ROWS || index >= self.count || index == self.selected {
            return false;
        }
        self.selected = index;
        true
    }

    /// Application at result index `index`, in the same order the rows are shown.
    #[must_use]
    pub fn app_at<'a>(
        &self,
        model: &'a StartModel,
        index: usize,
    ) -> Option<&'a DesktopApplication> {
        results(model, &self.query, &self.recent)
            .get(index)
            .copied()
    }

    /// Application for a visible row slot (0-based within the window).
    #[must_use]
    pub fn app_for_slot<'a>(
        &self,
        model: &'a StartModel,
        slot: usize,
    ) -> Option<&'a DesktopApplication> {
        self.app_at(model, self.first + slot)
            .filter(|_| self.first + slot < self.count)
    }

    /// Records a launch so it leads the empty-query list.
    pub fn note_launch(&mut self, id: &DesktopAppId) {
        self.recent.retain(|recent| recent != id);
        self.recent.insert(0, id.clone());
        self.recent.truncate(RECENT_LIMIT);
    }

    fn requery(&mut self, model: &StartModel) {
        self.selected = 0;
        self.first = 0;
        self.refresh(model);
    }

    fn refresh(&mut self, model: &StartModel) {
        self.count = results(model, &self.query, &self.recent).len();
        // A text change invalidates every icon binding made for the old result list.
        crate::async_projection::bump_start_view_epoch();
    }

    /// Row offset of the single selection highlight inside the visible window.
    #[must_use]
    pub fn highlight_y(&self) -> f32 {
        ((self.selected - self.first.min(self.selected)) as i32 * LAUNCHER_ROW_HEIGHT) as f32
    }

    /// Moves only the selection highlight node. Hover uses this, not `project`, so the
    /// row labels and icons are not rewritten on every selection change.
    pub fn project_highlight(&self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        document.position("launcher-highlight", 0.0, self.highlight_y())
    }

    /// Writes rows, labels, icons, the selection highlight and the query line.
    pub fn project(
        &self,
        document: &mut impl UiDocumentAccess,
        model: &StartModel,
        mut loader: Option<&mut IconLoader>,
    ) -> Result<(), String> {
        let apps = results(model, &self.query, &self.recent);
        let window = self.window();
        document.text(
            "launcher-query-label",
            if self.query.is_empty() {
                "Search...".to_owned()
            } else {
                self.query.clone()
            },
        )?;
        document.visible("launcher-empty", apps.is_empty())?;
        document.visible("launcher-highlight", !apps.is_empty())?;
        self.project_highlight(document)?;
        for slot in 0..LAUNCHER_ROWS {
            let row = slot + 1;
            let row_id = format!("launcher-row-{row}");
            let label_id = format!("launcher-row-{row}-label");
            let icon_id = format!("launcher-row-{row}-icon");
            match apps
                .get(window.start + slot)
                .filter(|_| window.start + slot < window.end)
            {
                Some(application) => {
                    document.text(&label_id, application.name.clone())?;
                    document.visible(&row_id, true)?;
                    document.visible(&icon_id, true)?;
                    crate::async_projection::schedule_visible(
                        document,
                        &icon_id,
                        &mut loader.as_deref_mut(),
                        IconTarget::LauncherSlot(slot as u8),
                        &application.icon_name,
                        ICON_SIZE,
                        ICON_SIZE,
                    )?;
                }
                None => {
                    document.text(&label_id, String::new())?;
                    document.visible(&row_id, false)?;
                    document.visible(&icon_id, false)?;
                }
            }
        }
        Ok(())
    }
}

/// Result list for a query. An empty query lists recent launches first, then every app.
#[must_use]
pub fn results<'a>(
    model: &'a StartModel,
    query: &str,
    recent: &[DesktopAppId],
) -> Vec<&'a DesktopApplication> {
    if !query.trim().is_empty() {
        return start_visible_results(model, StartCategory::All, query);
    }
    let all = start_visible_results(model, StartCategory::All, "");
    let mut ordered: Vec<&DesktopApplication> = recent
        .iter()
        .filter_map(|id| {
            all.iter()
                .copied()
                .find(|application| &application.id == id)
        })
        .collect();
    ordered.extend(
        all.into_iter()
            .filter(|application| !recent.contains(&application.id)),
    );
    ordered
}

/// Parsed typed-key payload `modifiers | key | text`, as the shell receives `__flamewm.ui.key`.
#[must_use]
pub fn parse_typed_key(payload: &str) -> Option<(&str, &str)> {
    let mut fields = payload.splitn(3, KEY_PAYLOAD_SEPARATOR);
    let modifiers = fields.next()?;
    let key = fields.next().filter(|key| !key.is_empty())?;
    Some((modifiers, key))
}

/// Selection delta for a navigation key: Up/Down, or Ctrl+K / Ctrl+J.
#[must_use]
pub fn navigation_delta(modifiers: &str, key: &str) -> Option<isize> {
    let ctrl = modifiers.split('+').any(|modifier| modifier == "Ctrl");
    match key {
        "Up" => Some(-1),
        "Down" => Some(1),
        "k" | "K" if ctrl => Some(-1),
        "j" | "J" if ctrl => Some(1),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_accepts_arrows_and_ctrl_jk_only() {
        assert_eq!(navigation_delta("", "Up"), Some(-1));
        assert_eq!(navigation_delta("", "Down"), Some(1));
        assert_eq!(navigation_delta("Ctrl", "k"), Some(-1));
        assert_eq!(navigation_delta("Ctrl", "j"), Some(1));
        assert_eq!(navigation_delta("", "k"), None, "plain letters are text");
        assert_eq!(navigation_delta("Shift", "j"), None);
    }

    #[test]
    fn typed_key_payload_splits_modifiers_key_and_text() {
        let payload = "Ctrl|j|";
        let joined = payload.replace('|', &KEY_PAYLOAD_SEPARATOR.to_string());
        assert_eq!(parse_typed_key(&joined), Some(("Ctrl", "j")));
        assert_eq!(parse_typed_key("no-separators"), None);
    }

    #[test]
    fn window_follows_selection_and_stays_clamped() {
        let mut state = LauncherState {
            count: 20,
            ..LauncherState::default()
        };
        state.move_by(-1);
        assert_eq!(state.selected(), 0, "clamped at the top");
        state.move_by(LAUNCHER_ROWS as isize);
        assert_eq!(state.selected(), LAUNCHER_ROWS);
        assert_eq!(state.window(), 1..LAUNCHER_ROWS + 1, "scrolled by one row");
        state.move_by(100);
        assert_eq!(state.selected(), 19, "clamped at the bottom");
        assert_eq!(state.window(), 12..20);
    }

    #[test]
    fn hover_moves_the_single_selection_and_arrows_continue_from_it() {
        let mut state = LauncherState {
            count: 20,
            ..LauncherState::default()
        };
        assert!(
            state.select_slot(3),
            "hovering another row changes selection"
        );
        assert_eq!(state.selected(), 3);
        assert_eq!(state.highlight_y(), (3 * LAUNCHER_ROW_HEIGHT) as f32);
        assert!(!state.select_slot(3), "same row again is a no-op");
        state.move_by(1);
        assert_eq!(
            state.selected(),
            4,
            "arrow key continues from the hovered row"
        );
        assert_eq!(state.highlight_y(), (4 * LAUNCHER_ROW_HEIGHT) as f32);
    }

    #[test]
    fn hover_keeps_the_window_and_ignores_empty_slots() {
        let mut state = LauncherState {
            count: 3,
            ..LauncherState::default()
        };
        assert!(
            !state.select_slot(5),
            "slot past the result count is ignored"
        );
        assert_eq!(state.selected(), 0);
        assert!(state.select_slot(2));
        assert_eq!(
            state.window(),
            0..3,
            "hover inside the window does not scroll"
        );
    }

    #[test]
    fn hover_on_a_scrolled_window_maps_slot_to_absolute_index() {
        let mut state = LauncherState {
            count: 20,
            ..LauncherState::default()
        };
        state.move_by(LAUNCHER_ROWS as isize);
        assert_eq!(state.window(), 1..LAUNCHER_ROWS + 1);
        assert!(state.select_slot(0));
        assert_eq!(state.selected(), 1, "slot 0 is the first visible row");
        assert_eq!(state.highlight_y(), 0.0);
    }

    #[test]
    fn empty_result_set_never_moves_selection() {
        let mut state = LauncherState::default();
        state.move_by(3);
        assert_eq!(state.selected(), 0);
        assert_eq!(state.window(), 0..0);
    }

    #[test]
    fn launch_history_is_most_recent_first_and_bounded() {
        let mut state = LauncherState::default();
        for index in 0..(RECENT_LIMIT + 3) {
            state.note_launch(&DesktopAppId::new(format!("app-{index}.desktop")));
        }
        assert_eq!(state.recent.len(), RECENT_LIMIT);
        assert_eq!(
            state.recent[0].as_str(),
            format!("app-{}.desktop", RECENT_LIMIT + 2)
        );
        state.note_launch(&DesktopAppId::new("app-4.desktop"));
        assert_eq!(state.recent[0].as_str(), "app-4.desktop");
        assert_eq!(
            state
                .recent
                .iter()
                .filter(|id| id.as_str() == "app-4.desktop")
                .count(),
            1
        );
    }
}

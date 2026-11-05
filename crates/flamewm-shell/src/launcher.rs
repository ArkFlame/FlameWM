//! Top-center application launcher: query, selection and recents state, the
//! typed-key reducer, and projection into `ui/shell/launcher.html`.
//!
//! Placement and surface ownership live in `runtime::launcher_surface`. Results
//! come from the indexed Start model, so filtering is the same search Start uses.

use std::borrow::Cow;
use std::ops::Range;

use flamewm_api::applications::DesktopApplication;
use flamewm_api::DesktopAppId;
use flamewm_shell_core::selection::ListSelection;
use flamewm_shell_core::StartModel;
use flamewm_ui_x11::UiDocumentAccess;

use crate::icon_loader::{IconLoader, IconTarget};

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
    selection: ListSelection,
    first: usize,
    /// Cached result count for the current query; navigation reads this, never re-filters.
    count: usize,
    recent: Vec<DesktopAppId>,
    /// Result order filtered once per query change; keyed by the model generation.
    order: Option<OrderCache>,
}

/// Application indices in result order for one query, valid for one model generation.
#[derive(Debug, Clone, PartialEq, Eq)]
struct OrderCache {
    generation: u64,
    query: String,
    indices: Vec<usize>,
}

impl LauncherState {
    /// Opening clears the query and selection; launch history survives across opens.
    pub fn open(&mut self, model: &StartModel) {
        self.query.clear();
        self.selection.reset();
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
    /// Highlighted result index; Enter with no highlight launches the first result.
    pub fn selected(&self) -> usize {
        self.selection.highlighted().unwrap_or(0)
    }

    /// Pointer left the list: drop the highlight.
    pub fn leave(&mut self) -> bool {
        self.selection.reset()
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
        self.selection.step(delta, self.count);
        let target = self.selected();
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
        slot < LAUNCHER_ROWS && self.selection.hover(index, self.count)
    }

    /// Application at result index `index`, in the same order the rows are shown.
    #[must_use]
    pub fn app_at<'a>(
        &self,
        model: &'a StartModel,
        index: usize,
    ) -> Option<&'a DesktopApplication> {
        let applications = model.applications();
        self.order(model)
            .get(index)
            .map(|&application| &applications[application])
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
        self.order = None;
    }

    /// Result order for the current query. Uses the cache filled by `refresh` while the
    /// query and model generation still match; otherwise filters again without caching.
    fn order<'c>(&'c self, model: &StartModel) -> Cow<'c, [usize]> {
        match &self.order {
            Some(cache)
                if cache.generation == model.generation() && cache.query == self.query =>
            {
                Cow::Borrowed(cache.indices.as_slice())
            }
            _ => Cow::Owned(ordered_indices(model, &self.query, &self.recent)),
        }
    }

    fn requery(&mut self, model: &StartModel) {
        self.selection.reset();
        self.first = 0;
        self.refresh(model);
    }

    fn refresh(&mut self, model: &StartModel) {
        let indices = ordered_indices(model, &self.query, &self.recent);
        self.count = indices.len();
        self.order = Some(OrderCache {
            generation: model.generation(),
            query: self.query.clone(),
            indices,
        });
        // A text change invalidates every icon binding made for the old result list.
        crate::async_projection::bump_start_view_epoch();
    }

    /// Row offset of the single selection highlight inside the visible window.
    #[must_use]
    pub fn highlight_y(&self) -> f32 {
        let selected = self.selected();
        ((selected - self.first.min(selected)) as i32 * LAUNCHER_ROW_HEIGHT) as f32
    }

    /// Moves only the selection highlight node. Hover uses this, not `project`, so the
    /// row labels and icons are not rewritten on every selection change.
    pub fn project_highlight(&self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        document.visible("launcher-highlight", self.selection.highlighted().is_some())?;
        document.position("launcher-highlight", 0.0, self.highlight_y())
    }

    /// Writes rows, labels, icons, the selection highlight and the query line.
    pub fn project(
        &self,
        document: &mut impl UiDocumentAccess,
        model: &StartModel,
        mut loader: Option<&mut IconLoader>,
    ) -> Result<(), String> {
        let apps = self.order(model);
        let applications = model.applications();
        let window = self.window();
        document.text_ref(
            "launcher-query-label",
            if self.query.is_empty() {
                "Search..."
            } else {
                self.query.as_str()
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
            if let Some(application) = apps
                .get(window.start + slot)
                .filter(|_| window.start + slot < window.end)
                .map(|&index| &applications[index])
            {
                document.text_ref(&label_id, &application.name)?;
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
            } else {
                document.text(&label_id, String::new())?;
                document.visible(&row_id, false)?;
                document.visible(&icon_id, false)?;
            }
        }
        Ok(())
    }
}

/// Result indices (into `model.applications()`) for a query. An empty query lists recent
/// launches first, then every app in alpha order.
fn ordered_indices(model: &StartModel, query: &str, recent: &[DesktopAppId]) -> Vec<usize> {
    if !query.trim().is_empty() {
        return model.filter_indices(flamewm_shell_core::start_menu::StartCategory::All, query);
    }
    let applications = model.applications();
    let all = model.sorted_order();
    let mut ordered: Vec<usize> = recent
        .iter()
        .filter_map(|id| {
            all.iter()
                .copied()
                .find(|&index| &applications[index].id == id)
        })
        .collect();
    ordered.extend(
        all.iter()
            .copied()
            .filter(|&index| !recent.contains(&applications[index].id)),
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
        // First arrow from no selection lands on row 0; the next step scrolls by one row.
        state.move_by(1);
        state.move_by(LAUNCHER_ROWS as isize);
        assert_eq!(state.window(), 1..LAUNCHER_ROWS + 1);
        assert!(state.select_slot(0));
        assert_eq!(state.selected(), 1, "slot 0 is the first visible row");
        assert_eq!(state.highlight_y(), 0.0);
    }

    #[test]
    fn pointer_leave_clears_hover_and_next_arrow_starts_at_top() {
        let mut state = LauncherState {
            count: 20,
            ..LauncherState::default()
        };
        state.select_slot(3);
        assert!(state.leave(), "leave drops the hover highlight");
        assert!(!state.leave(), "second leave is a no-op");
        assert_eq!(state.selection.highlighted(), None);
        state.move_by(5);
        assert_eq!(state.selected(), 0, "first arrow after leave picks row 0");
    }

    #[test]
    fn empty_result_set_never_moves_selection() {
        let mut state = LauncherState::default();
        state.move_by(3);
        assert_eq!(state.selected(), 0);
        assert_eq!(state.window(), 0..0);
    }

    fn app(id: &str, name: &str) -> DesktopApplication {
        DesktopApplication {
            id: DesktopAppId::new(id),
            name: name.to_owned(),
            generic_name: String::new(),
            comment: String::new(),
            startup_wm_class: String::new(),
            argv: Vec::new(),
            keywords: Vec::new(),
            icon_name: String::new(),
            categories: Vec::new(),
        }
    }

    fn model() -> StartModel {
        StartModel::new(vec![
            app("beta.desktop", "Beta"),
            app("alpha.desktop", "Alpha"),
            app("gamma.desktop", "Gamma Editor"),
        ])
    }

    #[test]
    fn cached_order_follows_each_query_and_matches_uncached_filter() {
        let model = model();
        let mut state = LauncherState::default();
        state.open(&model);
        assert_eq!(state.count(), 3);
        assert_eq!(state.app_at(&model, 0).unwrap().name, "Alpha", "alpha order");

        assert!(state.type_text(&model, "ed"));
        assert_eq!(state.count(), 1);
        assert_eq!(state.app_at(&model, 0).unwrap().name, "Gamma Editor");
        assert_eq!(
            &*state.order(&model),
            ordered_indices(&model, "ed", &[]).as_slice(),
            "cache equals an uncached filter"
        );
        assert!(state.app_at(&model, 1).is_none());
    }

    #[test]
    fn replaced_application_set_bypasses_stale_cache() {
        let mut model = model();
        let mut state = LauncherState::default();
        state.open(&model);
        let before = model.generation();
        model.replace_applications(vec![app("zeta.desktop", "Zeta")]);
        assert_ne!(model.generation(), before, "replacement bumps the generation");
        assert_eq!(
            state.app_at(&model, 0).unwrap().name,
            "Zeta",
            "stale indices are not used after the model changed"
        );
    }

    #[test]
    fn launch_reorders_the_cached_empty_query_list() {
        let model = model();
        let mut state = LauncherState::default();
        state.open(&model);
        state.note_launch(&DesktopAppId::new("gamma.desktop"));
        assert_eq!(state.app_at(&model, 0).unwrap().name, "Gamma Editor");
        assert_eq!(state.app_at(&model, 1).unwrap().name, "Alpha");
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

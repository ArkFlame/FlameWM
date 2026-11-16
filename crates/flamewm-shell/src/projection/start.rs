//! Start menu projection: visible results, category filter, error line and slot lookups.

use super::*;
use flamewm_shell_core::start_menu::StartView;
use std::collections::BTreeMap;

/// The single Start result owner: every projection and click path reads this.
/// A non-empty query searches ALL categories (the category rail is hidden);
/// an empty query shows the selected presentation category.
#[must_use]
pub fn start_visible_results<'a>(
    model: &'a StartModel,
    category: StartCategory,
    query: &str,
) -> Vec<&'a flamewm_api::applications::DesktopApplication> {
    if !query.trim().is_empty() {
        return model.query_view(query);
    }
    let base = model.results_view();
    match category {
        StartCategory::All => base,
        StartCategory::Power => Vec::new(),
        StartCategory::Favorites => {
            model.category_view(flamewm_shell_core::start_menu::StartCategory::Favorites)
        }
        _ => base
            .into_iter()
            .filter(|application| application_matches_category(application, category))
            .collect(),
    }
}

pub fn project_start(
    document: &mut impl UiDocumentAccess,
    model: &StartModel,
    category: StartCategory,
    query: &str,
    session: &SessionCapabilities,
) -> Result<(), String> {
    // Search mode hides the whole category rail (incl. Power) and searches all.
    let searching = !query.trim().is_empty();
    // All Programs replaces the category rail with one alphabetical list.
    let categories_shown = model.view == flamewm_shell_core::start_menu::StartView::Categories;
    document.visible("start-categories", !searching && categories_shown)?;
    project_start_view_toggle(document, model)?;
    // A search ignores the category, so one query serves every rail check and the count.
    let results = start_visible_results(model, category, query);
    for (name, value) in PRESENTATION_CATEGORIES {
        let visible = match value {
            StartCategory::Power => session_available(*session),
            // The Favorites entry exists only while at least one application is starred.
            StartCategory::Favorites => !model.favorites().is_empty(),
            _ if value == category => true,
            _ if searching => !results.is_empty(),
            _ => !start_visible_results(model, value, query).is_empty(),
        };
        document.visible(&format!("start-category-{name}"), visible)?;
    }
    let count = results.len();
    document.text(
        "start-search-label",
        if query.trim().is_empty() {
            "Search applications".to_owned()
        } else {
            format!("{query} ({count})")
        },
    )?;
    Ok(())
}

/// Start error line. Markup must provide this node (hidden by default).
pub const START_STATUS_NODE: &str = "start-status-label";

/// Project the Start error line: shown with `message`, hidden when `None`.
pub fn project_start_error(
    document: &mut impl UiDocumentAccess,
    message: Option<&str>,
) -> Result<(), String> {
    match message {
        Some(message) => {
            document.text_ref(START_STATUS_NODE, message)?;
            document.visible(START_STATUS_NODE, true)
        }
        None => document.visible(START_STATUS_NODE, false),
    }
}

/// Start list row. `Header` rows are letter headings in All Programs: never selectable or
/// launchable, and they take one slot like an app row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgramRow<'a> {
    Header(char),
    App(&'a DesktopApplication),
}

impl ProgramRow<'_> {
    #[must_use]
    pub fn is_app(&self) -> bool {
        matches!(self, Self::App(_))
    }
}

/// Letter heading for a name, case-insensitive: an uppercase letter, or `#` for any name that
/// does not start with a letter.
#[must_use]
pub fn program_bucket(name: &str) -> char {
    match name.trim_start().chars().next() {
        Some(first) if first.is_alphabetic() => first.to_uppercase().next().unwrap_or(first),
        _ => '#',
    }
}

/// Backspace or Escape with an empty query in All Programs returns to Categories.
#[must_use]
pub fn view_after_back(view: StartView, query_empty: bool) -> Option<StartView> {
    (view == StartView::AllPrograms && query_empty).then_some(StartView::Categories)
}

/// Next selectable slot from `cursor` moved by `delta` (sign only), skipping slots where
/// `selectable` is false (headers). `None` at either edge with no selectable slot left.
#[must_use]
pub fn next_selectable_slot(
    selectable: &[bool],
    cursor: Option<usize>,
    delta: isize,
) -> Option<usize> {
    let step = delta.signum();
    if step == 0 {
        return cursor.filter(|&slot| selectable.get(slot).copied().unwrap_or(false));
    }
    let len = isize::try_from(selectable.len()).unwrap_or(isize::MAX);
    let mut position = match cursor {
        Some(slot) => isize::try_from(slot).unwrap_or(isize::MAX),
        None if step > 0 => -1,
        None => len,
    };
    loop {
        position += step;
        if position < 0 || position >= len {
            return None;
        }
        let slot = usize::try_from(position).ok()?;
        if selectable[slot] {
            return Some(slot);
        }
    }
}

/// All Programs rows: letter headers (`#` first, then letters), each bucket in (name, id) order.
fn all_programs_rows<'a>(
    applications: &'a [DesktopApplication],
    order: &[usize],
) -> Vec<ProgramRow<'a>> {
    let mut buckets: BTreeMap<char, Vec<usize>> = BTreeMap::new();
    for &index in order {
        buckets
            .entry(program_bucket(&applications[index].name))
            .or_default()
            .push(index);
    }
    let mut rows = Vec::with_capacity(order.len() + buckets.len());
    for (letter, indices) in buckets {
        rows.push(ProgramRow::Header(letter));
        rows.extend(
            indices
                .into_iter()
                .map(|index| ProgramRow::App(&applications[index])),
        );
    }
    rows
}

/// The Start list as rows: category/search results as apps only, or the All Programs list
/// (letter headers included) when that view is active and no query is typed.
/// `slot` values everywhere index these rows, so a header row occupies one slot.
#[must_use]
pub fn start_program_rows<'a>(
    model: &'a StartModel,
    category: StartCategory,
    query: &str,
) -> Vec<ProgramRow<'a>> {
    if model.view == StartView::AllPrograms && query.trim().is_empty() {
        return all_programs_rows(model.applications(), model.sorted_order());
    }
    start_visible_results(model, category, query)
        .into_iter()
        .map(ProgramRow::App)
        .collect()
}

/// Per-slot selectability for the window of `count` rows starting at `first`: `false` for headers.
#[must_use]
pub fn start_selectable_window(
    model: &StartModel,
    category: StartCategory,
    query: &str,
    first: usize,
    count: usize,
) -> Vec<bool> {
    start_program_rows(model, category, query)
        .iter()
        .skip(first)
        .take(count)
        .map(ProgramRow::is_app)
        .collect()
}

#[must_use]
pub fn start_application_for_slot(
    model: &StartModel,
    category: StartCategory,
    query: &str,
    slot: usize,
) -> Option<flamewm_api::DesktopAppId> {
    match start_program_rows(model, category, query).get(slot)? {
        ProgramRow::App(application) => Some(application.id.clone()),
        ProgramRow::Header(_) => None,
    }
}

/// User-visible name of the application in `slot`, for Start error text. Headers have no name.
#[must_use]
pub fn start_application_name_for_slot(
    model: &StartModel,
    category: StartCategory,
    query: &str,
    slot: usize,
) -> Option<String> {
    match start_program_rows(model, category, query).get(slot)? {
        ProgramRow::App(application) => Some(application.name.clone()),
        ProgramRow::Header(_) => None,
    }
}

/// Toggle label names the destination view; the chevron points there (right to All Programs,
/// left back to Categories).
pub const START_VIEW_TOGGLE_LABEL: &str = "start-view-toggle-label";
pub const START_VIEW_CHEVRON_LEFT: &str = "start-view-chevron-left";
pub const START_VIEW_CHEVRON_RIGHT: &str = "start-view-chevron-right";

/// Projects the All Programs toggle button: label and chevron direction for the current view.
pub fn project_start_view_toggle(
    document: &mut impl UiDocumentAccess,
    model: &StartModel,
) -> Result<(), String> {
    let all_programs = model.view == flamewm_shell_core::start_menu::StartView::AllPrograms;
    document.text(
        START_VIEW_TOGGLE_LABEL,
        if all_programs {
            "Categories".to_owned()
        } else {
            "All Programs".to_owned()
        },
    )?;
    document.visible(START_VIEW_CHEVRON_LEFT, all_programs)?;
    document.visible(START_VIEW_CHEVRON_RIGHT, !all_programs)
}

/// Rows the Start application list moves per wheel tick.
pub const START_WHEEL_ROWS: isize = 3;

/// Largest Start list offset: the list shows `START_APP_SLOT_COUNT` rows at a time.
#[must_use]
pub fn start_scroll_max(total_rows: usize) -> usize {
    total_rows.saturating_sub(crate::async_projection::START_APP_SLOT_COUNT)
}

/// `offset` moved by `delta` rows, clamped to `0..=max`.
#[must_use]
pub fn start_scroll_step(offset: usize, delta: isize, max: usize) -> usize {
    offset.saturating_add_signed(delta).min(max)
}

fn application_matches_category(application: &DesktopApplication, category: StartCategory) -> bool {
    let bucket = match category {
        StartCategory::All => return true,
        // Favorites membership comes from the model, not the presentation bucket.
        StartCategory::Power | StartCategory::Favorites => return false,
        StartCategory::Development => StartBucket::Development,
        StartCategory::Games => StartBucket::Games,
        StartCategory::Graphics => StartBucket::Graphics,
        StartCategory::Internet => StartBucket::Internet,
        StartCategory::Multimedia => StartBucket::Multimedia,
        StartCategory::System => StartBucket::System,
        StartCategory::Utilities => StartBucket::Utilities,
    };
    bucket_for_application(application) == bucket
}

#[cfg(test)]
mod category_tests;

#[cfg(test)]
mod all_programs_tests {
    use super::*;
    use crate::async_projection::tests::{project_start_doc, start_test_app};
    use crate::start::{view_for_action, StartViewAction};

    fn model() -> StartModel {
        StartModel::new(vec![
            start_test_app("org.example.calc", "Calculator", "Utility"),
            start_test_app("org.example.archive", "Archive", "Utility"),
            start_test_app("org.example.chess", "Chess", "Game"),
        ])
    }

    #[test]
    fn all_programs_slots_map_to_apps_and_skip_headers() {
        let mut model = model();
        model.view = StartView::AllPrograms;
        // Rows: #A(0) Archive(1) #C(2) Calculator(3) Chess(4). Headers take a slot.
        let name = |slot| start_application_name_for_slot(&model, StartCategory::All, "", slot);
        assert_eq!(name(0), None);
        assert_eq!(name(1).as_deref(), Some("Archive"));
        assert_eq!(name(2), None);
        assert_eq!(name(4).as_deref(), Some("Chess"));
        assert_eq!(
            start_application_for_slot(&model, StartCategory::All, "", 2),
            None
        );
        assert_eq!(
            start_selectable_window(&model, StartCategory::All, "", 0, 5),
            [false, true, false, true, true]
        );
    }

    #[test]
    fn search_in_all_programs_has_no_headers() {
        let mut model = model();
        model.view = StartView::AllPrograms;
        let name = |slot| start_application_name_for_slot(&model, StartCategory::All, "ca", slot);
        assert_eq!(name(0).as_deref(), Some("Calculator"));
        assert_eq!(name(1), None);
        assert_eq!(
            start_selectable_window(&model, StartCategory::All, "ca", 0, 5),
            [true]
        );
    }

    #[test]
    fn categories_view_keeps_apps_in_every_slot() {
        let model = model();
        assert_eq!(
            start_application_name_for_slot(&model, StartCategory::All, "", 0).as_deref(),
            Some("Archive")
        );
    }

    #[test]
    fn view_actions_parse_and_reject_other_actions() {
        assert_eq!(
            view_for_action("start.view.all"),
            Some(StartViewAction::Show(StartView::AllPrograms))
        );
        assert_eq!(
            view_for_action("start.view.categories"),
            Some(StartViewAction::Show(StartView::Categories))
        );
        assert_eq!(
            view_for_action("start.view.toggle"),
            Some(StartViewAction::Toggle)
        );
        assert_eq!(view_for_action("start.category.games"), None);
    }

    #[test]
    fn projection_reports_view_label_chevron_and_headers() {
        let mut model = model();
        let doc = project_start_doc(&model, StartCategory::All, "");
        assert_eq!(
            doc.texts.get(START_VIEW_TOGGLE_LABEL).map(String::as_str),
            Some("All Programs")
        );
        assert_eq!(doc.visible.get(START_VIEW_CHEVRON_RIGHT), Some(&true));
        assert_eq!(doc.visible.get("start-categories"), Some(&true));

        model.view = StartView::AllPrograms;
        let doc = project_start_doc(&model, StartCategory::All, "");
        assert_eq!(
            doc.texts.get(START_VIEW_TOGGLE_LABEL).map(String::as_str),
            Some("Categories")
        );
        assert_eq!(doc.visible.get(START_VIEW_CHEVRON_LEFT), Some(&true));
        assert_eq!(doc.visible.get("start-categories"), Some(&false));
        assert_eq!(
            doc.texts.get("start-app-slot-1-label").map(String::as_str),
            Some("A")
        );
        assert_eq!(doc.visible.get("start-app-slot-1-icon"), Some(&false));
        assert_eq!(
            doc.texts.get("start-app-slot-2-label").map(String::as_str),
            Some("Archive")
        );
    }
}

#[cfg(test)]
mod all_programs_rule_tests {
    use super::*;
    use crate::async_projection::tests::start_test_app;

    #[test]
    fn all_programs_rows_sort_case_insensitively_under_headers() {
        let mut model = StartModel::new(vec![
            start_test_app("b.desktop", "beta", "Utility"),
            start_test_app("z.desktop", "Zed", "Utility"),
            start_test_app("a.desktop", "Alpha", "Utility"),
            start_test_app("p.desktop", "apple", "Utility"),
            start_test_app("n.desktop", "9Lives", "Utility"),
            start_test_app("t.desktop", "~tilde", "Utility"),
        ]);
        model.view = StartView::AllPrograms;
        let labels: Vec<String> = start_program_rows(&model, StartCategory::All, "")
            .iter()
            .map(|row| match row {
                ProgramRow::Header(letter) => format!("#{letter}"),
                ProgramRow::App(app) => app.name.clone(),
            })
            .collect();
        assert_eq!(
            labels,
            ["##", "9Lives", "~tilde", "#A", "Alpha", "apple", "#B", "beta", "#Z", "Zed"]
        );
    }

    #[test]
    fn bucket_folds_case_and_groups_non_letters() {
        assert_eq!(program_bucket("émile"), 'É');
        assert_eq!(program_bucket("  gimp"), 'G');
        assert_eq!(program_bucket("42"), '#');
        assert_eq!(program_bucket(""), '#');
    }

    #[test]
    fn back_only_leaves_all_programs_with_empty_query() {
        assert_eq!(
            view_after_back(StartView::AllPrograms, true),
            Some(StartView::Categories)
        );
        assert_eq!(view_after_back(StartView::AllPrograms, false), None);
        assert_eq!(view_after_back(StartView::Categories, true), None);
    }

    #[test]
    fn keyboard_steps_skip_header_slots() {
        let selectable = [false, true, false, true];
        assert_eq!(next_selectable_slot(&selectable, None, 1), Some(1));
        assert_eq!(next_selectable_slot(&selectable, Some(1), 1), Some(3));
        assert_eq!(next_selectable_slot(&selectable, Some(3), 1), None);
        assert_eq!(next_selectable_slot(&selectable, None, -1), Some(3));
        assert_eq!(next_selectable_slot(&selectable, Some(3), -1), Some(1));
        assert_eq!(next_selectable_slot(&selectable, Some(1), -1), None);
        assert_eq!(next_selectable_slot(&[false, false], None, 1), None);
    }
}

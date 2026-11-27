//! Start menu projection: visible results, category filter, error line and slot lookups.

use super::*;
use flamewm_shell_core::start_menu::StartView;
use std::fmt::Write as _;

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
    let mut base = model.results_view();
    match category {
        StartCategory::All => base,
        StartCategory::Power => Vec::new(),
        StartCategory::Favorites => {
            model.category_view(flamewm_shell_core::start_menu::StartCategory::Favorites)
        }
        _ => {
            // Retain in place: the filtered view used to be a second Vec over `base`.
            base.retain(|application| application_matches_category(application, category));
            base
        }
    }
}

pub fn project_start(
    document: &mut impl UiDocumentAccess,
    model: &StartModel,
    category: StartCategory,
    query: &str,
    session: &SessionCapabilities,
) -> Result<(), String> {
    let results = start_visible_results(model, category, query);
    project_start_with_results(document, model, category, query, session, &results)
}

/// Same projection as [`project_start`], with the visible results supplied by the caller. A
/// Start projection therefore filters the application set once for rows, rail state and count.
pub(crate) fn project_start_with_results(
    document: &mut impl UiDocumentAccess,
    model: &StartModel,
    category: StartCategory,
    query: &str,
    session: &SessionCapabilities,
    results: &[&DesktopApplication],
) -> Result<(), String> {
    // Search mode hides the whole category rail (incl. Power) and searches all.
    let searching = !query.trim().is_empty();
    // All Programs replaces the category rail with one alphabetical list.
    let categories_shown = model.view == flamewm_shell_core::start_menu::StartView::Categories;
    document.visible("start-categories", !searching && categories_shown)?;
    project_start_view_toggle(document, model)?;
    // A search ignores the category, so one query serves every rail check and the count.
    // Non-search rail checks used to build one filtered Vec per bucket and recompute each
    // application's bucket for every bucket; one pass over the applications serves all nine.
    let mut bucket_present = [false; PRESENTATION_CATEGORIES.len()];
    if !searching {
        let mut rail_buckets = [None; PRESENTATION_CATEGORIES.len()];
        for (slot, (_, value)) in PRESENTATION_CATEGORIES.iter().enumerate() {
            rail_buckets[slot] = category_bucket(*value);
        }
        for application in model.applications() {
            let bucket = bucket_for_application(application);
            for (slot, rail_bucket) in rail_buckets.iter().enumerate() {
                if *rail_bucket == Some(bucket) {
                    bucket_present[slot] = true;
                }
            }
        }
    }
    let mut category_node = String::with_capacity("start-category-".len() + 16);
    for (slot, (name, value)) in PRESENTATION_CATEGORIES.iter().enumerate() {
        let visible = match value {
            StartCategory::Power => session_available(*session),
            // The Favorites entry exists only while at least one application is starred.
            StartCategory::Favorites => !model.favorites().is_empty(),
            _ if *value == category => true,
            _ if searching => !results.is_empty(),
            _ => bucket_present[slot],
        };
        category_node.clear();
        category_node.push_str("start-category-");
        category_node.push_str(name);
        document.visible(&category_node, visible)?;
    }
    let count = results.len();
    if searching {
        let mut label = String::with_capacity(query.len() + 8);
        let _ = write!(&mut label, "{query} ({count})");
        document.text_ref("start-search-label", &label)?;
    } else {
        document.text_ref("start-search-label", "Search applications")?;
    }
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
    /// Power entry pinned at the bottom of All Programs.
    Session(flamewm_api::session::SessionAction),
}

impl ProgramRow<'_> {
    #[must_use]
    pub fn is_app(&self) -> bool {
        matches!(self, Self::App(_))
    }

    /// True for rows the highlight and Enter may land on: apps and session entries.
    #[must_use]
    pub fn is_selectable(&self) -> bool {
        !matches!(self, Self::Header(_))
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

/// Session rows pinned at the bottom of All Programs (Suspend, Reboot, Shutdown).
const SESSION_ROW_COUNT: usize = 3;

/// All Programs rows: letter headers (`#` first, then letters), each bucket in (name, id) order.
fn all_programs_rows<'a>(
    applications: &'a [DesktopApplication],
    order: &[usize],
) -> Vec<ProgramRow<'a>> {
    // A stable sort by bucket key keeps the (name, id) order within a letter and the key order
    // across letters, while replacing a map node and a Vec per letter with one indexed Vec.
    let mut keyed: Vec<(char, usize)> = order
        .iter()
        .map(|&index| (program_bucket(&applications[index].name), index))
        .collect();
    keyed.sort_by_key(|&(letter, _)| letter);
    let headers = keyed
        .windows(2)
        .filter(|pair| pair[0].0 != pair[1].0)
        .count()
        + usize::from(!keyed.is_empty());
    let mut rows = Vec::with_capacity(keyed.len() + headers + SESSION_ROW_COUNT);
    let mut current = None;
    for (letter, index) in keyed {
        if current != Some(letter) {
            rows.push(ProgramRow::Header(letter));
            current = Some(letter);
        }
        rows.push(ProgramRow::App(&applications[index]));
    }
    rows.extend([
        ProgramRow::Session(flamewm_api::session::SessionAction::Suspend),
        ProgramRow::Session(flamewm_api::session::SessionAction::Reboot),
        ProgramRow::Session(flamewm_api::session::SessionAction::Shutdown),
    ]);
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

/// Rows and the app-only results in one pass, for projections that need both. The Start view
/// projection uses this so a scroll or a keystroke does not filter the list twice.
#[must_use]
pub(crate) fn start_program_rows_and_results<'a>(
    model: &'a StartModel,
    category: StartCategory,
    query: &str,
) -> (Vec<ProgramRow<'a>>, Vec<&'a DesktopApplication>) {
    if model.view == StartView::AllPrograms && query.trim().is_empty() {
        // All Programs hides the rail, so the projected document never reads results here;
        // skip building the full application view that would be immediately discarded.
        return (
            all_programs_rows(model.applications(), model.sorted_order()),
            Vec::new(),
        );
    }
    let results = start_visible_results(model, category, query);
    let rows = results.iter().copied().map(ProgramRow::App).collect();
    (rows, results)
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
        .map(|row| row.is_selectable())
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
        ProgramRow::Header(_) | ProgramRow::Session(_) => None,
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
        ProgramRow::Header(_) | ProgramRow::Session(_) => None,
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
    document.text_ref(
        START_VIEW_TOGGLE_LABEL,
        if all_programs {
            "Categories"
        } else {
            "All Programs"
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

/// Presentation bucket of a rail category. `None` means the category has no bucket: All and
/// Favorites/Power are decided by the model or the session, not by an application's categories.
fn category_bucket(category: StartCategory) -> Option<StartBucket> {
    match category {
        StartCategory::All | StartCategory::Power | StartCategory::Favorites => None,
        StartCategory::Development => Some(StartBucket::Development),
        StartCategory::Games => Some(StartBucket::Games),
        StartCategory::Graphics => Some(StartBucket::Graphics),
        StartCategory::Internet => Some(StartBucket::Internet),
        StartCategory::Multimedia => Some(StartBucket::Multimedia),
        StartCategory::System => Some(StartBucket::System),
        StartCategory::Utilities => Some(StartBucket::Utilities),
    }
}

fn application_matches_category(application: &DesktopApplication, category: StartCategory) -> bool {
    match category_bucket(category) {
        None => category == StartCategory::All,
        // Favorites membership comes from the model, not the presentation bucket.
        Some(bucket) => bucket_for_application(application) == bucket,
    }
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
                ProgramRow::Session(_) => "session".to_owned(),
            })
            .collect();
        assert_eq!(
            labels,
            [
                "##", "9Lives", "~tilde", "#A", "Alpha", "apple", "#B", "beta", "#Z", "Zed",
                "session", "session", "session"
            ]
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

//! Start menu projection: unified start view, highlight and view notes.
use super::*;

/// Pixel height of one Start application row in the virtual list.
const START_APP_ROW_HEIGHT: u32 = 36;

#[derive(Debug, Default)]
pub struct StartViewNote {
    last_category: Option<StartCategory>,
    last_query: Option<String>,
}

impl StartViewNote {
    /// Returns true when the view semantically changed and needs reprojection.
    pub fn note_start_view(&mut self, category: StartCategory, query: &str) -> bool {
        if self.last_category == Some(category) && self.last_query.as_deref() == Some(query) {
            redundant_hover_counter().increment();
            return false;
        }
        self.last_category = Some(category);
        self.last_query = Some(query.to_owned());
        true
    }
}

/// Unified Start document projection (J04/J07): categories + search +
/// app rows + power rows in ONE document on the single Start surface.
/// Labels/visibility immediate; icons cached-or-Flame-fallback with queued
/// HIGH worker requests. Never resolves on the caller thread.
pub fn project_start_unified(
    document: &mut impl UiDocumentAccess,
    model: &StartModel,
    category: StartCategory,
    query: &str,
    session: &SessionCapabilities,
    mut loader: Option<&mut IconLoader>,
    scroll_rows: usize,
) -> Result<(), String> {
    bump_start_view_epoch(); // Canonical indexed path: borrowed query view from the precomputed
    // index, then the single presentation-bucket filter. No clone, no
    // set_query-on-temporary. (Shell StartCategory is the presentation
    // seam; core filter_view takes the core category type.)
    let applications = crate::projection::start_visible_results(model, category, query);
    document.visible("start-applications", !applications.is_empty())?;
    // Clamp here so the rows painted always match `start_scroll_rows` for slot mapping.
    let scroll_rows = crate::projection::start_scroll_step(
        scroll_rows,
        0,
        crate::projection::start_scroll_max(applications.len()),
    );
    let scroll_px = u32::try_from(scroll_rows.saturating_mul(START_APP_ROW_HEIGHT as usize))
        .unwrap_or(u32::MAX);
    let (first, last) = flamewm_shell_core::status::visible_window(
        applications.len(),
        START_APP_ROW_HEIGHT,
        320,
        scroll_px,
    );
    let window = applications
        .get(first..last.min(applications.len()))
        .unwrap_or_default();
    let (mut button_id, mut label_id, mut icon_id) = (String::new(), String::new(), String::new());
    for index in 0..START_APP_SLOT_COUNT {
        let slot = index + 1;
        button_id.clear();
        write!(button_id, "start-app-slot-{slot}").expect("write to String is infallible");
        label_id.clear();
        write!(label_id, "start-app-slot-{slot}-label").expect("write to String is infallible");
        icon_id.clear();
        write!(icon_id, "start-app-slot-{slot}-icon").expect("write to String is infallible");
        if let Some(application) = window.get(index) {
            document.text_ref(&label_id, &application.name)?;
            document.visible(&button_id, true)?;
            document.visible(&icon_id, true)?;
            schedule_visible(
                document,
                &icon_id,
                &mut loader.as_deref_mut(),
                IconTarget::StartSlot(index as u8),
                &application.icon_name,
                20,
                20,
            )?;
        } else {
            document.text(&label_id, String::new())?;
            document.visible(&button_id, false)?;
            document.visible(&icon_id, false)?;
        }
    }
    let show_session = query.trim().is_empty()
        && category == StartCategory::Power
        && crate::projection::session_available(*session);
    document.visible("start-group-power", show_session)?;
    document.visible("start-session-lock", show_session && session.lock)?;
    document.visible("start-session-logout", show_session && session.logout)?;
    document.visible("start-session-suspend", show_session && session.suspend)?;
    document.visible("start-session-reboot", show_session && session.reboot)?;
    document.visible("start-session-shutdown", show_session && session.shutdown)?;
    // Category rail + search label live in the same unified document, so
    // one intrinsic union covers the whole Start surface.
    crate::projection::project_start(document, model, category, query, session)?;
    Ok(())
}

/// Paints the highlighted Start row (keyboard or hover); every other row is cleared.
/// `None` means no selection, so all rows are cleared.
pub fn project_start_highlight(
    document: &mut impl UiDocumentAccess,
    highlighted: Option<usize>,
) -> Result<(), String> {
    let accent = crate::projection::task_accent();
    for index in 0..START_APP_SLOT_COUNT {
        let color = if Some(index) == highlighted {
            UiColor { a: 0x38, ..accent }
        } else {
            UiColor { a: 0, ..accent }
        };
        document.background(&format!("start-app-slot-{}", index + 1), color)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::async_projection::tests::*;

    #[test]
    fn async_start_submenu_hides_empty_slots() {
        let _guard = icon_test_lock().lock();
        let doc = project_start_doc(&StartModel::new(Vec::new()), StartCategory::All, "");
        assert_eq!(doc.visible.get("start-app-slot-1"), Some(&false));
    }

    #[test]
    fn search_under_category_finds_other_categories_and_hides_rail() {
        let _guard = icon_test_lock().lock();
        let model = StartModel::new(vec![
            start_test_app("org.example.calc", "Calculator", "Utility"),
            start_test_app("org.example.chess", "Chess", "Game"),
        ]);
        let doc = project_start_doc(&model, StartCategory::Games, "calc");
        assert_eq!(
            doc.texts.get("start-app-slot-1-label").map(String::as_str),
            Some("Calculator"),
            "search ignores the selected category"
        );
        assert_eq!(doc.visible.get("start-app-slot-2"), Some(&false));
        assert_eq!(doc.visible.get("start-categories"), Some(&false));
        assert_eq!(doc.visible.get("start-group-power"), Some(&false));
    }

    #[test]
    fn start_click_slot_maps_to_the_projected_row() {
        let _guard = icon_test_lock().lock();
        let model = StartModel::new(vec![
            start_test_app("org.example.calc", "Calculator", "Utility"),
            start_test_app("org.example.chess", "Chess", "Game"),
        ]);
        let doc = project_start_doc(&model, StartCategory::Games, "calc");
        let projected = doc.texts.get("start-app-slot-1-label").cloned();
        assert_eq!(
            crate::projection::start_application_name_for_slot(
                &model,
                StartCategory::Games,
                "calc",
                0
            ),
            projected,
            "click on slot 1 launches the row that was projected"
        );
    }

    #[test]
    fn empty_query_restores_selected_category_view() {
        let _guard = icon_test_lock().lock();
        let model = StartModel::new(vec![
            start_test_app("org.example.calc", "Calculator", "Utility"),
            start_test_app("org.example.chess", "Chess", "Game"),
        ]);
        let doc = project_start_doc(&model, StartCategory::Games, "");
        assert_eq!(
            doc.texts.get("start-app-slot-1-label").map(String::as_str),
            Some("Chess")
        );
        assert_eq!(doc.visible.get("start-app-slot-2"), Some(&false));
        assert_eq!(doc.visible.get("start-categories"), Some(&true));
    }

    #[test]
    fn start_content_pane_keeps_fixed_height_across_search_states() {
        // Search hides the category rail, so the content pane is the only tall
        // child left. Its min-height must equal its max-height (the shared
        // 420px cap) or the Start outer height drops during search and
        // zero-result states. The search bar stays bottom-anchored via auto top margin.
        let css = include_str!("../../../../ui/shell/flamewm.css");
        let rule = |selector: &str| -> &str {
            let start = css
                .find(&format!("{selector} {{"))
                .unwrap_or_else(|| panic!("missing rule {selector}"));
            let end = css[start..].find('}').expect("rule end") + start;
            &css[start..end]
        };
        let pane = rule(".start-content-pane");
        assert!(pane.contains("min-height:420px"), "{pane}");
        assert!(pane.contains("max-height:420px"), "{pane}");
        assert!(rule(".start-left-pane").contains("justify-content:flex-end"));
    }

    #[test]
    fn start_view_note_gates_redundant_hover() {
        let mut note = StartViewNote::default();
        assert!(note.note_start_view(StartCategory::All, ""));
        assert!(!note.note_start_view(StartCategory::All, ""));
        assert!(note.note_start_view(StartCategory::Games, ""));
    }

    #[test]
    fn popup_roles_are_distinct() {
        let roles = [
            flamewm_ui_x11::SurfaceRole::PopupMenu,
            flamewm_ui_x11::SurfaceRole::DropdownMenu,
        ];
        assert_ne!(roles[0], roles[1]);
        let _ = flamewm_ui_x11::SurfaceRole::Dock;
    }
}

#[cfg(test)]
mod start_selection_tests {
    use flamewm_shell_core::selection::ListSelection;

    #[test]
    fn selection_clamps_to_visible_rows() {
        let mut state = ListSelection::default();
        state.step(1, 3);
        assert_eq!(state.highlighted(), Some(0));
        state.step(1, 3);
        state.step(1, 3);
        state.step(1, 3);
        assert_eq!(state.highlighted(), Some(2));
        state.step(-1, 0);
        assert_eq!(state.highlighted(), None, "empty list clears the highlight");
    }
}

#[cfg(test)]
mod start_scroll_tests {
    use super::*;
    use crate::async_projection::tests::*;

    const START_HTML: &str = include_str!("../../../../ui/shell/start.html");
    use crate::projection::{start_application_name_for_slot, start_scroll_max, start_scroll_step};

    fn numbered_apps(count: usize, category: &str) -> StartModel {
        StartModel::new(
            (1..=count)
                .map(|n| {
                    start_test_app(
                        &format!("org.example.app{n:02}"),
                        &format!("App{n:02}"),
                        category,
                    )
                })
                .collect(),
        )
    }

    #[test]
    fn scroll_offset_clamps_to_row_range() {
        assert_eq!(start_scroll_max(0), 0);
        assert_eq!(start_scroll_max(8), 0);
        assert_eq!(start_scroll_max(11), 3);
        assert_eq!(start_scroll_step(0, -3, 3), 0, "wheel up at top stays at 0");
        assert_eq!(start_scroll_step(1, 3, 3), 3, "wheel down clamps to max");
        assert_eq!(
            start_scroll_step(3, 3, 3),
            3,
            "wheel down at bottom stays at max"
        );
        assert_eq!(
            start_scroll_step(5, 0, 2),
            2,
            "stale offset after list shrinks"
        );
    }

    #[test]
    fn projected_rows_and_slot_mapping_follow_the_offset() {
        let _guard = icon_test_lock().lock();
        let model = numbered_apps(12, "Utility");
        let doc = {
            let mut doc = FakeDoc::default();
            project_start_unified(
                &mut doc,
                &model,
                StartCategory::All,
                "",
                &SessionCapabilities::default(),
                None,
                3,
            )
            .expect("projects");
            doc
        };
        for slot in 0..START_APP_SLOT_COUNT {
            let label = format!("start-app-slot-{}-label", slot + 1);
            let expected =
                start_application_name_for_slot(&model, StartCategory::All, "", slot + 3);
            assert_eq!(
                doc.texts.get(&label).cloned(),
                expected,
                "slot {slot} at offset 3"
            );
        }
        assert_eq!(
            start_application_name_for_slot(&model, StartCategory::All, "", 3).as_deref(),
            Some("App04")
        );
    }

    #[test]
    fn oversized_offset_is_clamped_when_projected() {
        let _guard = icon_test_lock().lock();
        let model = numbered_apps(12, "Utility");
        let mut doc = FakeDoc::default();
        project_start_unified(
            &mut doc,
            &model,
            StartCategory::All,
            "",
            &SessionCapabilities::default(),
            None,
            99,
        )
        .expect("projects");
        assert_eq!(
            doc.texts.get("start-app-slot-1-label").map(String::as_str),
            Some("App05"),
            "offset 99 clamps to 12 - 8 = 4 rows"
        );
        assert_eq!(
            doc.texts.get("start-app-slot-8-label").map(String::as_str),
            Some("App12")
        );
    }

    #[test]
    fn favorites_offset_maps_to_the_favorite_row() {
        let _guard = icon_test_lock().lock();
        let mut model = numbered_apps(12, "Utility");
        model.set_favorites((1..=10).map(|n| format!("org.example.app{n:02}")).collect());
        let mut doc = FakeDoc::default();
        project_start_unified(
            &mut doc,
            &model,
            StartCategory::Favorites,
            "",
            &SessionCapabilities::default(),
            None,
            2,
        )
        .expect("projects");
        let projected = doc.texts.get("start-app-slot-1-label").cloned();
        assert_eq!(
            start_application_name_for_slot(&model, StartCategory::Favorites, "", 2),
            projected
        );
        let app =
            crate::projection::start_application_for_slot(&model, StartCategory::Favorites, "", 2)
                .expect("third favorite");
        assert!(model.favorites().iter().any(|id| id == app.as_str()));
        assert_ne!(
            projected.as_deref(),
            Some("App01"),
            "offset skips the first favorite"
        );
    }

    #[test]
    fn favorite_rows_have_no_star_slot_and_keep_their_launch_action() {
        let _guard = icon_test_lock().lock();
        let mut model = StartModel::new(vec![
            start_test_app("org.example.calc", "Calculator", "Utility"),
            start_test_app("org.example.chess", "Chess", "Game"),
        ]);
        model.set_favorites(vec!["org.example.chess".into()]);
        let doc = project_start_doc(&model, StartCategory::All, "");
        assert_eq!(doc.visible.get("start-app-slot-1"), Some(&true));
        assert_eq!(doc.visible.get("start-app-slot-2"), Some(&true));
        assert!(!doc.visible.contains_key("start-fav-slot-1"));
        assert!(!START_HTML.contains("start-fav-"));
        assert!(START_HTML.contains("data-action=\"start.app.1\""));
    }

    #[test]
    fn favorites_category_is_listed_only_while_it_has_favorites() {
        let _guard = icon_test_lock().lock();
        let mut model = StartModel::new(vec![start_test_app(
            "org.example.calc",
            "Calculator",
            "Utility",
        )]);
        let doc = project_start_doc(&model, StartCategory::All, "");
        assert_eq!(doc.visible.get("start-category-favorites"), Some(&false));
        model.set_favorites(vec!["org.example.calc".into()]);
        let doc = project_start_doc(&model, StartCategory::All, "");
        assert_eq!(doc.visible.get("start-category-favorites"), Some(&true));
    }
}

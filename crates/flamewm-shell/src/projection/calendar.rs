use std::fmt::Write as _;

use super::*;

// Today: neutral translucent white circle on its marker (or Months/Years tile). Only the active
// view tab carries the opaque neutral gray fill; the selected date/month/year has no fill.
const TODAY_NEUTRAL: UiColor = UiColor {
    r: 255,
    g: 255,
    b: 255,
    a: 56,
};
pub(super) const ACTIVE_TAB_FILL: UiColor = UiColor {
    r: 64,
    g: 68,
    b: 72,
    a: 255,
};

pub fn project_calendar(
    document: &mut impl UiDocumentAccess,
    state: &crate::calendar::CalendarState,
    today: (i32, u8, u8),
) -> Result<(), String> {
    // Adjacent-month cells render dim (cleared marker foreground).
    use crate::calendar::{year_page_start, CalendarView};
    const ADJACENT_FG: UiColor = UiColor {
        r: 98,
        g: 104,
        b: 109,
        a: 255,
    };
    const CURRENT_FG: UiColor = UiColor {
        r: 240,
        g: 240,
        b: 240,
        a: 255,
    };
    let view = state.view;
    document.visible("calendar-week", view == CalendarView::Days)?;
    document.visible("calendar-days", view == CalendarView::Days)?;
    document.visible("calendar-months", view == CalendarView::Months)?;
    document.visible("calendar-years", view == CalendarView::Years)?;
    for (id, active) in [
        ("calendar-tab-days", CalendarView::Days),
        ("calendar-tab-months", CalendarView::Months),
        ("calendar-tab-years", CalendarView::Years),
    ]
    .map(|(id, tab)| (id, tab == view))
    {
        if active {
            document.background(id, ACTIVE_TAB_FILL)?;
        } else {
            document.background_clear(id)?;
        }
    }
    document.text("calendar-month", month_name(state.month).to_owned())?;
    document.text("calendar-year", state.year.to_string())?;
    match view {
        CalendarView::Days => {
            let grid = state
                .grid(today)
                .ok_or_else(|| "invalid calendar month".to_owned())?;
            let (mut cell_id, mut marker) = (String::new(), String::new());
            for (index, cell) in grid.cells.iter().enumerate() {
                cell_id.clear();
                write!(cell_id, "calendar-day-{}", index + 1)
                    .expect("write to String is infallible");
                marker.clear();
                write!(marker, "calendar-day-{}-marker", index + 1)
                    .expect("write to String is infallible");
                document.text(&marker, cell.day.to_string())?;
                if cell.month_kind == flamewm_shell_core::CalendarMonth::Current {
                    document.foreground(&marker, CURRENT_FG)?;
                } else {
                    document.foreground(&marker, ADJACENT_FG)?;
                }
                let is_today = cell.today && cell.year == grid.year && cell.month == grid.month;
                if is_today {
                    document.background(&marker, TODAY_NEUTRAL)?;
                } else {
                    document.background_clear(&marker)?;
                }
                document.background_clear(&cell_id)?;
            }
        }
        CalendarView::Months => {
            for month in 1..=12u8 {
                let id = format!("calendar-pick-month-{month}");
                document.text(&id, month_name(month).to_owned())?;
                document.foreground(&id, CURRENT_FG)?;
                if state.year == today.0 && month == today.1 {
                    document.background(&id, TODAY_NEUTRAL)?;
                } else {
                    document.background_clear(&id)?;
                }
            }
        }
        CalendarView::Years => {
            let start = year_page_start(state.year);
            for offset in 0..12i32 {
                let year = start + offset;
                let id = format!("calendar-pick-year-{}", offset + 1);
                document.text(&id, year.to_string())?;
                document.foreground(&id, CURRENT_FG)?;
                if year == today.0 {
                    document.background(&id, TODAY_NEUTRAL)?;
                } else {
                    document.background_clear(&id)?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod calendar_tests {
    use super::*;
    use flamewm_ui_x11::test_support::RecordingUiDoc;

    /// Flame accent red (#ef4048 family) that must not appear in any calendar projection.
    const FLAME_RED: UiColor = UiColor {
        r: 239,
        g: 64,
        b: 72,
        a: 230,
    };

    #[test]
    fn taskbar_css_height_follows_logical_size_with_44_default() {
        assert_eq!(taskbar_css_height(0), 44.0);
        assert_eq!(taskbar_css_height(44), 44.0);
        assert_eq!(taskbar_css_height(72), 72.0);
        assert_eq!(taskbar_css_height(34), 34.0);
    }

    /// Every color the projection wrote, across backgrounds and foregrounds.
    fn all_colors(document: &RecordingUiDoc) -> Vec<UiColor> {
        document
            .backgrounds
            .values()
            .chain(document.foregrounds.values())
            .copied()
            .collect()
    }

    /// CSS rule body for a `#clock-popup .<class>` selector in calendar.html.
    fn css_rule(class: &str) -> &'static str {
        let source = include_str!("../../../../ui/shell/calendar.html");
        let head = format!("#clock-popup .{class} {{");
        let start = source
            .find(&head)
            .unwrap_or_else(|| panic!("missing rule {class}"))
            + head.len();
        let end = start + source[start..].find('}').expect("rule end");
        &source[start..end]
    }

    fn css_px(rule: &str, property: &str) -> i32 {
        let key = format!("{property}:");
        let start = rule
            .find(&key)
            .unwrap_or_else(|| panic!("missing {property}"))
            + key.len();
        let value = rule[start..].split(';').next().expect("value").trim();
        value.trim_end_matches("px").parse().expect("px value")
    }

    #[test]
    fn today_marker_is_neutral_translucent_fill() {
        let today = (2026, 9, 7);
        let state = crate::calendar::CalendarState::for_today(today);
        let grid = state.grid(today).expect("valid grid");
        let today_index = grid
            .cells
            .iter()
            .position(|cell| cell.today && cell.year == 2026 && cell.month == 9)
            .expect("known date is a current-month cell");
        let mut document = RecordingUiDoc::default();
        project_calendar(&mut document, &state, today).expect("projection succeeds");
        let marker = format!("calendar-day-{}-marker", today_index + 1);
        assert_eq!(document.backgrounds.get(&marker), Some(&TODAY_NEUTRAL));
        assert_eq!(TODAY_NEUTRAL.r, TODAY_NEUTRAL.g);
        assert_eq!(TODAY_NEUTRAL.g, TODAY_NEUTRAL.b);
        assert!(TODAY_NEUTRAL.a < 255, "today fill is translucent, not solid");
    }

    #[test]
    fn no_flame_red_in_any_calendar_view() {
        let today = (2026, 10, 13);
        for view in ["calendar.view.days", "calendar.view.months", "calendar.view.years"] {
            let mut state = crate::calendar::CalendarState::for_today(today);
            state.apply(view, today);
            let mut document = RecordingUiDoc::default();
            project_calendar(&mut document, &state, today).expect("projection succeeds");
            let colors = all_colors(&document);
            assert!(!colors.is_empty(), "{view} projects colors");
            assert!(
                !colors.contains(&FLAME_RED),
                "{view} must not project Flame red"
            );
            assert!(
                colors.iter().all(|c| !(c.r > 200 && c.g < 100)),
                "{view} must not project any red-dominant color"
            );
        }
    }

    /// Asserts every projected pick tile carries no fill except the today marker.
    fn assert_only_today_filled(document: &RecordingUiDoc, ids: impl Iterator<Item = String>) {
        for id in ids {
            match document.backgrounds.get(&id) {
                None => {}
                Some(color) => assert_eq!(*color, TODAY_NEUTRAL, "{id} has a non-today fill"),
            }
        }
    }

    #[test]
    fn selected_month_has_no_fill_and_only_today_is_marked() {
        let today = (2026, 10, 13);
        let mut state = crate::calendar::CalendarState::for_today(today);
        state.apply("calendar.view.months", today);
        state.month = 3;
        let mut document = RecordingUiDoc::default();
        project_calendar(&mut document, &state, today).expect("projection succeeds");
        assert_only_today_filled(&document, (1..=12u8).map(|m| format!("calendar-pick-month-{m}")));
        assert_eq!(document.backgrounds.get("calendar-pick-month-10"), Some(&TODAY_NEUTRAL));
        assert!(document.cleared.contains(&"calendar-pick-month-3".to_owned()));
    }

    #[test]
    fn selected_year_has_no_fill_and_only_today_is_marked() {
        let today = (2026, 10, 13);
        let mut state = crate::calendar::CalendarState::for_today(today);
        state.apply("calendar.view.years", today);
        state.year = 2027;
        let mut document = RecordingUiDoc::default();
        project_calendar(&mut document, &state, today).expect("projection succeeds");
        assert_only_today_filled(&document, (1..=12).map(|s| format!("calendar-pick-year-{s}")));
        let slot = today.0 - crate::calendar::year_page_start(today.0) + 1;
        assert_eq!(
            document.backgrounds.get(&format!("calendar-pick-year-{slot}")),
            Some(&TODAY_NEUTRAL),
        );
    }

    #[test]
    fn selected_day_cell_has_no_fill() {
        let today = (2026, 10, 13);
        let mut state = crate::calendar::CalendarState::for_today(today);
        state.selected = Some((2026, 10, 20));
        let grid = state.grid(today).expect("valid grid");
        let index = grid
            .cells
            .iter()
            .position(|cell| cell.year == 2026 && cell.month == 10 && cell.day == 20)
            .expect("selected date is in grid");
        let mut document = RecordingUiDoc::default();
        project_calendar(&mut document, &state, today).expect("projection succeeds");
        let cell = format!("calendar-day-{}", index + 1);
        assert_eq!(document.backgrounds.get(&cell), None, "selected cell has no fill");
        assert!(document.cleared.contains(&cell), "selected cell paint is cleared");
    }

    #[test]
    fn active_view_tab_keeps_fill_and_inactive_tabs_are_cleared() {
        let today = (2026, 10, 13);
        for (view, active) in [
            ("calendar.view.days", "calendar-tab-days"),
            ("calendar.view.months", "calendar-tab-months"),
            ("calendar.view.years", "calendar-tab-years"),
        ] {
            let mut state = crate::calendar::CalendarState::for_today(today);
            state.apply(view, today);
            let mut document = RecordingUiDoc::default();
            project_calendar(&mut document, &state, today).expect("projection succeeds");
            assert_eq!(document.backgrounds.get(active), Some(&ACTIVE_TAB_FILL), "{view}");
            for tab in ["calendar-tab-days", "calendar-tab-months", "calendar-tab-years"]
                .into_iter()
                .filter(|tab| *tab != active)
            {
                assert_eq!(document.backgrounds.get(tab), None, "{tab} inactive");
            }
        }
    }

    #[test]
    fn today_marker_is_a_centered_equal_circle() {
        let rule = css_rule("calendar-day-marker");
        let width = css_px(rule, "width");
        let height = css_px(rule, "height");
        assert_eq!(width, height, "marker must be square");
        assert_eq!(css_px(rule, "border-radius"), width / 2, "radius = w/2");
    }

    #[test]
    fn today_marker_styling_differs_from_day_cell() {
        let marker = css_rule("calendar-day-marker");
        let tile = css_rule("calendar-day");
        assert_ne!(
            css_px(marker, "border-radius"),
            css_px(tile, "border-radius"),
            "today circle is not the rounded day cell",
        );
        assert_ne!(TODAY_NEUTRAL, ACTIVE_TAB_FILL);
    }

    #[test]
    fn today_marker_stays_while_selected_cell_has_no_fill() {
        let today = (2026, 9, 7);
        let state = crate::calendar::CalendarState::for_today(today);
        let grid = state.grid(today).expect("valid grid");
        let today_index = grid
            .cells
            .iter()
            .position(|cell| cell.today && cell.year == 2026 && cell.month == 9)
            .expect("known date is a current-month cell");
        let mut document = RecordingUiDoc::default();
        project_calendar(&mut document, &state, today).expect("projection succeeds");
        let marker = format!("calendar-day-{}-marker", today_index + 1);
        let tile = format!("calendar-day-{}", today_index + 1);
        assert_eq!(document.backgrounds.get(&marker), Some(&TODAY_NEUTRAL));
        assert_eq!(document.backgrounds.get(&tile), None, "selected cell has no fill");
        let red_markers = document
            .backgrounds
            .iter()
            .filter(|(id, color)| id.ends_with("-marker") && **color == TODAY_NEUTRAL)
            .count();
        assert_eq!(red_markers, 1, "exactly one today marker");
        for (index, cell) in grid.cells.iter().enumerate() {
            let id = format!("calendar-day-{}-marker", index + 1);
            assert_eq!(
                document.texts.get(&id).map(String::as_str),
                Some(cell.day.to_string()).as_deref(),
                "day text lives on the marker, not the cell",
            );
            if index != today_index {
                assert!(
                    document.cleared.contains(&id),
                    "non-today marker {id} is cleared"
                );
            }
            if cell.month_kind != flamewm_shell_core::CalendarMonth::Current {
                assert!(
                    document.cleared.contains(&id),
                    "adjacent-month marker {id} is never marked",
                );
            }
        }
        assert!(
            !document.cleared.contains(&marker),
            "today marker keeps its fill"
        );
    }

    #[test]
    fn reprojection_clears_previous_marker_and_moves_selection() {
        let mut document = RecordingUiDoc::default();
        let first = crate::calendar::CalendarState::for_today((2026, 9, 7));
        project_calendar(&mut document, &first, (2026, 9, 7)).expect("first projection succeeds");
        let second = crate::calendar::CalendarState::for_today((2026, 9, 8));
        project_calendar(&mut document, &second, (2026, 9, 8)).expect("second projection succeeds");
        // Sep 1 2026 is a Tuesday: Sep 7 is cell 8 in the Monday-first grid.
        assert!(
            document
                .cleared
                .iter()
                .any(|id| id == "calendar-day-8-marker"),
            "previous marker is cleared on reprojection",
        );
        assert_eq!(
            document.backgrounds.get("calendar-day-9-marker"),
            Some(&TODAY_NEUTRAL),
        );
        assert_eq!(document.backgrounds.get("calendar-day-9"), None, "selected cell has no fill");
        assert!(document.cleared.iter().any(|id| id == "calendar-day-9"));
    }

    #[test]
    fn months_view_marks_current_month_neutral() {
        let today = (2026, 10, 13);
        let mut state = crate::calendar::CalendarState::for_today(today);
        state.apply("calendar.view.months", today);
        let mut document = RecordingUiDoc::default();
        project_calendar(&mut document, &state, today).expect("projection succeeds");
        assert_eq!(
            document
                .texts
                .get("calendar-pick-month-10")
                .map(String::as_str),
            Some("October"),
        );
        assert_eq!(
            document.backgrounds.get("calendar-pick-month-10"),
            Some(&TODAY_NEUTRAL),
        );
    }
}

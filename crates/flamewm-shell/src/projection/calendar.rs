use super::*;

pub fn project_calendar(
    document: &mut impl UiDocumentAccess,
    state: &crate::calendar::CalendarState,
    today: (i32, u8, u8),
) -> Result<(), String> {
    // Today: accent red circle on its marker (or Months/Years tile). Selected:
    // neutral rounded gray tile on the cell, so both can show together.
    // Adjacent-month cells render dim (cleared marker foreground).
    use crate::calendar::{year_page_start, CalendarView};
    const FLAME_RED: UiColor = UiColor {
        r: 239,
        g: 64,
        b: 72,
        a: 255,
    };
    const SELECTED_TILE: UiColor = UiColor {
        r: 64,
        g: 68,
        b: 72,
        a: 255,
    };
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
            document.background(id, SELECTED_TILE)?;
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
            for (index, cell) in grid.cells.iter().enumerate() {
                let cell_id = format!("calendar-day-{}", index + 1);
                let marker = format!("calendar-day-{}-marker", index + 1);
                document.text(&marker, cell.day.to_string())?;
                if cell.month_kind == flamewm_shell_core::CalendarMonth::Current {
                    document.foreground(&marker, CURRENT_FG)?;
                } else {
                    document.foreground(&marker, ADJACENT_FG)?;
                }
                let is_today = cell.today && cell.year == grid.year && cell.month == grid.month;
                if is_today {
                    document.background(&marker, FLAME_RED)?;
                } else {
                    document.background_clear(&marker)?;
                }
                if state.selected == Some((cell.year, cell.month, cell.day)) {
                    document.background(&cell_id, SELECTED_TILE)?;
                } else {
                    document.background_clear(&cell_id)?;
                }
            }
        }
        CalendarView::Months => {
            for month in 1..=12u8 {
                let id = format!("calendar-pick-month-{month}");
                document.text(&id, month_name(month).to_owned())?;
                document.foreground(&id, CURRENT_FG)?;
                if state.year == today.0 && month == today.1 {
                    document.background(&id, FLAME_RED)?;
                } else if month == state.month {
                    document.background(&id, SELECTED_TILE)?;
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
                    document.background(&id, FLAME_RED)?;
                } else if year == state.year {
                    document.background(&id, SELECTED_TILE)?;
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
    use std::collections::HashMap;

    #[test]
    fn taskbar_css_height_follows_logical_size_with_44_default() {
        assert_eq!(taskbar_css_height(0), 44.0);
        assert_eq!(taskbar_css_height(44), 44.0);
        assert_eq!(taskbar_css_height(72), 72.0);
        assert_eq!(taskbar_css_height(34), 34.0);
    }

    #[derive(Default)]
    struct FakeDocument {
        texts: HashMap<String, String>,
        backgrounds: HashMap<String, UiColor>,
        foregrounds: HashMap<String, UiColor>,
        cleared: Vec<String>,
    }

    impl UiDocumentAccess for FakeDocument {
        fn text(&mut self, id: &str, text: String) -> Result<(), String> {
            self.texts.insert(id.to_owned(), text);
            Ok(())
        }
        fn image_rgb8(&mut self, _id: &str, _image: RuntimeImage) -> Result<(), String> {
            Ok(())
        }
        fn image_rgba8(&mut self, _id: &str, _image: RuntimeImage) -> Result<(), String> {
            Ok(())
        }
        fn visible(&mut self, _id: &str, _visible: bool) -> Result<(), String> {
            Ok(())
        }
        fn toggle(&mut self, _track: &str, _knob: &str, _checked: bool) -> Result<(), String> {
            Ok(())
        }
        fn position(&mut self, _id: &str, _left: f32, _top: f32) -> Result<(), String> {
            Ok(())
        }
        fn size(&mut self, _id: &str, _width: f32, _height: f32) -> Result<(), String> {
            Ok(())
        }
        fn background(&mut self, id: &str, color: UiColor) -> Result<(), String> {
            self.backgrounds.insert(id.to_owned(), color);
            Ok(())
        }
        fn border(&mut self, _id: &str, _color: UiColor) -> Result<(), String> {
            Ok(())
        }
        fn foreground(&mut self, id: &str, color: UiColor) -> Result<(), String> {
            self.foregrounds.insert(id.to_owned(), color);
            Ok(())
        }
        fn foreground_clear(&mut self, _id: &str) -> Result<(), String> {
            Ok(())
        }
        fn background_clear(&mut self, id: &str) -> Result<(), String> {
            self.backgrounds.remove(id);
            self.cleared.push(id.to_owned());
            Ok(())
        }
        fn border_clear(&mut self, _id: &str) -> Result<(), String> {
            Ok(())
        }
        fn layer(
            &mut self,
            _id: &str,
            _layer: flamewm_ui_core::style::UiLayer,
        ) -> Result<(), String> {
            Ok(())
        }
        fn overflow(
            &mut self,
            _id: &str,
            _x: flamewm_ui_x11::Overflow,
            _y: flamewm_ui_x11::Overflow,
        ) -> Result<(), String> {
            Ok(())
        }
        fn font_size(&mut self, _: &str, _: f32) -> Result<(), String> {
            Ok(())
        }
        fn font_weight(&mut self, _: &str, _: u16) -> Result<(), String> {
            Ok(())
        }
    }

    const FLAME_RED: UiColor = UiColor {
        r: 239,
        g: 64,
        b: 72,
        a: 255,
    };

    const SELECTED_TILE: UiColor = UiColor {
        r: 64,
        g: 68,
        b: 72,
        a: 255,
    };

    #[test]
    fn today_marker_and_selected_tile_show_together() {
        let today = (2026, 9, 7);
        let state = crate::calendar::CalendarState::for_today(today);
        let grid = state.grid(today).expect("valid grid");
        let today_index = grid
            .cells
            .iter()
            .position(|cell| cell.today && cell.year == 2026 && cell.month == 9)
            .expect("known date is a current-month cell");
        let mut document = FakeDocument::default();
        project_calendar(&mut document, &state, today).expect("projection succeeds");
        let marker = format!("calendar-day-{}-marker", today_index + 1);
        let tile = format!("calendar-day-{}", today_index + 1);
        assert_eq!(document.backgrounds.get(&marker), Some(&FLAME_RED));
        assert_eq!(document.backgrounds.get(&tile), Some(&SELECTED_TILE));
        let red_markers = document
            .backgrounds
            .iter()
            .filter(|(id, color)| id.ends_with("-marker") && **color == FLAME_RED)
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
        let mut document = FakeDocument::default();
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
            Some(&FLAME_RED),
        );
        assert_eq!(
            document.backgrounds.get("calendar-day-9"),
            Some(&SELECTED_TILE)
        );
    }

    #[test]
    fn months_view_marks_current_month_in_accent() {
        let today = (2026, 10, 13);
        let mut state = crate::calendar::CalendarState::for_today(today);
        state.apply("calendar.view.months", today);
        let mut document = FakeDocument::default();
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
            Some(&FLAME_RED),
        );
    }
}

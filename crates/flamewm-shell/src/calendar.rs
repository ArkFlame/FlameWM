//! Clock calendar popup view state. Date math is owned by `flamewm_shell_core`;
//! this module only holds the Days/Months/Years presentation and its navigation.

pub use flamewm_shell_core::{shift_month, CalendarCell, CalendarGrid, CalendarMonth};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CalendarView {
    #[default]
    Days,
    Months,
    Years,
}

/// Popup state kept by the quick-control host. Benign system snapshots never
/// touch it; only calendar actions change it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CalendarState {
    pub view: CalendarView,
    pub year: i32,
    pub month: u8,
    pub selected: Option<(i32, u8, u8)>,
}

impl CalendarState {
    /// Days view on the month of `today`, with today selected.
    #[must_use]
    pub fn for_today(today: (i32, u8, u8)) -> Self {
        Self {
            view: CalendarView::Days,
            year: today.0,
            month: today.1,
            selected: Some(today),
        }
    }

    /// Applies one calendar action. Unknown actions are ignored.
    pub fn apply(&mut self, action: &str, today: (i32, u8, u8)) {
        match action {
            "calendar.view.days" => self.view = CalendarView::Days,
            "calendar.view.months" => self.view = CalendarView::Months,
            "calendar.view.years" => self.view = CalendarView::Years,
            "calendar.prev" => self.step(-1),
            "calendar.next" => self.step(1),
            "calendar.today" => {
                self.year = today.0;
                self.month = today.1;
                self.selected = Some(today);
            }
            _ => {
                if let Some(index) = action.strip_prefix("calendar.day.") {
                    self.pick_day(index, today);
                } else if let Some(index) = action.strip_prefix("calendar.pick.month.") {
                    if let Ok(month) = index.parse::<u8>() {
                        if (1..=12).contains(&month) {
                            self.month = month;
                            self.view = CalendarView::Days;
                        }
                    }
                } else if let Some(index) = action.strip_prefix("calendar.pick.year.") {
                    if let Ok(offset) = index.parse::<i32>() {
                        if (1..=12).contains(&offset) {
                            self.year = year_page_start(self.year) + offset - 1;
                            self.view = CalendarView::Months;
                        }
                    }
                }
            }
        }
    }

    /// Days grid for the displayed month, or `None` for an invalid state.
    #[must_use]
    pub fn grid(&self, today: (i32, u8, u8)) -> Option<CalendarGrid> {
        CalendarGrid::build_for_date(self.year, self.month, today.0, today.1, today.2)
    }

    fn step(&mut self, delta: i32) {
        match self.view {
            CalendarView::Days => {
                let (year, month) = shift_month(self.year, self.month, delta);
                self.year = year;
                self.month = month;
            }
            CalendarView::Months => self.year += delta,
            CalendarView::Years => self.year += delta * 12,
        }
    }

    fn pick_day(&mut self, index: &str, today: (i32, u8, u8)) {
        let Ok(index) = index.parse::<usize>() else {
            return;
        };
        if !(1..=42).contains(&index) {
            return;
        }
        let Some(grid) = self.grid(today) else {
            return;
        };
        let cell = &grid.cells[index - 1];
        self.selected = Some((cell.year, cell.month, cell.day));
    }
}

/// First year shown by the Years grid: 12-year pages aligned on multiples of 12.
#[must_use]
pub fn year_page_start(year: i32) -> i32 {
    year.div_euclid(12) * 12
}

#[cfg(test)]
mod tests {
    use super::*;

    const TODAY: (i32, u8, u8) = (2026, 10, 13);

    #[test]
    fn days_prev_next_move_one_month_across_year_boundary() {
        let mut state = CalendarState::for_today((2026, 12, 1));
        state.apply("calendar.next", TODAY);
        assert_eq!((state.year, state.month), (2027, 1));
        state.apply("calendar.prev", TODAY);
        state.apply("calendar.prev", TODAY);
        assert_eq!((state.year, state.month), (2026, 11));
    }

    #[test]
    fn months_prev_next_move_one_year_and_years_move_a_page() {
        let mut state = CalendarState::for_today(TODAY);
        state.apply("calendar.view.months", TODAY);
        state.apply("calendar.next", TODAY);
        assert_eq!((state.year, state.month), (2027, 10));
        state.apply("calendar.view.years", TODAY);
        state.apply("calendar.next", TODAY);
        assert_eq!(state.year, 2039);
        state.apply("calendar.prev", TODAY);
        assert_eq!(state.year, 2027);
    }

    #[test]
    fn years_pick_drills_to_months_then_to_days() {
        let mut state = CalendarState::for_today(TODAY);
        state.apply("calendar.view.years", TODAY);
        // Page of 2026 is 2016..=2027; slot 11 is 2026.
        state.apply("calendar.pick.year.11", TODAY);
        assert_eq!(state.year, 2026);
        assert_eq!(state.view, CalendarView::Months);
        state.apply("calendar.pick.month.2", TODAY);
        assert_eq!((state.year, state.month), (2026, 2));
        assert_eq!(state.view, CalendarView::Days);
    }

    #[test]
    fn today_returns_to_current_month_and_selects_today() {
        let mut state = CalendarState::for_today((2028, 2, 29));
        state.apply("calendar.view.years", TODAY);
        state.apply("calendar.today", TODAY);
        assert_eq!((state.year, state.month), (2026, 10));
        assert_eq!(state.selected, Some(TODAY));
    }

    #[test]
    fn selecting_a_grid_cell_uses_its_real_date() {
        let mut state = CalendarState::for_today(TODAY);
        // Row 1 of Oct 2026 starts Mon Sep 28; cell 1 is that day.
        state.apply("calendar.day.1", TODAY);
        assert_eq!(state.selected, Some((2026, 9, 28)));
        state.apply("calendar.day.16", TODAY);
        assert_eq!(state.selected, Some((2026, 10, 13)));
    }

    #[test]
    fn unknown_or_malformed_actions_leave_state_unchanged() {
        let before = CalendarState::for_today(TODAY);
        let mut state = before;
        state.apply("calendar.day.99", TODAY);
        state.apply("calendar.day.x", TODAY);
        state.apply("calendar.pick.month.13", TODAY);
        state.apply("audio.tab.devices", TODAY);
        assert_eq!(state, before);
    }

    #[test]
    fn year_page_start_aligns_to_twelve() {
        assert_eq!(year_page_start(2026), 2016);
        assert_eq!(year_page_start(2028), 2028);
        assert_eq!(year_page_start(-1), -12);
    }
}

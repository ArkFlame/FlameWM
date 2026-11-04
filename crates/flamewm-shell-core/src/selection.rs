//! Shared keyboard + pointer selection for list menus (Start, launcher, context menus).
//!
//! Model: no selection by default. An arrow step with no selection picks the first row;
//! otherwise it moves from the last cursor (clamped, no wrap). A pointer over a row clears
//! any keyboard selection and highlights that row. A pointer leaving the list resets it.

/// Which input owns the single highlight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Highlight {
    Keyboard(usize),
    Hover(usize),
}

/// Single-highlight list selection. Every mutator returns whether the highlight changed,
/// so callers repaint only the rows that actually moved.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ListSelection {
    cursor: Option<usize>,
    highlight: Option<Highlight>,
}

impl ListSelection {
    #[must_use]
    pub const fn highlight(&self) -> Option<Highlight> {
        self.highlight
    }

    #[must_use]
    pub const fn highlighted(&self) -> Option<usize> {
        match self.highlight {
            Some(Highlight::Keyboard(index) | Highlight::Hover(index)) => Some(index),
            None => None,
        }
    }

    /// Arrow navigation over `count` rows. Returns true when the highlight changed.
    pub fn step(&mut self, delta: isize, count: usize) -> bool {
        if count == 0 {
            return self.reset();
        }
        let next = match self.cursor {
            None => 0,
            Some(cursor) => cursor.saturating_add_signed(delta).min(count - 1),
        };
        self.select(Highlight::Keyboard(next))
    }

    /// Pointer over row `index` of `count`. Replaces any keyboard highlight.
    pub fn hover(&mut self, index: usize, count: usize) -> bool {
        index < count && self.select(Highlight::Hover(index))
    }

    /// Clears the selection and cursor (pointer left the list, or the list content changed).
    pub fn reset(&mut self) -> bool {
        self.cursor = None;
        self.highlight.take().is_some()
    }

    fn select(&mut self, next: Highlight) -> bool {
        let index = match next {
            Highlight::Keyboard(index) | Highlight::Hover(index) => index,
        };
        self.cursor = Some(index);
        let changed = self.highlight != Some(next);
        self.highlight = Some(next);
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::{Highlight, ListSelection};

    #[test]
    fn default_has_no_selection() {
        let state = ListSelection::default();
        assert_eq!(state.highlighted(), None);
        assert_eq!(state.highlight(), None);
    }

    #[test]
    fn first_arrow_selects_first_row_in_either_direction() {
        let mut down = ListSelection::default();
        assert!(down.step(1, 3));
        assert_eq!(down.highlight(), Some(Highlight::Keyboard(0)));

        let mut up = ListSelection::default();
        assert!(up.step(-1, 3));
        assert_eq!(up.highlighted(), Some(0));
    }

    #[test]
    fn arrows_step_from_cursor_and_clamp_without_wrap() {
        let mut state = ListSelection::default();
        state.step(1, 3);
        state.step(1, 3);
        assert_eq!(state.highlighted(), Some(1));
        assert!(state.step(1, 3));
        assert!(!state.step(1, 3), "bottom clamp reports no change");
        assert_eq!(state.highlighted(), Some(2));
        state.step(-1, 3);
        state.step(-1, 3);
        state.step(-1, 3);
        assert_eq!(state.highlighted(), Some(0), "top clamp, no wrap");
    }

    #[test]
    fn hover_clears_keyboard_and_arrows_continue_from_hovered_row() {
        let mut state = ListSelection::default();
        state.step(1, 5);
        assert!(state.hover(3, 5));
        assert_eq!(state.highlight(), Some(Highlight::Hover(3)));
        state.step(1, 5);
        assert_eq!(state.highlighted(), Some(4));
    }

    #[test]
    fn hover_on_unchanged_row_or_out_of_range_reports_no_change() {
        let mut state = ListSelection::default();
        assert!(state.hover(2, 4));
        assert!(!state.hover(2, 4), "same row skips repaint");
        assert!(!state.hover(4, 4), "empty slot ignored");
        assert_eq!(state.highlighted(), Some(2));
    }

    #[test]
    fn hover_on_keyboard_row_switches_source_once() {
        let mut state = ListSelection::default();
        state.step(1, 3);
        assert!(state.hover(0, 3), "same index, new source is a change");
        assert!(!state.hover(0, 3));
    }

    #[test]
    fn reset_returns_to_no_selection_and_next_arrow_picks_first() {
        let mut state = ListSelection::default();
        state.hover(2, 3);
        assert!(state.reset());
        assert!(!state.reset(), "already empty");
        assert_eq!(state.highlighted(), None);
        state.step(1, 3);
        assert_eq!(state.highlighted(), Some(0));
    }

    #[test]
    fn empty_list_never_highlights() {
        let mut state = ListSelection::default();
        state.step(1, 0);
        assert_eq!(state.highlighted(), None);
        assert!(!state.hover(0, 0));
    }
}

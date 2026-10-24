//! Small deterministic control-state models. Native adapters trigger callbacks only when these models change.

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ToggleState {
    checked: bool,
}
impl ToggleState {
    #[must_use]
    pub const fn checked(self) -> bool {
        self.checked
    }
    pub fn set_checked(&mut self, checked: bool) -> bool {
        if self.checked == checked {
            return false;
        }
        self.checked = checked;
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SliderState {
    minimum: i32,
    maximum: i32,
    value: i32,
}
impl SliderState {
    #[must_use]
    pub fn new(a: i32, b: i32, value: i32) -> Self {
        let minimum = a.min(b);
        let maximum = a.max(b);
        Self {
            minimum,
            maximum,
            value: value.clamp(minimum, maximum),
        }
    }
    #[must_use]
    pub const fn minimum(self) -> i32 {
        self.minimum
    }
    #[must_use]
    pub const fn maximum(self) -> i32 {
        self.maximum
    }
    #[must_use]
    pub const fn value(self) -> i32 {
        self.value
    }
    pub fn set_value(&mut self, value: i32) -> bool {
        let next = value.clamp(self.minimum, self.maximum);
        if next == self.value {
            return false;
        }
        self.value = next;
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListNavigation {
    Up,
    Down,
    Home,
    End,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListState {
    rows: usize,
    selected: Option<usize>,
    scroll: usize,
    visible_rows: usize,
}
impl Default for ListState {
    fn default() -> Self {
        Self {
            rows: 0,
            selected: None,
            scroll: 0,
            visible_rows: 1,
        }
    }
}
impl ListState {
    pub fn set_rows(&mut self, rows: usize) {
        self.rows = rows;
        if self.selected.is_some_and(|i| i >= rows) {
            self.selected = None;
        }
        self.scroll = self.scroll.min(rows.saturating_sub(1));
        self.ensure_selected_visible();
    }
    pub fn set_visible_rows(&mut self, visible_rows: usize) {
        self.visible_rows = visible_rows.max(1);
        self.ensure_selected_visible();
    }
    pub fn set_selected(&mut self, index: Option<usize>) -> bool {
        let next = index.filter(|i| *i < self.rows);
        if next == self.selected {
            return false;
        }
        self.selected = next;
        self.ensure_selected_visible();
        true
    }
    pub fn navigate(&mut self, navigation: ListNavigation) -> bool {
        if self.rows == 0 {
            return self.set_selected(None);
        }
        let next = match (navigation, self.selected) {
            (ListNavigation::Home, _) => 0,
            (ListNavigation::End, _) => self.rows - 1,
            (ListNavigation::Up, Some(index)) => index.saturating_sub(1),
            (ListNavigation::Down, Some(index)) => (index + 1).min(self.rows - 1),
            (ListNavigation::Up, None) => self.rows - 1,
            (ListNavigation::Down, None) => 0,
        };
        self.set_selected(Some(next))
    }
    #[must_use]
    pub const fn selected(&self) -> Option<usize> {
        self.selected
    }
    #[must_use]
    pub const fn scroll(&self) -> usize {
        self.scroll
    }
    fn ensure_selected_visible(&mut self) {
        let Some(selected) = self.selected else {
            return;
        };
        if selected < self.scroll {
            self.scroll = selected;
        }
        let end = self.scroll.saturating_add(self.visible_rows);
        if selected >= end {
            self.scroll = selected + 1 - self.visible_rows;
        }
    }
}

#[must_use]
pub fn slider_value_from_pointer(
    minimum: i32,
    maximum: i32,
    track_start: i32,
    track_length: i32,
    pointer: i32,
) -> i32 {
    let low = minimum.min(maximum);
    let high = minimum.max(maximum);
    if track_length <= 0 || low == high {
        return low;
    }
    let offset = (pointer - track_start).clamp(0, track_length);
    low + ((i64::from(high - low) * i64::from(offset)) / i64::from(track_length)) as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn toggle_is_idempotent() {
        let mut t = ToggleState::default();
        assert!(t.set_checked(true));
        assert!(!t.set_checked(true));
    }
    #[test]
    fn inverted_slider_range_is_normalized_and_clamped() {
        let s = SliderState::new(100, 0, 150);
        assert_eq!((s.minimum(), s.maximum(), s.value()), (0, 100, 100));
    }
    #[test]
    fn unchanged_slider_value_does_not_emit_change() {
        let mut s = SliderState::new(0, 100, 50);
        assert!(!s.set_value(50));
    }
    #[test]
    fn list_keyboard_navigation_keeps_selection_visible() {
        let mut l = ListState::default();
        l.set_rows(10);
        l.set_visible_rows(3);
        let _ = l.set_selected(Some(1));
        let _ = l.navigate(ListNavigation::End);
        assert_eq!(l.scroll(), 7);
    }
    #[test]
    fn slider_pointer_is_clamped_outside_track() {
        assert_eq!(slider_value_from_pointer(0, 100, 10, 100, 200), 100);
    }
}

use std::collections::BTreeSet;

use flamewm_api::Rect;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionModel {
    selected: BTreeSet<String>,
    fill_opacity: u8,
    border_opacity: u8,
    rubber: Option<Rect>,
}

impl Default for SelectionModel {
    fn default() -> Self {
        Self {
            selected: BTreeSet::new(),
            fill_opacity: 20,
            border_opacity: 60,
            rubber: None,
        }
    }
}

impl SelectionModel {
    pub fn set_fill_opacity(&mut self, opacity: u8) {
        self.fill_opacity = opacity.min(60);
    }

    #[must_use]
    pub const fn fill_opacity(&self) -> u8 {
        self.fill_opacity
    }

    pub fn set_border_opacity(&mut self, opacity: u8) {
        self.border_opacity = opacity.min(100);
    }

    #[must_use]
    pub const fn border_opacity(&self) -> u8 {
        self.border_opacity
    }

    #[must_use]
    pub const fn fill_alpha(&self) -> u8 {
        ((self.fill_opacity as u16 * 255 + 50) / 100) as u8
    }

    pub fn select(&mut self, id: impl Into<String>) {
        self.selected.insert(id.into());
    }

    /// Right-click targets one entry: discard any rubber multi-selection and
    /// select exactly the pressed item. No open/sticky side effect here.
    pub fn select_exclusive(&mut self, id: impl Into<String>) {
        self.selected.clear();
        self.selected.insert(id.into());
    }

    pub fn deselect(&mut self, id: &str) {
        self.selected.remove(id);
    }

    pub fn clear(&mut self) {
        self.selected.clear();
    }

    #[must_use]
    pub fn selected(&self) -> &BTreeSet<String> {
        &self.selected
    }

    pub fn set_rubber(&mut self, rect: Rect) {
        self.rubber = Some(rect);
    }

    pub fn clear_rubber(&mut self) {
        self.rubber = None;
    }

    #[must_use]
    pub const fn rubber(&self) -> Option<Rect> {
        self.rubber
    }

    #[must_use]
    pub fn hit_test<'a>(
        &self,
        item_rects: impl IntoIterator<Item = (&'a str, Rect)>,
    ) -> BTreeSet<String> {
        let Some(rubber) = self.rubber else {
            return BTreeSet::new();
        };
        item_rects
            .into_iter()
            .filter_map(|(id, rect)| rect.intersects(rubber).then(|| id.to_owned()))
            .collect()
    }
}

/// Double-click window: second press on the same item within 500ms and 4px.
pub const DOUBLE_CLICK_TIMEOUT_MS: u64 = 500;
pub const DOUBLE_CLICK_MAX_DIST_PX: f32 = 4.0;

/// Pure double-click decision. Times are X event milliseconds; callers pass a
/// monotonic fallback when the event carries no timestamp.
#[must_use]
pub fn double_click_opens(
    prev_id: &str,
    prev_x: f32,
    prev_y: f32,
    prev_time_ms: u64,
    next_id: &str,
    next_x: f32,
    next_y: f32,
    next_time_ms: u64,
) -> bool {
    if prev_id != next_id {
        return false;
    }
    let delta = next_time_ms.saturating_sub(prev_time_ms);
    if delta > DOUBLE_CLICK_TIMEOUT_MS {
        return false;
    }
    (next_x - prev_x).hypot(next_y - prev_y) <= DOUBLE_CLICK_MAX_DIST_PX
}

/// Rubber overlay stays hidden until pointer motion passes the drag threshold.
/// The pending press still tracks selection state; only visibility is gated.
#[must_use]
pub fn rubber_visible(dx: f32, dy: f32) -> bool {
    crate::layout::threshold_passed(dx, dy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_fill_still_preserves_selection_state_for_visible_border_renderer() {
        let mut selection = SelectionModel::default();
        selection.set_fill_opacity(0);
        selection.select("a");
        assert_eq!(selection.fill_alpha(), 0);
        assert_eq!(selection.border_opacity(), 60);
        assert!(selection.selected().contains("a"));
    }

    #[test]
    fn rubber_band_uses_rectangle_intersection_not_origin_only() {
        let mut selection = SelectionModel::default();
        selection.set_rubber(Rect::new(50, 50, 20, 20));
        let rects = [("a", Rect::new(40, 40, 15, 15))];
        assert!(selection.hit_test(rects).contains("a"));
    }

    #[test]
    fn double_click_requires_same_item_within_time_and_distance() {
        assert!(double_click_opens(
            "a", 10.0, 10.0, 1000, "a", 12.0, 11.0, 1200
        ));
        assert!(!double_click_opens(
            "a", 10.0, 10.0, 1000, "b", 10.0, 10.0, 1100
        ));
        assert!(!double_click_opens(
            "a", 10.0, 10.0, 1000, "a", 10.0, 10.0, 1600
        ));
        assert!(!double_click_opens(
            "a", 10.0, 10.0, 1000, "a", 20.0, 10.0, 1100
        ));
    }

    #[test]
    fn rubber_overlay_gated_by_drag_threshold() {
        assert!(!rubber_visible(3.0, 3.9));
        assert!(rubber_visible(3.0, 4.0));
    }

    #[test]
    fn right_click_selects_exactly_the_pressed_entry() {
        let mut selection = SelectionModel::default();
        selection.select("a");
        selection.select("b");
        selection.set_rubber(Rect::new(0, 0, 200, 200));
        selection.select_exclusive("c");
        assert_eq!(selection.selected().iter().collect::<Vec<_>>(), vec!["c"]);
    }

    #[test]
    fn exclusive_select_preserves_pending_rubber_for_caller_to_clear() {
        let mut selection = SelectionModel::default();
        selection.set_rubber(Rect::new(0, 0, 10, 10));
        selection.select_exclusive("a");
        assert!(selection.rubber().is_some());
        selection.clear_rubber();
        assert!(selection.rubber().is_none());
    }
}

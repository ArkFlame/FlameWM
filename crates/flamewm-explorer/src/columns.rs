//! List column widths. Name fills the space left by the fixed columns; Modified, Size and Type are
//! fixed-width and resized by dragging the divider on their left edge. Dividers move, so the total is preserved.

pub const NAME_MIN: f32 = 140.0;
pub const MODIFIED_MIN: f32 = 80.0;
pub const SIZE_MIN: f32 = 60.0;
pub const KIND_MIN: f32 = 80.0;

/// Column indices used by header dividers. Divider `Modified` sits between Name and Modified, and so on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Divider { Modified, Size, Kind }

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Widths {
    pub modified: f32,
    pub size: f32,
    pub kind: f32,
}

impl Default for Widths {
    fn default() -> Self {
        Self { modified: 120.0, size: 100.0, kind: 130.0 }
    }
}

impl Widths {
    /// Widths read from preferences, raised to their minimums so a hand-edited file cannot break the layout.
    pub fn clamped(self) -> Self {
        Self { modified: self.modified.max(MODIFIED_MIN), size: self.size.max(SIZE_MIN), kind: self.kind.max(KIND_MIN) }
    }

    /// Width of the Name column for a list `total` px wide.
    pub fn name_width(&self, total: f32, show_kind: bool) -> f32 {
        (total - self.modified - self.size - if show_kind { self.kind } else { 0.0 }).max(NAME_MIN)
    }

    /// Moves a divider by `dx` pixels (positive is rightwards). Each move keeps every column at or above its minimum.
    /// Name gives up or takes up the difference for the Modified divider. The other dividers trade width between neighbours.
    pub fn drag_divider(&mut self, divider: Divider, dx: f32, total: f32, show_kind: bool) {
        match divider {
            Divider::Modified => {
                // The divider moves right when Name grows, so Modified shrinks by the same amount.
                let room = total - self.size - if show_kind { self.kind } else { 0.0 } - NAME_MIN;
                let low = MODIFIED_MIN - self.modified;
                let high = (room - self.modified).max(low);
                self.modified += (-dx).clamp(low, high);
            }
            Divider::Size => {
                let dx = dx.clamp(MODIFIED_MIN - self.modified, self.size - SIZE_MIN);
                self.modified += dx;
                self.size -= dx;
            }
            Divider::Kind => {
                let dx = dx.clamp(SIZE_MIN - self.size, self.kind - KIND_MIN);
                self.size += dx;
                self.kind -= dx;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_takes_the_remaining_width_and_has_a_floor() {
        let w = Widths::default();
        assert_eq!(w.name_width(1000.0, false), 1000.0 - 120.0 - 100.0);
        assert_eq!(w.name_width(1000.0, true), 1000.0 - 120.0 - 100.0 - 130.0);
        assert_eq!(w.name_width(200.0, true), NAME_MIN);
    }

    #[test]
    fn modified_divider_moves_name_and_respects_minimums() {
        let mut w = Widths::default();
        w.drag_divider(Divider::Modified, -30.0, 1000.0, false);
        assert_eq!(w.modified, 150.0, "dragging left widens Modified");
        w.drag_divider(Divider::Modified, 10_000.0, 1000.0, false);
        assert_eq!(w.modified, MODIFIED_MIN, "dragging right narrows Modified to its minimum");
        w.drag_divider(Divider::Modified, -10_000.0, 1000.0, false);
        assert_eq!(w.name_width(1000.0, false), NAME_MIN, "Name never drops below its minimum");
        // A window too narrow for any valid split must not panic.
        let mut tiny = Widths::default();
        tiny.drag_divider(Divider::Modified, -5.0, 100.0, true);
        assert_eq!(tiny.modified, MODIFIED_MIN, "no room to grow, so the column stays at its minimum");
    }

    #[test]
    fn neighbour_dividers_trade_width_and_keep_the_total() {
        let mut w = Widths::default();
        let before = w.modified + w.size + w.kind;
        w.drag_divider(Divider::Size, 25.0, 1000.0, true);
        assert_eq!((w.modified, w.size), (145.0, 75.0));
        w.drag_divider(Divider::Kind, -1000.0, 1000.0, true);
        assert_eq!(w.size, SIZE_MIN, "Size stops at its minimum");
        assert_eq!(w.kind, 130.0 + (75.0 - SIZE_MIN), "Kind absorbed the change");
        assert!((w.modified + w.size + w.kind - before).abs() < 1e-3);
    }
}

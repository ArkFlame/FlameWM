use super::*;

/// Retained scroll offset for one scrollable node.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollState {
    pub offset_x: f32,
    pub offset_y: f32,
}

impl ScrollState {
    #[must_use]
    pub fn clamped(self, max_x: f32, max_y: f32) -> Self {
        Self {
            offset_x: self.offset_x.clamp(0.0, max_x.max(0.0)),
            offset_y: self.offset_y.clamp(0.0, max_y.max(0.0)),
        }
    }
}

/// Damage for one surface frame: None skips redraw, Region is bounding-box
/// repaint bookkeeping, Full rebuilds layout+paint. Merge takes max coverage.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum SurfaceDamage {
    #[default]
    None,
    Region(Rect),
    Full,
}

impl SurfaceDamage {
    #[must_use]
    pub fn is_empty(self) -> bool {
        matches!(self, Self::None)
    }

    #[must_use]
    pub fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::None, b) => b,
            (a, Self::None) => a,
            (Self::Full, _) | (_, Self::Full) => Self::Full,
            (Self::Region(a), Self::Region(b)) => {
                let x0 = a.x.min(b.x);
                let y0 = a.y.min(b.y);
                let x1 = (a.x + a.width.max(0.0)).max(b.x + b.width.max(0.0));
                let y1 = (a.y + a.height.max(0.0)).max(b.y + b.height.max(0.0));
                Self::Region(Rect {
                    x: x0,
                    y: y0,
                    width: (x1 - x0).max(0.0),
                    height: (y1 - y0).max(0.0),
                })
            }
        }
    }
}

/// Semantic scroll delta (Button4/5, wheel, thumb drag all funnel here).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollDelta {
    pub dx: f32,
    pub dy: f32,
}

/// Viewport/content geometry for one scrollable node.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollMetrics {
    pub viewport: Rect,
    pub content: Rect,
    pub max_x: f32,
    pub max_y: f32,
    pub thumb_x: Rect,
    pub thumb_y: Rect,
}

impl RuntimeDocument {
    /// Damage bookkeeping: merge coverage (max), take clears the latch.
    pub fn mark_dirty(&mut self, damage: SurfaceDamage) {
        self.pending_damage = self.pending_damage.merge(damage);
        self.dirty.full = true;
    }

    pub fn mark_full(&mut self) {
        self.pending_damage = SurfaceDamage::Full;
        self.dirty.full = true;
    }

    pub fn take_damage(&mut self) -> SurfaceDamage {
        let damage = self.pending_damage;
        self.pending_damage = SurfaceDamage::None;
        damage
    }

    pub fn peek_damage(&self) -> SurfaceDamage {
        self.pending_damage
    }

    /// Retained scroll offset for one node (dead state fixed: render-owned).
    pub fn scroll_offset(&self, index: u32) -> ScrollState {
        self.scroll_offsets.get(&index).copied().unwrap_or_default()
    }

    pub fn apply_scroll_delta(
        &mut self,
        index: u32,
        delta: ScrollDelta,
        max: (f32, f32),
    ) -> ScrollState {
        let cur = self.scroll_offset(index);
        let next = ScrollState {
            offset_x: cur.offset_x + delta.dx,
            offset_y: cur.offset_y + delta.dy,
        }
        .clamped(max.0, max.1);
        if next == cur {
            return cur;
        }
        self.scroll_offsets.insert(index, next);
        self.touch_light();
        self.mark_dirty(SurfaceDamage::Full);
        next
    }

    pub fn set_scroll_offset(
        &mut self,
        index: u32,
        offset: ScrollState,
        max: (f32, f32),
    ) -> ScrollState {
        let next = offset.clamped(max.0, max.1);
        self.scroll_offsets.insert(index, next);
        self.touch_light();
        self.mark_dirty(SurfaceDamage::Full);
        next
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InteractionState {
    pub hover: Option<u32>,
    pub active: Option<u32>,
}

impl InteractionState {
    pub fn style_for<'a>(&self, index: u32, node: &'a CompiledNode) -> &'a Style {
        node.active_style
            .as_deref()
            .filter(|_| self.active == Some(index))
            .or_else(|| {
                node.hover_style
                    .as_deref()
                    .filter(|_| self.hover == Some(index))
            })
            .unwrap_or(&node.style)
    }
}

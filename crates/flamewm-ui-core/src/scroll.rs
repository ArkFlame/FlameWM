//! Pure scroll-state store: retained per-node offsets plus thumb-drag
//! mapping. Dependency-free; the runtime owns retention keyed by node id.

use std::collections::HashMap;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ScrollOffset {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, Default)]
pub struct ScrollStore {
    offsets: HashMap<u32, ScrollOffset>,
}

impl ScrollStore {
    #[must_use]
    pub fn offset(&self, node: u32) -> ScrollOffset {
        self.offsets.get(&node).copied().unwrap_or_default()
    }

    pub fn apply_delta(
        &mut self,
        node: u32,
        delta_x: f32,
        delta_y: f32,
        max_x: f32,
        max_y: f32,
    ) -> ScrollOffset {
        let current = self.offset(node);
        let next = ScrollOffset {
            x: (current.x + delta_x).clamp(0.0, max_x.max(0.0)),
            y: (current.y + delta_y).clamp(0.0, max_y.max(0.0)),
        };
        self.offsets.insert(node, next);
        next
    }

    pub fn set_offset(
        &mut self,
        node: u32,
        offset: ScrollOffset,
        max_x: f32,
        max_y: f32,
    ) -> ScrollOffset {
        let next = ScrollOffset {
            x: offset.x.clamp(0.0, max_x.max(0.0)),
            y: offset.y.clamp(0.0, max_y.max(0.0)),
        };
        self.offsets.insert(node, next);
        next
    }

}

/// Pure thumb-drag mapping: pointer position on a track of `track_len` with
/// thumb length `thumb_len` maps to a content offset in [0, max_offset].
#[must_use]
pub fn thumb_drag_offset(
    pointer_in_track: f32,
    track_len: f32,
    thumb_len: f32,
    max_offset: f32,
) -> f32 {
    let travel = (track_len - thumb_len).max(0.0);
    if travel <= 0.0 || max_offset <= 0.0 {
        return 0.0;
    }
    let pos = (pointer_in_track - thumb_len / 2.0).clamp(0.0, travel);
    pos / travel * max_offset
}

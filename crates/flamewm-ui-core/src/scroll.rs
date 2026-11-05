//! Pure scroll-state store: retained per-node offsets. Dependency-free; the
//! runtime owns retention keyed by node id.

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
}

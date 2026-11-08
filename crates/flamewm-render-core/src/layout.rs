use crate::model::*;
use std::collections::BTreeSet;

thread_local! {
    static LAYOUT_PASSES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Whole-document layout passes run on the calling thread (diagnostic counter for cache tests).
pub fn layout_passes_on_this_thread() -> u64 {
    LAYOUT_PASSES.with(std::cell::Cell::get)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LayoutBox {
    pub rect: Rect,
}

#[derive(Clone, Debug)]
pub struct LayoutResult {
    pub boxes: Vec<LayoutBox>,
    /// Content extent per node in surface coordinates (viewport origin +
    /// intrinsic content size). Viewport is `boxes[index].rect`.
    pub contents: Vec<Rect>,
    /// Document revision the retained boxes were computed from.
    pub revision: u64,
    /// Cached z-order (ascending) for paint + hit-test walks. No re-sort.
    pub z_order: Vec<u32>,
}

impl LayoutResult {
    pub fn content_extent(&self, index: u32) -> Rect {
        self.contents
            .get(index as usize)
            .copied()
            .unwrap_or_default()
    }

    /// Pure viewport/content/offset -> metrics (thumb geometry included).
    /// Thumb minimum is 12px; no thumb when content fits.
    pub fn scroll_metrics(viewport: Rect, content: Rect, offset: ScrollState) -> ScrollMetrics {
        let max_x = (content.width - viewport.width).max(0.0);
        let max_y = (content.height - viewport.height).max(0.0);
        let offset = offset.clamped(max_x, max_y);
        ScrollMetrics {
            viewport,
            content,
            max_x,
            max_y,
            thumb_x: thumb_rect(
                viewport,
                content.width,
                viewport.width,
                offset.offset_x,
                true,
            ),
            thumb_y: thumb_rect(
                viewport,
                content.height,
                viewport.height,
                offset.offset_y,
                false,
            ),
        }
    }

    pub fn metrics_for(&self, index: u32, offset: ScrollState) -> ScrollMetrics {
        let viewport = self
            .boxes
            .get(index as usize)
            .map(|b| b.rect)
            .unwrap_or_default();
        Self::scroll_metrics(viewport, self.content_extent(index), offset)
    }

    pub fn hit_test_action(&self, document: &RuntimeDocument, x: f32, y: f32) -> Option<u32> {
        for index in self.z_order.iter().copied().rev() {
            let index = index as usize;
            let node = &document.document.nodes[index];
            if node.action.is_empty() || !document.is_effectively_visible(index as u32) {
                continue;
            }
            if self.boxes.get(index)?.rect.contains(x, y) {
                return Some(index as u32);
            }
        }
        None
    }
}

pub struct LayoutEngine;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IntrinsicSize {
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntrinsicMeasureError {
    NonFinite,
    NonPositive,
    ExceedsConstraint,
}

impl std::fmt::Display for IntrinsicMeasureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonFinite => write!(f, "intrinsic constraint must be finite"),
            Self::NonPositive => write!(f, "intrinsic constraint must be positive"),
            Self::ExceedsConstraint => write!(f, "intrinsic size exceeds constraint"),
        }
    }
}

impl std::error::Error for IntrinsicMeasureError {}

impl LayoutEngine {
    /// Outer intrinsic size of one document node under finite max constraints.
    pub fn measure_node(
        document: &RuntimeDocument,
        node: u32,
        max_w: f32,
        max_h: f32,
        interaction: InteractionState,
    ) -> Result<IntrinsicSize, IntrinsicMeasureError> {
        if !max_w.is_finite() || !max_h.is_finite() {
            return Err(IntrinsicMeasureError::NonFinite);
        }
        if max_w <= 0.0 || max_h <= 0.0 {
            return Err(IntrinsicMeasureError::NonPositive);
        }
        if (node as usize) >= document.document.nodes.len() {
            return Err(IntrinsicMeasureError::NonPositive);
        }
        let mut empty: Vec<LayoutBox> = Vec::new();
        let probe = Engine {
            document,
            interaction,
            boxes: &mut empty,
            measures: Default::default(),
        };
        // Finite popup constraint only; never f32::MAX here.
        let (w, h) = probe.measure_node(node, max_w, max_h);
        if !w.is_finite() || !h.is_finite() {
            return Err(IntrinsicMeasureError::NonFinite);
        }
        if w <= 0.0 || h <= 0.0 {
            return Err(IntrinsicMeasureError::NonPositive);
        }
        if w > max_w || h > max_h {
            return Err(IntrinsicMeasureError::ExceedsConstraint);
        }
        Ok(IntrinsicSize {
            width: w,
            height: h,
        })
    }

    /// Outer intrinsic size of the document root under finite max constraints.
    /// Uses `measure_node(root, max)` only; never reads
    /// `layout.contents[root]` / `content_extent(root)` (scroll metrics owner).
    pub fn measure_root(
        document: &RuntimeDocument,
        max_w: f32,
        max_h: f32,
        interaction: InteractionState,
    ) -> Result<IntrinsicSize, IntrinsicMeasureError> {
        Self::measure_node(document, document.document.root, max_w, max_h, interaction)
    }
    pub fn compute(
        document: &RuntimeDocument,
        viewport_width: f32,
        viewport_height: f32,
        interaction: InteractionState,
    ) -> LayoutResult {
        LAYOUT_PASSES.with(|passes| passes.set(passes.get() + 1));
        // Full layout is the sync point: no isolated geometry move is pending against it.
        *document.layout_patch.borrow_mut() = Some(BTreeSet::new());
        let mut boxes = vec![LayoutBox::default(); document.document.nodes.len()];
        if document.document.nodes.is_empty() {
            return LayoutResult {
                boxes,
                contents: Vec::new(),
                revision: document.revision(),
                z_order: Vec::new(),
            };
        }
        let mut engine = Engine {
            document,
            interaction,
            boxes: &mut boxes,
            measures: Default::default(),
        };
        engine.layout_node(
            document.document.root,
            0.0,
            0.0,
            viewport_width.max(0.0),
            viewport_height.max(0.0),
            Some(viewport_width.max(0.0)),
            Some(viewport_height.max(0.0)),
        );
        // Content extent: intrinsic measure unconstrained on the scroll axis.
        let mut contents = vec![Rect::default(); document.document.nodes.len()];
        for (index, entry) in boxes.iter().enumerate() {
            if !document.is_visible(index as u32) {
                contents[index] = entry.rect;
                continue;
            }
            let style = document.runtime_style_cow(index as u32, interaction);
            let scrolls_x = matches!(
                style.overflow_x,
                Overflow::Auto | Overflow::Scroll | Overflow::Hidden
            );
            let scrolls_y = matches!(
                style.overflow_y,
                Overflow::Auto | Overflow::Scroll | Overflow::Hidden
            );
            if !scrolls_x && !scrolls_y {
                contents[index] = entry.rect;
                continue;
            }
            let mut empty: Vec<LayoutBox> = Vec::new();
            let probe = Engine {
                document,
                interaction,
                boxes: &mut empty,
                measures: Default::default(),
            };
            let avail_w = if scrolls_x {
                f32::MAX / 1024.0
            } else {
                entry.rect.width.max(0.0)
            };
            let avail_h = if scrolls_y {
                f32::MAX / 1024.0
            } else {
                entry.rect.height.max(0.0)
            };
            let measured = probe.measure_node(index as u32, avail_w, avail_h);
            let content_w = if scrolls_x {
                measured.0.max(entry.rect.width)
            } else {
                entry.rect.width
            };
            let content_h = if scrolls_y {
                measured.1.max(entry.rect.height)
            } else {
                entry.rect.height
            };
            contents[index] = Rect {
                x: entry.rect.x,
                y: entry.rect.y,
                width: content_w,
                height: content_h,
            };
        }
        let mut z_order: Vec<u32> = (0..document.document.nodes.len() as u32).collect();
        if let Some(z_indices) = document.effective_z_indices() {
            // Keys are unique (index tiebreak), so unstable sort yields the same
            // order as the stable sort without its scratch allocation.
            z_order.sort_unstable_by_key(|&index| (z_indices[index as usize], index));
        }
        LayoutResult {
            boxes,
            contents,
            revision: document.revision(),
            z_order,
        }
    }
}

impl LayoutResult {
    /// Re-places the isolated absolute leaves in `nodes` inside the retained boxes. A leaf's box
    /// depends only on its parent's box, so the result equals a full compute. Changes nothing and
    /// returns false unless every node qualifies (see `isolated_leaf_parent`).
    pub(crate) fn patch_isolated(
        &mut self,
        document: &RuntimeDocument,
        interaction: InteractionState,
        nodes: &BTreeSet<u32>,
    ) -> bool {
        let count = document.document.nodes.len();
        if self.boxes.len() != count || self.contents.len() != count {
            return false;
        }
        let mut plan = Vec::with_capacity(nodes.len());
        for &index in nodes {
            let Some(parent) = isolated_leaf_parent(document, interaction, index, nodes) else {
                return false;
            };
            plan.push((index, parent));
        }
        let nodes = &document.document.nodes;
        let mut engine = Engine {
            document,
            interaction,
            boxes: &mut self.boxes,
            measures: Default::default(),
        };
        for &(index, parent) in &plan {
            // A full compute never visits a hidden box or its text, so they keep the default box.
            engine.boxes[index as usize] = LayoutBox::default();
            for child in engine.children(index) {
                engine.boxes[child as usize] = LayoutBox::default();
            }
            let rect = engine.boxes[parent as usize].rect;
            let padding = engine.style(parent).padding;
            engine.place_absolute_child(
                index,
                rect.x + padding.left,
                rect.y + padding.top,
                (rect.width - padding.horizontal()).max(0.0),
                (rect.height - padding.vertical()).max(0.0),
            );
        }
        for &(index, _) in &plan {
            // Non-scrolling box and text: their content extent is their box (see `compute`).
            self.contents[index as usize] = self.boxes[index as usize].rect;
            let mut child = nodes[index as usize].first_child;
            while let Some(current) = child {
                self.contents[current as usize] = self.boxes[current as usize].rect;
                child = nodes[current as usize].next_sibling;
            }
        }
        true
    }
}

/// Parent index when `index` is an absolutely positioned element holding nothing but text, whose
/// box is fixed by its parent's box alone. The element itself may be hidden. Rejects any node in
/// `moved` on the path to the root (its box moves too), any scrolling (Auto/Scroll) or
/// hidden-display ancestor, a scrolling box or text, flex parents, and non-element parents.
fn isolated_leaf_parent(
    document: &RuntimeDocument,
    interaction: InteractionState,
    index: u32,
    moved: &BTreeSet<u32>,
) -> Option<u32> {
    let node = &document.document.nodes[index as usize];
    let parent = node.parent?;
    if node.kind != NodeKind::Element {
        return None;
    }
    let scrolls = |style: &Style| {
        style.overflow_x != Overflow::Visible || style.overflow_y != Overflow::Visible
    };
    let style = document.runtime_style_cow(index, interaction);
    if style.position != Position::Absolute || scrolls(&style) {
        return None;
    }
    let mut child = node.first_child;
    while let Some(current) = child {
        let text = &document.document.nodes[current as usize];
        if text.kind != NodeKind::Text || scrolls(&document.runtime_style_cow(current, interaction))
        {
            return None;
        }
        child = text.next_sibling;
    }
    // A flex container offsets its absolute children by the flow cursor in a full compute, so a
    // block-origin placement would disagree with it. Such a parent takes the full compute.
    if document.runtime_style_cow(parent, interaction).display == Display::Flex {
        return None;
    }
    let mut ancestor = Some(parent);
    while let Some(current) = ancestor {
        let node = &document.document.nodes[current as usize];
        let style = document.runtime_style_cow(current, interaction);
        // Scrolling ancestors (Auto/Scroll) get their extents and offsets re-fitted after
        // compute; hidden ones do not move children and absolute children never size them.
        if moved.contains(&current)
            || node.kind != NodeKind::Element
            || style.display == Display::None
            || matches!(style.overflow_x, Overflow::Auto | Overflow::Scroll)
            || matches!(style.overflow_y, Overflow::Auto | Overflow::Scroll)
        {
            return None;
        }
        ancestor = node.parent;
    }
    Some(parent)
}

mod engine;
mod geometry;

use engine::*;
use geometry::*;

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "layout_tests.rs"]
mod layout_tests;

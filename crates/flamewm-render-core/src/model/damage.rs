use super::*;
use crate::layout::LayoutResult;
use std::collections::BTreeMap;

/// Node-scoped mutation recorded by a `touch_*_node` setter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirtyReason {
    /// Paint-only change (colour): the node box is unchanged, its pixels are not.
    Paint,
    /// Pure left/top/width/height override on an absolutely positioned node. Parent layout
    /// never measures or positions absolute children, so only this node's subtree moves.
    Geometry,
    /// Text content change. Layout is verified by box diff, not assumed.
    Text,
}

/// Node-tracked mutations since the last incremental resolve. `full` latches any mutation
/// that carries no node identity (or is structural) and forces a full repaint.
#[derive(Clone, Debug, Default)]
pub(super) struct DirtyTracker {
    pub(super) full: bool,
    pub(super) nodes: BTreeMap<u32, DirtyReason>,
}

/// More dirty nodes than this is cheaper to repaint whole than to resolve region by region.
pub const DAMAGE_DIRTY_NODE_LIMIT: usize = 64;

/// Regions covering more than this fraction of the viewport are promoted to `Full`.
pub const DAMAGE_FULL_AREA_FRACTION: f32 = 0.5;

/// Outward paint extent beyond a node box that its own style can paint (whole pixels).
/// Covers borders and text shadows. Node backdrops sample everything beneath them, so
/// their presence disables incremental damage entirely (see `resolve_node_damage`).
#[must_use]
pub fn paint_halo(style: &Style) -> f32 {
    let mut halo = 0.0f32;
    if style.border_width > 0.0 {
        halo = halo.max(style.border_width.ceil() + 1.0);
    }
    if let Some(shadow) = style.text_shadow {
        let reach =
            shadow.offset_x.abs().max(shadow.offset_y.abs()) + 2.0 * shadow.blur_radius.max(0.0);
        halo = halo.max(reach.ceil() + 1.0);
    }
    halo
}

fn union(a: Option<Rect>, b: Option<Rect>) -> Option<Rect> {
    match (a, b) {
        (None, b) => b,
        (a, None) => a,
        (Some(a), Some(b)) => {
            let x0 = a.x.min(b.x);
            let y0 = a.y.min(b.y);
            let x1 = (a.x + a.width).max(b.x + b.width);
            let y1 = (a.y + a.height).max(b.y + b.height);
            Some(Rect {
                x: x0,
                y: y0,
                width: x1 - x0,
                height: y1 - y0,
            })
        }
    }
}

/// Inflates `rect` by `halo`, rounds outward to whole pixels and clips to `viewport`.
fn pixel_rect(rect: Rect, halo: f32, viewport: Rect) -> Option<Rect> {
    let vx1 = viewport.x + viewport.width;
    let vy1 = viewport.y + viewport.height;
    let x0 = (rect.x - halo).floor().max(viewport.x);
    let y0 = (rect.y - halo).floor().max(viewport.y);
    let x1 = (rect.x + rect.width + halo).ceil().min(vx1);
    let y1 = (rect.y + rect.height + halo).ceil().min(vy1);
    (x1 > x0 && y1 > y0).then_some(Rect {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    })
}

fn has_geometry_ancestor(
    document: &RuntimeDocument,
    mut index: u32,
    dirty: &BTreeMap<u32, DirtyReason>,
) -> bool {
    while let Some(parent) = document.document.nodes[index as usize].parent {
        if dirty.get(&parent) == Some(&DirtyReason::Geometry) {
            return true;
        }
        index = parent;
    }
    false
}

/// Resolves node-tracked mutations into surface damage.
///
/// Every node whose old or new box (or content extent) differs is damaged at both places.
/// Any box that moved without a tracked mutation explaining it, a z-order change, or a
/// node-backdrop in the document yields `Full`. Pixels inside the viewport only.
fn resolve_node_damage(
    document: &RuntimeDocument,
    dirty: &BTreeMap<u32, DirtyReason>,
    old: &LayoutResult,
    new: &LayoutResult,
    interaction: InteractionState,
    viewport: Rect,
) -> SurfaceDamage {
    let count = document.document.nodes.len();
    if [
        old.boxes.len(),
        old.contents.len(),
        new.boxes.len(),
        new.contents.len(),
    ]
    .iter()
    .any(|&len| len != count)
        || old.z_order != new.z_order
        || !document.node_backdrops.is_empty()
    {
        return SurfaceDamage::Full;
    }
    let mut region = None;
    for (index, (old_box, new_box)) in old.boxes.iter().zip(&new.boxes).enumerate() {
        let node = index as u32;
        let tracked = dirty.get(&node);
        let moved = old_box != new_box || old.contents[index] != new.contents[index];
        if !moved && tracked.is_none() {
            continue;
        }
        if moved && tracked.is_none() && !has_geometry_ancestor(document, node, dirty) {
            return SurfaceDamage::Full;
        }
        let halo = paint_halo(&document.runtime_style_cow(node, interaction));
        for rect in [old_box.rect, new_box.rect] {
            region = union(region, pixel_rect(rect, halo, viewport));
        }
    }
    let Some(region) = region else {
        return SurfaceDamage::None;
    };
    let viewport_area = viewport.width.max(0.0) * viewport.height.max(0.0);
    if region.width * region.height > viewport_area * DAMAGE_FULL_AREA_FRACTION {
        return SurfaceDamage::Full;
    }
    SurfaceDamage::Region(region)
}

impl RuntimeDocument {
    /// Node-scoped mutation. `layout` says whether the layout revision must advance.
    pub(super) fn touch_node(&mut self, index: u32, reason: DirtyReason, layout: bool) {
        self.revision = self.revision.wrapping_add(1);
        if layout {
            self.layout_revision = self.layout_revision.wrapping_add(1);
            *self.layout_patch.get_mut() = None;
        } else if reason == DirtyReason::Geometry {
            // Isolated geometry: patchable in place, see `RuntimeDocument::patch_layout`.
            if let Some(nodes) = self.layout_patch.get_mut().as_mut() {
                nodes.insert(index);
            }
        }
        self.pending_damage = self.pending_damage.merge(SurfaceDamage::Full);
        self.dirty.nodes.insert(index, reason);
    }

    /// Whether a pure geometry override on `index` leaves its parent's layout untouched.
    /// Only absolutely positioned non-root nodes qualify; hover/active styles must not
    /// change position either.
    pub(super) fn geometry_is_isolated(&self, index: u32) -> bool {
        let node = &self.document.nodes[index as usize];
        node.parent.is_some()
            && node.style.position == Position::Absolute
            && node
                .hover_style
                .as_ref()
                .is_none_or(|style| style.position == Position::Absolute)
            && node
                .active_style
                .as_ref()
                .is_none_or(|style| style.position == Position::Absolute)
    }

    /// The isolated absolute element whose boxes a text, visibility or font change of `index`
    /// can move: `index` itself, or the parent of a text node, when that element is isolated
    /// (see [`Self::geometry_is_isolated`]) and holds nothing but text. Such a change resizes
    /// that element's own subtree only; absolute children never size or move their parent.
    fn isolated_box_owner(&self, index: u32) -> Option<u32> {
        let node = &self.document.nodes[index as usize];
        let owner = if node.kind == NodeKind::Text {
            node.parent?
        } else {
            index
        };
        let element = &self.document.nodes[owner as usize];
        if element.kind != NodeKind::Element || !self.geometry_is_isolated(owner) {
            return None;
        }
        let mut child = element.first_child;
        while let Some(current) = child {
            let node = &self.document.nodes[current as usize];
            if node.kind != NodeKind::Text {
                return None;
            }
            child = node.next_sibling;
        }
        Some(owner)
    }

    /// Records a layout-affecting change of `index` as a patch of its isolated box owner instead
    /// of a full layout invalidation. Returns false, recording nothing, when the change is not
    /// provably local or a full compute is already pending; the caller then invalidates layout.
    pub(super) fn touch_isolated_box(&mut self, index: u32, reason: DirtyReason) -> bool {
        let Some(owner) = self.isolated_box_owner(index) else {
            return false;
        };
        let Some(nodes) = self.layout_patch.get_mut().as_mut() else {
            return false;
        };
        nodes.insert(owner);
        self.revision = self.revision.wrapping_add(1);
        self.geometry_revision = self.geometry_revision.wrapping_add(1);
        self.pending_damage = self.pending_damage.merge(SurfaceDamage::Full);
        self.dirty.nodes.insert(index, reason);
        true
    }

    /// Resolves the mutations since the previous call into surface damage and clears them.
    ///
    /// `old` must be the layout that was last painted; `new` the layout for the current
    /// revision. Anything not provably local returns `SurfaceDamage::Full`.
    pub fn take_incremental_damage(
        &mut self,
        old: &LayoutResult,
        new: &LayoutResult,
        interaction: InteractionState,
        viewport: Rect,
    ) -> SurfaceDamage {
        let dirty = std::mem::take(&mut self.dirty);
        self.pending_damage = SurfaceDamage::None;
        if dirty.full
            || new.revision != self.revision
            || dirty.nodes.len() > DAMAGE_DIRTY_NODE_LIMIT
        {
            return SurfaceDamage::Full;
        }
        resolve_node_damage(self, &dirty.nodes, old, new, interaction, viewport)
    }
}

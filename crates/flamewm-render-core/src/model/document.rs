use super::*;
use crate::layout::LayoutResult;
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Element,
    Text,
    Image,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompiledNode {
    pub kind: NodeKind,
    pub parent: Option<u32>,
    pub first_child: Option<u32>,
    pub next_sibling: Option<u32>,
    pub id: Cow<'static, str>,
    pub action: Cow<'static, str>,
    pub text: Cow<'static, str>,
    pub image: Option<u16>,
    pub style: Style,
    /// Boxed: most nodes have no interaction styles, so keeping them inline bloats every node.
    pub hover_style: Option<Box<Style>>,
    pub active_style: Option<Box<Style>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ColorVariable {
    pub name: Cow<'static, str>,
    pub default: Color,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImageAsset {
    pub source: Cow<'static, str>,
    pub width: u32,
    pub height: u32,
    /// Straight (non-premultiplied) RGBA8 pixels in row-major order.
    /// Opaque PPM build inputs convert to alpha=255 at compile time.
    /// Borrowed from the embedded document bytes when decoded with `decode_static`.
    pub pixels: Cow<'static, [u8]>,
}

impl ImageAsset {
    #[must_use]
    pub fn is_fully_opaque(&self) -> bool {
        self.pixels.chunks_exact(4).all(|pixel| pixel[3] == 255)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompiledDocument {
    pub source_fingerprint: u64,
    pub root: u32,
    pub variables: Vec<ColorVariable>,
    pub assets: Vec<ImageAsset>,
    pub nodes: Vec<CompiledNode>,
}

impl CompiledDocument {
    pub fn validate(&self) -> Result<(), String> {
        if self.nodes.is_empty() {
            return Err("compiled document has no nodes".to_string());
        }
        if self.root as usize >= self.nodes.len() {
            return Err("root index is outside node table".to_string());
        }
        if self.assets.len() > u16::MAX as usize {
            return Err("compiled document has too many image assets".to_string());
        }
        for (index, asset) in self.assets.iter().enumerate() {
            if asset.width == 0 || asset.height == 0 {
                return Err(format!("image asset {index} has zero dimensions"));
            }
            let expected = asset
                .width
                .checked_mul(asset.height)
                .and_then(|pixels| pixels.checked_mul(4))
                .ok_or_else(|| format!("image asset {index} dimensions overflow"))?
                as usize;
            if asset.pixels.len() != expected {
                return Err(format!(
                    "image asset {index} has {} bytes; expected {expected}",
                    asset.pixels.len()
                ));
            }
        }
        for (index, node) in self.nodes.iter().enumerate() {
            for (field, link) in [
                ("parent", node.parent),
                ("first_child", node.first_child),
                ("next_sibling", node.next_sibling),
            ] {
                if let Some(link) = link.filter(|&link| link as usize >= self.nodes.len()) {
                    return Err(format!("node {index} has invalid {field} index {link}"));
                }
            }
            if let Some(asset) = node.image {
                if asset as usize >= self.assets.len() {
                    return Err(format!(
                        "node {index} references invalid image asset {asset}"
                    ));
                }
                if node.kind != NodeKind::Image {
                    return Err(format!(
                        "node {index} has image data but is not an image node"
                    ));
                }
            }
            if node.kind == NodeKind::Image && node.image.is_none() {
                return Err(format!("image node {index} has no image asset"));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GeometryOverride {
    pub left: Option<f32>,
    pub top: Option<f32>,
    pub width: Option<f32>,
    pub height: Option<f32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RuntimeStyleOverride {
    pub background: Option<ColorValue>,
    pub color: Option<ColorValue>,
    pub border_color: Option<ColorValue>,
    pub border_radius: Option<f32>,
    pub opacity: Option<f32>,
    pub flex_direction: Option<FlexDirection>,
    pub font_size: Option<f32>,
    pub font_weight: Option<u16>,
    pub overflow_x: Option<Overflow>,
    pub overflow_y: Option<Overflow>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeImageOverride {
    pub source: String,
    pub width: u32,
    pub height: u32,
    pub rgba8: Arc<Vec<u8>>,
    pub revision: u64,
}

#[derive(Clone, Debug)]
pub struct RuntimeDocument {
    pub document: CompiledDocument,
    pub variable_values: Vec<Color>,
    pub text_overrides: HashMap<u32, String>,
    pub(super) hidden_nodes: HashSet<u32>,
    pub(super) image_revision: HashMap<u16, u64>,
    pub(super) node_image_overrides: HashMap<u32, RuntimeImageOverride>,
    pub(super) geometry_overrides: HashMap<u32, GeometryOverride>,
    pub(super) style_overrides: HashMap<u32, RuntimeStyleOverride>,
    pub(super) node_backdrops: HashMap<u32, u8>,
    pub(super) z_index_overrides: HashMap<u32, i32>,
    pub(super) scroll_offsets: HashMap<u32, ScrollState>,
    pub(super) pending_damage: SurfaceDamage,
    /// Node-tracked changes for `take_incremental_damage`; `full` latches untracked mutations.
    pub(super) dirty: DirtyTracker,
    pub(super) revision: u64,
    /// Bumped only by mutations that can change layout (text, size, visibility, position,
    /// scroll, fonts, ...). Paint-only mutations (colors) bump `revision` but not this.
    pub(super) layout_revision: u64,
    /// Bumped by isolated geometry overrides only. Retained layout is patched in place for these
    /// (see `patch_layout`), so `layout_revision` stays put.
    pub(super) geometry_revision: u64,
    /// Isolated geometry nodes moved since the last layout sync (`LayoutEngine::compute` or a
    /// successful `patch_layout`). `None` means a layout-affecting change happened and a full
    /// compute is required.
    pub(crate) layout_patch: RefCell<Option<BTreeSet<u32>>>,
    /// Node indices with non-empty ids, sorted by id. Keys live in `nodes`, not duplicated here.
    pub(super) id_index: Vec<u32>,
    /// Variable indices sorted by (name, index). Keys live in `document.variables`.
    pub(super) variable_index: Vec<u16>,
    pub(super) ui_font_family: String,
    pub(super) ui_font_size_offset: f32,
    pub(super) ui_font_bold: bool,
    pub(super) ui_scale: f32,
}

impl RuntimeDocument {
    pub fn new(document: CompiledDocument) -> Result<Self, String> {
        document.validate()?;
        let nodes = &document.nodes;
        let mut id_index: Vec<u32> = (0..nodes.len() as u32)
            .filter(|&index| !nodes[index as usize].id.is_empty())
            .collect();
        id_index.sort_by(|&a, &b| {
            (*nodes[a as usize].id)
                .cmp(&*nodes[b as usize].id)
                .then(a.cmp(&b))
        });
        if let Some(pair) = id_index
            .windows(2)
            .find(|pair| nodes[pair[0] as usize].id == nodes[pair[1] as usize].id)
        {
            return Err(format!(
                "duplicate runtime id '{}'",
                nodes[pair[1] as usize].id
            ));
        }
        id_index.shrink_to_fit();
        let variables = &document.variables;
        let mut variable_index: Vec<u16> = (0..variables.len() as u16).collect();
        variable_index.sort_by(|&a, &b| {
            (*variables[a as usize].name)
                .cmp(&*variables[b as usize].name)
                .then(a.cmp(&b))
        });
        let variable_values = document
            .variables
            .iter()
            .map(|variable| variable.default)
            .collect();
        Ok(Self {
            document,
            variable_values,
            text_overrides: HashMap::new(),
            hidden_nodes: HashSet::new(),
            image_revision: HashMap::new(),
            node_image_overrides: HashMap::new(),
            geometry_overrides: HashMap::new(),
            style_overrides: HashMap::new(),
            node_backdrops: HashMap::new(),
            z_index_overrides: HashMap::new(),
            scroll_offsets: HashMap::new(),
            pending_damage: SurfaceDamage::Full,
            dirty: DirtyTracker {
                full: true,
                ..DirtyTracker::default()
            },
            revision: 0,
            layout_revision: 0,
            geometry_revision: 0,
            layout_patch: RefCell::new(None),
            id_index,
            variable_index,
            ui_font_family: "IBM Plex Sans".to_string(),
            ui_font_size_offset: 0.0,
            ui_font_bold: false,
            ui_scale: 1.0,
        })
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Revision of the layout-relevant document state. Retained layout is valid while this is
    /// unchanged; paint-only mutations leave it alone.
    pub fn layout_revision(&self) -> u64 {
        self.layout_revision
    }

    pub fn geometry_revision(&self) -> u64 {
        self.geometry_revision
    }

    pub(super) fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.layout_revision = self.layout_revision.wrapping_add(1);
        *self.layout_patch.get_mut() = None;
        self.pending_damage = self.pending_damage.merge(SurfaceDamage::Full);
        self.dirty.full = true;
    }

    /// Patches `layout` in place for the isolated geometry moves recorded since the last sync.
    /// Returns false, leaving the layout untouched, when a full compute is required.
    pub fn patch_layout(&self, layout: &mut LayoutResult, interaction: InteractionState) -> bool {
        let mut pending = self.layout_patch.borrow_mut();
        let Some(nodes) = pending.as_ref() else {
            return false;
        };
        if !layout.patch_isolated(self, interaction, nodes) {
            return false;
        }
        *pending = Some(BTreeSet::new());
        layout.revision = self.revision;
        true
    }

    /// Paint-only mutation (colors): invalidates paint and damage but keeps retained layout.
    /// Damage stays `Full` because no node bounds are known here.
    pub(super) fn touch_paint(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.pending_damage = self.pending_damage.merge(SurfaceDamage::Full);
        self.dirty.full = true;
    }

    pub(super) fn touch_light(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.layout_revision = self.layout_revision.wrapping_add(1);
        *self.layout_patch.get_mut() = None;
        self.dirty.full = true;
    }
}

pub type NodeImage<'a> = (Option<u16>, u32, u32, &'a str, &'a [u8]);

impl RuntimeDocument {
    pub fn node_by_id(&self, id: &str) -> Option<u32> {
        self.id_index
            .binary_search_by(|&index| (*self.document.nodes[index as usize].id).cmp(id))
            .ok()
            .map(|position| self.id_index[position])
    }

    /// Last variable with `name` wins, matching the previous map-insert semantics.
    pub(super) fn variable_by_name(&self, name: &str) -> Option<u16> {
        let end = self
            .variable_index
            .partition_point(|&index| (*self.document.variables[index as usize].name) <= *name);
        let last = *self.variable_index.get(end.checked_sub(1)?)?;
        (*self.document.variables[last as usize].name == *name).then_some(last)
    }

    pub(crate) fn require_node(&self, id: &str) -> Result<u32, String> {
        self.node_by_id(id)
            .ok_or_else(|| format!("no node with id '{id}'"))
    }

    pub fn set_text(&mut self, id: &str, text: impl Into<String>) -> Result<(), String> {
        let index = self.require_node(id)?;
        let target = if self.document.nodes[index as usize].kind == NodeKind::Text {
            index
        } else {
            let first = self.document.nodes[index as usize]
                .first_child
                .ok_or_else(|| format!("node '{id}' has no text child"))?;
            let child = &self.document.nodes[first as usize];
            if child.kind != NodeKind::Text || child.next_sibling.is_some() {
                return Err(format!("node '{id}' is not a single-text-node element"));
            }
            first
        };
        let text = text.into();
        let current: &str = self
            .text_overrides
            .get(&target)
            .map(String::as_str)
            .unwrap_or(&self.document.nodes[target as usize].text);
        if *current == text {
            return Ok(());
        }
        self.text_overrides.insert(target, text);
        if !self.touch_isolated_box(target, DirtyReason::Text) {
            self.touch_node(target, DirtyReason::Text, true);
        }
        Ok(())
    }

    pub fn set_visible(&mut self, id: &str, visible: bool) -> Result<(), String> {
        let index = self.require_node(id)?;
        if self.hidden_nodes.contains(&index) != visible {
            return Ok(());
        }
        if visible {
            self.hidden_nodes.remove(&index);
        } else {
            self.hidden_nodes.insert(index);
        }
        // An isolated absolute box only shows or hides its own subtree: patch the retained layout.
        // The repaint stays whole, exactly as before.
        if self.touch_isolated_box(index, DirtyReason::Geometry) {
            self.dirty.full = true;
        } else {
            self.touch();
        }
        Ok(())
    }

    pub fn replace_image_for_node(
        &mut self,
        node_id: &str,
        source: String,
        width: u32,
        height: u32,
        rgba8: Vec<u8>,
    ) -> Result<(), String> {
        self.replace_image_for_node_shared(node_id, source, width, height, Arc::new(rgba8))
    }

    /// Like [`Self::replace_image_for_node`], but stores a caller-held shared buffer without copying it.
    pub fn replace_image_for_node_shared(
        &mut self,
        node_id: &str,
        source: String,
        width: u32,
        height: u32,
        rgba8: Arc<Vec<u8>>,
    ) -> Result<(), String> {
        let index = self.require_node(node_id)?;
        let node = &self.document.nodes[index as usize];
        if node.kind != NodeKind::Image {
            return Err(format!("node '{node_id}' is not an image node"));
        }
        if width == 0 || height == 0 {
            return Err("image replacement dimensions must be non-zero".to_string());
        }
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| "image replacement dimensions overflow".to_string())?;
        if rgba8.len() != expected {
            return Err(format!(
                "image replacement has {} bytes; expected {expected}",
                rgba8.len()
            ));
        }
        if let Some(entry) = self.node_image_overrides.get_mut(&index)
            && entry.width == width
            && entry.height == height
            && entry.source == source
            && (Arc::ptr_eq(&entry.rgba8, &rgba8) || entry.rgba8 == rgba8)
        {
            entry.rgba8 = rgba8;
            return Ok(());
        }
        let revision = self
            .node_image_overrides
            .get(&index)
            .map_or(0, |entry| entry.revision)
            .wrapping_add(1);
        self.node_image_overrides.insert(
            index,
            RuntimeImageOverride {
                source,
                width,
                height,
                rgba8,
                revision,
            },
        );
        self.touch();
        Ok(())
    }

    /// Drops a node-local image override so the node falls back to its compiled asset.
    pub fn clear_image_for_node(&mut self, node_id: &str) -> Result<(), String> {
        let index = self.require_node(node_id)?;
        if self.node_image_overrides.remove(&index).is_some() {
            self.touch();
        }
        Ok(())
    }

    /// Node-local override pixels (no per-call copies beyond borrow).
    pub fn node_image_override(&self, index: u32) -> Option<&RuntimeImageOverride> {
        self.node_image_overrides.get(&index)
    }

    /// Effective image for a node: node-local override wins, else compiled asset.
    pub fn image_for_node(&self, index: u32) -> Option<NodeImage<'_>> {
        if let Some(entry) = self.node_image_overrides.get(&index) {
            return Some((
                self.document
                    .nodes
                    .get(index as usize)
                    .and_then(|node| node.image),
                entry.width,
                entry.height,
                entry.source.as_str(),
                entry.rgba8.as_slice(),
            ));
        }
        let asset_id = self.document.nodes.get(index as usize)?.image?;
        let asset = self.document.assets.get(asset_id as usize)?;
        Some((
            Some(asset_id),
            asset.width,
            asset.height,
            &*asset.source,
            &asset.pixels,
        ))
    }

    /// Per-node image revision: node-local counter when overridden, else legacy asset revision.
    pub fn image_revision_for_node(&self, index: u32) -> u64 {
        if let Some(entry) = self.node_image_overrides.get(&index) {
            return entry.revision;
        }
        let asset_id = self
            .document
            .nodes
            .get(index as usize)
            .and_then(|node| node.image);
        asset_id.map_or(0, |asset_id| self.image_revision(asset_id))
    }

    pub fn image_revision(&self, asset_id: u16) -> u64 {
        self.image_revision.get(&asset_id).copied().unwrap_or(0)
    }

    pub fn is_visible(&self, index: u32) -> bool {
        !self.hidden_nodes.contains(&index)
    }

    pub fn is_effectively_visible(&self, mut index: u32) -> bool {
        loop {
            if !self.is_visible(index) {
                return false;
            }
            let Some(parent) = self.document.nodes[index as usize].parent else {
                return true;
            };
            index = parent;
        }
    }
}

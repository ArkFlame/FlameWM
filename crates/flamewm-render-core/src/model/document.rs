use super::*;
use std::collections::{HashMap, HashSet};

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
    pub id: String,
    pub action: String,
    pub text: String,
    pub image: Option<u16>,
    pub style: Style,
    pub hover_style: Option<Style>,
    pub active_style: Option<Style>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ColorVariable {
    pub name: String,
    pub default: Color,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImageAsset {
    pub source: String,
    pub width: u32,
    pub height: u32,
    /// Straight (non-premultiplied) RGBA8 pixels in row-major order.
    /// Opaque PPM build inputs convert to alpha=255 at compile time.
    pub pixels: Vec<u8>,
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
    pub rgba8: Vec<u8>,
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
    pub(super) revision: u64,
    pub(super) id_index: HashMap<String, u32>,
    pub(super) variable_index: HashMap<String, u16>,
    pub(super) ui_font_family: String,
    pub(super) ui_font_size_offset: f32,
    pub(super) ui_font_bold: bool,
    pub(super) ui_scale: f32,
}

impl RuntimeDocument {
    pub fn new(document: CompiledDocument) -> Result<Self, String> {
        document.validate()?;
        let mut id_index = HashMap::new();
        for (index, node) in document.nodes.iter().enumerate() {
            if !node.id.is_empty() && id_index.insert(node.id.clone(), index as u32).is_some() {
                return Err(format!("duplicate runtime id '{}'", node.id));
            }
        }
        let variable_index = document
            .variables
            .iter()
            .enumerate()
            .map(|(index, variable)| (variable.name.clone(), index as u16))
            .collect();
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
            revision: 0,
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

    pub(super) fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.pending_damage = self.pending_damage.merge(SurfaceDamage::Full);
    }

    pub(super) fn touch_light(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

impl RuntimeDocument {
    pub fn node_by_id(&self, id: &str) -> Option<u32> {
        self.id_index.get(id).copied()
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
        let current = self
            .text_overrides
            .get(&target)
            .unwrap_or(&self.document.nodes[target as usize].text);
        if *current == text {
            return Ok(());
        }
        self.text_overrides.insert(target, text);
        self.touch();
        Ok(())
    }

    pub fn set_visible(&mut self, id: &str, visible: bool) -> Result<(), String> {
        let index = self.require_node(id)?;
        if self.hidden_nodes.contains(&index) == !visible {
            return Ok(());
        }
        if visible {
            self.hidden_nodes.remove(&index);
        } else {
            self.hidden_nodes.insert(index);
        }
        self.touch();
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
        if self.node_image_overrides.get(&index).is_some_and(|entry| {
            entry.width == width
                && entry.height == height
                && entry.source == source
                && entry.rgba8 == rgba8
        }) {
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
    pub fn image_for_node(&self, index: u32) -> Option<(Option<u16>, u32, u32, &str, &[u8])> {
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
            asset.source.as_str(),
            asset.pixels.as_slice(),
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

use std::collections::{HashMap, HashSet};

pub const FORMAT_MAGIC: [u8; 4] = *b"RWRB";
/// Current asset pixel encoding: straight (non-premultiplied) RGBA8.
/// Version 5 appends text-wrap bytes per style; version 4 is identical
/// RGBA8 without them (but with the image-treatment byte). Version 3 is
/// identical RGBA8 without that byte. Version 2 documents carry opaque
/// RGB8 assets; the codec upconverts them to RGBA8 with alpha=255 so PPM
/// build inputs keep working.
pub const FORMAT_VERSION: u16 = 5;
pub const FORMAT_VERSION_TREATMENT_LEGACY: u16 = 4;
pub const FORMAT_VERSION_RGBA8_LEGACY: u16 = 3;
pub const FORMAT_VERSION_RGB8_LEGACY: u16 = 2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const TRANSPARENT: Self = Self {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };
    pub const BLACK: Self = Self {
        r: 0,
        g: 0,
        b: 0,
        a: 255,
    };
    pub const WHITE: Self = Self {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    };

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColorValue {
    Literal(Color),
    Variable(u16),
}

impl Default for ColorValue {
    fn default() -> Self {
        Self::Literal(Color::TRANSPARENT)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Length {
    Auto,
    Px(f32),
    Percent(f32),
}

impl Default for Length {
    fn default() -> Self {
        Self::Auto
    }
}

impl Length {
    pub fn resolve(self, available: f32) -> Option<f32> {
        match self {
            Self::Auto => None,
            Self::Px(value) => Some(value.max(0.0)),
            Self::Percent(value) => Some((available * value).max(0.0)),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Edges {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Edges {
    pub const ZERO: Self = Self {
        top: 0.0,
        right: 0.0,
        bottom: 0.0,
        left: 0.0,
    };

    pub fn horizontal(self) -> f32 {
        self.left + self.right
    }

    pub fn vertical(self) -> f32 {
        self.top + self.bottom
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Display {
    None,
    #[default]
    Block,
    Flex,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FlexDirection {
    #[default]
    Row,
    Column,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum JustifyContent {
    #[default]
    Start,
    Center,
    End,
    SpaceBetween,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AlignItems {
    Start,
    Center,
    End,
    #[default]
    Stretch,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Position {
    #[default]
    Flow,
    Absolute,
}

/// Raw X button translated once at the native boundary (ui-x11 input).
/// Wheel buttons never act as press/release; Button4/5 map to scroll delta.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerButton {
    Primary,
    Middle,
    Secondary,
    WheelUp,
    WheelDown,
    Other(u32),
}

impl PointerButton {
    #[must_use]
    pub const fn from_raw_x(button: u32) -> Self {
        match button {
            1 => Self::Primary,
            2 => Self::Middle,
            3 => Self::Secondary,
            4 => Self::WheelUp,
            5 => Self::WheelDown,
            other => Self::Other(other),
        }
    }

    #[must_use]
    pub const fn is_wheel(self) -> bool {
        matches!(self, Self::WheelUp | Self::WheelDown)
    }
}

/// Overflow behavior per axis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Overflow {
    #[default]
    Visible,
    Hidden,
    Auto,
    Scroll,
}

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CursorKind {
    #[default]
    Default,
    Pointer,
    Text,
    Move,
    ResizeHorizontal,
    ResizeVertical,
    ResizeNorthWestSouthEast,
    ResizeNorthEastSouthWest,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImageTreatment {
    #[default]
    Original,
    SymbolicForeground,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextWrap {
    #[default]
    NoWrap,
    Wrap,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    pub display: Display,
    pub flex_direction: FlexDirection,
    pub justify_content: JustifyContent,
    pub align_items: AlignItems,
    pub position: Position,
    pub width: Length,
    pub height: Length,
    pub min_width: Length,
    pub min_height: Length,
    pub max_width: Length,
    pub max_height: Length,
    pub top: Length,
    pub right: Length,
    pub bottom: Length,
    pub left: Length,
    pub margin: Edges,
    pub padding: Edges,
    pub gap: f32,
    pub flex_grow: f32,
    pub background: ColorValue,
    pub color: ColorValue,
    pub border_color: ColorValue,
    pub border_width: f32,
    pub border_radius: f32,
    pub font_size: f32,
    pub font_weight: u16,
    pub text_wrap: TextWrap,
    pub break_anywhere: bool,
    pub opacity: f32,
    pub cursor: CursorKind,
    pub overflow_x: Overflow,
    pub overflow_y: Overflow,
    pub image_treatment: ImageTreatment,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            display: Display::Block,
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::Start,
            align_items: AlignItems::Stretch,
            position: Position::Flow,
            width: Length::Auto,
            height: Length::Auto,
            min_width: Length::Auto,
            min_height: Length::Auto,
            max_width: Length::Auto,
            max_height: Length::Auto,
            top: Length::Auto,
            right: Length::Auto,
            bottom: Length::Auto,
            left: Length::Auto,
            margin: Edges::ZERO,
            padding: Edges::ZERO,
            gap: 0.0,
            flex_grow: 0.0,
            background: ColorValue::Literal(Color::TRANSPARENT),
            color: ColorValue::Literal(Color::BLACK),
            border_color: ColorValue::Literal(Color::TRANSPARENT),
            border_width: 0.0,
            border_radius: 0.0,
            font_size: 14.0,
            font_weight: 400,
            text_wrap: TextWrap::NoWrap,
            break_anywhere: false,
            opacity: 1.0,
            cursor: CursorKind::Default,
            overflow_x: Overflow::Visible,
            overflow_y: Overflow::Visible,
            image_treatment: ImageTreatment::Original,
        }
    }
}

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
                if let Some(link) = link {
                    if link as usize >= self.nodes.len() {
                        return Err(format!("node {index} has invalid {field} index {link}"));
                    }
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
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
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
    hidden_nodes: HashSet<u32>,
    image_revision: HashMap<u16, u64>,
    node_image_overrides: HashMap<u32, RuntimeImageOverride>,
    geometry_overrides: HashMap<u32, GeometryOverride>,
    style_overrides: HashMap<u32, RuntimeStyleOverride>,
    z_index_overrides: HashMap<u32, i32>,
    scroll_offsets: HashMap<u32, ScrollState>,
    pending_damage: SurfaceDamage,
    revision: u64,
    id_index: HashMap<String, u32>,
    variable_index: HashMap<String, u16>,
    ui_font_family: String,
    ui_font_size_offset: f32,
    ui_font_bold: bool,
    ui_scale: f32,
}

impl RuntimeDocument {
    pub fn new(document: CompiledDocument) -> Result<Self, String> {
        document.validate()?;
        let mut id_index = HashMap::new();
        for (index, node) in document.nodes.iter().enumerate() {
            if !node.id.is_empty() {
                if id_index.insert(node.id.clone(), index as u32).is_some() {
                    return Err(format!("duplicate runtime id '{}'", node.id));
                }
            }
        }
        let mut variable_index = HashMap::new();
        let mut variable_values = Vec::with_capacity(document.variables.len());
        for (index, variable) in document.variables.iter().enumerate() {
            variable_index.insert(variable.name.clone(), index as u16);
            variable_values.push(variable.default);
        }
        Ok(Self {
            document,
            variable_values,
            text_overrides: HashMap::new(),
            hidden_nodes: HashSet::new(),
            image_revision: HashMap::new(),
            node_image_overrides: HashMap::new(),
            geometry_overrides: HashMap::new(),
            style_overrides: HashMap::new(),
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

    fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.pending_damage = self.pending_damage.merge(SurfaceDamage::Full);
    }

    fn touch_light(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// Retained-cache key: layout/paint caches are valid while this is
    /// unchanged for a given viewport size + interaction state.

    /// Damage bookkeeping: merge coverage (max), take clears the latch.
    pub fn mark_dirty(&mut self, damage: SurfaceDamage) {
        self.pending_damage = self.pending_damage.merge(damage);
    }

    pub fn mark_full(&mut self) {
        self.pending_damage = SurfaceDamage::Full;
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

    /// Node-local rect for damage marks (viewport box, unscaled doc coords).
    pub fn node_rect_for_damage(
        &self,
        index: u32,
        layout: &crate::layout::LayoutResult,
    ) -> Option<Rect> {
        layout.boxes.get(index as usize).map(|b| b.rect)
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

    pub fn node_by_id(&self, id: &str) -> Option<u32> {
        self.id_index.get(id).copied()
    }

    pub fn set_text(&mut self, id: &str, text: impl Into<String>) -> Result<(), String> {
        let index = self
            .node_by_id(id)
            .ok_or_else(|| format!("no node with id '{id}'"))?;
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
        self.text_overrides.insert(target, text.into());
        self.touch();
        Ok(())
    }

    pub fn set_visible(&mut self, id: &str, visible: bool) -> Result<(), String> {
        let index = self
            .node_by_id(id)
            .ok_or_else(|| format!("no node with id '{id}'"))?;
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
        let index = self
            .node_by_id(node_id)
            .ok_or_else(|| format!("no node with id '{node_id}'"))?;
        let node = self
            .document
            .nodes
            .get(index as usize)
            .ok_or_else(|| format!("no node with id '{node_id}'"))?;
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
        let revision = self
            .node_image_overrides
            .get(&index)
            .map(|entry| entry.revision)
            .unwrap_or(0)
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
        let node = self.document.nodes.get(index as usize)?;
        let asset_id = node.image?;
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
        self.document
            .nodes
            .get(index as usize)
            .and_then(|node| node.image)
            .map(|asset_id| self.image_revision(asset_id))
            .unwrap_or(0)
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

    pub fn set_color_variable(&mut self, name: &str, color: Color) -> Result<(), String> {
        let index = self
            .variable_index
            .get(name)
            .copied()
            .ok_or_else(|| format!("no color variable named '{name}'"))?;
        self.variable_values[index as usize] = color;
        Ok(())
    }

    pub fn set_position_px(&mut self, id: &str, left: f32, top: f32) -> Result<(), String> {
        if !left.is_finite() || !top.is_finite() {
            return Err("runtime position must be finite".to_string());
        }
        let index = self
            .node_by_id(id)
            .ok_or_else(|| format!("no node with id '{id}'"))?;
        let entry = self.geometry_overrides.entry(index).or_default();
        entry.left = Some(left);
        entry.top = Some(top);
        Ok(())
    }

    pub fn set_size_px(&mut self, id: &str, width: f32, height: f32) -> Result<(), String> {
        if !width.is_finite() || !height.is_finite() || width < 0.0 || height < 0.0 {
            return Err("runtime size must be finite and non-negative".to_string());
        }
        let index = self
            .node_by_id(id)
            .ok_or_else(|| format!("no node with id '{id}'"))?;
        let entry = self.geometry_overrides.entry(index).or_default();
        entry.width = Some(width);
        entry.height = Some(height);
        Ok(())
    }

    pub fn clear_geometry_override(&mut self, id: &str) -> Result<(), String> {
        let index = self
            .node_by_id(id)
            .ok_or_else(|| format!("no node with id '{id}'"))?;
        self.geometry_overrides.remove(&index);
        Ok(())
    }

    pub fn geometry_override(&self, index: u32) -> Option<GeometryOverride> {
        self.geometry_overrides.get(&index).copied()
    }

    pub fn set_background_color(&mut self, id: &str, color: Color) -> Result<(), String> {
        let index = self
            .node_by_id(id)
            .ok_or_else(|| format!("no node with id '{id}'"))?;
        self.style_overrides.entry(index).or_default().background =
            Some(ColorValue::Literal(color));
        Ok(())
    }

    pub fn set_border_color(&mut self, id: &str, color: Color) -> Result<(), String> {
        let index = self
            .node_by_id(id)
            .ok_or_else(|| format!("no node with id '{id}'"))?;
        self.style_overrides.entry(index).or_default().border_color =
            Some(ColorValue::Literal(color));
        Ok(())
    }

    pub fn clear_background_color(&mut self, id: &str) -> Result<(), String> {
        let index = self
            .node_by_id(id)
            .ok_or_else(|| format!("no node with id '{id}'"))?;
        if let Some(override_style) = self.style_overrides.get_mut(&index) {
            override_style.background = None;
        }
        Ok(())
    }

    pub fn clear_border_color(&mut self, id: &str) -> Result<(), String> {
        let index = self
            .node_by_id(id)
            .ok_or_else(|| format!("no node with id '{id}'"))?;
        if let Some(override_style) = self.style_overrides.get_mut(&index) {
            override_style.border_color = None;
        }
        Ok(())
    }

    pub fn set_foreground_color(&mut self, id: &str, color: ColorValue) -> Result<(), String> {
        let index = self
            .node_by_id(id)
            .ok_or_else(|| format!("no node with id '{id}'"))?;
        self.style_overrides.entry(index).or_default().color = Some(color);
        Ok(())
    }

    pub fn clear_foreground_color(&mut self, id: &str) -> Result<(), String> {
        let index = self
            .node_by_id(id)
            .ok_or_else(|| format!("no node with id '{id}'"))?;
        if let Some(override_style) = self.style_overrides.get_mut(&index) {
            override_style.color = None;
        }
        Ok(())
    }

    pub fn set_opacity(&mut self, id: &str, opacity: f32) -> Result<(), String> {
        if !opacity.is_finite() {
            return Err("runtime opacity must be finite".to_string());
        }
        let index = self
            .node_by_id(id)
            .ok_or_else(|| format!("no node with id '{id}'"))?;
        self.style_overrides.entry(index).or_default().opacity = Some(opacity.clamp(0.0, 1.0));
        Ok(())
    }

    pub fn set_flex_direction(&mut self, id: &str, direction: FlexDirection) -> Result<(), String> {
        let index = self
            .node_by_id(id)
            .ok_or_else(|| format!("no node with id '{id}'"))?;
        self.style_overrides
            .entry(index)
            .or_default()
            .flex_direction = Some(direction);
        Ok(())
    }

    pub fn set_overflow(&mut self, id: &str, x: Overflow, y: Overflow) -> Result<(), String> {
        let index = self
            .node_by_id(id)
            .ok_or_else(|| format!("no node with id '{id}'"))?;
        let entry = self.style_overrides.entry(index).or_default();
        entry.overflow_x = Some(x);
        entry.overflow_y = Some(y);
        Ok(())
    }

    pub fn set_font_size_px(&mut self, id: &str, size: f32) -> Result<(), String> {
        if !size.is_finite() || size <= 0.0 {
            return Err("runtime font size must be finite and positive".to_string());
        }
        let index = self
            .node_by_id(id)
            .ok_or_else(|| format!("no node with id '{id}'"))?;
        self.style_overrides.entry(index).or_default().font_size = Some(size);
        Ok(())
    }

    pub fn set_font_weight(&mut self, id: &str, weight: u16) -> Result<(), String> {
        let index = self
            .node_by_id(id)
            .ok_or_else(|| format!("no node with id '{id}'"))?;
        self.style_overrides.entry(index).or_default().font_weight = Some(weight.clamp(100, 900));
        Ok(())
    }

    pub fn set_ui_font(
        &mut self,
        family: impl Into<String>,
        size_offset: f32,
        bold: bool,
    ) -> Result<(), String> {
        let family = family.into();
        if family.trim().is_empty() {
            return Err("UI font family cannot be empty".to_string());
        }
        if !size_offset.is_finite() {
            return Err("UI font size offset must be finite".to_string());
        }
        self.set_ui_font_family(family)?;
        self.ui_font_size_offset = size_offset.clamp(-3.0, 6.0);
        self.ui_font_bold = bold;
        Ok(())
    }

    pub fn set_ui_font_family(&mut self, family: impl Into<String>) -> Result<(), String> {
        let family = family.into();
        if family.trim().is_empty() {
            return Err("UI font family cannot be empty".to_string());
        }
        self.ui_font_family = family;
        Ok(())
    }

    pub fn ui_font_family(&self) -> &str {
        &self.ui_font_family
    }

    pub fn set_ui_scale(&mut self, scale: f32) -> Result<(), String> {
        if !scale.is_finite() || scale <= 0.0 {
            return Err("UI scale must be finite and positive".to_string());
        }
        self.ui_scale = scale.clamp(0.5, 4.0);
        Ok(())
    }

    pub fn ui_scale(&self) -> f32 {
        self.ui_scale
    }

    pub fn set_z_index(&mut self, id: &str, z: i32) -> Result<(), String> {
        let index = self
            .node_by_id(id)
            .ok_or_else(|| format!("no node with id '{id}'"))?;
        self.z_index_overrides.insert(index, z);
        Ok(())
    }

    pub fn effective_z_index(&self, mut index: u32) -> i32 {
        loop {
            if let Some(z) = self.z_index_overrides.get(&index) {
                return *z;
            }
            let Some(parent) = self.document.nodes[index as usize].parent else {
                return 0;
            };
            index = parent;
        }
    }

    pub fn runtime_style(&self, index: u32, interaction: InteractionState) -> Style {
        let node = &self.document.nodes[index as usize];
        let mut style = interaction.style_for(index, node).clone();
        if !self.is_visible(index) {
            style.display = Display::None;
        }
        if let Some(geometry) = self.geometry_override(index) {
            if let Some(left) = geometry.left {
                style.left = Length::Px(left);
            }
            if let Some(top) = geometry.top {
                style.top = Length::Px(top);
            }
            if let Some(width) = geometry.width {
                style.width = Length::Px(width);
            }
            if let Some(height) = geometry.height {
                style.height = Length::Px(height);
            }
        }
        if let Some(override_style) = self.style_overrides.get(&index) {
            if let Some(background) = override_style.background {
                style.background = background;
            }
            if let Some(color) = override_style.color {
                style.color = color;
            }
            if let Some(border_color) = override_style.border_color {
                style.border_color = border_color;
            }
            if let Some(opacity) = override_style.opacity {
                style.opacity = opacity;
            }
            if let Some(direction) = override_style.flex_direction {
                style.flex_direction = direction;
            }
            if let Some(font_size) = override_style.font_size {
                style.font_size = font_size;
            }
            if let Some(font_weight) = override_style.font_weight {
                style.font_weight = font_weight;
            }
            if let Some(overflow_x) = override_style.overflow_x {
                style.overflow_x = overflow_x;
            }
            if let Some(overflow_y) = override_style.overflow_y {
                style.overflow_y = overflow_y;
            }
        }
        style.font_size = (style.font_size + self.ui_font_size_offset).max(6.0);
        if self.ui_font_bold && style.font_weight < 600 {
            style.font_weight = 600;
        }
        style
    }

    pub fn text_for(&self, index: u32) -> &str {
        self.text_overrides
            .get(&index)
            .map(String::as_str)
            .unwrap_or_else(|| self.document.nodes[index as usize].text.as_str())
    }

    pub fn resolve_color(&self, value: ColorValue) -> Color {
        match value {
            ColorValue::Literal(color) => color,
            ColorValue::Variable(index) => self
                .variable_values
                .get(index as usize)
                .copied()
                .unwrap_or(Color::TRANSPARENT),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InteractionState {
    pub hover: Option<u32>,
    pub active: Option<u32>,
}

impl InteractionState {
    pub fn style_for<'a>(&self, index: u32, node: &'a CompiledNode) -> &'a Style {
        if self.active == Some(index) {
            if let Some(style) = node.active_style.as_ref() {
                return style;
            }
        }
        if self.hover == Some(index) {
            if let Some(style) = node.hover_style.as_ref() {
                return style;
            }
        }
        &node.style
    }
}

#[cfg(test)]
mod image_replace_tests {
    use super::*;

    fn image_doc() -> RuntimeDocument {
        let asset = ImageAsset {
            source: "old".to_string(),
            width: 2,
            height: 1,
            pixels: vec![10, 20, 30, 255, 40, 50, 60, 255],
        };
        let image_node = CompiledNode {
            kind: NodeKind::Image,
            parent: None,
            first_child: None,
            next_sibling: None,
            id: "icon".to_string(),
            action: String::new(),
            text: String::new(),
            image: Some(0),
            style: Style::default(),
            hover_style: None,
            active_style: None,
        };
        let text_node = CompiledNode {
            kind: NodeKind::Text,
            parent: None,
            first_child: None,
            next_sibling: None,
            id: "label".to_string(),
            action: String::new(),
            text: "hi".to_string(),
            image: None,
            style: Style::default(),
            hover_style: None,
            active_style: None,
        };
        RuntimeDocument::new(CompiledDocument {
            source_fingerprint: 0,
            root: 0,
            variables: Vec::new(),
            assets: vec![asset],
            nodes: vec![image_node, text_node],
        })
        .expect("fixture document validates")
    }

    #[test]
    fn success_replaces_and_bumps_revision() {
        let mut doc = image_doc();
        let before = doc.document.assets[0].clone();
        doc.replace_image_for_node("icon", "new".to_string(), 1, 1, vec![1, 2, 3, 200])
            .expect("replace succeeds");
        // Compiled assets stay immutable; override is node-local.
        assert_eq!(doc.document.assets[0], before);
        let entry = doc.node_image_override(0).expect("override stored");
        assert_eq!(entry.source, "new");
        assert_eq!((entry.width, entry.height), (1, 1));
        assert_eq!(entry.rgba8, vec![1, 2, 3, 200]);
        assert_eq!(doc.image_revision_for_node(0), 1);
        assert_eq!(doc.image_revision(0), 0);
    }

    #[test]
    fn second_replace_bumps_again() {
        let mut doc = image_doc();
        doc.replace_image_for_node("icon", "a".to_string(), 1, 1, vec![1, 2, 3, 255])
            .expect("first replace");
        doc.replace_image_for_node("icon", "b".to_string(), 2, 1, vec![4u8; 8])
            .expect("second replace");
        let entry = doc.node_image_override(0).expect("override stored");
        assert_eq!(entry.source, "b");
        assert_eq!(doc.image_revision_for_node(0), 2);
        assert_eq!(doc.document.assets[0].source, "old");
    }

    #[test]
    fn wrong_kind_errs() {
        let mut doc = image_doc();
        let err = doc
            .replace_image_for_node("label", "x".to_string(), 1, 1, vec![0, 0, 0, 255])
            .expect_err("text node must fail");
        assert!(err.contains("not an image node"), "unexpected: {err}");
        assert_eq!(doc.image_revision_for_node(1), 0);
    }

    #[test]
    fn zero_dims_err() {
        let mut doc = image_doc();
        assert!(
            doc.replace_image_for_node("icon", "x".to_string(), 0, 1, vec![0, 0, 0, 255])
                .is_err()
        );
        assert!(
            doc.replace_image_for_node("icon", "x".to_string(), 1, 0, Vec::new())
                .is_err()
        );
        assert!(doc.node_image_override(0).is_none());
    }

    #[test]
    fn byte_count_mismatch_errs() {
        let mut doc = image_doc();
        let err = doc
            .replace_image_for_node("icon", "x".to_string(), 2, 2, vec![0u8; 8])
            .expect_err("must fail");
        assert!(err.contains("expected 16"), "unexpected: {err}");
        assert!(doc.node_image_override(0).is_none());
    }

    #[test]
    fn unknown_id_errs() {
        let mut doc = image_doc();
        assert!(
            doc.replace_image_for_node("missing", "x".to_string(), 1, 1, vec![0, 0, 0, 255])
                .is_err()
        );
    }

    #[test]
    fn opaque_rgba_fixture_reports_fully_opaque() {
        let doc = image_doc();
        assert!(doc.document.assets[0].is_fully_opaque());
    }

    fn shared_asset_doc() -> RuntimeDocument {
        let asset = ImageAsset {
            source: "shared".to_string(),
            width: 1,
            height: 1,
            pixels: vec![9, 9, 9, 255],
        };
        let mk = |id: &str| CompiledNode {
            kind: NodeKind::Image,
            parent: None,
            first_child: None,
            next_sibling: None,
            id: id.to_string(),
            action: String::new(),
            text: String::new(),
            image: Some(0),
            style: Style::default(),
            hover_style: None,
            active_style: None,
        };
        RuntimeDocument::new(CompiledDocument {
            source_fingerprint: 0,
            root: 0,
            variables: Vec::new(),
            assets: vec![asset],
            nodes: vec![mk("a"), mk("b")],
        })
        .expect("fixture validates")
    }

    #[test]
    fn shared_asset_isolation() {
        let mut doc = shared_asset_doc();
        doc.replace_image_for_node("a", "solo".to_string(), 1, 1, vec![1, 2, 3, 255])
            .expect("replace a");
        assert_eq!(doc.document.assets[0].pixels, vec![9, 9, 9, 255]);
        assert!(doc.node_image_override(1).is_none());
        let (_, _, _, _, pixels) = doc.image_for_node(1).expect("b intact");
        assert_eq!(pixels, &[9, 9, 9, 255]);
        assert_eq!(doc.image_revision_for_node(0), 1);
        assert_eq!(doc.image_revision_for_node(1), 0);
    }

    #[test]
    fn repeated_same_node_replace_stays_bounded() {
        let mut doc = image_doc();
        for i in 0..100u8 {
            doc.replace_image_for_node("icon", format!("f{i}"), 1, 1, vec![i, i, i, 255])
                .expect("replace");
        }
        let entry = doc.node_image_override(0).expect("override");
        assert_eq!(entry.rgba8.len(), 4);
        assert_eq!(doc.image_revision_for_node(0), 100);
        assert_eq!(doc.document.assets[0].pixels.len(), 8);
    }

    #[test]
    fn paint_identity_tracks_node_revision() {
        let mut doc = image_doc();
        let layout =
            crate::layout::LayoutEngine::compute(&doc, 50.0, 50.0, InteractionState::default());
        let before = crate::paint::build_paint_commands(&doc, &layout, InteractionState::default());
        doc.replace_image_for_node("icon", "n".to_string(), 1, 1, vec![5, 6, 7, 255])
            .expect("replace");
        let layout2 =
            crate::layout::LayoutEngine::compute(&doc, 50.0, 50.0, InteractionState::default());
        let after = crate::paint::build_paint_commands(&doc, &layout2, InteractionState::default());
        let rev = |cmds: &Vec<crate::paint::PaintCommand>| match cmds.iter().find_map(|c| match c {
            crate::paint::PaintCommand::Image { revision, .. } => Some(*revision),
            _ => None,
        }) {
            Some(v) => v,
            None => panic!("expected image command"),
        };
        assert_eq!(rev(&before), 0);
        assert_eq!(rev(&after), 1);
    }
}

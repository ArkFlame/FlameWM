use super::*;

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

/// Bounded Gaussian alpha-mask shadow. Zero blur preserves the legacy offset glyph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextShadow {
    pub offset_x: f32,
    pub offset_y: f32,
    pub blur_radius: f32,
    pub color: ColorValue,
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
    pub text_shadow: Option<TextShadow>,
    pub text_wrap: TextWrap,
    pub break_anywhere: bool,
    pub opacity: f32,
    pub cursor: CursorKind,
    pub overflow_x: Overflow,
    pub overflow_y: Overflow,
    /// CSS `scrollbar-width: none`: scrolling still works, no track or thumb is painted.
    pub hide_scrollbars: bool,
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
            text_shadow: None,
            text_wrap: TextWrap::NoWrap,
            break_anywhere: false,
            opacity: 1.0,
            cursor: CursorKind::Default,
            overflow_x: Overflow::Visible,
            overflow_y: Overflow::Visible,
            hide_scrollbars: false,
            image_treatment: ImageTreatment::Original,
        }
    }
}

impl RuntimeDocument {
    pub fn set_color_variable(&mut self, name: &str, color: Color) -> Result<(), String> {
        let index = self
            .variable_by_name(name)
            .ok_or_else(|| format!("no color variable named '{name}'"))?;
        if self.variable_values[index as usize] != color {
            self.variable_values[index as usize] = color;
            self.touch_paint();
        }
        Ok(())
    }
}

/// Stores `value` in `slot` and reports whether the stored value actually changed.
fn set_slot<T: PartialEq>(slot: &mut T, value: T) -> bool {
    if *slot == value {
        return false;
    }
    *slot = value;
    true
}

impl RuntimeDocument {
    fn override_entry(&mut self, id: &str) -> Result<&mut RuntimeStyleOverride, String> {
        let index = self.require_node(id)?;
        Ok(self.style_overrides.entry(index).or_default())
    }

    fn set_literal_color(
        &mut self,
        id: &str,
        field: impl Fn(&mut RuntimeStyleOverride) -> &mut Option<ColorValue>,
        color: Color,
    ) -> Result<(), String> {
        let index = self.require_node(id)?;
        let value = Some(ColorValue::Literal(color));
        if set_slot(field(self.style_overrides.entry(index).or_default()), value) {
            self.touch_node(index, DirtyReason::Paint, false);
        }
        Ok(())
    }

    fn clear_color_field(
        &mut self,
        id: &str,
        field: impl Fn(&mut RuntimeStyleOverride) -> &mut Option<ColorValue>,
    ) -> Result<(), String> {
        let index = self.require_node(id)?;
        let changed = self
            .style_overrides
            .get_mut(&index)
            .is_some_and(|style| field(style).take().is_some());
        if changed {
            self.touch_node(index, DirtyReason::Paint, false);
        }
        Ok(())
    }

    pub fn set_background_color(&mut self, id: &str, color: Color) -> Result<(), String> {
        self.set_literal_color(id, |style| &mut style.background, color)
    }

    /// Per-node backdrop request: the root pixels beneath a translucent node are captured,
    /// blurred by `blur_percent` and laid under its fill. `None` or `Some(0)` clears it.
    pub fn set_node_backdrop(&mut self, id: &str, blur_percent: Option<u8>) -> Result<(), String> {
        if blur_percent.is_some_and(|blur| blur > 100) {
            return Err("node backdrop blur must be 0..=100".to_string());
        }
        let index = self.require_node(id)?;
        let changed = match blur_percent.filter(|blur| *blur > 0) {
            Some(blur) => self.node_backdrops.insert(index, blur) != Some(blur),
            None => self.node_backdrops.remove(&index).is_some(),
        };
        if changed {
            self.touch();
        }
        Ok(())
    }

    /// Liquid Glass material for a node (drop shadow outside its rect, gradient rim). `None` clears it.
    pub fn set_surface_material(
        &mut self,
        id: &str,
        material: Option<crate::surface_material::SurfaceMaterial>,
    ) -> Result<(), String> {
        let index = self.require_node(id)?;
        let changed = match material {
            Some(material) => self.node_surfaces.insert(index, material) != Some(material),
            None => self.node_surfaces.remove(&index).is_some(),
        };
        if changed {
            self.touch();
        }
        Ok(())
    }

    /// Surface material requested for a node, if any.
    pub fn surface_material(&self, index: u32) -> Option<crate::surface_material::SurfaceMaterial> {
        self.node_surfaces.get(&index).copied()
    }

    /// Blur percent requested for a node's backdrop, if any.
    pub fn node_backdrop_blur(&self, index: u32) -> Option<u8> {
        self.node_backdrops.get(&index).copied()
    }

    /// Runtime corner radius in px for a node (popup/menu shape follows the decoration setting).
    pub fn set_border_radius(&mut self, id: &str, radius: f32) -> Result<(), String> {
        if !radius.is_finite() || radius < 0.0 {
            return Err("runtime border radius must be finite and non-negative".to_string());
        }
        // Panel geometry may be projected on every settings/reconcile tick.
        // Unchanged shape values must not invalidate the retained scene.
        let entry = self.override_entry(id)?;
        if entry.border_radius != Some(radius) {
            entry.border_radius = Some(radius);
            self.touch();
        }
        Ok(())
    }

    pub fn set_border_color(&mut self, id: &str, color: Color) -> Result<(), String> {
        self.set_literal_color(id, |style| &mut style.border_color, color)
    }

    pub fn clear_background_color(&mut self, id: &str) -> Result<(), String> {
        self.clear_color_field(id, |style| &mut style.background)
    }

    pub fn clear_border_color(&mut self, id: &str) -> Result<(), String> {
        self.clear_color_field(id, |style| &mut style.border_color)
    }

    pub fn set_foreground_color(&mut self, id: &str, color: ColorValue) -> Result<(), String> {
        let index = self.require_node(id)?;
        if self.set_override_and_mirror(index, |style| &mut style.color, color) {
            self.touch_paint();
        }
        Ok(())
    }

    pub fn clear_foreground_color(&mut self, id: &str) -> Result<(), String> {
        let index = self.require_node(id)?;
        let changed = self
            .style_overrides
            .get_mut(&index)
            .is_some_and(|style| style.color.take().is_some());
        if changed {
            self.touch_paint();
        }
        Ok(())
    }

    pub fn set_overflow(&mut self, id: &str, x: Overflow, y: Overflow) -> Result<(), String> {
        let entry = self.override_entry(id)?;
        let changed =
            set_slot(&mut entry.overflow_x, Some(x)) | set_slot(&mut entry.overflow_y, Some(y));
        if changed {
            self.touch();
        }
        Ok(())
    }

    pub fn set_font_size_px(&mut self, id: &str, size: f32) -> Result<(), String> {
        if !size.is_finite() || size <= 0.0 {
            return Err("runtime font size must be finite and positive".to_string());
        }
        let index = self.require_node(id)?;
        if self.set_override_and_mirror(index, |style| &mut style.font_size, size) {
            self.touch();
        }
        Ok(())
    }

    /// Text children are measured with their own compile-time style, so an element override alone
    /// never reaches the glyphs: apply the same override to the element's direct text children.
    fn mirror_to_text_children(
        &mut self,
        index: u32,
        apply: impl Fn(&mut RuntimeStyleOverride) -> bool,
    ) -> bool {
        let mut changed = false;
        let mut child = self.document.nodes[index as usize].first_child;
        while let Some(current) = child {
            let node = &self.document.nodes[current as usize];
            let next = node.next_sibling;
            if node.kind == NodeKind::Text {
                changed |= apply(self.style_overrides.entry(current).or_default());
            }
            child = next;
        }
        changed
    }

    /// Sets one override field on `index` and the same value on its direct text children.
    /// Returns whether any stored value changed.
    fn set_override_and_mirror<T: PartialEq + Copy>(
        &mut self,
        index: u32,
        field: impl Fn(&mut RuntimeStyleOverride) -> &mut Option<T>,
        value: T,
    ) -> bool {
        let mut changed = set_slot(
            field(self.style_overrides.entry(index).or_default()),
            Some(value),
        );
        changed |= self.mirror_to_text_children(index, |style| set_slot(field(style), Some(value)));
        changed
    }

    pub fn set_font_weight(&mut self, id: &str, weight: u16) -> Result<(), String> {
        let weight = weight.clamp(100, 900);
        let index = self.require_node(id)?;
        if self.set_override_and_mirror(index, |style| &mut style.font_weight, weight) {
            // The weight only re-measures this element's own text when it is an isolated box.
            if self.touch_isolated_box(index, DirtyReason::Text) {
                self.dirty.full = true;
            } else {
                self.touch();
            }
        }
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

    /// Applies the global text size offset and minimum weight to every computed style.
    pub fn set_ui_font_defaults(&mut self, size_offset: f32, bold: bool) -> Result<(), String> {
        if !size_offset.is_finite() {
            return Err("UI font size offset must be finite".to_string());
        }
        if self.ui_font_size_offset != size_offset || self.ui_font_bold != bold {
            self.ui_font_size_offset = size_offset;
            self.ui_font_bold = bold;
            self.touch();
        }
        Ok(())
    }

    pub fn set_ui_scale(&mut self, scale: f32) -> Result<(), String> {
        if !scale.is_finite() || scale <= 0.0 {
            return Err("UI scale must be finite and positive".to_string());
        }
        let scale = scale.clamp(0.5, 4.0);
        if self.ui_scale != scale {
            self.ui_scale = scale;
            self.touch();
        }
        Ok(())
    }

    pub fn ui_scale(&self) -> f32 {
        self.ui_scale
    }

    pub fn set_z_index(&mut self, id: &str, z: i32) -> Result<(), String> {
        let index = self.require_node(id)?;
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
            macro_rules! apply {
                ($($field:ident),+) => {
                    $(if let Some(value) = override_style.$field {
                        style.$field = value;
                    })+
                };
            }
            apply!(background, color, border_color, border_radius, opacity);
            apply!(flex_direction, font_size, font_weight);
            apply!(overflow_x, overflow_y);
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
            .unwrap_or_else(|| &*self.document.nodes[index as usize].text)
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

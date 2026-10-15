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

impl RuntimeDocument {
    pub fn set_color_variable(&mut self, name: &str, color: Color) -> Result<(), String> {
        let index = self
            .variable_index
            .get(name)
            .copied()
            .ok_or_else(|| format!("no color variable named '{name}'"))?;
        if self.variable_values[index as usize] != color {
            self.variable_values[index as usize] = color;
            self.touch();
        }
        Ok(())
    }
}

impl RuntimeDocument {
    pub fn set_background_color(&mut self, id: &str, color: Color) -> Result<(), String> {
        let index = self
            .node_by_id(id)
            .ok_or_else(|| format!("no node with id '{id}'"))?;
        self.style_overrides.entry(index).or_default().background =
            Some(ColorValue::Literal(color));
        self.touch();
        Ok(())
    }

    /// Per-node backdrop request: the root pixels beneath a translucent node are captured,
    /// blurred by `blur_percent` and laid under its fill. `None` or `Some(0)` clears it.
    pub fn set_node_backdrop(&mut self, id: &str, blur_percent: Option<u8>) -> Result<(), String> {
        if blur_percent.is_some_and(|blur| blur > 100) {
            return Err("node backdrop blur must be 0..=100".to_string());
        }
        let index = self
            .node_by_id(id)
            .ok_or_else(|| format!("no node with id '{id}'"))?;
        match blur_percent.filter(|blur| *blur > 0) {
            Some(blur) => {
                self.node_backdrops.insert(index, blur);
            }
            None => {
                self.node_backdrops.remove(&index);
            }
        }
        self.touch();
        Ok(())
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
        let index = self
            .node_by_id(id)
            .ok_or_else(|| format!("no node with id '{id}'"))?;
        self.style_overrides.entry(index).or_default().border_radius = Some(radius);
        self.touch();
        Ok(())
    }

    pub fn set_border_color(&mut self, id: &str, color: Color) -> Result<(), String> {
        let index = self
            .node_by_id(id)
            .ok_or_else(|| format!("no node with id '{id}'"))?;
        self.style_overrides.entry(index).or_default().border_color =
            Some(ColorValue::Literal(color));
        self.touch();
        Ok(())
    }

    pub fn clear_background_color(&mut self, id: &str) -> Result<(), String> {
        let index = self
            .node_by_id(id)
            .ok_or_else(|| format!("no node with id '{id}'"))?;
        if let Some(override_style) = self.style_overrides.get_mut(&index) {
            override_style.background = None;
        }
        self.touch();
        Ok(())
    }

    pub fn clear_border_color(&mut self, id: &str) -> Result<(), String> {
        let index = self
            .node_by_id(id)
            .ok_or_else(|| format!("no node with id '{id}'"))?;
        if let Some(override_style) = self.style_overrides.get_mut(&index) {
            override_style.border_color = None;
        }
        self.touch();
        Ok(())
    }

    pub fn set_foreground_color(&mut self, id: &str, color: ColorValue) -> Result<(), String> {
        let index = self
            .node_by_id(id)
            .ok_or_else(|| format!("no node with id '{id}'"))?;
        self.style_overrides.entry(index).or_default().color = Some(color);
        self.touch();
        Ok(())
    }

    pub fn clear_foreground_color(&mut self, id: &str) -> Result<(), String> {
        let index = self
            .node_by_id(id)
            .ok_or_else(|| format!("no node with id '{id}'"))?;
        if let Some(override_style) = self.style_overrides.get_mut(&index) {
            override_style.color = None;
        }
        self.touch();
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
        // Text children are measured with their own compile-time style, so the element override alone
        // never reaches the glyphs: mirror it onto the element's direct text children.
        let mut child = self.document.nodes[index as usize].first_child;
        while let Some(current) = child {
            let node = &self.document.nodes[current as usize];
            let next = node.next_sibling;
            if node.kind == NodeKind::Text {
                self.style_overrides.entry(current).or_default().font_size = Some(size);
            }
            child = next;
        }
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
            if let Some(border_radius) = override_style.border_radius {
                style.border_radius = border_radius;
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

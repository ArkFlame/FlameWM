use super::*;

mod runtime;

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

    /// Resolves `left`/`right`/`top`/`bottom` offsets, which CSS allows to be negative
    /// (an absolute child may sit outside its containing block). Sizes use `resolve`.
    pub fn resolve_offset(self, available: f32) -> Option<f32> {
        match self {
            Self::Auto => None,
            Self::Px(value) => Some(value),
            Self::Percent(value) => Some(available * value),
        }
    }
}

#[cfg(test)]
mod length_tests {
    use super::Length;

    #[test]
    fn negative_offsets_survive_while_sizes_clamp() {
        assert_eq!(Length::Px(-15.0).resolve_offset(200.0), Some(-15.0));
        assert_eq!(Length::Percent(-0.5).resolve_offset(200.0), Some(-100.0));
        assert_eq!(Length::Px(-15.0).resolve(200.0), Some(0.0));
        assert_eq!(Length::Auto.resolve_offset(200.0), None);
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

/// CSS `text-overflow`: `Ellipsis` truncates a single-line overflowing run and appends U+2026.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextOverflow {
    #[default]
    Clip,
    Ellipsis,
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
    pub text_overflow: TextOverflow,
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
            text_overflow: TextOverflow::Clip,
            opacity: 1.0,
            cursor: CursorKind::Default,
            overflow_x: Overflow::Visible,
            overflow_y: Overflow::Visible,
            hide_scrollbars: false,
            image_treatment: ImageTreatment::Original,
        }
    }
}

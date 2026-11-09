use crate::model::*;

// One table per wire enum: the tag bytes live here once, for both directions.
pub(super) trait Wire: Sized {
    const NAME: &'static str;
    fn to_wire(&self) -> u8;
    fn from_wire(value: u8) -> Option<Self>;
}

macro_rules! wire_enum {
    ($name:literal $ty:ident { $($variant:ident = $tag:literal),+ $(,)? }) => {
        impl Wire for $ty {
            const NAME: &'static str = $name;
            fn to_wire(&self) -> u8 {
                match self {
                    $($ty::$variant => $tag,)+
                }
            }
            fn from_wire(value: u8) -> Option<Self> {
                match value {
                    $($tag => Some($ty::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

wire_enum!("node kind" NodeKind { Element = 0, Text = 1, Image = 2 });
wire_enum!("display" Display { None = 0, Block = 1, Flex = 2 });
wire_enum!("flex direction" FlexDirection { Row = 0, Column = 1 });
wire_enum!("justify content" JustifyContent { Start = 0, Center = 1, End = 2, SpaceBetween = 3 });
wire_enum!("align items" AlignItems { Start = 0, Center = 1, End = 2, Stretch = 3 });
wire_enum!("position" Position { Flow = 0, Absolute = 1 });
wire_enum!("cursor kind" CursorKind {
    Default = 0,
    Pointer = 1,
    Text = 2,
    Move = 3,
    ResizeHorizontal = 4,
    ResizeVertical = 5,
    ResizeNorthWestSouthEast = 6,
    ResizeNorthEastSouthWest = 7,
});
wire_enum!("overflow" Overflow { Visible = 0, Hidden = 1, Auto = 2, Scroll = 3 });
wire_enum!("image treatment" ImageTreatment { Original = 0, SymbolicForeground = 1 });
wire_enum!("text wrap" TextWrap { NoWrap = 0, Wrap = 1 });
wire_enum!("text overflow" TextOverflow { Clip = 0, Ellipsis = 1 });

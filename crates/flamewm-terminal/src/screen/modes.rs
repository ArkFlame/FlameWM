//! Cursor-shape and mouse-reporting mode enums shared with the view and input layers.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Underline,
    Bar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseMode {
    Off,
    Press,
    Drag,
    Any,
}

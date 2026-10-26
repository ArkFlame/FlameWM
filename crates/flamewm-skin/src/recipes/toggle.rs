use crate::palette::{ACCENT, Rgb};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToggleRecipe {
    pub width: u16,
    pub height: u16,
    pub knob: u16,
    pub on: Rgb,
    pub off: Rgb,
}

/// CSS `.toggle-on/.toggle-off`: 40x22, 18px knob at 2px inset.
pub const TOGGLE: ToggleRecipe = ToggleRecipe {
    width: crate::DEFAULT.settings.toggle_width,
    height: crate::DEFAULT.settings.toggle_height,
    knob: crate::DEFAULT.settings.knob,
    on: ACCENT,
    off: Rgb(0x0055_5a5f),
};

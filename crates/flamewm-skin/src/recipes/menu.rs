use crate::palette::ACCENT_SOFT;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuRecipe {
    pub width: u16,
    pub row_width: u16,
    pub row_height: u16,
    pub separator_width: u16,
    pub padding: u16,
    pub radius: u16,
    pub hover: crate::palette::AlphaColor,
}

/// CSS `.context-menu`, `.menu-row`, and `.menu-separator` exact geometry.
pub const CONTEXT_MENU: MenuRecipe = MenuRecipe {
    width: 205,
    row_width: 195,
    row_height: 31,
    separator_width: 185,
    padding: 5,
    radius: 12,
    hover: ACCENT_SOFT,
};

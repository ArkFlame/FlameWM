use super::*;

/// Bit position of the lowest set bit in a channel mask. Zero mask -> 0.
pub(super) fn mask_shift(mask: u64) -> u32 {
    if mask == 0 {
        return 0;
    }
    mask.trailing_zeros()
}

/// Bit width of the contiguous set field in a channel mask, from its lowest
/// set bit. Zero mask -> 0.
pub(super) fn mask_width(mask: u64) -> u32 {
    if mask == 0 {
        return 0;
    }
    64 - (mask >> mask_shift(mask)).leading_zeros()
}

/// Scale one extracted channel field to 8 bits using the live mask geometry.
pub(super) fn scale_channel(value: u64, mask: u64) -> u8 {
    let width = mask_width(mask);
    if width == 0 {
        return 0;
    }
    let field = (value & mask) >> mask_shift(mask);
    if width >= 8 {
        (field >> (width - 8)) as u8
    } else {
        ((field * 255) / ((1u64 << width) - 1)) as u8
    }
}

/// Decode one server pixel word into (r, g, b) using live visual masks.
/// No hardcoded channel order: shifts/widths come from the server.
#[must_use]
pub fn decode_pixel(pixel: u64, red_mask: u64, green_mask: u64, blue_mask: u64) -> (u8, u8, u8) {
    (
        scale_channel(pixel, red_mask),
        scale_channel(pixel, green_mask),
        scale_channel(pixel, blue_mask),
    )
}

/// Translucent accent fill over a captured backdrop. Pure and testable:
/// 20% default maps to alpha 51 to match the existing preview contract.
#[must_use]
pub fn fill_color(opacity_percent: u8) -> (u8, u8, u8, u8) {
    let opacity = u32::from(opacity_percent.min(100));
    let alpha = ((opacity * 255 + 50) / 100) as u8;
    (
        BACKDROP_ACCENT_RGB.0,
        BACKDROP_ACCENT_RGB.1,
        BACKDROP_ACCENT_RGB.2,
        alpha,
    )
}

pub type SelectionMaterial = ((u8, u8, u8, u8), (u8, u8, u8, u8));

/// Shared desktop-selection material for one material, two consumers
/// (desktop selection renderer and snap-preview backdrop): translucent
/// accent fill plus strong accent border ring. Pure and headless-testable.
#[must_use]
pub fn selection_material(opacity_percent: u8) -> SelectionMaterial {
    (
        fill_color(opacity_percent),
        (
            BACKDROP_ACCENT_RGB.0,
            BACKDROP_ACCENT_RGB.1,
            BACKDROP_ACCENT_RGB.2,
            BACKDROP_BORDER_ALPHA,
        ),
    )
}

/// Channel masks to decode with. XGetImage on a pixmap reports zero masks, so a zero triple
/// falls back to the 24/32-bpp TrueColor layout (red, green, blue in the low three bytes).
/// Window images keep the masks their visual reports.
pub fn channel_masks(red: u64, green: u64, blue: u64) -> (u64, u64, u64) {
    if red | green | blue == 0 {
        (0xff_0000, 0xff_00, 0xff)
    } else {
        (red, green, blue)
    }
}

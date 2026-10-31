use super::*;

/// Explicit OpaqueFallback flatten: only used when SurfaceAlphaMode is
/// OpaqueFallback. Composited/shape paths never flatten through black.
pub(crate) fn flatten_opaque_fallback(color: Color) -> Color {
    let alpha = color.a as u16;
    Color::rgb(
        ((color.r as u16 * alpha) / 255) as u8,
        ((color.g as u16 * alpha) / 255) as u8,
        ((color.b as u16 * alpha) / 255) as u8,
    )
}

impl X11App {
    pub(crate) unsafe fn pixel(&mut self, color: Color) -> Result<u64, String> {
        // Retired: normal blend_over_black. Only explicit OpaqueFallback
        // flattens; composited/shape paths never flatten.
        let opaque = if color.a == 255
            || self.alpha_mode == SurfaceAlphaMode::CompositedArgb32
            || self.alpha_mode == SurfaceAlphaMode::ShapeBackedArgb32
        {
            color
        } else {
            flatten_opaque_fallback(color)
        };
        if let Some(pixel) = self.colors.get(&opaque) {
            return Ok(*pixel);
        }
        let mut xcolor = XColor {
            pixel: 0,
            red: u16::from(opaque.r) * 257,
            green: u16::from(opaque.g) * 257,
            blue: u16::from(opaque.b) * 257,
            flags: DO_RED | DO_GREEN | DO_BLUE,
            pad: 0,
        };
        // SAFETY: display and colormap are valid X11 resources owned by this app; xcolor is local.
        if unsafe { XAllocColor(self.display, self.colormap, &mut xcolor) } == 0 {
            return Err(format!(
                "XAllocColor failed for #{:02x}{:02x}{:02x}",
                opaque.r, opaque.g, opaque.b
            ));
        }
        self.colors.insert(opaque, xcolor.pixel as u64);
        Ok(xcolor.pixel as u64)
    }
}
pub(crate) fn rgb_to_pixel(
    r: u8,
    g: u8,
    b: u8,
    red_mask: u64,
    green_mask: u64,
    blue_mask: u64,
) -> u64 {
    channel_to_mask(r, red_mask) | channel_to_mask(g, green_mask) | channel_to_mask(b, blue_mask)
}

pub(crate) fn channel_to_mask(value: u8, mask: u64) -> u64 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let normalized = mask >> shift;
    let max = normalized;
    let scaled = ((value as u128 * max as u128) + 127) / 255;
    ((scaled as u64) << shift) & mask
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_rgb_into_common_truecolor_masks() {
        assert_eq!(
            rgb_to_pixel(0x12, 0x34, 0x56, 0x00ff0000, 0x0000ff00, 0x000000ff),
            0x00123456
        );
    }

    #[test]
    fn alpha_blends_over_black() {
        assert_eq!(
            flatten_opaque_fallback(Color {
                r: 200,
                g: 100,
                b: 50,
                a: 128
            }),
            Color::rgb(100, 50, 25)
        );
    }
}

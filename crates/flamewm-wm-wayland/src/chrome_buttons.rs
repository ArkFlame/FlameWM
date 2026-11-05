//! Titlebar window-control painting for the Wayland chrome.
//!
//! Colours come from the shared skin style (`flamewm_skin::recipes::window_control`), the same
//! owner the X11 frame uses. This module only maps a button to its style and rasterises the
//! hover disk and glyph into the titlebar bitmap.

use flamewm_skin::recipes::window_chrome::WindowControlRole;
use flamewm_skin::recipes::window_control::{ControlKind, ControlState, control_colors};

use crate::model::Button;
use crate::render::{packaged_root, put};

/// Pointer feedback for the titlebar controls: the button under the pointer, and the one held down.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Feedback {
    pub hover: Option<Button>,
    pub pressed: Option<Button>,
}

#[must_use]
pub(crate) const fn control_kind(button: Button) -> ControlKind {
    match button {
        Button::Minimize => ControlKind::Minimize,
        Button::Maximize => ControlKind::Maximize,
        Button::Close => ControlKind::Close,
    }
}

/// Paint one control over the titlebar pixels: the hover disk (radius = half the glyph edge)
/// when the state has a fill, then the glyph in the state's colour.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_control(
    pixels: &mut [u8],
    width: usize,
    height: usize,
    center: i32,
    cy: i32,
    edge: u32,
    role: WindowControlRole,
    button: Button,
    feedback: Feedback,
) {
    let state = ControlState::from_pointer(
        feedback.hover == Some(button),
        feedback.pressed == Some(button),
    );
    let colors = control_colors(
        control_kind(button),
        state,
        flamewm_skin::palette::Palette::RWR_0_0_9,
    );
    let half = edge as i32 / 2;
    if let Some(fill) = colors.background {
        let radius = edge as f32 / 2.0;
        let (w, h) = (width as i32, height as i32);
        for y in (cy - half).max(0)..(cy + half + 1).min(h) {
            for x in (center - half).max(0)..(center + half + 1).min(w) {
                let dx = x as f32 + 0.5 - center as f32;
                let dy = y as f32 + 0.5 - cy as f32;
                let coverage = (radius + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
                if coverage > 0.0 {
                    put(
                        pixels,
                        width,
                        height,
                        x,
                        y,
                        fill.0,
                        (coverage * 255.0).round() as u8,
                    );
                }
            }
        }
    }
    if let Some(glyph) = glyph_pixels(role, edge, colors.glyph.0) {
        let start_x = center - half;
        let start_y = cy - half;
        for (index, rgba) in glyph.chunks_exact(4).enumerate() {
            let alpha = rgba[3];
            if alpha > 0 {
                let x = start_x + (index % edge as usize) as i32;
                let y = start_y + (index / edge as usize) as i32;
                put(pixels, width, height, x, y, colors.glyph.0, alpha);
            }
        }
    }
}

/// Breeze control glyph rasterised in `rgb` (cached per role, edge and colour).
fn glyph_pixels(role: WindowControlRole, edge: u32, rgb: u32) -> Option<Vec<u8>> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    type Cache = Mutex<HashMap<(u8, u32, u32), Option<Vec<u8>>>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let key = (role as u8, edge, rgb);
    let mut cache = CACHE.get_or_init(Default::default).lock().ok()?;
    cache
        .entry(key)
        .or_insert_with(|| {
            let rel = role.icon_role().source().path;
            let bytes = std::fs::read(packaged_root().join(rel)).ok()?;
            let mut scheme = flamewm_image_core::SvgColorScheme::flame_default();
            scheme.text = [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255];
            flamewm_image_core::svg::render_with_color_scheme(&bytes, edge, edge, &scheme)
                .ok()
                .map(|image| image.pixels)
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 80;
    const H: usize = 40;

    fn px(pixels: &[u8], x: usize, y: usize) -> [u8; 4] {
        let at = (y * W + x) * 4;
        [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
    }

    #[test]
    fn button_kind_folds_maximize_and_restore() {
        assert_eq!(control_kind(Button::Minimize), ControlKind::Minimize);
        assert_eq!(control_kind(Button::Maximize), ControlKind::Maximize);
        assert_eq!(control_kind(Button::Close), ControlKind::Close);
    }

    #[test]
    fn hovered_minimize_paints_white_disk_under_dark_glyph() {
        let mut pixels = vec![0u8; W * H * 4];
        paint_control(
            &mut pixels,
            W,
            H,
            40,
            20,
            16,
            WindowControlRole::Minimize,
            Button::Minimize,
            Feedback { hover: Some(Button::Minimize), pressed: None },
        );
        // Top-centre of the disk sits above the glyph strokes.
        let disk = px(&pixels, 40, 13);
        assert_eq!(disk[3], 255, "disk must be opaque at its interior");
        // Buffer bytes are BGRA: #f1f2f3 is stored as [b, g, r].
        assert_eq!(&disk[..3], &[0xf3, 0xf2, 0xf1], "min hover fill is white");
        // The 16px bar is thinner than a pixel of full coverage, so antialiasing keeps no pixel at
        // the exact glyph colour. Check the glyph darkens the white disk instead: the darkest
        // pixel in the glyph box must be far below the fill.
        let darkest = (0..16)
            .flat_map(|dy| (0..16).map(move |dx| (dx, dy)))
            .map(|(dx, dy)| px(&pixels, 40 - 8 + dx, 20 - 8 + dy)[2])
            .min()
            .unwrap_or(255);
        assert!(darkest < 0x60, "hovered minimize glyph must be dark (darkest red channel {darkest:#x})");
    }

    fn paint_at(role: WindowControlRole, button: Button, feedback: Feedback) -> Vec<u8> {
        let mut pixels = vec![0u8; W * H * 4];
        paint_control(&mut pixels, W, H, 40, 20, 16, role, button, feedback);
        pixels
    }

    #[test]
    fn pressed_minimize_uses_pressed_fill_not_hover_fill() {
        let pressed = paint_at(
            WindowControlRole::Minimize,
            Button::Minimize,
            Feedback { hover: Some(Button::Minimize), pressed: Some(Button::Minimize) },
        );
        // #c7c9cb stored as BGRA [0xcb, 0xc9, 0xc7]; distinct from the #f1f2f3 hover fill.
        assert_eq!(&px(&pressed, 40, 13)[..3], &[0xcb, 0xc9, 0xc7]);
    }

    #[test]
    fn pressed_close_is_darker_red_than_hover() {
        let pressed = paint_at(
            WindowControlRole::Close,
            Button::Close,
            Feedback { hover: Some(Button::Close), pressed: Some(Button::Close) },
        );
        // #c42b1c stored as BGRA [0x1c, 0x2b, 0xc4].
        assert_eq!(&px(&pressed, 40, 13)[..3], &[0x1c, 0x2b, 0xc4]);
    }

    #[test]
    fn press_on_other_button_does_not_change_this_control() {
        let rest = paint_at(WindowControlRole::Minimize, Button::Minimize, Feedback::default());
        let other = paint_at(
            WindowControlRole::Minimize,
            Button::Minimize,
            Feedback { hover: None, pressed: Some(Button::Close) },
        );
        assert_eq!(rest, other);
    }

    #[test]
    fn hovered_close_paints_red_disk() {
        let mut pixels = vec![0u8; W * H * 4];
        paint_control(
            &mut pixels,
            W,
            H,
            40,
            20,
            16,
            WindowControlRole::Close,
            Button::Close,
            Feedback { hover: Some(Button::Close), pressed: None },
        );
        let disk = px(&pixels, 40, 13);
        // #e81123 stored as BGRA [0x23, 0x11, 0xe8].
        assert_eq!(&disk[..3], &[0x23, 0x11, 0xe8], "close hover fill is red");
    }

    #[test]
    fn rest_paints_no_disk() {
        let mut pixels = vec![0u8; W * H * 4];
        paint_control(
            &mut pixels,
            W,
            H,
            40,
            20,
            16,
            WindowControlRole::Minimize,
            Button::Minimize,
            Feedback::default(),
        );
        // Corner of the control cell is outside the disk and outside the glyph.
        assert_eq!(px(&pixels, 40 - 8, 20 - 8), [0, 0, 0, 0]);
    }
}

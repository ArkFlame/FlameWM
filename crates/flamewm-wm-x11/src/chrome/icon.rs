//! Validated `_NET_WM_ICON` parsing, scaling and squircle masking for window chrome.

/// Validated `_NET_WM_ICON` image (EWMH ARGB32 cardinals, non-premultiplied).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconImage {
    pub width: u32,
    pub height: u32,
    pub argb: Vec<u32>,
}

/// Parse concatenated EWMH `_NET_WM_ICON` cardinals; select largest valid image.
/// Rejects zero/overflow dimensions and truncated payloads.
pub fn parse_net_wm_icon(cardinals: &[u32]) -> Option<IconImage> {
    let mut cursor = 0_usize;
    let mut best: Option<IconImage> = None;
    while cursor.saturating_add(2) <= cardinals.len() {
        let width = cardinals[cursor];
        let height = cardinals[cursor + 1];
        cursor += 2;
        if width == 0 || height == 0 {
            return None;
        }
        let count = width
            .checked_mul(height)
            .and_then(|pixels| usize::try_from(pixels).ok())?;
        if cursor.saturating_add(count) > cardinals.len() {
            return None;
        }
        let argb = cardinals[cursor..cursor + count].to_vec();
        cursor += count;
        let area = u64::from(width) * u64::from(height);
        let best_area = best.as_ref().map_or(0, |image: &IconImage| {
            u64::from(image.width) * u64::from(image.height)
        });
        if area >= best_area {
            best = Some(IconImage {
                width,
                height,
                argb,
            });
        }
    }
    if cursor != cardinals.len() {
        return None;
    }
    best.filter(|image| !image.argb.is_empty())
}

/// Nearest-neighbor scale into a square titlebar slot, alpha preserved.
pub fn scale_icon_to_slot(icon: &IconImage, slot: u32) -> Option<IconImage> {
    if slot == 0 || icon.width == 0 || icon.height == 0 {
        return None;
    }
    let slot_usize = usize::try_from(slot).ok()?;
    let width_usize = usize::try_from(icon.width).ok()?;
    let height_usize = usize::try_from(icon.height).ok()?;
    let mut argb = Vec::with_capacity(slot_usize * slot_usize);
    for y in 0..slot_usize {
        let source_y = y * height_usize / slot_usize;
        for x in 0..slot_usize {
            let source_x = x * width_usize / slot_usize;
            argb.push(icon.argb[source_y * width_usize + source_x]);
        }
    }
    Some(IconImage {
        width: slot,
        height: slot,
        argb,
    })
}

/// Corner radius of the app-icon squircle, as a fraction of the slot edge (Tahoe app icons).
pub const ICON_SQUIRCLE_RADIUS_PERMILLE: u32 = 220;

/// Clip an app icon to a rounded squircle, antialiased on the edge.
///
/// Coverage comes from the signed distance to a rounded square of radius `22%` of the edge and
/// multiplies the existing alpha. Square or antialiased-square tiles lose their sharp corners;
/// round or inset artwork lies inside the squircle and is left unchanged.
pub fn squircle_mask_opaque_tile(argb: &mut [u32], width: u32, height: u32) {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || argb.len() != w * h {
        return;
    }
    let edge = w.min(h) as f32;
    let half = edge / 2.0;
    let radius = edge * ICON_SQUIRCLE_RADIUS_PERMILLE as f32 / 1000.0;
    let inner = half - radius;
    for y in 0..h {
        for x in 0..w {
            let qx = (x as f32 + 0.5 - half).abs() - inner;
            let qy = (y as f32 + 0.5 - half).abs() - inner;
            let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
            let distance = outside + qx.max(qy).min(0.0) - radius;
            let coverage = (0.5 - distance).clamp(0.0, 1.0);
            let pixel = &mut argb[y * w + x];
            let a = *pixel >> 24;
            let masked = (a as f32 * coverage).round() as u32;
            *pixel = (*pixel & 0x00ff_ffff) | (masked << 24);
        }
    }
}

/// Straight RGBA8 raster for a native `_NET_WM_ICON` selection, scaled into
/// the square titlebar slot (canonical blit input for `blit_rgba`).
#[must_use]
pub fn native_icon_rgba(icon: &IconImage, slot: u32) -> Option<flamewm_image_core::RgbaImage> {
    let mut scaled = scale_icon_to_slot(icon, slot)?;
    squircle_mask_opaque_tile(&mut scaled.argb, scaled.width, scaled.height);
    let mut pixels = Vec::with_capacity(scaled.argb.len() * 4);
    for pixel in &scaled.argb {
        pixels.push(((pixel >> 16) & 0xff) as u8);
        pixels.push(((pixel >> 8) & 0xff) as u8);
        pixels.push((pixel & 0xff) as u8);
        pixels.push(((pixel >> 24) & 0xff) as u8);
    }
    flamewm_image_core::RgbaImage::from_rgba8(scaled.width, scaled.height, pixels)
}

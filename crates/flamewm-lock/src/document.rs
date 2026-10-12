//! Lock screen document: compiled template, theme application, and the
//! cover-scaled background image.
use std::fs;
use std::path::Path;

use flamewm_image_core::scale::{ScaleFilter, scale_rgba};
use flamewm_image_core::{ImageSourceFormat, RgbaImage, decode_bytes};
use flamewm_ui_x11::{
    RuntimeImage, UiColor, UiDocumentAccess, UiDocumentView, UiTemplate, decode_document,
};

use crate::settings::{DEFAULT_ACCENT, LockSettings, parse_hex_color};
use crate::view::LockView;

const LOCK_ARTIFACT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-lock.rwr"));
const BACKGROUND_ID: &str = "lock-background";

/// Builds the compiled lock template.
pub fn template() -> Result<UiTemplate, String> {
    Ok(UiTemplate::new(decode_document(LOCK_ARTIFACT)?))
}

/// Headless check that the compiled document decodes and projects; no X.
pub fn self_test() -> Result<(), String> {
    template()?;
    let settings = LockSettings::default();
    let view = LockView::project(
        &settings,
        None,
        "self-test",
        &crate::secret::PasswordBuffer::default(),
        crate::state::Status::Idle,
    );
    if view.clock != "--:--" {
        return Err("lock view projection changed unexpectedly".to_string());
    }
    Ok(())
}

/// Reads and cover-scales the configured background image to the screen.
/// Any failure returns `None`, so the lock falls back to the background colour.
pub fn background_for(settings: &LockSettings, width: u32, height: u32) -> Option<RuntimeImage> {
    let path = settings.background_image.as_deref()?;
    let format = ImageSourceFormat::from_extension(Path::new(path))?;
    let bytes = fs::read(path).ok()?;
    let natural = decode_bytes(&bytes, format).ok()?;
    let cover = cover_rgba(&natural, width, height)?;
    Some(RuntimeImage {
        source: path.display().to_string(),
        width: cover.width,
        height: cover.height,
        pixels: cover.pixels,
    })
}

/// Scales `image` to fully cover `width` x `height` and crops the centre.
pub fn cover_rgba(image: &RgbaImage, width: u32, height: u32) -> Option<RgbaImage> {
    if image.width == 0 || image.height == 0 || width == 0 || height == 0 {
        return None;
    }
    let scale_x = f64::from(width) / f64::from(image.width);
    let scale_y = f64::from(height) / f64::from(image.height);
    let scale = scale_x.max(scale_y);
    let scaled_w = ((f64::from(image.width) * scale).ceil() as u32).max(width);
    let scaled_h = ((f64::from(image.height) * scale).ceil() as u32).max(height);
    let scaled = scale_rgba(image, scaled_w, scaled_h, ScaleFilter::Bilinear)?;
    let left = (scaled_w - width) / 2;
    let top = (scaled_h - height) / 2;
    let row_bytes = width as usize * 4;
    let mut pixels = Vec::with_capacity(row_bytes * height as usize);
    for row in 0..height {
        let start = ((top + row) as usize * scaled_w as usize + left as usize) * 4;
        pixels.extend_from_slice(&scaled.pixels[start..start + row_bytes]);
    }
    RgbaImage::from_rgba8(width, height, pixels)
}

/// Applies settings that do not change while the lock is shown.
pub fn apply_theme(
    view: &mut UiDocumentView<'_>,
    settings: &LockSettings,
    background: Option<RuntimeImage>,
) -> Result<(), String> {
    let accent = ui_color(parse_hex_color(&settings.accent).unwrap_or_else(|| {
        parse_hex_color(DEFAULT_ACCENT).unwrap_or([0x3d, 0xae, 0xe9])
    }));
    view.background("lock-root", ui_color(settings.background_color))?;
    view.background("lock-avatar", accent)?;
    view.border("lock-field", accent)?;
    view.font_family(settings.font_family.clone())?;
    view.visible("lock-date", settings.show_date)?;
    view.visible("lock-username", settings.show_username)?;
    match background {
        Some(image) => view.image_rgba8(BACKGROUND_ID, image)?,
        None => view.visible(BACKGROUND_ID, false)?,
    }
    Ok(())
}

/// Writes the per-second content: clock, date, avatar, name, mask and status.
pub fn apply_view(view: &mut UiDocumentView<'_>, lock: &LockView) -> Result<(), String> {
    view.text("lock-clock", lock.clock.clone())?;
    view.text("lock-date", lock.date.clone())?;
    view.visible("lock-date", lock.show_date)?;
    view.text("lock-avatar-initial", lock.initial.clone())?;
    view.text("lock-username", lock.username.clone())?;
    view.visible("lock-username", lock.show_username)?;
    view.text("lock-dots", lock.mask.clone())?;
    view.text("lock-status", lock.status.to_string())?;
    Ok(())
}

fn ui_color([r, g, b]: [u8; 3]) -> UiColor {
    UiColor { r, g, b, a: 255 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32) -> RgbaImage {
        // Left half red, right half blue, so cropping is observable.
        let mut pixels = Vec::new();
        for _ in 0..height {
            for x in 0..width {
                let color = if x < width / 2 { [255, 0, 0, 255] } else { [0, 0, 255, 255] };
                pixels.extend_from_slice(&color);
            }
        }
        RgbaImage::from_rgba8(width, height, pixels).expect("valid image")
    }

    #[test]
    fn cover_scales_and_crops_to_exact_size() {
        let source = solid(4, 2);
        let cover = cover_rgba(&source, 3, 3).expect("cover");
        assert_eq!((cover.width, cover.height), (3, 3));
        assert_eq!(cover.pixels.len(), 3 * 3 * 4);
        assert!(cover.is_fully_opaque());
    }

    #[test]
    fn cover_keeps_aspect_by_cropping_not_stretching() {
        // A 2x1 image onto a 1x1 target: the wide image crops its sides.
        let source = solid(2, 1);
        let cover = cover_rgba(&source, 1, 1).expect("cover");
        assert_eq!(cover.pixels.len(), 4);
    }

    #[test]
    fn cover_rejects_empty_dimensions() {
        let source = solid(2, 2);
        assert!(cover_rgba(&source, 0, 4).is_none());
    }

    /// Every node the per-second update writes text into must have a text child:
    /// the compiler gives an empty element no text node, and `update` then fails.
    #[test]
    fn every_text_target_has_a_text_child() {
        const HTML: &str = include_str!("../../../ui/lock/lock.html");
        for id in [
            "lock-clock",
            "lock-date",
            "lock-avatar-initial",
            "lock-username",
            "lock-dots",
            "lock-status",
        ] {
            let marker = format!("id=\"{id}\"");
            let start = HTML.find(&marker).unwrap_or_else(|| panic!("missing #{id}"));
            let after_tag = start + HTML[start..].find('>').expect("tag end") + 1;
            let text = HTML[after_tag..].split('<').next().unwrap_or_default();
            assert!(!text.is_empty(), "#{id} has no text child in lock.html");
        }
    }

    #[test]
    fn self_test_document_decodes() {
        self_test().expect("compiled lock document decodes");
    }
}

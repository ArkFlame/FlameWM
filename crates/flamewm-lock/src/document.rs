//! Lock screen document: compiled template, layout-driven geometry, theme,
//! the per-output background, and the icon and avatar pixels.
use std::borrow::Cow;
use std::fs;
use std::path::Path;

use flamewm_image_core::scale::{ScaleFilter, scale_rgba};
use flamewm_image_core::{ImageSourceFormat, RgbaImage, SvgColorScheme, decode_bytes, svg};
use flamewm_ui_x11::{
    RuntimeImage, UiColor, UiDocumentAccess, UiTemplate, decode_document,
};

use crate::background;
use crate::layout::{self, LockLayout, Placed, RefRect};
use crate::power::{POWER_ORDER, PowerView};
use crate::settings::{DEFAULT_BACKGROUND, LockBackgroundMode, LockSettings};
use crate::view::LockView;

const LOCK_ARTIFACT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-lock.rwr"));
const BACKGROUND_ID: &str = "lock-bg";
/// Lock background used when `lockBackgroundImage` is empty (Unsplash License, see LICENSES/).
const BUILTIN_BACKGROUND: &[u8] = include_bytes!("../../../assets/wallpapers/fitz-roy-laguna.webp");
const BUILTIN_BACKGROUND_SOURCE: &str = "builtin:fitz-roy-laguna.webp";
const POWER_ICONS: [&[u8]; 4] = [
    include_bytes!("../../../assets/web/breeze/lock/system-suspend.svg"),
    include_bytes!("../../../assets/web/breeze/lock/system-reboot.svg"),
    include_bytes!("../../../assets/web/breeze/lock/system-shutdown.svg"),
    include_bytes!("../../../assets/web/breeze/lock/system-switch-user.svg"),
];
const AVATAR_OUTLINE: &[u8] = include_bytes!("../../../assets/web/lock/avatar-outline.svg");
/// Profile picture diameter as a share of the avatar ring, so the ring stays visible.
const AVATAR_PICTURE_SCALE: f64 = 0.8;
/// Liquid-glass password field: a translucent white fill and a lighter rim (iOS style).
const FIELD_GLASS: UiColor = UiColor { r: 255, g: 255, b: 255, a: 46 };
const FIELD_RIM: UiColor = UiColor { r: 255, g: 255, b: 255, a: 97 };
/// The error state keeps the glass but tints it red.
const FIELD_FAILED_GLASS: UiColor = UiColor { r: 255, g: 90, b: 90, a: 56 };
const FIELD_FAILED_RIM: UiColor = UiColor { r: 255, g: 90, b: 90, a: 140 };

/// Reference-canvas font sizes for elements the shared layout does not size.
const INITIAL_FONT: f64 = 20.0;
const DOTS_FONT: f64 = 14.0;
const CHEVRON_FONT: f64 = 16.0;
const MESSAGE_FONT: f64 = 11.0;
/// Reference-canvas inset of the masked dots from the field edge.
const FIELD_INSET: f64 = 10.0;
/// Reference-canvas gap between the password field and the status message.
const MESSAGE_GAP: f64 = 8.0;
const MESSAGE_H: f64 = 14.0;
/// Opacity kept by a disabled power icon (the 38% dim).
const DISABLED_OPACITY: f64 = 0.38;
const CONFIRM_PROMPT: &str = "Are you sure?";

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

/// Background for a `width` x `height` output: the cover-scaled background image
/// when one decodes, else the reference navy gradient (default colour) or a flat
/// fill of the configured `lockBackgroundColor`.
pub fn background_image(settings: &LockSettings, width: u32, height: u32) -> RuntimeImage {
    let mut image = build_background(settings, width, height);
    // Blur and dim once here; a failed blur keeps the sharp picture rather than no background.
    let _ = background::post_process(
        &mut image.pixels,
        width,
        height,
        settings.blur_percent,
        settings.dim_percent,
    );
    image
}

fn build_background(settings: &LockSettings, width: u32, height: u32) -> RuntimeImage {
    decoded_background(settings, width, height).unwrap_or_else(|| {
        let pixels = if settings.background_color == DEFAULT_BACKGROUND {
            background::reference_navy(width, height)
        } else {
            background::flat(width, height, settings.background_color)
        };
        RuntimeImage {
            source: "lock-background".to_string(),
            width,
            height,
            pixels,
        }
    })
}

/// Reads and cover-scales the configured background image to the screen, or the
/// built-in lock background when none is configured. Any failure returns `None`,
/// so the caller falls back to a colour.
fn decoded_background(settings: &LockSettings, width: u32, height: u32) -> Option<RuntimeImage> {
    let (source, format, bytes) = match (settings.background_mode, settings.background_image.as_deref()) {
        (LockBackgroundMode::Custom, Some(path)) => (
            path.display().to_string(),
            ImageSourceFormat::from_extension(Path::new(path))?,
            Cow::Owned(fs::read(path).ok()?),
        ),
        (LockBackgroundMode::Default, _) => (
            BUILTIN_BACKGROUND_SOURCE.to_string(),
            ImageSourceFormat::Webp,
            Cow::Borrowed(BUILTIN_BACKGROUND),
        ),
        // Solid colour, or a custom mode without a saved path: the caller paints the colour.
        _ => return None,
    };
    let natural = decode_bytes(&bytes, format).ok()?;
    let cover = cover_rgba(&natural, width, height)?;
    Some(RuntimeImage {
        source,
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

/// Applies the settings that do not change while the lock is shown: the
/// background, the reference-layout geometry and fonts, icons, labels, and theme.
pub fn apply_document<V: UiDocumentAccess>(
    view: &mut V,
    settings: &LockSettings,
    width: u32,
    height: u32,
    background: RuntimeImage,
) -> Result<(), String> {
    let layout = LockLayout::for_output(width, height);
    let s = layout.fit.s;
    view.background("lock-root", ui_color(settings.background_color))?;
    view.font_family(settings.font_family.clone())?;

    view.image_rgba8(BACKGROUND_ID, background)?;
    place(
        view,
        BACKGROUND_ID,
        Placed {
            rect: RefRect {
                x: 0.0,
                y: 0.0,
                w: f64::from(width),
                h: f64::from(height),
            },
            font_px: 0,
        },
    )?;

    place(view, "lock-clock", layout.clock)?;
    place(view, "lock-date", layout.date)?;
    place(view, "lock-username", layout.username)?;
    place(view, "lock-footer", layout.footer)?;

    let avatar = snapped(layout.avatar);
    view.image_rgba8("lock-avatar", avatar_runtime(settings, px(avatar))?)?;
    place(view, "lock-avatar", avatar)?;
    place(
        view,
        "lock-avatar-initial",
        Placed {
            rect: layout.avatar.rect,
            font_px: scaled(INITIAL_FONT, s),
        },
    )?;

    let field = layout.password.rect;
    let inset = FIELD_INSET * s;
    // The lower edge: a darker node one reference px below the glass, drawn under it.
    place(
        view,
        "lock-field-shade",
        Placed {
            rect: RefRect { y: field.y + s, ..field },
            font_px: 0,
        },
    )?;
    place(view, "lock-field", layout.password)?;
    place(
        view,
        "lock-dots",
        Placed {
            rect: RefRect {
                x: field.x + inset,
                y: field.y,
                w: field.w - 2.0 * inset,
                h: field.h,
            },
            font_px: scaled(DOTS_FONT, s),
        },
    )?;
    place(
        view,
        "lock-caret",
        Placed {
            rect: RefRect {
                x: field.x + inset,
                y: field.y + field.h * 0.25,
                w: (1.5 * s).max(1.0),
                h: field.h * 0.5,
            },
            font_px: 0,
        },
    )?;
    place(view, "lock-submit", layout.submit)?;
    // The chevron is its own node so the button keeps its hit area; its box is centred on the circle.
    place(
        view,
        "lock-submit-glyph",
        Placed {
            rect: layout::chevron_box(layout.submit.rect, f64::from(scaled(CHEVRON_FONT, s))),
            font_px: scaled(CHEVRON_FONT, s),
        },
    )?;
    let message = RefRect {
        x: field.x,
        y: field.y + field.h + MESSAGE_GAP * s,
        w: field.w,
        h: MESSAGE_H * s,
    };
    for id in ["lock-message", "lock-power-message"] {
        place(
            view,
            id,
            Placed {
                rect: message,
                font_px: scaled(MESSAGE_FONT, s),
            },
        )?;
    }

    for (index, button) in POWER_ORDER.iter().enumerate() {
        let icon = snapped(layout.power_icons[index]);
        let icon_id = format!("lock-power-{index}");
        view.image_rgba8(&icon_id, svg_image(POWER_ICONS[index], px(icon))?)?;
        place(view, &icon_id, icon)?;
        let label_id = format!("lock-power-label-{index}");
        place(view, &label_id, layout.power_labels[index])?;
        view.text(&label_id, button.label().to_string())?;
    }

    // Containers carry no text; the labelled children take the label font.
    let label_px = scaled(layout::LABEL_FONT, s);
    for (id, rect, font_px) in [
        ("lock-confirm", layout::CONFIRM, 0),
        ("lock-confirm-text", layout::CONFIRM_TEXT, label_px),
        ("lock-confirm-ok", layout::CONFIRM_OK, label_px),
        ("lock-confirm-cancel", layout::CONFIRM_CANCEL, label_px),
        ("lock-other", layout::OTHER_LIST, 0),
        ("lock-other-logout", layout::OTHER_LOGOUT, label_px),
        ("lock-other-cancel", layout::OTHER_CANCEL, label_px),
    ] {
        place(view, id, Placed { rect: layout.fit.map_rect(rect), font_px })?;
    }
    view.text("lock-confirm-text", CONFIRM_PROMPT.to_string())?;
    view.text("lock-confirm-cancel", "Cancel".to_string())?;
    view.text("lock-other-logout", "Log out".to_string())?;
    view.text("lock-other-cancel", "Cancel".to_string())?;
    Ok(())
}

/// Shows the power row state: icons (disabled ones at 38% opacity), the confirm
/// strip for Restart or Shut Down, the Other list, and any power error message.
/// The status line and the error line share one place, so only one is visible.
pub fn apply_power<V: UiDocumentAccess>(
    view: &mut V,
    width: u32,
    height: u32,
    power: &PowerView,
) -> Result<(), String> {
    let layout = LockLayout::for_output(width, height);
    for (index, enabled) in power.enabled.iter().enumerate() {
        let icon = snapped(layout.power_icons[index]);
        let mut image = svg_image(POWER_ICONS[index], px(icon))?;
        if !enabled {
            image = dimmed(image);
        }
        view.image_rgba8(&format!("lock-power-{index}"), image)?;
    }
    let confirming = power.confirm.is_some();
    for id in ["lock-confirm", "lock-confirm-text", "lock-confirm-ok", "lock-confirm-cancel"] {
        view.visible(id, confirming)?;
    }
    if let Some(action) = power.confirm {
        view.text("lock-confirm-ok", action.label().to_string())?;
    }
    for id in ["lock-other", "lock-other-logout", "lock-other-cancel"] {
        view.visible(id, power.other_open)?;
    }
    let has_message = !power.message.is_empty();
    view.visible("lock-message", !has_message)?;
    view.visible("lock-power-message", has_message)?;
    view.text("lock-power-message", power.message.clone())
}

/// Multiplies each pixel's alpha by [`DISABLED_OPACITY`]: the icon is drawn dimmed.
fn dimmed(mut image: RuntimeImage) -> RuntimeImage {
    for pixel in image.pixels.chunks_exact_mut(4) {
        pixel[3] = (f64::from(pixel[3]) * DISABLED_OPACITY).round() as u8;
    }
    image
}

/// Replaces the background picture (the reference background or a Ribbons frame).
pub fn set_background<V: UiDocumentAccess>(view: &mut V, image: RuntimeImage) -> Result<(), String> {
    view.image_rgba8(BACKGROUND_ID, image)
}

/// Every lock UI node except the root and the background. Hidden nodes are neither drawn
/// nor hit-testable, so the Ribbons screensaver shows no widget and takes no click.
const LOCK_UI_IDS: [&str; 29] = [
    "lock-avatar",
    "lock-avatar-initial",
    "lock-caret",
    "lock-clock",
    "lock-confirm",
    "lock-confirm-cancel",
    "lock-confirm-ok",
    "lock-confirm-text",
    "lock-date",
    "lock-dots",
    "lock-field",
    "lock-field-shade",
    "lock-footer",
    "lock-message",
    "lock-other",
    "lock-other-cancel",
    "lock-other-logout",
    "lock-power-0",
    "lock-power-1",
    "lock-power-2",
    "lock-power-3",
    "lock-power-label-0",
    "lock-power-label-1",
    "lock-power-label-2",
    "lock-power-label-3",
    "lock-power-message",
    "lock-submit",
    "lock-submit-glyph",
    "lock-username",
];

/// Shows or hides every lock UI node. Restoring is followed by a full [`apply_view`] and
/// [`apply_power`], which re-apply the per-state visibility of dates, power rows and dialogs.
pub fn set_lock_ui_shown<V: UiDocumentAccess>(view: &mut V, shown: bool) -> Result<(), String> {
    for id in LOCK_UI_IDS {
        view.visible(id, shown)?;
    }
    Ok(())
}

/// Writes the per-second content: clock, date, avatar, name, mask and status.
pub fn apply_view<V: UiDocumentAccess>(view: &mut V, lock: &LockView) -> Result<(), String> {
    view.text("lock-clock", lock.clock.clone())?;
    view.text("lock-date", lock.date.clone())?;
    view.visible("lock-date", lock.show_date)?;
    // The avatar shows the ring icon only; no initial letter is drawn.
    view.text("lock-avatar-initial", String::new())?;
    view.text("lock-username", lock.username.clone())?;
    view.visible("lock-username", lock.show_username)?;
    view.background("lock-field", if lock.failed { FIELD_FAILED_GLASS } else { FIELD_GLASS })?;
    view.border("lock-field", if lock.failed { FIELD_FAILED_RIM } else { FIELD_RIM })?;
    view.text("lock-dots", lock.mask.clone())?;
    view.visible("lock-caret", lock.mask.is_empty())?;
    view.text("lock-message", lock.status.to_string())?;
    Ok(())
}

/// Sets a node's absolute geometry and, for text, its font size.
fn place<V: UiDocumentAccess + ?Sized>(view: &mut V, id: &str, placed: Placed) -> Result<(), String> {
    let rect = placed.rect;
    view.position(id, rect.x as f32, rect.y as f32)?;
    view.size(id, rect.w as f32, rect.h as f32)?;
    if placed.font_px > 0 {
        view.font_size(id, placed.font_px as f32)?;
    }
    Ok(())
}

/// Rounds an image node's rectangle to whole pixels so the rendered image maps 1:1.
fn snapped(placed: Placed) -> Placed {
    let r = placed.rect;
    Placed {
        rect: RefRect {
            x: r.x.round(),
            y: r.y.round(),
            w: r.w.round(),
            h: r.h.round(),
        },
        font_px: 0,
    }
}

/// Square pixel size of a snapped image rectangle.
fn px(placed: Placed) -> u32 {
    placed.rect.w as u32
}

/// Renders an SVG icon to a square `size` x `size` image, resolving
/// `currentColor` the same way the compiled shell assets do.
fn svg_image(bytes: &[u8], size: u32) -> Result<RuntimeImage, String> {
    let image = if svg::uses_current_color_scheme(bytes) {
        svg::render_with_color_scheme(bytes, size, size, &SvgColorScheme::flame_default())?
    } else {
        svg::render(bytes, size, size)?
    };
    Ok(RuntimeImage {
        source: format!("lock-svg-{size}"),
        width: image.width,
        height: image.height,
        pixels: image.pixels,
    })
}

/// The avatar ring at `size`, with the profile picture composited inside it when one decodes.
/// Any decode failure falls back to the default ring icon.
fn avatar_runtime(settings: &LockSettings, size: u32) -> Result<RuntimeImage, String> {
    let ring = svg_image(AVATAR_OUTLINE, size)?;
    let inner = ((f64::from(size) * AVATAR_PICTURE_SCALE).round() as u32).max(1);
    let picture = settings
        .avatar_image
        .as_deref()
        .and_then(decode_file)
        .and_then(|image| circle_crop(&image, inner));
    let Some(picture) = picture else {
        return Ok(ring);
    };
    let Some(mut base) = RgbaImage::from_rgba8(ring.width, ring.height, ring.pixels.clone()) else {
        return Ok(ring);
    };
    let offset = (size - inner) / 2;
    composite_over(&mut base, &picture, offset, offset);
    Ok(RuntimeImage {
        source: format!("lock-avatar-picture-{size}"),
        width: base.width,
        height: base.height,
        pixels: base.pixels,
    })
}

fn decode_file(path: &Path) -> Option<RgbaImage> {
    let format = ImageSourceFormat::from_extension(path)?;
    let bytes = fs::read(path).ok()?;
    decode_bytes(&bytes, format).ok()
}

/// Square `diameter` crop of `image` (cover-scaled, centred) clipped to a circle. The edge
/// is anti-aliased over one pixel; everything outside the circle is transparent.
pub fn circle_crop(image: &RgbaImage, diameter: u32) -> Option<RgbaImage> {
    let square = cover_rgba(image, diameter, diameter)?;
    let (width, height) = (square.width, square.height);
    let mut pixels = square.pixels;
    let radius = f64::from(diameter) / 2.0;
    for y in 0..height {
        for x in 0..width {
            let dx = f64::from(x) + 0.5 - radius;
            let dy = f64::from(y) + 0.5 - radius;
            let coverage = (radius - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0);
            let alpha = ((y * width + x) * 4 + 3) as usize;
            pixels[alpha] = (f64::from(pixels[alpha]) * coverage).round() as u8;
        }
    }
    RgbaImage::from_rgba8(width, height, pixels)
}

/// Straight-alpha "over" of `top` onto `base` with its top-left corner at (`x0`, `y0`).
fn composite_over(base: &mut RgbaImage, top: &RgbaImage, x0: u32, y0: u32) {
    for ty in 0..top.height {
        for tx in 0..top.width {
            let (bx, by) = (x0 + tx, y0 + ty);
            if bx >= base.width || by >= base.height {
                continue;
            }
            let src = ((ty * top.width + tx) * 4) as usize;
            let dst = ((by * base.width + bx) * 4) as usize;
            let sa = f64::from(top.pixels[src + 3]) / 255.0;
            if sa == 0.0 {
                continue;
            }
            let da = f64::from(base.pixels[dst + 3]) / 255.0;
            let oa = sa + da * (1.0 - sa);
            for channel in 0..3 {
                let sc = f64::from(top.pixels[src + channel]);
                let dc = f64::from(base.pixels[dst + channel]);
                let oc = if oa > 0.0 { (sc * sa + dc * da * (1.0 - sa)) / oa } else { 0.0 };
                base.pixels[dst + channel] = oc.round() as u8;
            }
            base.pixels[dst + 3] = (oa * 255.0).round() as u8;
        }
    }
}

fn scaled(reference_px: f64, s: f64) -> u32 {
    (reference_px * s).round().max(f64::from(layout::MIN_FONT_PX)) as u32
}

fn ui_color([r, g, b]: [u8; 3]) -> UiColor {
    UiColor { r, g, b, a: 255 }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use flamewm_ui_core::style::UiLayer;
    use flamewm_ui_x11::{Overflow, UiDocument};

    use super::*;
    use crate::clock::ClockFormat;
    use crate::native_time::LocalTime;
    use crate::power::Confirmable;
    use crate::secret::PasswordBuffer;
    use crate::state::Status;

    /// Records what the document layer writes, keyed by node id.
    #[derive(Default)]
    struct Recorder {
        text: HashMap<String, String>,
        visible: HashMap<String, bool>,
        position: HashMap<String, (f32, f32)>,
        size: HashMap<String, (f32, f32)>,
        font: HashMap<String, f32>,
        images: HashMap<String, (u32, u32)>,
        fills: HashMap<String, UiColor>,
        rims: HashMap<String, UiColor>,
    }

    impl UiDocumentAccess for Recorder {
        fn text(&mut self, id: &str, text: String) -> Result<(), String> {
            self.text.insert(id.to_string(), text);
            Ok(())
        }
        fn image_rgb8(&mut self, id: &str, image: RuntimeImage) -> Result<(), String> {
            self.image_rgba8(id, image)
        }
        fn image_rgba8(&mut self, id: &str, image: RuntimeImage) -> Result<(), String> {
            self.images.insert(id.to_string(), (image.width, image.height));
            Ok(())
        }
        fn visible(&mut self, id: &str, visible: bool) -> Result<(), String> {
            self.visible.insert(id.to_string(), visible);
            Ok(())
        }
        fn toggle(&mut self, _: &str, _: &str, _: bool) -> Result<(), String> {
            Ok(())
        }
        fn position(&mut self, id: &str, left: f32, top: f32) -> Result<(), String> {
            self.position.insert(id.to_string(), (left, top));
            Ok(())
        }
        fn size(&mut self, id: &str, width: f32, height: f32) -> Result<(), String> {
            self.size.insert(id.to_string(), (width, height));
            Ok(())
        }
        fn background(&mut self, id: &str, color: UiColor) -> Result<(), String> {
            self.fills.insert(id.to_string(), color);
            Ok(())
        }
        fn border(&mut self, id: &str, color: UiColor) -> Result<(), String> {
            self.rims.insert(id.to_string(), color);
            Ok(())
        }
        fn foreground(&mut self, _: &str, _: UiColor) -> Result<(), String> {
            Ok(())
        }
        fn background_clear(&mut self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn border_clear(&mut self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn foreground_clear(&mut self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn font_size(&mut self, id: &str, size_px: f32) -> Result<(), String> {
            self.font.insert(id.to_string(), size_px);
            Ok(())
        }
        fn font_weight(&mut self, _: &str, _: u16) -> Result<(), String> {
            Ok(())
        }
        fn font_family(&mut self, _: String) -> Result<(), String> {
            Ok(())
        }
        fn overflow(
            &mut self,
            _: &str,
            _: Overflow,
            _: Overflow,
        ) -> Result<(), String> {
            Ok(())
        }
        fn layer(&mut self, _: &str, _: UiLayer) -> Result<(), String> {
            Ok(())
        }
    }

    fn background(width: u32, height: u32) -> RuntimeImage {
        RuntimeImage {
            source: "test".to_string(),
            width,
            height,
            pixels: background::flat(width, height, [0, 0, 0]),
        }
    }

    fn sample_time() -> LocalTime {
        LocalTime {
            year: 2026,
            month: 10,
            day: 12,
            hour: 13,
            minute: 7,
        }
    }

    fn project(settings: &LockSettings, password: &PasswordBuffer, status: Status) -> Recorder {
        let mut recorder = Recorder::default();
        let view = LockView::project(settings, Some(sample_time()), "alice", password, status);
        apply_view(&mut recorder, &view).expect("apply view");
        recorder
    }

    fn masked(count: usize) -> PasswordBuffer {
        let mut password = PasswordBuffer::default();
        for _ in 0..count {
            password.push('x');
        }
        password
    }

    #[test]
    fn default_projection_shows_24h_time_date_name_and_no_message() {
        let recorder = project(&LockSettings::default(), &PasswordBuffer::default(), Status::Idle);
        assert_eq!(recorder.text["lock-clock"], "13:07");
        assert_eq!(recorder.text["lock-date"], "Monday, 12 October 2026");
        assert_eq!(recorder.text["lock-username"], "alice");
        assert_eq!(recorder.text["lock-avatar-initial"], "", "avatar shows no letter");
        assert_eq!(recorder.text["lock-dots"], "");
        assert_eq!(recorder.text["lock-message"], "");
        assert!(recorder.visible["lock-date"] && recorder.visible["lock-username"]);
        assert!(recorder.visible["lock-caret"], "caret shows while the field is empty");
    }

    #[test]
    fn twelve_hour_clock_with_date_and_name_hidden() {
        let settings = LockSettings {
            clock_format: ClockFormat::H12,
            show_date: false,
            show_username: false,
            ..LockSettings::default()
        };
        let recorder = project(&settings, &PasswordBuffer::default(), Status::Idle);
        assert_eq!(recorder.text["lock-clock"], "1:07 PM");
        assert!(!recorder.visible["lock-date"]);
        assert!(!recorder.visible["lock-username"]);
    }

    #[test]
    fn failure_message_and_masked_dots() {
        let recorder = project(&LockSettings::default(), &masked(3), Status::Failed);
        assert_eq!(recorder.text["lock-message"], "Authentication failed");
        // The error state keeps the glass but tints it red.
        assert_eq!(recorder.fills["lock-field"].r, 255);
        assert!(recorder.fills["lock-field"].g < 128 && recorder.rims["lock-field"].g < 128);
        assert_eq!(recorder.text["lock-dots"], "\u{2022}\u{2022}\u{2022}");
        assert!(!recorder.visible["lock-caret"], "caret hides once dots are shown");
    }

    #[test]
    fn power_labels_come_from_the_power_buttons() {
        let mut document = Recorder::default();
        apply_document(&mut document, &LockSettings::default(), 1280, 720, background(1280, 720))
            .expect("apply document");
        assert_eq!(document.text["lock-power-label-0"], "Sleep");
        assert_eq!(document.text["lock-power-label-1"], "Restart");
        assert_eq!(document.text["lock-power-label-2"], "Shut Down");
        assert_eq!(document.text["lock-power-label-3"], "Other\u{2026}");
    }

    #[test]
    fn document_geometry_equals_the_layout_for_the_output() {
        for (width, height) in [(1280, 720), (701, 438), (1920, 1080)] {
            let mut document = Recorder::default();
            apply_document(
                &mut document,
                &LockSettings::default(),
                width,
                height,
                background(width, height),
            )
            .expect("apply document");
            let layout = LockLayout::for_output(width, height);
            let expect = |placed: Placed| (placed.rect.x as f32, placed.rect.y as f32);
            assert_eq!(document.position["lock-clock"], expect(layout.clock));
            assert_eq!(document.position["lock-username"], expect(layout.username));
            assert_eq!(document.font["lock-clock"], layout.clock.font_px as f32);
            assert_eq!(document.font["lock-power-label-3"], layout.power_labels[3].font_px as f32);
            let icon = snapped(layout.power_icons[0]);
            assert_eq!(document.size["lock-power-0"], (icon.rect.w as f32, icon.rect.h as f32));
            assert_eq!(document.images["lock-bg"], (width, height));
        }
    }

    /// Every text target must carry a text child in the compiled document, or
    /// the per-second update fails.
    #[test]
    fn every_text_target_has_a_text_child() {
        const HTML: &str = include_str!("../../../ui/lock/lock.html");
        for id in [
            "lock-clock",
            "lock-date",
            "lock-avatar-initial",
            "lock-username",
            "lock-dots",
            "lock-message",
            "lock-submit",
            "lock-submit-glyph",
            "lock-footer",
            "lock-power-label-0",
            "lock-power-label-1",
            "lock-power-label-2",
            "lock-power-label-3",
            "lock-power-message",
            "lock-confirm-text",
            "lock-confirm-ok",
            "lock-confirm-cancel",
            "lock-other-logout",
            "lock-other-cancel",
        ] {
            let marker = format!("id=\"{id}\"");
            let start = HTML.find(&marker).unwrap_or_else(|| panic!("missing #{id}"));
            let after_tag = start + HTML[start..].find('>').expect("tag end") + 1;
            let text = HTML[after_tag..].split('<').next().unwrap_or_default();
            assert!(!text.is_empty(), "#{id} has no text child in lock.html");
        }
    }

    /// The compiled CSS must place the nodes at the layout rectangles, not at
    /// their stacking position: this checks the real layout engine.
    #[test]
    fn compiled_nodes_land_on_layout_rects() {
        let mut document: UiDocument = decode_document(LOCK_ARTIFACT).expect("decode lock UI");
        let (width, height) = (1280, 720);
        apply_document(
            &mut document,
            &LockSettings::default(),
            width,
            height,
            background(width, height),
        )
        .expect("apply document");
        let layout = LockLayout::for_output(width, height);
        for (id, placed) in [
            ("lock-clock", layout.clock),
            ("lock-username", layout.username),
            ("lock-field", layout.password),
            ("lock-submit", layout.submit),
            ("lock-power-label-0", layout.power_labels[0]),
            ("lock-footer", layout.footer),
        ] {
            let (x, y, w, h) = document
                .layout_box_by_id(id, (width as f32, height as f32))
                .unwrap_or_else(|| panic!("#{id} has no layout box"));
            let r = placed.rect;
            assert!(
                (x - r.x as f32).abs() < 0.5
                    && (y - r.y as f32).abs() < 0.5
                    && (w - r.w as f32).abs() < 0.5
                    && (h - r.h as f32).abs() < 0.5,
                "#{id} at ({x},{y},{w},{h}), layout wants {r:?}"
            );
        }
    }

    #[test]
    fn empty_image_uses_builtin_background_and_undecodable_falls_back_to_colour() {
        let builtin = background_image(&LockSettings::default(), 64, 36);
        assert_eq!(builtin.source, BUILTIN_BACKGROUND_SOURCE);
        assert_eq!((builtin.width, builtin.height), (64, 36));
        assert_ne!(builtin.pixels, background::reference_navy(64, 36));

        let missing = LockSettings {
            background_image: Some("/nonexistent/flamewm-lock-background.png".into()),
            background_mode: LockBackgroundMode::Custom,
            blur_percent: 0,
            dim_percent: 0,
            ..LockSettings::default()
        };
        let navy = background_image(&missing, 64, 36);
        assert_eq!(navy.pixels, background::reference_navy(64, 36));

        let custom = LockSettings {
            background_color: [1, 2, 3],
            background_image: Some("/nonexistent/flamewm-lock-background.png".into()),
            background_mode: LockBackgroundMode::Custom,
            blur_percent: 0,
            dim_percent: 0,
            ..LockSettings::default()
        };
        let flat = background_image(&custom, 4, 4);
        assert_eq!(&flat.pixels[..4], &[1, 2, 3, 255]);
    }

    #[test]
    fn solid_mode_paints_the_colour_and_default_mode_keeps_the_builtin_picture() {
        let builtin = background_image(&LockSettings::default(), 64, 36);
        let solid = LockSettings {
            background_mode: LockBackgroundMode::Solid,
            background_image: Some("/nonexistent/flamewm-lock-background.png".into()),
            blur_percent: 0,
            dim_percent: 0,
            ..LockSettings::default()
        };
        let cleared = background_image(&solid, 64, 36);
        assert_eq!(cleared.pixels, background::reference_navy(64, 36));
        assert_ne!(builtin.pixels, cleared.pixels, "Clear must not look like Default");
    }

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

    /// Every `lock-*` node in the template must be hidden by the saver, or a widget stays
    /// visible and clickable during Ribbons. Only the root and background stay shown.
    #[test]
    fn saver_hides_every_lock_widget_except_root_and_background() {
        let html = include_str!("../../../ui/lock/lock.html");
        let mut ids: Vec<&str> = html
            .split("id=\"")
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .filter(|id| id.starts_with("lock-") && *id != "lock-root" && *id != "lock-bg")
            .collect();
        ids.sort_unstable();
        ids.dedup();
        let mut hidden = LOCK_UI_IDS.to_vec();
        hidden.sort_unstable();
        assert_eq!(hidden, ids);
    }

    /// The action hit-test skips nodes that are not effectively visible, so hidden power
    /// and dialog actions cannot be clicked at their former centres.
    #[test]
    fn hidden_lock_ui_clears_every_power_and_dialog_action() {
        let mut recorder = Recorder::default();
        set_lock_ui_shown(&mut recorder, false).expect("hide lock UI");
        for id in [
            "lock-power-0",
            "lock-power-1",
            "lock-power-2",
            "lock-power-3",
            "lock-confirm-ok",
            "lock-confirm-cancel",
            "lock-other-logout",
            "lock-other-cancel",
        ] {
            assert_eq!(recorder.visible.get(id), Some(&false), "{id} must be hidden");
        }
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

    #[test]
    fn disabled_icons_keep_38_percent_of_their_alpha() {
        let image = RuntimeImage {
            source: "icon".to_string(),
            width: 2,
            height: 1,
            pixels: vec![255, 255, 255, 255, 9, 9, 9, 0],
        };
        let dim = dimmed(image);
        assert_eq!(dim.pixels, vec![255, 255, 255, 97, 9, 9, 9, 0]);
    }

    #[test]
    fn power_row_shows_only_the_open_strip_and_the_error_line() {
        let mut document = Recorder::default();
        let idle = PowerView {
            enabled: [true, false, true, true],
            confirm: None,
            other_open: false,
            message: String::new(),
        };
        apply_power(&mut document, 1280, 720, &idle).expect("apply power");
        assert!(!document.visible["lock-confirm"] && !document.visible["lock-other"]);
        assert!(document.visible["lock-message"] && !document.visible["lock-power-message"]);

        let restart = PowerView {
            confirm: Some(Confirmable::Restart),
            message: "control call failed".to_string(),
            ..idle
        };
        apply_power(&mut document, 1280, 720, &restart).expect("apply power");
        assert!(document.visible["lock-confirm"] && document.visible["lock-confirm-cancel"]);
        assert_eq!(document.text["lock-confirm-ok"], "Restart");
        assert!(!document.visible["lock-message"] && document.visible["lock-power-message"]);
        assert_eq!(document.text["lock-power-message"], "control call failed");
    }

    #[test]
    fn other_list_opens_with_log_out_and_cancel() {
        let mut document = Recorder::default();
        let open = PowerView {
            enabled: [true, true, true, true],
            confirm: None,
            other_open: true,
            message: String::new(),
        };
        apply_document(&mut document, &LockSettings::default(), 1280, 720, background(1280, 720))
            .expect("apply document");
        apply_power(&mut document, 1280, 720, &open).expect("apply power");
        assert!(document.visible["lock-other"] && document.visible["lock-other-logout"]);
        assert!(document.visible["lock-other-cancel"]);
        assert_eq!(document.text["lock-other-logout"], "Log out");
    }

    fn picture(width: u32, height: u32) -> RgbaImage {
        RgbaImage::from_rgba8(width, height, [200, 30, 30, 255].repeat((width * height) as usize))
            .expect("valid image")
    }

    #[test]
    fn circle_crop_is_opaque_in_the_centre_and_transparent_in_the_corners() {
        let circle = circle_crop(&picture(40, 20), 20).expect("crop");
        assert_eq!((circle.width, circle.height), (20, 20));
        let alpha = |x: usize, y: usize| circle.pixels[(y * 20 + x) * 4 + 3];
        assert_eq!(alpha(10, 10), 255);
        assert_eq!(alpha(0, 0), 0);
        assert_eq!(alpha(19, 19), 0);
        assert!(circle.pixels.chunks(4).any(|p| p[3] > 0 && p[3] < 255), "edge is anti-aliased");
    }

    #[test]
    fn composite_puts_an_opaque_picture_over_the_ring() {
        let mut base = RgbaImage::from_rgba8(4, 4, [0, 0, 0, 0].repeat(16)).expect("base");
        composite_over(&mut base, &picture(2, 2), 1, 1);
        assert_eq!(&base.pixels[(1 * 4 + 1) * 4..(1 * 4 + 1) * 4 + 4], &[200, 30, 30, 255]);
        assert_eq!(base.pixels[3], 0, "outside the picture stays transparent");
    }

    #[test]
    fn default_avatar_has_no_picture_when_path_is_unset_or_undecodable() {
        let default = avatar_runtime(&LockSettings::default(), 68).expect("default");
        let broken = LockSettings {
            avatar_image: Some("/nonexistent/flamewm-avatar.png".into()),
            ..LockSettings::default()
        };
        let fallback = avatar_runtime(&broken, 68).expect("fallback");
        assert_eq!(default.pixels, fallback.pixels);
        assert_eq!(default.source, format!("lock-svg-68"));
    }

    #[test]
    fn self_test_document_decodes() {
        self_test().expect("compiled lock document decodes");
    }
}

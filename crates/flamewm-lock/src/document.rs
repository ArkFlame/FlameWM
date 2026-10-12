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
use crate::power::PowerButton;
use crate::settings::{DEFAULT_ACCENT, DEFAULT_BACKGROUND, LockSettings, parse_hex_color};
use crate::view::LockView;

const LOCK_ARTIFACT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-lock.rwr"));
const BACKGROUND_ID: &str = "lock-bg";
/// Lock background used when `lockBackgroundImage` is empty (Unsplash License, see LICENSES/).
const BUILTIN_BACKGROUND: &[u8] = include_bytes!("../../../assets/wallpapers/fitz-roy-laguna.webp");
const BUILTIN_BACKGROUND_SOURCE: &str = "builtin:fitz-roy-laguna.webp";
const POWER_BUTTONS: [PowerButton; 4] = [
    PowerButton::Sleep,
    PowerButton::Restart,
    PowerButton::Shutdown,
    PowerButton::Other,
];
const POWER_ICONS: [&[u8]; 4] = [
    include_bytes!("../../../assets/web/breeze/lock/system-suspend.svg"),
    include_bytes!("../../../assets/web/breeze/lock/system-reboot.svg"),
    include_bytes!("../../../assets/web/breeze/lock/system-shutdown.svg"),
    include_bytes!("../../../assets/web/breeze/lock/system-switch-user.svg"),
];
const AVATAR_OUTLINE: &[u8] = include_bytes!("../../../assets/web/lock/avatar-outline.svg");

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
    let (source, format, bytes) = match settings.background_image.as_deref() {
        Some(path) => (
            path.display().to_string(),
            ImageSourceFormat::from_extension(Path::new(path))?,
            Cow::Owned(fs::read(path).ok()?),
        ),
        None => (
            BUILTIN_BACKGROUND_SOURCE.to_string(),
            ImageSourceFormat::Webp,
            Cow::Borrowed(BUILTIN_BACKGROUND),
        ),
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
    let accent = ui_color(parse_hex_color(&settings.accent).unwrap_or_else(|| {
        parse_hex_color(DEFAULT_ACCENT).unwrap_or([0x3d, 0xae, 0xe9])
    }));
    view.background("lock-root", ui_color(settings.background_color))?;
    view.font_family(settings.font_family.clone())?;
    view.border("lock-field", accent)?;

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
    view.image_rgba8("lock-avatar", svg_image(AVATAR_OUTLINE, px(avatar))?)?;
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
    place(
        view,
        "lock-submit",
        Placed {
            rect: layout.submit.rect,
            font_px: scaled(CHEVRON_FONT, s),
        },
    )?;
    place(
        view,
        "lock-message",
        Placed {
            rect: RefRect {
                x: field.x,
                y: field.y + field.h + MESSAGE_GAP * s,
                w: field.w,
                h: MESSAGE_H * s,
            },
            font_px: scaled(MESSAGE_FONT, s),
        },
    )?;

    for (index, button) in POWER_BUTTONS.iter().enumerate() {
        let icon = snapped(layout.power_icons[index]);
        let icon_id = format!("lock-power-{index}");
        view.image_rgba8(&icon_id, svg_image(POWER_ICONS[index], px(icon))?)?;
        place(view, &icon_id, icon)?;
        let label_id = format!("lock-power-label-{index}");
        place(view, &label_id, layout.power_labels[index])?;
        view.text(&label_id, button.label().to_string())?;
    }
    Ok(())
}

/// Writes the per-second content: clock, date, avatar, name, mask and status.
pub fn apply_view<V: UiDocumentAccess>(view: &mut V, lock: &LockView) -> Result<(), String> {
    view.text("lock-clock", lock.clock.clone())?;
    view.text("lock-date", lock.date.clone())?;
    view.visible("lock-date", lock.show_date)?;
    view.text("lock-avatar-initial", lock.initial.clone())?;
    view.text("lock-username", lock.username.clone())?;
    view.visible("lock-username", lock.show_username)?;
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
        fn background(&mut self, _: &str, _: UiColor) -> Result<(), String> {
            Ok(())
        }
        fn border(&mut self, _: &str, _: UiColor) -> Result<(), String> {
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
        assert_eq!(recorder.text["lock-avatar-initial"], "A");
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
            "lock-footer",
            "lock-power-label-0",
            "lock-power-label-1",
            "lock-power-label-2",
            "lock-power-label-3",
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
            ..LockSettings::default()
        };
        let navy = background_image(&missing, 64, 36);
        assert_eq!(navy.pixels, background::reference_navy(64, 36));

        let custom = LockSettings {
            background_color: [1, 2, 3],
            background_image: Some("/nonexistent/flamewm-lock-background.png".into()),
            ..LockSettings::default()
        };
        let flat = background_image(&custom, 4, 4);
        assert_eq!(&flat.pixels[..4], &[1, 2, 3, 255]);
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
    fn self_test_document_decodes() {
        self_test().expect("compiled lock document decodes");
    }
}

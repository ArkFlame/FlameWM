//! Pure pseudo-transparency helpers for translucent shell surfaces (the taskbar).
//!
//! The snapshot is the root pixels under the bar, captured once before the bar is first mapped
//! (so the bar's own translucent pixels are never captured). It is static: it does not follow
//! wallpaper or window changes. Blur is applied from the stored snapshot, never recaptured.

use std::path::{Path, PathBuf};

use flamewm_api::background::WallpaperFit;
use flamewm_image_core::wallpaper::{self, DEFAULT_WALLPAPER_PATH};
use flamewm_image_core::RgbaImage;
pub use flamewm_ui_x11::backdrop::{blur_radius_px, MAX_BLUR_RADIUS_PX};

/// Fitted wallpaper pixels under `rect` (root-space device px): the wallpaper placed on the root
/// canvas exactly as the desktop owner does, computed for `rect` only. Pure: reads no root pixels,
/// so a bar's own paint or a not-yet-painted desktop cannot leak into the snapshot.
#[must_use]
pub fn wallpaper_backdrop(
    natural: &RgbaImage,
    root: (u32, u32),
    fit: WallpaperFit,
    rect: (i32, i32, u32, u32),
) -> Option<RgbaImage> {
    wallpaper::compose_region(natural, root.0, root.1, fit, rect)
}

/// Fitted wallpaper pixels under `rect`, decoded from the `wallpaper` setting (empty means the
/// packaged default). `None` when the wallpaper is unavailable, so the caller falls back to a
/// root capture.
#[must_use]
pub fn wallpaper_source(
    wallpaper_setting: &str,
    fit: WallpaperFit,
    root: (u32, u32),
    rect: (i32, i32, u32, u32),
    packaged_root: &Path,
) -> Option<RgbaImage> {
    let path = if wallpaper_setting.trim().is_empty() {
        packaged_root.join(DEFAULT_WALLPAPER_PATH)
    } else {
        PathBuf::from(wallpaper_setting)
    };
    match wallpaper::decode_file(&path) {
        Ok(natural) => wallpaper_backdrop(&natural, root, fit, rect),
        Err(error) => {
            backdrop_trace(format_args!(
                "wallpaper source unavailable, root capture fallback: {error}"
            ));
            None
        }
    }
}

/// Opaque ARGB32 words (0xFFRRGGBB) for the snapshot, blurred per [`blur_radius_px`].
#[must_use]
pub fn blurred_argb(snapshot: &RgbaImage, opacity_percent: u8, blur_percent: u8) -> Vec<u32> {
    flamewm_ui_x11::backdrop::blurred_argb_words(
        snapshot.width,
        snapshot.height,
        snapshot.pixels.clone(),
        opacity_percent,
        blur_percent,
    )
}

/// Debug trace for taskbar pseudo-transparency, enabled by `FLAMEWM_DEBUG_BACKDROP=1` (stderr).
pub fn backdrop_trace(message: std::fmt::Arguments<'_>) {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ENABLED.get_or_init(|| std::env::var_os("FLAMEWM_DEBUG_BACKDROP").is_some_and(|v| v == "1"))
    {
        eprintln!("debug backdrop {message}");
    }
}

/// Captures attempted for one snapshot key before the bar stays solid.
pub const MAX_CAPTURE_ATTEMPTS: u8 = 64;

/// What to do with a capture: install it, retry on the next refresh, or give up for this key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureStep {
    Use,
    Retry,
    GiveUp,
}

/// Decision after capture number `attempts_made` (1-based). A blank or uniform root is retried
/// until `MAX_CAPTURE_ATTEMPTS`; the caller retries on its next event-driven refresh, never on a timer.
#[must_use]
pub fn capture_step(ready: bool, attempts_made: u8) -> CaptureStep {
    match (ready, attempts_made >= MAX_CAPTURE_ATTEMPTS) {
        (true, _) => CaptureStep::Use,
        (false, false) => CaptureStep::Retry,
        (false, true) => CaptureStep::GiveUp,
    }
}

/// Inputs that change what the root looks like under the bar: wallpaper, its fit, and the bar rect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureKey {
    pub wallpaper: String,
    pub fit: String,
    pub rect: (i32, i32, u32, u32),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_capture_is_used_at_any_attempt() {
        assert_eq!(capture_step(true, 1), CaptureStep::Use);
        assert_eq!(capture_step(true, MAX_CAPTURE_ATTEMPTS), CaptureStep::Use);
    }

    #[test]
    fn blank_capture_retries_until_the_bound_then_gives_up() {
        assert_eq!(capture_step(false, 1), CaptureStep::Retry);
        assert_eq!(
            capture_step(false, MAX_CAPTURE_ATTEMPTS - 1),
            CaptureStep::Retry
        );
        assert_eq!(
            capture_step(false, MAX_CAPTURE_ATTEMPTS),
            CaptureStep::GiveUp
        );
    }

    fn solid(width: u32, height: u32, rgba: [u8; 4]) -> RgbaImage {
        RgbaImage {
            width,
            height,
            pixels: rgba.repeat((width * height) as usize),
        }
    }

    #[test]
    fn full_opacity_never_blurs() {
        assert_eq!(blur_radius_px(100, 80), 0.0);
        assert_eq!(blur_radius_px(80, 0), 0.0);
        assert!((blur_radius_px(80, 100) - 24.0).abs() < f32::EPSILON);
    }

    #[test]
    fn snapshot_words_are_opaque_argb_in_rgb_order() {
        let words = blurred_argb(&solid(2, 1, [0x11, 0x22, 0x33, 0x00]), 100, 80);
        assert_eq!(words, vec![0xff11_2233, 0xff11_2233]);
    }

    #[test]
    fn blur_of_uniform_snapshot_keeps_colour() {
        let words = blurred_argb(&solid(8, 8, [0x40, 0x80, 0xc0, 0xff]), 40, 100);
        assert!(words.iter().all(|&w| w == 0xff40_80c0));
    }
}

#[cfg(test)]
mod wallpaper_backdrop_tests {
    use super::*;
    use flamewm_api::background::WallpaperFit;

    /// Root canvas the desktop composes the wallpaper onto (device px).
    const ROOT: (u32, u32) = (160, 120);
    const FITS: [WallpaperFit; 5] = [
        WallpaperFit::Fill,
        WallpaperFit::Fit,
        WallpaperFit::Stretch,
        WallpaperFit::Center,
        WallpaperFit::Tile,
    ];

    fn natural() -> RgbaImage {
        let (w, h) = (7u32, 5u32);
        let mut pixels = Vec::new();
        for y in 0..h {
            for x in 0..w {
                pixels.extend_from_slice(&[
                    (x * 30 + 5) as u8,
                    (y * 40 + 9) as u8,
                    ((x * y) * 7) as u8,
                    255,
                ]);
            }
        }
        RgbaImage {
            width: w,
            height: h,
            pixels,
        }
    }

    /// Bottom panel rect for a bar of `height` device px.
    fn bar_rect(height: u32) -> (i32, i32, u32, u32) {
        (0, (ROOT.1 - height) as i32, ROOT.0, height)
    }

    /// The fitted root wallpaper cropped to `rect`, computed independently of the production path.
    fn expected_crop(fit: WallpaperFit, rect: (i32, i32, u32, u32)) -> RgbaImage {
        let root = flamewm_image_core::wallpaper::compose(&natural(), ROOT.0, ROOT.1, fit)
            .expect("root compose");
        let (x0, y0, w, h) = (
            rect.0 as usize,
            rect.1 as usize,
            rect.2 as usize,
            rect.3 as usize,
        );
        let mut pixels = Vec::with_capacity(w * h * 4);
        for y in y0..y0 + h {
            let start = (y * ROOT.0 as usize + x0) * 4;
            pixels.extend_from_slice(&root.pixels[start..start + w * 4]);
        }
        RgbaImage {
            width: w as u32,
            height: h as u32,
            pixels,
        }
    }

    #[test]
    fn backdrop_equals_fitted_wallpaper_crop_across_bar_resizes() {
        for fit in FITS {
            let mut first_72 = None;
            for height in [72u32, 44, 52, 34, 72] {
                let rect = bar_rect(height);
                let got =
                    wallpaper_backdrop(&natural(), ROOT, fit, rect).expect("wallpaper backdrop");
                assert_eq!(
                    (got.width, got.height),
                    (rect.2, rect.3),
                    "{fit:?} {height}"
                );
                assert_eq!(got, expected_crop(fit, rect), "{fit:?} {height}");
                if height == 72 {
                    match &first_72 {
                        None => first_72 = Some(got),
                        Some(first) => assert_eq!(&got, first, "{fit:?} revisit 72"),
                    }
                }
            }
        }
    }

    #[test]
    fn backdrop_is_a_crop_of_the_root_fit_not_a_rescale_to_the_bar() {
        let rect = bar_rect(44);
        let cropped = wallpaper_backdrop(&natural(), ROOT, WallpaperFit::Fill, rect)
            .expect("wallpaper backdrop");
        let rescaled =
            flamewm_image_core::wallpaper::compose(&natural(), rect.2, rect.3, WallpaperFit::Fill)
                .expect("bar compose");
        assert_ne!(cropped.pixels, rescaled.pixels);
    }

    #[test]
    fn popup_region_matches_full_root_crop_for_fit_scale_and_output_offsets() {
        // Device dimensions follow the panel's scale conversion, including its rounding rule.
        for scale in [100u16, 150] {
            let width = flamewm_platform::panel::logical_to_physical(41, scale) as u32;
            let height = flamewm_platform::panel::logical_to_physical(29, scale) as u32;
            // Top/left, right/bottom edge alignment, and a normalized secondary-output offset.
            let origins = [
                (0u32, 0u32),
                (ROOT.0 - width, 0),
                (0, ROOT.1 - height),
                (ROOT.0 - width, ROOT.1 - height),
                (53, 31),
            ];
            for (x, y) in origins {
                let rect = (x as i32, y as i32, width, height);
                for fit in FITS {
                    let direct = wallpaper_backdrop(&natural(), ROOT, fit, rect)
                        .expect("direct popup-region composition");
                    assert_eq!(
                        direct,
                        expected_crop(fit, rect),
                        "scale={scale} {rect:?} {fit:?}"
                    );
                    assert_eq!(
                        direct.pixels.len(),
                        rect.2 as usize * rect.3 as usize * 4,
                        "region allocation only: scale={scale} {rect:?} {fit:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn rect_outside_the_root_has_no_wallpaper_backdrop() {
        assert!(
            wallpaper_backdrop(&natural(), ROOT, WallpaperFit::Fill, (0, 100, 160, 44)).is_none()
        );
        assert!(
            wallpaper_backdrop(&natural(), ROOT, WallpaperFit::Fill, (-1, 0, 10, 10)).is_none()
        );
    }
}

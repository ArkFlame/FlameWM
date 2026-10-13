//! Pure pseudo-transparency helpers for translucent shell surfaces (the taskbar).
//!
//! The snapshot is the root pixels under the bar, captured once before the bar is first mapped
//! (so the bar's own translucent pixels are never captured). It is static: it does not follow
//! wallpaper or window changes. Blur is applied from the stored snapshot, never recaptured.

use flamewm_image_core::RgbaImage;
pub use flamewm_render_x11::backdrop::{MAX_BLUR_RADIUS_PX, blur_radius_px};

/// Opaque ARGB32 words (0xFFRRGGBB) for the snapshot, blurred per [`blur_radius_px`].
#[must_use]
pub fn blurred_argb(snapshot: &RgbaImage, opacity_percent: u8, blur_percent: u8) -> Vec<u32> {
    flamewm_render_x11::backdrop::blurred_argb_words(
        snapshot.width,
        snapshot.height,
        &snapshot.pixels,
        opacity_percent,
        blur_percent,
    )
}

/// Debug trace for taskbar pseudo-transparency, enabled by `FLAMEWM_DEBUG_BACKDROP=1` (stderr).
pub fn backdrop_trace(message: std::fmt::Arguments<'_>) {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ENABLED.get_or_init(|| std::env::var_os("FLAMEWM_DEBUG_BACKDROP").is_some_and(|v| v == "1")) {
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
        assert_eq!(capture_step(false, MAX_CAPTURE_ATTEMPTS - 1), CaptureStep::Retry);
        assert_eq!(capture_step(false, MAX_CAPTURE_ATTEMPTS), CaptureStep::GiveUp);
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

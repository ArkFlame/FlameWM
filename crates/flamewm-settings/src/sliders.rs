//! Split from `lib.rs` (`sliders`); see crate docs.

use super::*;

pub(crate) const ANIMATION_SPEED_SLIDER: &str = "slider.animation-speed";

/// One glass slider: its drag action, the integer-valued percent or px key it writes, its
/// inclusive range, and the HTML slug of its value text, fill and thumb nodes.
pub(crate) struct GlassSlider {
    pub(crate) action: &'static str,
    pub(crate) key: &'static str,
    pub(crate) minimum: i64,
    pub(crate) maximum: i64,
    pub(crate) slug: &'static str,
}

pub(crate) static GLASS_SLIDERS: [GlassSlider; 3] = [
    GlassSlider {
        action: "slider.glass-opacity",
        key: "glassOpacityPercent",
        minimum: 4,
        maximum: 42,
        slug: "glass-opacity",
    },
    GlassSlider {
        action: "slider.glass-blur",
        key: "glassBlurPx",
        minimum: 0,
        maximum: 54,
        slug: "glass-blur",
    },
    GlassSlider {
        action: "slider.taskbar-opacity",
        key: "taskbarOpacityPercent",
        minimum: 0,
        maximum: 75,
        slug: "taskbar-opacity",
    },
];

/// Must match the `.slider-track` width in ui/settings.css.
pub(crate) const GLASS_SLIDER_TRACK_WIDTH: f64 = 180.0;
pub(crate) const GLASS_SLIDER_THUMB: f64 = 15.0;
/// Track height and thumb top from `.slider-track` / `.slider-thumb` in ui/settings.css.
pub(crate) const GLASS_SLIDER_TRACK_HEIGHT: f64 = 6.0;
pub(crate) const GLASS_SLIDER_THUMB_TOP: f64 = 0.0;
pub(crate) const GLASS_SLIDER_TRAVEL: f64 = GLASS_SLIDER_TRACK_WIDTH - GLASS_SLIDER_THUMB;

pub(crate) fn glass_slider(action: &str) -> Option<&'static GlassSlider> {
    GLASS_SLIDERS.iter().find(|slider| slider.action == action)
}

/// Maps a native track percent (0..=100) into `minimum..=maximum`, rounded to the nearest unit.
pub(crate) fn glass_slider_value(text: &str, slider: &GlassSlider) -> Result<i64, String> {
    let percent: f64 = text
        .trim()
        .parse()
        .map_err(|_| format!("invalid {} slider value '{text}'", slider.key))?;
    if !percent.is_finite() {
        return Err(format!("invalid {} slider value '{text}'", slider.key));
    }
    let span = (slider.maximum - slider.minimum) as f64;
    let raw = slider.minimum as f64 + percent.clamp(0.0, 100.0) / 100.0 * span;
    Ok((raw.round() as i64).clamp(slider.minimum, slider.maximum))
}
pub(crate) const DISPLAY_SCALE_SLIDER: &str = "slider.display-scale";

pub(crate) fn display_scale_from_slider(text: &str) -> Result<u16, String> {
    let fraction: f64 = text
        .trim()
        .parse()
        .map_err(|_| format!("invalid display scale slider value '{text}'"))?;
    if !fraction.is_finite() {
        return Err("display scale slider requires a finite number".to_owned());
    }
    let ticks = (fraction.clamp(0.0, 100.0) / 5.0).round() as u16;
    Ok(100 + ticks * 5)
}

pub(crate) const ANIMATION_SPEED_MIN: i64 = 25;
pub(crate) const ANIMATION_SPEED_MAX: i64 = 200;
pub(crate) const ANIMATION_SPEED_STEP: f64 = 5.0;
/// Must match the `.slider-track` width in ui/settings.css.
pub(crate) const ANIMATION_SPEED_TRACK_WIDTH: f64 = 180.0;
pub(crate) const ANIMATION_SPEED_THUMB: f64 = 15.0;
pub(crate) const ANIMATION_SPEED_TRACK_TRAVEL: f64 =
    ANIMATION_SPEED_TRACK_WIDTH - ANIMATION_SPEED_THUMB;

/// Maps a native slider value (track percent 0..=100) to `animationSpeed`
/// hundredths in 25..=200, snapped to 5.
pub(crate) fn animation_speed_from_slider(text: &str) -> Result<i64, String> {
    let percent: f64 = text
        .trim()
        .parse()
        .map_err(|_| format!("invalid animation speed value '{text}'"))?;
    if !percent.is_finite() {
        return Err(format!("invalid animation speed value '{text}'"));
    }
    let span = (ANIMATION_SPEED_MAX - ANIMATION_SPEED_MIN) as f64;
    let raw = ANIMATION_SPEED_MIN as f64 + percent.clamp(0.0, 100.0) / 100.0 * span;
    let steps = ((raw - ANIMATION_SPEED_MIN as f64) / ANIMATION_SPEED_STEP).round();
    let snapped = ANIMATION_SPEED_MIN + (steps * ANIMATION_SPEED_STEP) as i64;
    Ok(snapped.clamp(ANIMATION_SPEED_MIN, ANIMATION_SPEED_MAX))
}

/// Surface glass steppers: action prefix and the percent key each one moves.
pub(crate) const GLASS_STEP_KEYS: [(&str, &str); 1] =
    [("settings.surface-saturation.", "surfaceSaturationPercent")];

pub(crate) fn step(value: &str) -> Option<i8> {
    match value {
        "down" => Some(-1),
        "up" => Some(1),
        _ => None,
    }
}

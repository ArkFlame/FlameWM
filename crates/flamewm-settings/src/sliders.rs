//! Split from `lib.rs` (`sliders`); see crate docs.

use flamewm_settings_core::{DISPLAY_SCALE_MIN, DISPLAY_SCALE_STEP};

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
        maximum: 100,
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

/// Target of a WAI-ARIA slider key: arrows step by one unit, PageUp/PageDown by ten, Home and End
/// jump to the bounds. `None` for keys a slider does not handle.
pub(crate) fn slider_key_target(
    key: &str,
    current: i64,
    minimum: i64,
    maximum: i64,
) -> Option<i64> {
    let target = match key {
        "Right" | "Up" => current + 1,
        "Left" | "Down" => current - 1,
        "PageUp" => current + 10,
        "PageDown" => current - 10,
        "Home" => minimum,
        "End" => maximum,
        _ => return None,
    };
    Some(target.clamp(minimum, maximum))
}

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
    let ticks = (fraction.clamp(0.0, 100.0) / f64::from(DISPLAY_SCALE_STEP)).round() as u16;
    Ok(DISPLAY_SCALE_MIN + ticks * DISPLAY_SCALE_STEP)
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

/// Surface glass steppers: action prefix and the percent key each one moves. The dock surface
/// overrides (`dock.surface*`) use the same stepper pattern.
pub(crate) const GLASS_STEP_KEYS: [(&str, &str); 4] = [
    ("settings.surface-saturation.", "surfaceSaturationPercent"),
    ("settings.dock-surface-opacity.", "dock.surfaceOpacity"),
    ("settings.dock-surface-blur.", "dock.surfaceBlurPercent"),
    (
        "settings.dock-outline-opacity.",
        "dock.surfaceOutlineOpacity",
    ),
];

pub(crate) fn step(value: &str) -> Option<i8> {
    match value {
        "down" => Some(-1),
        "up" => Some(1),
        _ => None,
    }
}

#[cfg(test)]
mod key_target_tests {
    use super::slider_key_target;

    #[test]
    fn arrows_step_one_unit_and_clamp_to_the_range() {
        assert_eq!(slider_key_target("Right", 50, 0, 100), Some(51));
        assert_eq!(slider_key_target("Up", 50, 0, 100), Some(51));
        assert_eq!(slider_key_target("Left", 50, 0, 100), Some(49));
        assert_eq!(slider_key_target("Down", 50, 0, 100), Some(49));
        assert_eq!(slider_key_target("Right", 100, 0, 100), Some(100));
        assert_eq!(slider_key_target("Left", 0, 0, 100), Some(0));
    }

    #[test]
    fn page_keys_step_ten_and_home_end_jump_to_the_bounds() {
        assert_eq!(slider_key_target("PageUp", 95, 4, 100), Some(100));
        assert_eq!(slider_key_target("PageDown", 10, 4, 100), Some(4));
        assert_eq!(slider_key_target("Home", 50, 4, 100), Some(4));
        assert_eq!(slider_key_target("End", 50, 4, 100), Some(100));
    }

    #[test]
    fn other_keys_are_not_slider_keys() {
        assert_eq!(slider_key_target("Tab", 50, 0, 100), None);
        assert_eq!(slider_key_target("a", 50, 0, 100), None);
    }
}

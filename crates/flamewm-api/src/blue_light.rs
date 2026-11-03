//! Blue light filter and Night Shift schedule: one owner for the product maths.
//!
//! Every session and the settings UI read the same functions, so the slider value, the schedule
//! window and the applied colour cannot drift between X11, Wayland, and the preview. Nothing here
//! touches a display: the session decides how to apply the returned ramp.

use crate::settings::{SettingValue, SettingsSnapshot};

/// Key of the filter intensity (0..=100). `0` is neutral; `100` is the warmest step.
pub const BLUE_LIGHT_FILTER_KEY: &str = "blueLightFilter";

/// Key of the Night Shift master switch.
pub const NIGHT_SHIFT_KEY: &str = "nightShift";

/// Key of the schedule switch. Only meaningful while [`NIGHT_SHIFT_KEY`] is on.
pub const NIGHT_SHIFT_SCHEDULE_KEY: &str = "nightShiftSchedule";

/// Key of the schedule start, as minutes after local midnight (0..=1439).
pub const NIGHT_SHIFT_START_KEY: &str = "nightShiftStart";

/// Key of the schedule end, as minutes after local midnight (0..=1439).
pub const NIGHT_SHIFT_END_KEY: &str = "nightShiftEnd";

/// Default intensity: a visible but gentle warm shift.
pub const DEFAULT_INTENSITY: u8 = 40;

/// Default schedule window, 21:00 to 07:00, the common desktop-night range.
pub const DEFAULT_START_MINUTES: u32 = 21 * 60;
pub const DEFAULT_END_MINUTES: u32 = 7 * 60;

/// A resolved filter state: whether it is on now and how warm it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BlueLightState {
    /// Intensity to apply, `0` when the filter is off.
    pub intensity: u8,
    /// Whether a schedule is configured and currently inside its window.
    pub scheduled_inside: bool,
}

impl BlueLightState {
    /// Whether any warmth should be applied.
    #[must_use]
    pub const fn is_active(self) -> bool {
        self.intensity > 0
    }
}

/// The configured filter values, independent of the current time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlueLightConfig {
    /// Slider value, already clamped to `0..=100`.
    pub intensity: u8,
    /// `nightShift`: the feature is enabled at all.
    pub night_shift: bool,
    /// `nightShiftSchedule`: the enabled state follows the start/end window.
    pub schedule: bool,
    pub start_minutes: u32,
    pub end_minutes: u32,
}

impl Default for BlueLightConfig {
    fn default() -> Self {
        Self {
            intensity: DEFAULT_INTENSITY,
            night_shift: false,
            schedule: false,
            start_minutes: DEFAULT_START_MINUTES,
            end_minutes: DEFAULT_END_MINUTES,
        }
    }
}

impl BlueLightConfig {
    /// Reads the config from a settings snapshot, keeping product defaults for missing or
    /// ill-typed values so a hand-edited file cannot put a session in an odd state.
    #[must_use]
    pub fn from_snapshot(snapshot: &SettingsSnapshot) -> Self {
        let defaults = Self::default();
        BlueLightConfig {
            intensity: percent_value(snapshot, BLUE_LIGHT_FILTER_KEY)
                .map_or(defaults.intensity, |value| value.min(100)),
            night_shift: boolean_value(snapshot, NIGHT_SHIFT_KEY).unwrap_or(defaults.night_shift),
            schedule: boolean_value(snapshot, NIGHT_SHIFT_SCHEDULE_KEY)
                .unwrap_or(defaults.schedule),
            start_minutes: minutes_value(snapshot, NIGHT_SHIFT_START_KEY)
                .unwrap_or(defaults.start_minutes),
            end_minutes: minutes_value(snapshot, NIGHT_SHIFT_END_KEY)
                .unwrap_or(defaults.end_minutes),
        }
    }
}

fn boolean_value(snapshot: &SettingsSnapshot, key: &str) -> Option<bool> {
    match snapshot.values.get(key) {
        Some(SettingValue::Boolean(value)) => Some(*value),
        _ => None,
    }
}

fn percent_value(snapshot: &SettingsSnapshot, key: &str) -> Option<u8> {
    match snapshot.values.get(key) {
        Some(SettingValue::Percent(value)) => Some(*value),
        Some(SettingValue::Integer(value)) => u8::try_from(*value).ok().filter(|v| *v <= 100),
        _ => None,
    }
}

fn minutes_value(snapshot: &SettingsSnapshot, key: &str) -> Option<u32> {
    match snapshot.values.get(key) {
        Some(SettingValue::Integer(value)) => u32::try_from(*value)
            .ok()
            .filter(|minutes| *minutes < MINUTES_PER_DAY),
        _ => None,
    }
}

/// Minutes in one day.
pub const MINUTES_PER_DAY: u32 = 24 * 60;

/// Formats minutes after midnight as `HH:MM`, saturating outside the day.
#[must_use]
pub fn format_minutes(minutes: u32) -> String {
    let minutes = minutes.min(MINUTES_PER_DAY - 1);
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// Parses `HH:MM` into minutes after midnight; accepts `H:MM` too.
#[must_use]
pub fn parse_minutes(text: &str) -> Option<u32> {
    let (hours, minutes) = text.trim().split_once(':')?;
    let hours: u32 = hours.trim().parse().ok()?;
    let minutes: u32 = minutes.trim().parse().ok()?;
    (hours < 24 && minutes < 60)
        .then_some(hours * 60 + minutes)
        .filter(|value| *value < MINUTES_PER_DAY)
}

/// Whether `now` falls inside the `[start, end)` window, wrapping over midnight when
/// `start > end`. An empty window (`start == end`) is never active: it is the only reading that
/// does not make the schedule either always-on or undefined.
#[must_use]
pub const fn schedule_active(now_minutes: u32, start_minutes: u32, end_minutes: u32) -> bool {
    let now = now_minutes % MINUTES_PER_DAY;
    let start = start_minutes % MINUTES_PER_DAY;
    let end = end_minutes % MINUTES_PER_DAY;
    if start == end {
        return false;
    }
    if start < end {
        now >= start && now < end
    } else {
        now >= start || now < end
    }
}

/// Resolves the filter for `now_minutes`: the schedule decides whether the filter is applied, and
/// the slider decides how warm it is.
#[must_use]
pub fn resolve_at(config: BlueLightConfig, now_minutes: u32) -> BlueLightState {
    if !config.night_shift {
        return BlueLightState::default();
    }
    let inside = if config.schedule {
        schedule_active(now_minutes, config.start_minutes, config.end_minutes)
    } else {
        true
    };
    BlueLightState {
        intensity: if inside { config.intensity.min(100) } else { 0 },
        scheduled_inside: config.schedule && inside,
    }
}

/// One channel's ramp multiplier in thousandths of full scale.
///
/// Blue is reduced the most and red the least, which is what a warm/amber filter does. `0` is
/// exactly neutral (every channel at 1000) so an off filter can never tint the output.
#[must_use]
pub fn channel_permille(intensity: u8) -> (u32, u32, u32) {
    let intensity = u32::from(intensity.min(100));
    // Warmth 0..=1000. Red keeps full scale; green loses up to 18% and blue up to 45%, the
    // range a 6500K -> ~3400K shift needs without turning white into orange.
    let red = 1000;
    let green = 1000 - (intensity * 180) / 100;
    let blue = 1000 - (intensity * 450) / 100;
    (red, green, blue)
}

/// A gamma ramp of `size` entries per channel, scaled by [`channel_permille`].
///
/// The linear ramp is deliberate: it matches what `xrandr --gamma` and most night-light tools
/// upload, so the visual result is the familiar one. Returns empty vectors for a zero size.
#[must_use]
pub fn gamma_ramp(size: usize, intensity: u8) -> (Vec<u16>, Vec<u16>, Vec<u16>) {
    if size == 0 {
        return (Vec::new(), Vec::new(), Vec::new());
    }
    let (red, green, blue) = channel_permille(intensity);
    let scale = |permille: u32| -> Vec<u16> {
        (0..size)
            .map(|index| {
                let full = u64::from(u16::MAX) * index as u64 / (size as u64 - 1).max(1);
                u16::try_from(u64::from(full) * u64::from(permille) / 1000).unwrap_or(u16::MAX)
            })
            .collect()
    };
    (scale(red), scale(green), scale(blue))
}

/// Premultiplied-free RGBA for a compositor that can only blend an overlay.
///
/// Wayland has no output gamma ramp exposed through Smithay, so the compositor warms the frame by
/// covering it with an amber veil instead of recolouring the scanout. This is an approximation,
/// not a colourimetric calibration: it lifts the black level toward amber, where a real gamma ramp
/// would keep blacks black. It is the honest option that still works on every Wayland backend, and
/// the intensity curve matches [`channel_permille`]'s warmth so both sessions feel the same.
///
/// `0` intensity returns `[0.0, 0.0, 0.0, 0.0]`, so an off filter never tints the output.
#[must_use]
pub fn overlay_rgba(intensity: u8) -> [f32; 4] {
    let intensity = u32::from(intensity.min(100));
    if intensity == 0 {
        return [0.0, 0.0, 0.0, 0.0];
    }
    // Amber: full red, a little green, no blue, which is what a 3400K-ish look reads as.
    let alpha = intensity as f32 * 0.0025_f32;
    [1.0, 0.62, 0.18, alpha]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_intensity_is_exactly_neutral() {
        assert_eq!(channel_permille(0), (1000, 1000, 1000));
        let (red, green, blue) = gamma_ramp(4, 0);
        assert_eq!(red, green);
        assert_eq!(green, blue);
        assert_eq!(red, vec![0, 21_845, 43_690, u16::MAX]);
    }

    #[test]
    fn more_intensity_is_warmer_and_clamped() {
        let (_, green_50, blue_50) = channel_permille(50);
        let (_, green_100, blue_100) = channel_permille(100);
        assert!(blue_50 < green_50, "blue must fall further than green");
        assert!(blue_100 < blue_50);
        assert!(green_100 < green_50);
        // An out-of-range slider value cannot produce a colder-than-neutral or negative ramp.
        assert_eq!(channel_permille(250), channel_permille(100));
        assert_eq!(channel_permille(0).0, 1000);
    }

    #[test]
    fn schedule_handles_daytime_window_and_an_exclusive_end() {
        let (start, end) = (8 * 60, 17 * 60);
        assert!(!schedule_active(start - 1, start, end));
        assert!(schedule_active(start, start, end));
        assert!(schedule_active(end - 1, start, end));
        assert!(
            !schedule_active(end, start, end),
            "the end boundary is exclusive"
        );
    }

    #[test]
    fn schedule_wraps_across_midnight() {
        let (start, end) = (22 * 60, 6 * 60);
        assert!(schedule_active(23 * 60, start, end));
        assert!(schedule_active(0, start, end));
        assert!(schedule_active(5 * 60 + 59, start, end));
        assert!(!schedule_active(6 * 60, start, end));
        assert!(!schedule_active(12 * 60, start, end));
    }

    #[test]
    fn an_empty_schedule_window_is_never_active() {
        for now in [0, 6 * 60, 12 * 60, 23 * 60] {
            assert!(!schedule_active(now, 9 * 60, 9 * 60));
        }
    }

    #[test]
    fn night_shift_off_never_applies_the_filter() {
        let config = BlueLightConfig {
            intensity: 80,
            night_shift: false,
            schedule: false,
            ..BlueLightConfig::default()
        };
        assert_eq!(resolve_at(config, 0), BlueLightState::default());
        assert!(!resolve_at(config, 0).is_active());
    }

    #[test]
    fn a_disabled_schedule_applies_the_slider_all_day() {
        let config = BlueLightConfig {
            intensity: 60,
            night_shift: true,
            schedule: false,
            ..BlueLightConfig::default()
        };
        for now in [0, 12 * 60, 23 * 60] {
            let state = resolve_at(config, now);
            assert_eq!(state.intensity, 60);
            assert!(
                !state.scheduled_inside,
                "no schedule means nothing is 'inside'"
            );
        }
    }

    #[test]
    fn an_enabled_schedule_only_applies_inside_the_window() {
        let config = BlueLightConfig {
            intensity: 70,
            night_shift: true,
            schedule: true,
            start_minutes: 22 * 60,
            end_minutes: 6 * 60,
        };
        assert_eq!(resolve_at(config, 23 * 60).intensity, 70);
        assert!(resolve_at(config, 23 * 60).scheduled_inside);
        assert_eq!(resolve_at(config, 12 * 60).intensity, 0);
        assert!(!resolve_at(config, 12 * 60).scheduled_inside);
    }

    #[test]
    fn config_reads_typed_values_and_keeps_defaults_otherwise() {
        let mut snapshot = SettingsSnapshot::new(1);
        assert_eq!(
            BlueLightConfig::from_snapshot(&snapshot),
            BlueLightConfig::default()
        );

        snapshot
            .values
            .insert(BLUE_LIGHT_FILTER_KEY.into(), SettingValue::Percent(75));
        snapshot
            .values
            .insert(NIGHT_SHIFT_KEY.into(), SettingValue::Boolean(true));
        snapshot
            .values
            .insert(NIGHT_SHIFT_SCHEDULE_KEY.into(), SettingValue::Boolean(true));
        snapshot
            .values
            .insert(NIGHT_SHIFT_START_KEY.into(), SettingValue::Integer(1_320));
        snapshot
            .values
            .insert(NIGHT_SHIFT_END_KEY.into(), SettingValue::Integer(360));
        let config = BlueLightConfig::from_snapshot(&snapshot);
        assert_eq!(
            config,
            BlueLightConfig {
                intensity: 75,
                night_shift: true,
                schedule: true,
                start_minutes: 1_320,
                end_minutes: 360,
            }
        );

        // A wrong type or an out-of-day value keeps the default instead of being trusted.
        snapshot
            .values
            .insert(NIGHT_SHIFT_START_KEY.into(), SettingValue::Integer(10_000));
        snapshot.values.insert(
            BLUE_LIGHT_FILTER_KEY.into(),
            SettingValue::Text("warm".into()),
        );
        let config = BlueLightConfig::from_snapshot(&snapshot);
        assert_eq!(config.start_minutes, DEFAULT_START_MINUTES);
        assert_eq!(config.intensity, DEFAULT_INTENSITY);
    }

    #[test]
    fn the_overlay_is_off_at_zero_and_warmer_with_intensity() {
        assert_eq!(overlay_rgba(0), [0.0, 0.0, 0.0, 0.0]);
        let half = overlay_rgba(50);
        let full = overlay_rgba(100);
        assert_eq!(half[0], 1.0);
        assert!(half[2] < half[1], "the veil must be amber, not white");
        assert!(full[3] > half[3], "more intensity is a stronger veil");
        assert!(full[3] < 0.3, "even full strength must not hide the screen");
        assert_eq!(overlay_rgba(250), full, "out-of-range values are clamped");
    }

    #[test]
    fn minutes_round_trip_through_the_ui_format() {
        for minutes in [0, 1, 59, 60, 9 * 60 + 5, 23 * 60 + 59] {
            assert_eq!(parse_minutes(&format_minutes(minutes)), Some(minutes));
        }
        assert_eq!(parse_minutes("7:05"), Some(7 * 60 + 5));
        assert_eq!(parse_minutes("24:00"), None);
        assert_eq!(parse_minutes("12:60"), None);
        assert_eq!(parse_minutes("noon"), None);
    }
}

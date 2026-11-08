//! Taskbar weather glyph tint: the one table mapping a condition (and day or night) to the
//! icon colour. Pure data; the shell writes the result as the glyph's foreground when the
//! `weatherTint` setting is on, and clears it otherwise.

use crate::weather::WeatherCondition;
use flamewm_api::settings::{SettingValue, SettingsSnapshot};

/// Straight-alpha RGBA, one byte per channel.
pub type Rgba = [u8; 4];

/// Setting key that enables condition tints (default on; absent means on).
pub const TINT_SETTINGS_KEY: &str = "weatherTint";

const CLEAR_DAY: [u8; 3] = [255, 200, 61];
const CLEAR_NIGHT: [u8; 3] = [214, 228, 255];
const FEW_CLOUDS: [u8; 3] = [214, 190, 98];
const OVERCAST: [u8; 3] = [143, 163, 184];
const FOG: [u8; 3] = [200, 205, 210];
const DRIZZLE: [u8; 3] = [110, 178, 240];
const RAIN: [u8; 3] = [60, 127, 224];
const SNOW: [u8; 3] = [181, 238, 247];
const STORM: [u8; 3] = [167, 122, 240];

/// Whether `weatherTint` is on. An absent key (no snapshot yet, or the default) is on; only an
/// explicit `false` turns tinting off.
#[must_use]
pub fn tint_enabled(snapshot: Option<&SettingsSnapshot>) -> bool {
    !matches!(
        snapshot.and_then(|s| s.values.get(TINT_SETTINGS_KEY)),
        Some(SettingValue::Boolean(false))
    )
}

/// Tint for the glyph of `condition`. `None` keeps the inherited foreground, which is the
/// unknown-data icon; `is_day` only changes the clear-sky tint.
#[must_use]
pub fn icon_tint(condition: WeatherCondition, is_day: bool) -> Option<Rgba> {
    let [r, g, b] = match condition {
        WeatherCondition::Clear if is_day => CLEAR_DAY,
        WeatherCondition::Clear => CLEAR_NIGHT,
        WeatherCondition::FewClouds => FEW_CLOUDS,
        WeatherCondition::Overcast => OVERCAST,
        WeatherCondition::Fog => FOG,
        WeatherCondition::Drizzle => DRIZZLE,
        WeatherCondition::Rain => RAIN,
        WeatherCondition::Snow => SNOW,
        WeatherCondition::Storm => STORM,
        WeatherCondition::Unknown => return None,
    };
    Some([r, g, b, 0xff])
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [WeatherCondition; 9] = [
        WeatherCondition::Clear,
        WeatherCondition::FewClouds,
        WeatherCondition::Overcast,
        WeatherCondition::Fog,
        WeatherCondition::Drizzle,
        WeatherCondition::Rain,
        WeatherCondition::Snow,
        WeatherCondition::Storm,
        WeatherCondition::Unknown,
    ];

    fn rgb(condition: WeatherCondition, is_day: bool) -> [u8; 3] {
        let [r, g, b, _] = icon_tint(condition, is_day).expect("tinted");
        [r, g, b]
    }

    // OBSERVABLE: every known condition has an opaque tint; only Unknown has none.
    // MUTATION: returning Some for Unknown or a transparent alpha fails here.
    #[test]
    fn every_known_condition_is_opaque_and_unknown_is_untinted() {
        for condition in ALL {
            let tint = icon_tint(condition, true);
            if condition == WeatherCondition::Unknown {
                assert_eq!(tint, None);
            } else {
                assert_eq!(tint.map(|c| c[3]), Some(0xff), "{condition:?}");
            }
        }
    }

    // OBSERVABLE: the setting is on without a snapshot or a stored key, and off only for an
    // explicit false. Non-boolean stored values do not switch it off.
    #[test]
    fn tint_setting_defaults_on_and_only_explicit_false_turns_it_off() {
        use flamewm_api::settings::SettingValue;
        assert!(tint_enabled(None));
        let mut snapshot = flamewm_api::settings::SettingsSnapshot {
            revision: 0,
            values: Default::default(),
        };
        assert!(tint_enabled(Some(&snapshot)));
        snapshot
            .values
            .insert(TINT_SETTINGS_KEY.to_owned(), SettingValue::Boolean(false));
        assert!(!tint_enabled(Some(&snapshot)));
        snapshot
            .values
            .insert(TINT_SETTINGS_KEY.to_owned(), SettingValue::Boolean(true));
        assert!(tint_enabled(Some(&snapshot)));
    }

    // OBSERVABLE: sun is warm yellow (red and green high, blue low).
    #[test]
    fn clear_day_is_warm_yellow() {
        let [r, g, b] = rgb(WeatherCondition::Clear, true);
        assert!(r > 230 && g > 170 && b < 120, "{r} {g} {b}");
    }

    // OBSERVABLE: night clear is pale blue-white (every channel bright, blue dominant).
    #[test]
    fn clear_night_is_pale_blue_white() {
        let [r, g, b] = rgb(WeatherCondition::Clear, false);
        assert!(r > 190 && g > 200 && b > 240, "{r} {g} {b}");
        assert!(b >= r, "blue must dominate red");
    }

    // OBSERVABLE: day/night changes only the clear sky; other conditions ignore it.
    #[test]
    fn night_flag_only_changes_clear_sky() {
        for condition in ALL {
            if condition != WeatherCondition::Clear {
                assert_eq!(icon_tint(condition, true), icon_tint(condition, false));
            }
        }
        assert_ne!(
            rgb(WeatherCondition::Clear, true),
            rgb(WeatherCondition::Clear, false)
        );
    }

    // OBSERVABLE: rain and drizzle are blue; rain is the deeper of the two.
    #[test]
    fn precipitation_is_blue() {
        for condition in [WeatherCondition::Rain, WeatherCondition::Drizzle] {
            let [r, _, b] = rgb(condition, true);
            assert!(b > 200 && b > r + 60, "{condition:?} {r} {b}");
        }
        // Rain is the deeper blue: less red than drizzle.
        let [rain_r, _, _] = rgb(WeatherCondition::Rain, true);
        let [drizzle_r, _, _] = rgb(WeatherCondition::Drizzle, true);
        assert!(rain_r < drizzle_r);
    }

    // OBSERVABLE: storm is purple (blue and red above green).
    #[test]
    fn storm_is_purple() {
        let [r, g, b] = rgb(WeatherCondition::Storm, true);
        assert!(r > g + 60 && b > g + 60, "{r} {g} {b}");
    }

    // OBSERVABLE: snow is light cyan-white (every channel bright, green and blue above red).
    #[test]
    fn snow_is_light_cyan_white() {
        let [r, g, b] = rgb(WeatherCondition::Snow, true);
        assert!(r > 160 && g > 220 && b > 230, "{r} {g} {b}");
        assert!(g > r && b > r);
    }

    // OBSERVABLE: fog is light neutral grey (channels nearly equal and bright).
    #[test]
    fn fog_is_light_grey() {
        let [r, g, b] = rgb(WeatherCondition::Fog, true);
        assert!(r > 180 && r.abs_diff(g) < 12 && g.abs_diff(b) < 12);
    }

    // OBSERVABLE: overcast is grey-blue (desaturated, blue above red); partly cloudy is
    // yellow-tinged grey (red above blue).
    #[test]
    fn cloud_tints_are_grey_blue_and_yellow_grey() {
        let [or, _, ob] = rgb(WeatherCondition::Overcast, true);
        assert!(ob > or, "overcast must lean blue");
        let [fr, _, fb] = rgb(WeatherCondition::FewClouds, true);
        assert!(fr > fb + 60, "partly cloudy must lean yellow");
    }
}

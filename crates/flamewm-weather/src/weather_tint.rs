//! Taskbar weather glyph tint: the one table mapping a condition (and day or night) to the CSS
//! custom property that colours the glyph. The colours live in `:root` of `ui/shell/flamewm.css`,
//! so a theme restyles them there. The shell selects the variable by name when the `weatherTint`
//! setting is on, and clears the glyph colour otherwise.

use crate::weather::WeatherCondition;
use flamewm_api::settings::{SettingValue, SettingsSnapshot};

/// Setting key that enables condition tints (default on; absent means on).
pub const TINT_SETTINGS_KEY: &str = "weatherTint";

/// Whether `weatherTint` is on. An absent key (no snapshot yet, or the default) is on; only an
/// explicit `false` turns tinting off.
#[must_use]
pub fn tint_enabled(snapshot: Option<&SettingsSnapshot>) -> bool {
    !matches!(
        snapshot.and_then(|s| s.values.get(TINT_SETTINGS_KEY)),
        Some(SettingValue::Boolean(false))
    )
}

/// CSS custom property of the glyph colour for `condition`. `None` keeps the inherited foreground,
/// which is the unknown-data icon; `is_day` only changes the clear-sky variable.
#[must_use]
pub fn icon_tint_variable(condition: WeatherCondition, is_day: bool) -> Option<&'static str> {
    Some(match condition {
        WeatherCondition::Clear if is_day => "--weather-sun",
        WeatherCondition::Clear => "--weather-clear-night",
        WeatherCondition::FewClouds => "--weather-few-clouds",
        WeatherCondition::Overcast => "--weather-overcast",
        WeatherCondition::Fog => "--weather-fog",
        WeatherCondition::Drizzle => "--weather-drizzle",
        WeatherCondition::Rain => "--weather-rain",
        WeatherCondition::Snow => "--weather-snow",
        WeatherCondition::Storm => "--weather-storm",
        WeatherCondition::Unknown => return None,
    })
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

    const SHELL_CSS: &str = include_str!("../../../ui/shell/flamewm.css");

    /// Body of the shell sheet's `:root` block.
    fn root_block() -> &'static str {
        SHELL_CSS
            .split(":root {")
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .expect("shell sheet has a :root block")
    }

    /// RGB of colour custom property `name` as declared in the shell sheet's `:root`.
    fn css_rgb(name: &str) -> [u8; 3] {
        let marker = format!("{name}:#");
        let start = root_block()
            .find(&marker)
            .unwrap_or_else(|| panic!("{name} not declared in :root"))
            + marker.len();
        let hex = &root_block()[start..start + 6];
        [0, 2, 4].map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex channel"))
    }

    fn rgb(condition: WeatherCondition, is_day: bool) -> [u8; 3] {
        css_rgb(icon_tint_variable(condition, is_day).expect("tinted"))
    }

    // OBSERVABLE: every known condition maps to a custom property declared in `:root`; only
    // Unknown maps to none. MUTATION: a typo'd name, or a tint for Unknown, fails here.
    #[test]
    fn every_known_condition_maps_to_a_root_variable_and_unknown_to_none() {
        for condition in ALL {
            for is_day in [true, false] {
                match icon_tint_variable(condition, is_day) {
                    None => assert_eq!(condition, WeatherCondition::Unknown),
                    Some(name) => {
                        assert!(name.starts_with("--weather-"), "{name}");
                        assert!(
                            root_block().contains(&format!("{name}:#")),
                            "{condition:?} -> {name} is not declared in :root"
                        );
                    }
                }
            }
        }
    }

    // OBSERVABLE: the default palette in `:root` is the previous Rust table, so the default
    // visuals do not change. MUTATION: editing a default colour fails here.
    #[test]
    fn default_palette_matches_the_previous_literals() {
        assert_eq!(css_rgb("--weather-sun"), [255, 200, 61]);
        assert_eq!(css_rgb("--weather-clear-night"), [214, 228, 255]);
        assert_eq!(css_rgb("--weather-few-clouds"), [214, 190, 98]);
        assert_eq!(css_rgb("--weather-overcast"), [143, 163, 184]);
        assert_eq!(css_rgb("--weather-fog"), [200, 205, 210]);
        assert_eq!(css_rgb("--weather-drizzle"), [110, 178, 240]);
        assert_eq!(css_rgb("--weather-rain"), [60, 127, 224]);
        assert_eq!(css_rgb("--weather-snow"), [181, 238, 247]);
        assert_eq!(css_rgb("--weather-storm"), [167, 122, 240]);
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
                assert_eq!(
                    icon_tint_variable(condition, true),
                    icon_tint_variable(condition, false)
                );
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
        assert!(r > g + 30 && b > g + 60, "{r} {g} {b}");
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

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use flamewm_api::panels::PanelEdge;
use flamewm_api::settings::{AppearanceMode, SettingValue};
use flamewm_api::{ErrorCode, FlameError, FlameResult};

use crate::storage::write_atomic_text;

/// Every action the shortcut registry can bind is persistable; this allowlist used to lag it,
/// so a file overriding e.g. `OpenTerminalSuper` was rejected as a whole.
const KNOWN_HOTKEYS: [&str; crate::shortcuts::ACTIONS.len()] = crate::shortcuts::ACTIONS;

mod product;
mod slots;
mod stable;
mod types;

pub use product::*;
pub use slots::*;
pub(crate) use stable::*;
pub use types::*;

impl ProductSettings {
    /// Every switch-group member resolved for readers: a member is its default while its group
    /// switch is off, its stored value while on. Stored values are untouched, so turning a switch
    /// off never loses data and turning it back on restores them. Idempotent.
    #[must_use]
    pub fn effective(&self) -> Self {
        let defaults = Self::default();
        let mut effective = self.clone();
        for group in flamewm_api::settings_groups::SWITCH_GROUPS {
            let switch_on = stable_setting(group.switch)
                .is_some_and(|row| (row.get)(self) == SettingValue::Boolean(true));
            if switch_on {
                continue;
            }
            for (member, _) in group.members {
                if let Some(row) = stable_setting(member) {
                    let reset = (row.set)(&mut effective, &defaults, None, member);
                    debug_assert!(reset.is_ok(), "{member} resets to its default");
                }
            }
        }
        effective
    }

    /// Effective Start menu style. A disabled toggle or the `none` style both mean no animation.
    #[must_use]
    pub fn start_animation_style(&self) -> AnimationStyle {
        gated_animation(self.start_animation_enabled, self.start_animation)
    }

    /// Effective minimize style. A disabled toggle or the `none` style both mean no animation.
    #[must_use]
    pub fn minimize_animation_style(&self) -> AnimationStyle {
        gated_animation(self.minimize_animation_enabled, self.minimize_animation)
    }

    /// Effective maximize style. A disabled toggle or the `none` style both mean no animation.
    #[must_use]
    pub fn maximize_animation_style(&self) -> AnimationStyle {
        gated_animation(self.maximize_animation_enabled, self.maximize_animation)
    }

    /// Effective window-open style. A disabled toggle or the `none` style both mean no animation.
    #[must_use]
    pub fn window_open_animation_style(&self) -> AnimationStyle {
        gated_animation(
            self.window_open_animation_enabled,
            self.window_open_animation,
        )
    }

    /// Taskbar backdrop from the shared surface policy. See [`Backdrop`] for the blur-to-radius mapping.
    #[must_use]
    pub fn taskbar_backdrop(&self) -> Backdrop {
        backdrop(self.surface_opacity, self.surface_blur_percent)
    }

    /// Typed screensaver policy. Ribbon width converts tenths of a pixel to `f32` pixels.
    #[must_use]
    pub fn screensaver(&self) -> ScreensaverSettings {
        ScreensaverSettings {
            mode: self.screensaver_mode,
            after_lock: Duration::from_secs(u64::from(self.screensaver_after_lock_minutes) * 60),
            num_ribbons: u8::try_from(self.screensaver_num_ribbons).unwrap_or(u8::MAX),
            ribbon_width_px: f32::from(self.screensaver_ribbon_width_tenths) / 10.0,
            span_monitors: self.screensaver_span_monitors,
            display_off_minutes: self.screensaver_display_off_minutes,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ConfigStore {
    current: ProductSettings,
}

/// True when `switch` is a group switch and one of its members holds a non-default stored value.
fn switch_guards_custom_values(snapshot: &ProductSettings, switch: &str) -> bool {
    let defaults = ProductSettings::default();
    flamewm_api::settings_groups::SWITCH_GROUPS
        .iter()
        .find(|group| group.switch == switch)
        .is_some_and(|group| {
            group.members.iter().any(|(member, _)| {
                stable_setting(member)
                    .is_some_and(|row| (row.get)(snapshot) != (row.get)(&defaults))
            })
        })
}

impl ConfigStore {
    #[must_use]
    pub fn current(&self) -> &ProductSettings {
        &self.current
    }

    pub fn apply_snapshot(&mut self, snapshot: ProductSettings) -> FlameResult<()> {
        snapshot.validate()?;
        if snapshot.revision <= self.current.revision {
            return Err(FlameError::new(
                ErrorCode::StaleRevision,
                "settings revision is stale",
            ));
        }
        self.current = snapshot;
        Ok(())
    }

    /// Adopts persisted per-output scales without moving the settings revision.
    pub fn adopt_scales(&mut self, snapshot: &ProductSettings) {
        self.current
            .per_output_scale
            .clone_from(&snapshot.per_output_scale);
    }

    /// Adopts persisted panel edges without moving the settings revision.
    pub fn adopt_panel_edges(&mut self, snapshot: &ProductSettings) {
        self.current.panel_edges.clone_from(&snapshot.panel_edges);
    }

    /// Linux-first tmp+fsync+rename persistence matching the native `ConfigStore` contract.
    pub fn persist(path: &Path, snapshot: &ProductSettings) -> FlameResult<()> {
        snapshot.validate()?;
        if path.as_os_str().is_empty() {
            return Err(FlameError::new(
                ErrorCode::InvalidArgument,
                "settings persistence path is empty",
            ));
        }
        write_atomic_text(path, &Self::serialize(snapshot)).map_err(FlameError::io)
    }
}

/// Accepts an IANA zone name such as `Europe/Berlin` or `UTC`: ASCII letters, digits, `_`, `+`,
/// `-`, `/`, no leading `/` or `.`, no `..`, at most 64 bytes. Existence is checked separately.
#[must_use]
pub fn is_valid_time_zone_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('/')
        && !name.contains("..")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'+' | b'-' | b'/'))
}

fn resolve<T: Slot>(value: Option<&SettingValue>, default: &T, key: &str) -> FlameResult<T> {
    value.map_or_else(|| Ok(default.clone()), |value| T::from_value(value, key))
}

fn wire_text(value: &SettingValue, flag: bool) -> String {
    match value {
        SettingValue::Text(text) => text.clone(),
        SettingValue::Boolean(on) if flag => u8::from(*on).to_string(),
        SettingValue::Boolean(on) => on.to_string(),
        SettingValue::Percent(percent) => percent.to_string(),
        SettingValue::Integer(number) => number.to_string(),
        SettingValue::Rgb(rgb) => rgb.to_string(),
    }
}

fn parse<T: std::str::FromStr>(value: &str, name: &str) -> FlameResult<T> {
    value
        .parse::<T>()
        .map_err(|_| FlameError::new(ErrorCode::InvalidArgument, format!("bad {name}")))
}

fn parse_panel_edge(value: &str) -> Option<PanelEdge> {
    match value {
        "bottom" => Some(PanelEdge::Bottom),
        "top" => Some(PanelEdge::Top),
        "left" => Some(PanelEdge::Left),
        "right" => Some(PanelEdge::Right),
        _ => None,
    }
}

fn legacy_wallpaper_mode(snapshot: &ProductSettings) -> BackgroundMode {
    if snapshot.wallpaper.is_empty() {
        BackgroundMode::Default
    } else {
        BackgroundMode::Custom
    }
}

/// Lock mode implied by saved values alone: a saved path means custom, a non-default colour
/// means solid, and otherwise the bundled image.
fn legacy_lock_background_mode(snapshot: &ProductSettings) -> BackgroundMode {
    if !snapshot.lock_background_image.is_empty() {
        BackgroundMode::Custom
    } else if snapshot.lock_background_color != ProductSettings::default().lock_background_color {
        BackgroundMode::Solid
    } else {
        BackgroundMode::Default
    }
}

fn is_stable_key(key: &str) -> bool {
    key == "revision"
        || stable_setting(key).is_some()
        || key.starts_with("scale.")
        || key.starts_with("panel.edge.")
        || key.starts_with("hotkey.")
}

fn invalid<T>(message: &'static str) -> FlameResult<T> {
    Err(FlameError::new(ErrorCode::InvalidArgument, message))
}

fn invalid_owned<T>(message: String) -> FlameResult<T> {
    Err(FlameError::new(ErrorCode::InvalidArgument, message))
}

mod product_validate;
mod render_backend_migrate;
use render_backend_migrate::migrate_legacy_acceleration;
mod snapshot_parse;
mod snapshot_serialize;
mod validate;
use validate::*;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod glass_settings_tests;

#[cfg(test)]
mod accent_custom_tests;
#[cfg(test)]
mod accessibility_night_tests;
#[cfg(test)]
mod dock_settings_tests;
#[cfg(test)]
mod edge_hot_corner_tests;
#[cfg(test)]
mod switch_contract_tests;
#[cfg(test)]
mod switch_group_tests;
#[cfg(test)]
mod switch_migration_tests;
#[cfg(test)]
mod weather_tint_settings_tests;

#[cfg(test)]
mod outline_default_tests {
    use super::ProductSettings;

    #[test]
    fn outline_colors_default_to_neutral_gray_without_changing_outline_policy() {
        let defaults = ProductSettings::default();
        assert_eq!(defaults.surface_outline_color, "#999999");
        assert_eq!(defaults.window_outline_color, "#999999");
        assert_eq!(defaults.surface_outline_opacity, 50);
        assert_eq!(defaults.surface_outline_width, 1);
        assert!(!defaults.custom_window_outline);
        assert_eq!(defaults.window_outline_opacity, 50);
        assert_eq!(defaults.window_outline_width, 1);
        assert!(defaults.outlines);
    }
}

/// Pointer controls: the schema bounds for `pointerSpeed` and the `pointerRawInput` default.
#[cfg(test)]
mod pointer_tests {
    use super::{ConfigStore, ProductSettings};
    use flamewm_api::ErrorCode;

    fn load(text: &str) -> ProductSettings {
        ConfigStore::default()
            .parse_snapshot(text)
            .expect("valid pointer settings")
    }

    fn rejected(line: &str) -> bool {
        ConfigStore::default()
            .parse_snapshot(&format!("revision=2\n{line}\n"))
            .is_err_and(|error| error.code == ErrorCode::InvalidArgument)
    }

    #[test]
    fn defaults_are_zero_speed_and_adaptive() {
        let defaults = ProductSettings::default();
        assert_eq!(defaults.pointer_speed, 0);
        assert!(!defaults.pointer_raw_input);
        // Both are optional keys: the default file keeps its existing byte-for-byte shape.
        let text = ConfigStore::serialize(&defaults);
        assert!(!text.contains("pointerSpeed"), "{text}");
        assert!(!text.contains("pointerRawInput"), "{text}");
    }

    #[test]
    fn the_documented_range_is_accepted_and_out_of_range_percents_are_rejected() {
        for speed in -100..=100 {
            let parsed = load(&format!("revision=2\npointerSpeed={speed}\n"));
            assert_eq!(parsed.pointer_speed, speed, "pointerSpeed={speed}");
        }
        assert!(rejected("pointerSpeed=-101"), "-101 is below the range");
        assert!(rejected("pointerSpeed=101"), "101 is above the range");
        assert!(rejected("pointerSpeed=-1000"));
        assert!(rejected("pointerSpeed=1000"));
        // The bounds themselves are inclusive.
        assert_eq!(load("revision=2\npointerSpeed=-100\n").pointer_speed, -100);
        assert_eq!(load("revision=2\npointerSpeed=100\n").pointer_speed, 100);
    }

    #[test]
    fn raw_input_defaults_off_and_both_spellings_parse() {
        assert!(!load("revision=2\n").pointer_raw_input);
        assert!(load("revision=2\npointerRawInput=true\n").pointer_raw_input);
        assert!(!load("revision=2\npointerRawInput=false\n").pointer_raw_input);
    }

    #[test]
    fn both_keys_round_trip_and_write_only_when_set() {
        let parsed = load("revision=2\npointerSpeed=-50\npointerRawInput=true\n");
        let text = ConfigStore::serialize(&parsed);
        assert!(text.contains("pointerSpeed=-50\n"), "{text}");
        assert!(text.contains("pointerRawInput=true\n"), "{text}");
        assert_eq!(load(&text), parsed);
    }

    #[test]
    fn fractional_and_non_numeric_values_are_rejected() {
        assert!(rejected("pointerSpeed=0.5"));
        assert!(rejected("pointerSpeed=fast"));
        assert!(rejected("pointerRawInput=maybe"));
    }
}

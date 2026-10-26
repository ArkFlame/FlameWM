use flamewm_api::system::{
    PowerSnapshot, PowerState, ServiceAvailability, SystemAction, SystemSnapshot,
};
use flamewm_control_core::ControlRequest;

/// Battery glyph: 0..=10% uses the Breeze caution asset, otherwise the fill
/// level in 10% steps (Breeze `battery-NNN`), so every bucket has its own file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryGlyph {
    Caution,
    Level(u8),
}

/// Every glyph the tray and menu can show, in asset order.
pub const BATTERY_GLYPHS: [BatteryGlyph; 10] = [
    BatteryGlyph::Caution,
    BatteryGlyph::Level(20),
    BatteryGlyph::Level(30),
    BatteryGlyph::Level(40),
    BatteryGlyph::Level(50),
    BatteryGlyph::Level(60),
    BatteryGlyph::Level(70),
    BatteryGlyph::Level(80),
    BatteryGlyph::Level(90),
    BatteryGlyph::Level(100),
];

impl BatteryGlyph {
    /// Element-id suffix shared by the tray and menu templates.
    #[must_use]
    pub fn id_suffix(self) -> String {
        match self {
            Self::Caution => "caution".to_owned(),
            Self::Level(level) => format!("{level:03}"),
        }
    }

    /// Breeze asset file stem; `charging` selects the plug variant (menu only).
    #[must_use]
    pub fn asset_stem(self, charging: bool) -> String {
        let base = match self {
            Self::Caution => "battery-caution".to_owned(),
            Self::Level(level) => format!("battery-{level:03}"),
        };
        if charging {
            format!("{base}-charging")
        } else {
            base
        }
    }
}

/// Tray visibility: the `taskbarShowBattery` setting, service available, and a
/// battery present. With `only_when_relevant` the Unknown state is also hidden
/// (`UPower` unknown). Without it a present battery shows in every state.
/// Charging shows the charging glyph; the rest show the normal glyph.
#[must_use]
pub fn tray_visible(power: &PowerSnapshot, enabled: bool, only_when_relevant: bool) -> bool {
    enabled
        && power.available == ServiceAvailability::Available
        && power.present
        && !(only_when_relevant && power.state == PowerState::Unknown)
}

/// Battery energy flow for the menu label. Battery power only, never system
/// power; empty when the rate is unknown.
#[must_use]
pub fn rate_text(rate_mw: Option<u32>) -> String {
    rate_mw
        .map(|milliwatts| {
            format!(
                "Battery {}.{} W",
                milliwatts / 1000,
                (milliwatts % 1000) / 100
            )
        })
        .unwrap_or_default()
}

/// Bucket rule: 0..=10 is Caution; above that the level rounds UP to the next
/// 10% step, so 11..=20 -> 020, 21..=30 -> 030, ..., 91..=100 -> 100.
#[must_use]
pub fn glyph_for_percent(percent: u8) -> BatteryGlyph {
    match percent.min(100) {
        0..=10 => BatteryGlyph::Caution,
        level => BatteryGlyph::Level(level.div_ceil(10) * 10),
    }
}

#[must_use]
pub const fn state_text(state: PowerState) -> &'static str {
    match state {
        PowerState::Charging => "Charging",
        PowerState::Discharging => "Discharging",
        PowerState::Full => "Fully charged",
        PowerState::Empty | PowerState::Unknown => "Not charging",
    }
}

#[must_use]
pub fn cpu_text(cpu_percent: Option<u8>) -> String {
    match cpu_percent {
        Some(percent) => format!("CPU usage: {}%", percent.min(100)),
        None => "CPU usage: --%".to_owned(),
    }
}

/// Progress fill width in px for a track of `track_width` px.
#[must_use]
pub fn progress_width(percent: u8, track_width: f32) -> f32 {
    track_width * f32::from(percent.min(100)) / 100.0
}

/// Menu-open/close control; `expected_revision` fences the typed action.
#[must_use]
pub fn menu_action(snapshot: &SystemSnapshot, opened: bool) -> ControlRequest {
    ControlRequest::SystemAction {
        action: if opened {
            SystemAction::BatteryMenuOpened
        } else {
            SystemAction::BatteryMenuClosed
        },
        expected_revision: snapshot.revision,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn power(state: PowerState, percent: u8) -> PowerSnapshot {
        PowerSnapshot {
            available: ServiceAvailability::Available,
            present: true,
            percent,
            state,
            cpu_percent: None,
            power_rate_mw: None,
        }
    }

    #[test]
    fn tray_visible_for_every_state_when_not_only_relevant() {
        for percent in [0, 45, 79, 80, 100] {
            for state in [
                PowerState::Charging,
                PowerState::Discharging,
                PowerState::Full,
                PowerState::Empty,
                PowerState::Unknown,
            ] {
                assert!(
                    tray_visible(&power(state, percent), true, false),
                    "{state:?} {percent}"
                );
            }
        }
    }

    #[test]
    fn only_when_relevant_hides_unknown_state() {
        assert!(tray_visible(
            &power(PowerState::Discharging, 40),
            true,
            true
        ));
        assert!(!tray_visible(&power(PowerState::Unknown, 40), true, true));
    }

    #[test]
    fn tray_hidden_when_setting_off_battery_missing_or_service_unavailable() {
        assert!(!tray_visible(
            &power(PowerState::Discharging, 40),
            false,
            false
        ));
        let mut absent = power(PowerState::Discharging, 40);
        absent.present = false;
        assert!(!tray_visible(&absent, true, false));
        let mut unavailable = power(PowerState::Discharging, 40);
        unavailable.available = ServiceAvailability::Unavailable;
        assert!(!tray_visible(&unavailable, true, false));
    }

    #[test]
    fn rate_text_is_battery_power_with_one_decimal_or_empty() {
        assert_eq!(rate_text(Some(12_345)), "Battery 12.3 W");
        assert_eq!(rate_text(Some(800)), "Battery 0.8 W");
        assert_eq!(rate_text(None), "");
    }

    #[test]
    fn glyph_buckets_round_up_to_ten_percent_steps() {
        assert_eq!(glyph_for_percent(0), BatteryGlyph::Caution);
        assert_eq!(glyph_for_percent(5), BatteryGlyph::Caution);
        assert_eq!(glyph_for_percent(10), BatteryGlyph::Caution);
        assert_eq!(glyph_for_percent(11), BatteryGlyph::Level(20));
        assert_eq!(glyph_for_percent(15), BatteryGlyph::Level(20));
        assert_eq!(glyph_for_percent(20), BatteryGlyph::Level(20));
        assert_eq!(glyph_for_percent(21), BatteryGlyph::Level(30));
        assert_eq!(glyph_for_percent(45), BatteryGlyph::Level(50));
        assert_eq!(glyph_for_percent(79), BatteryGlyph::Level(80));
        assert_eq!(glyph_for_percent(80), BatteryGlyph::Level(80));
        assert_eq!(glyph_for_percent(91), BatteryGlyph::Level(100));
        assert_eq!(glyph_for_percent(100), BatteryGlyph::Level(100));
        assert_eq!(glyph_for_percent(250), BatteryGlyph::Level(100));
    }

    #[test]
    fn every_bucket_has_panel_node_and_distinct_asset() {
        let panel = include_str!("../../../../../ui/shell/panel.html");
        let assets = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/web/breeze/");
        let mut seen: Vec<Vec<u8>> = Vec::new();
        for glyph in BATTERY_GLYPHS {
            let node = format!("id=\"tray-battery-{}\"", glyph.id_suffix());
            assert!(panel.contains(&node), "panel.html lacks {node}");
            let charging_node = format!("id=\"tray-battery-{}-charging\"", glyph.id_suffix());
            assert!(
                panel.contains(&charging_node),
                "panel.html lacks {charging_node}"
            );
            let stem = glyph.asset_stem(false);
            let bytes = std::fs::read(format!("{assets}{stem}.svg"))
                .unwrap_or_else(|error| panic!("{stem}.svg missing: {error}"));
            std::fs::read(format!("{assets}{}.svg", glyph.asset_stem(true)))
                .unwrap_or_else(|error| panic!("{stem} charging missing: {error}"));
            assert!(
                !seen.contains(&bytes),
                "{stem} repeats another bucket's asset"
            );
            seen.push(bytes);
        }
        assert!(
            !panel.contains("tray-battery-low"),
            "retired low node still in panel.html"
        );
    }

    #[test]
    fn every_glyph_names_a_breeze_asset_and_id() {
        assert_eq!(BatteryGlyph::Level(40).asset_stem(false), "battery-040");
        assert_eq!(
            BatteryGlyph::Level(40).asset_stem(true),
            "battery-040-charging"
        );
        assert_eq!(BatteryGlyph::Caution.asset_stem(false), "battery-caution");
        assert_eq!(BatteryGlyph::Level(100).id_suffix(), "100");
        for glyph in BATTERY_GLYPHS {
            assert!(glyph.asset_stem(true).starts_with("battery-"));
        }
    }

    #[test]
    fn state_and_cpu_text_match_menu_copy() {
        assert_eq!(state_text(PowerState::Charging), "Charging");
        assert_eq!(state_text(PowerState::Discharging), "Discharging");
        assert_eq!(state_text(PowerState::Full), "Fully charged");
        assert_eq!(state_text(PowerState::Empty), "Not charging");
        assert_eq!(cpu_text(Some(7)), "CPU usage: 7%");
        assert_eq!(cpu_text(None), "CPU usage: --%");
    }

    #[test]
    fn progress_width_scales_percent_into_track() {
        assert_eq!(progress_width(0, 200.0), 0.0);
        assert_eq!(progress_width(50, 200.0), 100.0);
        assert_eq!(progress_width(150, 200.0), 200.0);
    }

    #[test]
    fn menu_action_sends_typed_open_and_close_with_revision() {
        let snapshot = SystemSnapshot {
            revision: 9,
            ..Default::default()
        };
        assert_eq!(
            menu_action(&snapshot, true),
            ControlRequest::SystemAction {
                action: SystemAction::BatteryMenuOpened,
                expected_revision: 9,
            }
        );
        assert_eq!(
            menu_action(&snapshot, false),
            ControlRequest::SystemAction {
                action: SystemAction::BatteryMenuClosed,
                expected_revision: 9,
            }
        );
    }
}

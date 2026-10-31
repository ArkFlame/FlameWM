use super::*;

fn taskbar_visibility_snapshot(key: &str, value: Option<bool>) -> SettingsSnapshot {
    let mut snapshot = SettingsSnapshot::new(1);
    if let Some(value) = value {
        snapshot
            .values
            .insert(key.to_owned(), SettingValue::Boolean(value));
    }
    snapshot
}

#[test]
fn taskbar_visibility_twins_show_exactly_one_side_for_missing_default_and_explicit_values() {
    for (key, on_id, off_id, default) in [
        (
            "taskbarShowWeather",
            "taskbar-weather-on",
            "taskbar-weather-off",
            true,
        ),
        (
            "taskbarShowVolume",
            "taskbar-volume-on",
            "taskbar-volume-off",
            true,
        ),
        (
            "taskbarShowNetwork",
            "taskbar-network-on",
            "taskbar-network-off",
            true,
        ),
        (
            "taskbarShowBattery",
            "taskbar-battery-on",
            "taskbar-battery-off",
            true,
        ),
        (
            "taskbarBatteryOnlyWhenRelevant",
            "taskbar-battery-relevant-on",
            "taskbar-battery-relevant-off",
            false,
        ),
        (
            "taskbarShowClock",
            "taskbar-clock-on",
            "taskbar-clock-off",
            true,
        ),
        (
            "taskbarShowWorkspaces",
            "taskbar-workspaces-on",
            "taskbar-workspaces-off",
            true,
        ),
        (
            "weatherEnabled",
            "weather-enabled-on",
            "weather-enabled-off",
            true,
        ),
        (
            "autoLocationEnabled",
            "weather-auto-location-on",
            "weather-auto-location-off",
            false,
        ),
        (
            "autoTimeZoneEnabled",
            "time-auto-zone-on",
            "time-auto-zone-off",
            false,
        ),
    ] {
        for (value, expected_on) in [
            (None, default),
            (Some(default), default),
            (Some(!default), !default),
        ] {
            let mut transport = RecordingTransport::default();
            transport.responses.push_back(Ok(ControlResponse::Settings(
                taskbar_visibility_snapshot(key, value),
            )));
            let application = settings_application(transport);
            let mut document = ProbeDocument::default();
            application.sync(&mut document).expect("sync");
            assert_eq!(
                document.visible.get(on_id),
                Some(&expected_on),
                "{key} {value:?} on"
            );
            assert_eq!(
                document.visible.get(off_id),
                Some(&!expected_on),
                "{key} {value:?} off"
            );
        }
    }
}

#[test]
fn visible_settings_buttons_cannot_be_actionless() {
    let html = include_str!("../../../../../ui/settings.html");
    let orphaned = html
        .split("<button")
        .skip(1)
        .filter_map(|button| button.split("</button>").next())
        .filter(|button| !button.contains("data-action=\""))
        .collect::<Vec<_>>();
    assert!(
        orphaned.is_empty(),
        "RED admission: visible settings controls have no action: {orphaned:?}"
    );
}

#[test]
fn opacity_stepper_controls_are_admitted_before_controller_repair() {
    let desktop = include_str!("../../../../../ui/settings.html")
        .split("id=\"page-desktop\"")
        .nth(1)
        .and_then(|page| page.split("</section>").next())
        .expect("desktop page exists");
    let opacity = desktop
        .split("Selection rectangle opacity")
        .nth(1)
        .expect("selection opacity control exists");
    assert!(
        opacity.contains("data-action=\""),
        "RED admission: current source has no opacity stepper action; exact action names remain unsupported by source"
    );
}

#[test]
fn taskbar_and_screensaver_action_ids_parse_to_typed_writes_and_reject_off_list_values() {
    for (action, ok) in [
        ("settings.taskbar.style.floating", true),
        ("settings.taskbar.style.bar", false),
        ("settings.taskbar.clock.12h", true),
        ("settings.taskbar.clock.13h", false),
        ("settings.cursor.size.48", true),
        ("settings.cursor.size.40", false),
        ("settings.wallpaper.fit", true),
        ("settings.screensaver.mode.none", true),
        ("settings.screensaver.mode.off", false),
        ("settings.surface-blur.up", true),
        ("settings.surface-blur.sideways", false),
        ("settings.taskbar-floating-margin.up", true),
        ("settings.taskbar-floating-radius.down", true),
        ("settings.screensaver-after-lock.up", true),
        ("settings.screensaver-ribbons.down", true),
        ("settings.screensaver-ribbon-width.up", true),
        ("settings.screensaver-display-off.down", true),
        ("settings.lock-bg-blur.up", true),
        ("settings.lock-bg-dim.down", true),
        ("settings.lock-bg-dim.sideways", false),
        ("settings.screensaver-ribbons.sideways", false),
        ("settings.toggle.taskbarShowSeconds", true),
        ("settings.toggle.taskbarShowDate", true),
        ("settings.toggle.taskbarShowWeather", true),
        ("settings.toggle.weatherEnabled", true),
        ("settings.toggle.autoLocationEnabled", true),
        ("settings.toggle.autoTimeZoneEnabled", true),
        ("settings.weather.units.celsius", true),
        ("settings.weather.units.fahrenheit", true),
        ("settings.weather.units.kelvin", false),
        ("settings.weather.location.clear", true),
        ("settings.toggle.taskbarShowVolume", true),
        ("settings.toggle.taskbarShowNetwork", true),
        ("settings.toggle.taskbarShowBattery", true),
        ("settings.toggle.taskbarBatteryOnlyWhenRelevant", true),
        ("settings.toggle.taskbarShowClock", true),
        ("settings.toggle.taskbarShowWorkspaces", true),
        ("settings.toggle.taskbarShowUnknown", false),
        ("settings.toggle.snapPreviewEnabled", true),
        ("settings.toggle.focusFollowsMouse", true),
        ("settings.toggle.titlebars", true),
        ("settings.toggle.titlebar", false),
        ("settings.toggle.taskbarActivities", true),
        ("settings.toggle.taskbarSeparate", true),
        ("settings.toggle.taskbarActivity", false),
    ] {
        assert_eq!(crate::parse_action(action).is_ok(), ok, "{action}");
    }
}

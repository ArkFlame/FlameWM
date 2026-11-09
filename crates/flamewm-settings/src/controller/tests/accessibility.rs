use super::*;

fn accessibility_snapshot(revision: u64, key: &str, value: SettingValue) -> SettingsSnapshot {
    let mut snapshot = SettingsSnapshot::new(revision);
    snapshot.values.insert(key.to_owned(), value);
    snapshot
}

#[test]
fn sidebar_row_opens_the_accessibility_page_only() {
    let (_requests, mut transport) = recording_transport();
    transport
        .responses
        .push_back(Ok(ControlResponse::Settings(SettingsSnapshot::new(1))));
    let mut application = settings_application(transport);
    let mut document = ProbeDocument::default();
    application
        .handle_action(&release("settings.page.accessibility"), &mut document)
        .expect("open accessibility page");
    application.sync(&mut document).expect("sync page");
    assert!(document.visible["page-accessibility"]);
    assert!(!document.visible["page-fonts"]);
    assert!(!document.visible["page-hotkeys"]);
    assert_eq!(application.nav_index(), 11);
}

#[test]
fn sidebar_row_opens_the_dock_page_only() {
    let (_requests, mut transport) = recording_transport();
    transport
        .responses
        .push_back(Ok(ControlResponse::Settings(SettingsSnapshot::new(1))));
    let mut application = settings_application(transport);
    let mut document = ProbeDocument::default();
    application
        .handle_action(&release("settings.page.dock"), &mut document)
        .expect("open dock page");
    application.sync(&mut document).expect("sync page");
    assert!(document.visible["page-dock"]);
    assert!(!document.visible["page-taskbar"]);
    assert!(!document.visible["page-displays"]);
}

#[test]
fn accessibility_toggles_write_booleans() {
    for key in [
        "highContrast",
        "reducedMotion",
        "stickyKeys",
        "slowKeys",
        "shakeToLocate",
    ] {
        let (requests, mut transport) = recording_transport();
        transport.responses.extend([
            Ok(ControlResponse::Settings(accessibility_snapshot(
                1,
                key,
                SettingValue::Boolean(false),
            ))),
            Ok(ControlResponse::Settings(accessibility_snapshot(
                2,
                key,
                SettingValue::Boolean(true),
            ))),
        ]);
        let mut application = settings_application(transport);
        let mut document = ProbeDocument::default();
        application
            .handle_action(&release(&format!("settings.toggle.{key}")), &mut document)
            .expect("toggle accessibility key");
        assert_eq!(
            written_values(&requests, key),
            vec![SettingValue::Boolean(true)],
            "{key}"
        );
    }
}

#[test]
fn text_scale_steps_write_integer_five_percent_steps_within_100_to_200() {
    for (start, action, expected, label) in [
        (100, "settings.text-scale.up", 105, "105%"),
        (105, "settings.text-scale.down", 100, "100%"),
        (100, "settings.text-scale.down", 100, "100%"),
        (198, "settings.text-scale.up", 200, "200%"),
    ] {
        let (requests, mut transport) = recording_transport();
        transport.responses.extend([
            Ok(ControlResponse::Settings(accessibility_snapshot(
                1,
                "textScalePercent",
                SettingValue::Integer(start),
            ))),
            Ok(ControlResponse::Settings(accessibility_snapshot(
                2,
                "textScalePercent",
                SettingValue::Integer(expected),
            ))),
        ]);
        let mut application = settings_application(transport);
        let mut document = ProbeDocument::default();
        application
            .handle_action(&release(action), &mut document)
            .expect("text scale step");
        let written = written_values(&requests, "textScalePercent");
        assert_eq!(
            written,
            vec![SettingValue::Integer(expected)],
            "{action} from {start}"
        );
        application.sync(&mut document).expect("sync label");
        assert_eq!(document.texts["text-scale-value"], label, "{action} label");
    }
}

#[test]
fn night_light_schedule_selects_canonical_mode_and_hides_manual_times_for_sunset() {
    for (initial, target) in [("manual", "sunset"), ("sunset", "manual")] {
        let (requests, mut transport) = recording_transport();
        let snapshot = |revision, mode: &str| {
            let mut snapshot = accessibility_snapshot(
                revision,
                "nightShiftMode",
                SettingValue::Text(mode.to_owned()),
            );
            snapshot
                .values
                .insert("nightShift".into(), SettingValue::Boolean(true));
            snapshot
                .values
                .insert("nightShiftSchedule".into(), SettingValue::Boolean(true));
            snapshot
        };
        transport.responses.extend([
            Ok(ControlResponse::Settings(snapshot(1, initial))),
            Ok(ControlResponse::Settings(snapshot(2, target))),
        ]);
        let mut application = settings_application(transport);
        let mut document = ProbeDocument::default();
        application
            .handle_action(
                &release(&format!("settings.night-shift.mode.{target}")),
                &mut document,
            )
            .expect("select schedule");
        application.sync(&mut document).expect("sync schedule");
        assert_eq!(
            written_values(&requests, "nightShiftMode"),
            vec![SettingValue::Text(target.to_owned())]
        );
        assert!(document.visible["night-shift-mode-row"]);
        assert_eq!(
            document.visible["night-shift-start-row"],
            target == "manual"
        );
        assert_eq!(document.visible["night-shift-end-row"], target == "manual");
        assert!(document.highlighted[&format!("night-shift-mode-{target}")]);
    }
}

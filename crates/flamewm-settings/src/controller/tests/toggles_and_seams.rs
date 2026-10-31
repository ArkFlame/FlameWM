use super::*;

#[test]
fn focus_follows_mouse_toggle_flips_its_boolean_and_moves_the_highlight() {
    let (requests, mut transport) = recording_transport();
    transport.responses.extend([
        Ok(ControlResponse::Settings(lock_snapshot(
            1,
            "focusFollowsMouse",
            SettingValue::Boolean(false),
        ))),
        Ok(ControlResponse::Settings(lock_snapshot(
            2,
            "focusFollowsMouse",
            SettingValue::Boolean(true),
        ))),
    ]);
    let mut application = settings_application(transport);
    let mut document = ProbeDocument::default();
    application
        .sync(&mut document)
        .expect("sync initial snapshot");
    assert!(document.visible["focus-mouse-off"]);
    assert!(!document.visible["focus-mouse-on"]);

    application
        .handle_action(&release("settings.toggle.focusFollowsMouse"), &mut document)
        .expect("toggle focus follows mouse");

    assert!(document.visible["focus-mouse-on"]);
    assert!(!document.visible["focus-mouse-off"]);
    let written = written_values(&requests, "focusFollowsMouse");
    assert_eq!(written, vec![SettingValue::Boolean(true)]);
}

#[test]
fn titlebar_and_taskbar_toggles_flip_their_boolean_and_move_the_highlight() {
    for (key, on_id, off_id) in [
        ("titlebars", "titlebars-on", "titlebars-off"),
        (
            "taskbarActivities",
            "taskbar-activities-on",
            "taskbar-activities-off",
        ),
        (
            "taskbarSeparate",
            "taskbar-separate-on",
            "taskbar-separate-off",
        ),
    ] {
        let (requests, mut transport) = recording_transport();
        transport.responses.extend([
            Ok(ControlResponse::Settings(lock_snapshot(
                1,
                key,
                SettingValue::Boolean(false),
            ))),
            Ok(ControlResponse::Settings(lock_snapshot(
                2,
                key,
                SettingValue::Boolean(true),
            ))),
        ]);
        let mut application = settings_application(transport);
        let mut document = ProbeDocument::default();
        application
            .sync(&mut document)
            .expect("sync initial snapshot");
        assert!(document.visible[off_id], "{key} starts off");
        assert!(!document.visible[on_id], "{key} starts off");

        application
            .handle_action(&release(&format!("settings.toggle.{key}")), &mut document)
            .expect("toggle");

        assert!(document.visible[on_id], "{key} highlight moves to ON");
        assert!(!document.visible[off_id], "{key} highlight moves to ON");
        let written = written_values(&requests, key);
        assert_eq!(
            written,
            vec![SettingValue::Boolean(true)],
            "{key} writes the flipped boolean"
        );
    }
}

#[test]
fn opacity_clicks_use_one_apply_settings_transaction_at_each_step_boundary() {
    let (requests, mut transport) = recording_transport();
    for revision in 1..=3 {
        transport
            .responses
            .push_back(Ok(ControlResponse::Settings(SettingsSnapshot::new(
                revision,
            ))));
    }
    let mut client = SettingsClient::new(transport);
    for value in [0, 5, 60] {
        client
            .apply_setting("DesktopSelectionFillOpacity", SettingValue::Percent(value))
            .expect("one opacity click applies");
    }
    let requests = requests.borrow();
    let applies = requests
        .iter()
        .filter_map(|request| match request {
            ControlRequest::ApplySettings(transaction) => Some(transaction),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(applies.len(), 3, "one ApplySettings transaction per click");
    assert_eq!(
        applies
            .iter()
            .map(|transaction| &transaction.changes[0].value)
            .collect::<Vec<_>>(),
        vec![
            &Some(SettingValue::Percent(0)),
            &Some(SettingValue::Percent(5)),
            &Some(SettingValue::Percent(60)),
        ]
    );
}

#[test]
fn display_controller_seam_selects_only_authoritative_outputs() {
    let (_, mut transport) = recording_transport();
    transport
        .responses
        .push_back(Ok(ControlResponse::Displays(display_snapshot())));
    let mut application = displays_application(transport);
    application
        .handle_action(
            &release("settings.display.select.0"),
            &mut ProbeDocument::default(),
        )
        .expect("display select");
    assert_eq!(
        application.client().ui().selected_output.as_ref(),
        Some(&OutputId::new("HDMI-1"))
    );
}

#[test]
fn font_controller_seam_submits_one_authoritative_change() {
    let (requests, mut transport) = recording_transport();
    let mut initial = SettingsSnapshot::new(4);
    initial
        .values
        .insert("fontBold".to_owned(), SettingValue::Boolean(false));
    let mut updated = SettingsSnapshot::new(5);
    updated
        .values
        .insert("fontBold".to_owned(), SettingValue::Boolean(true));
    transport.responses.extend([
        Ok(ControlResponse::Settings(initial)),
        Ok(ControlResponse::Settings(updated)),
    ]);
    let mut application = settings_application(transport);
    application
        .handle_action(
            &release("settings.toggle.fontBold"),
            &mut ProbeDocument::default(),
        )
        .expect("font toggle");
    let requests = requests.borrow();
    assert!(matches!(
        requests.get(1),
        Some(ControlRequest::ApplySettings(transaction))
            if transaction.changes == vec![flamewm_api::settings::SettingsChange {
                key: "fontBold".to_owned(),
                value: Some(SettingValue::Boolean(true)),
            }]
    ));
}

use super::*;

#[test]
fn picker_palette_cells_write_their_exact_hex_values() {
    let (mut application, requests) = color_session("#191B1D", 2);
    let mut document = ProbeDocument::default();
    application
        .handle_action(&release("settings.picker.open.surface"), &mut document)
        .expect("open picker");
    assert_eq!(document.visible.get("color-picker"), Some(&true));
    // Hue 0 (red) at the darkest shade is #3E0909; greys step from black to white.
    application
        .handle_action(&release("settings.picker.pick.0.5"), &mut document)
        .expect("red cell");
    assert_eq!(
        last_apply(&requests, "surfaceColor").as_deref(),
        Some("#3E0909")
    );
    application
        .handle_action(&release("settings.picker.pick.grey.5"), &mut document)
        .expect("white cell");
    assert_eq!(
        last_apply(&requests, "surfaceColor").as_deref(),
        Some("#FFFFFF")
    );
}

#[test]
fn picker_typed_hex_applies_on_enter_and_invalid_hex_writes_nothing() {
    let (mut application, requests) = color_session("#191B1D", 1);
    let mut document = ProbeDocument::default();
    application
        .handle_action(&release("settings.picker.open.surface"), &mut document)
        .expect("open picker");
    for text in ["#", "3", "d", "A", "e", "E", "9"] {
        application
            .handle_action(&typed(text), &mut document)
            .expect("typed digit");
    }
    application
        .handle_action(&typed("\n"), &mut document)
        .expect("enter");
    assert_eq!(
        last_apply(&requests, "surfaceColor").as_deref(),
        Some("#3DAEE9")
    );

    let before = requests.borrow().len();
    application
        .handle_action(&release("settings.picker.hex.focus"), &mut document)
        .expect("focus");
    application
        .handle_action(&release("settings.picker.close"), &mut document)
        .expect("close");
    application
        .handle_action(&release("settings.picker.open.surface"), &mut document)
        .expect("reopen");
    for text in ["1", "2", "3", "4", "5"] {
        application
            .handle_action(&typed(text), &mut document)
            .expect("typed digit");
    }
    application
        .handle_action(&typed("\n"), &mut document)
        .expect("enter invalid");
    assert_eq!(
        requests.borrow().len(),
        before,
        "invalid #RRGGBB writes nothing"
    );
    assert!(
        document
            .texts
            .get("color-picker-error")
            .is_some_and(|text| text.contains("#RRGGBB")),
        "invalid buffer is shown inline"
    );
}

#[test]
fn picker_escape_closes_and_unfocused_keys_do_not_edit_the_buffer() {
    let (mut application, requests) = color_session("#191B1D", 0);
    let mut document = ProbeDocument::default();
    application
        .handle_action(
            &release("settings.picker.open.lockbackground"),
            &mut document,
        )
        .expect("open picker");
    // Apply with an empty buffer is inline-invalid and writes nothing, and it
    // moves focus off the hex field.
    application
        .handle_action(&release("settings.picker.apply"), &mut document)
        .expect("empty apply");
    let writes = requests.borrow().len();
    application
        .handle_action(&typed("7"), &mut document)
        .expect("unfocused digit");
    application
        .handle_action(&typed("\n"), &mut document)
        .expect("enter empty buffer");
    assert_eq!(
        requests.borrow().len(),
        writes,
        "unfocused digits are not buffered"
    );
    assert_eq!(document.visible.get("color-picker"), Some(&true));
    application
        .handle_action(&typed("\u{1b}"), &mut document)
        .expect("escape");
    assert_eq!(document.visible.get("color-picker"), Some(&false));
}

#[test]
fn accent_swatches_dispatch_only_release_inside_events_and_apply_selected_value() {
    let (requests, mut transport) = recording_transport();
    let initial = SettingsSnapshot::new(1);
    let mut blue = SettingsSnapshot::new(2);
    blue.values.insert(
        "accent".to_owned(),
        SettingValue::Text("#3DAEE9".to_owned()),
    );
    let mut teal = SettingsSnapshot::new(3);
    teal.values.insert(
        "accent".to_owned(),
        SettingValue::Text("#1ABC9C".to_owned()),
    );
    transport.responses.extend([
        Ok(ControlResponse::Settings(initial)),
        Ok(ControlResponse::Settings(blue)),
        Ok(ControlResponse::Settings(teal)),
    ]);
    let mut application = settings_application(transport);

    let mut press = release("settings.accent.blue");
    press.phase = UiActionPhase::Press;
    application
        .handle_action(&press, &mut ProbeDocument::default())
        .expect("press does not dispatch an accent mutation");
    let mut outside = release("settings.accent.teal");
    outside.inside = false;
    application
        .handle_action(&outside, &mut ProbeDocument::default())
        .expect("release outside does not dispatch an accent mutation");
    assert_eq!(
        requests.borrow().len(),
        1,
        "only the initial snapshot was requested"
    );

    application
        .handle_action(
            &release("settings.accent.blue"),
            &mut ProbeDocument::default(),
        )
        .expect("blue swatch release");
    application
        .handle_action(
            &release("settings.accent.teal"),
            &mut ProbeDocument::default(),
        )
        .expect("teal swatch release");

    let requests = requests.borrow();
    assert!(matches!(
        requests.get(1),
        Some(ControlRequest::ApplySettings(transaction))
            if transaction.expected_revision == 1
                && transaction.reset_section.is_none()
                && transaction.changes == vec![SettingsChange {
                    key: "accent".to_owned(),
                    value: Some(SettingValue::Text("#3DAEE9".to_owned())),
                }]
    ));
    assert!(matches!(
        requests.get(2),
        Some(ControlRequest::ApplySettings(transaction))
            if transaction.expected_revision == 2
                && transaction.reset_section.is_none()
                && transaction.changes == vec![SettingsChange {
                    key: "accent".to_owned(),
                    value: Some(SettingValue::Text("#1ABC9C".to_owned())),
                }]
    ));
    assert_eq!(
        application
            .client()
            .settings()
            .and_then(|snapshot| snapshot.values.get("accent")),
        Some(&SettingValue::Text("#1ABC9C".to_owned())),
        "the client must retain the authoritative response after each release"
    );
}

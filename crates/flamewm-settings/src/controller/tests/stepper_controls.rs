use super::*;

#[test]
fn blur_steppers_write_percent_steps_clamped_and_show_the_label() {
    for (action, key, label_id, start, expected) in [
        (
            "settings.surface-blur.up",
            "surfaceBlurPercent",
            "surface-blur-value",
            80,
            85,
        ),
        (
            "settings.surface-blur.down",
            "surfaceBlurPercent",
            "surface-blur-value",
            80,
            75,
        ),
        (
            "settings.surface-blur.up",
            "surfaceBlurPercent",
            "surface-blur-value",
            98,
            100,
        ),
        (
            "settings.surface-opacity.down",
            "surfaceOpacity",
            "surface-opacity-value",
            2,
            0,
        ),
    ] {
        let (requests, mut transport) = recording_transport();
        let mut before = SettingsSnapshot::new(1);
        before
            .values
            .insert(key.to_owned(), SettingValue::Percent(start));
        let mut after = SettingsSnapshot::new(2);
        after
            .values
            .insert(key.to_owned(), SettingValue::Percent(expected));
        transport.responses.extend([
            Ok(ControlResponse::Settings(before)),
            Ok(ControlResponse::Settings(after)),
        ]);
        let mut application = settings_application(transport);
        let mut document = ProbeDocument::default();

        application
            .handle_action(&release(action), &mut document)
            .expect("blur step");

        let written = written_values(&requests, key);
        assert_eq!(written, vec![SettingValue::Percent(expected)], "{action}");
        assert_eq!(
            document.texts[label_id],
            format!("{expected}%"),
            "{action} label"
        );
    }
}

#[test]
fn avatar_clear_writes_empty_default_and_never_touches_background() {
    let (requests, mut transport) = recording_transport();
    transport.responses.extend([
        Ok(ControlResponse::Settings(lock_snapshot(
            1,
            "avatarImage",
            SettingValue::Text("/home/u/avatar-0123456789abcdef.png".to_owned()),
        ))),
        Ok(ControlResponse::Settings(lock_snapshot(
            2,
            "avatarImage",
            SettingValue::Text(String::new()),
        ))),
    ]);
    let mut application = settings_application(transport);
    let mut document = ProbeDocument::default();

    application
        .handle_action(&release("settings.avatar.clear"), &mut document)
        .expect("avatar clear");

    assert_eq!(
        applied_writes(&requests),
        vec![("avatarImage".to_owned(), SettingValue::Text(String::new()))]
    );
}

#[test]
fn lock_background_steppers_clamp_at_their_maximums() {
    for (action, key, start, expected) in [
        (
            "settings.lock-bg-blur.up",
            "lockBackgroundBlurPercent",
            98,
            100,
        ),
        (
            "settings.lock-bg-dim.up",
            "lockBackgroundDimPercent",
            78,
            80,
        ),
    ] {
        let (requests, mut transport) = recording_transport();
        transport.responses.extend([
            Ok(ControlResponse::Settings(lock_snapshot(
                1,
                key,
                SettingValue::Integer(start),
            ))),
            Ok(ControlResponse::Settings(lock_snapshot(
                2,
                key,
                SettingValue::Integer(expected),
            ))),
        ]);
        let mut application = settings_application(transport);
        let mut document = ProbeDocument::default();

        application
            .handle_action(&release(action), &mut document)
            .expect("lock background step");

        let written = written_values(&requests, key);
        assert_eq!(
            written,
            vec![SettingValue::Integer(expected)],
            "{action} clamps, never exceeds maximum"
        );
    }
}

#[test]
fn floating_margin_stepper_clamps_at_its_maximum_and_writes_an_integer() {
    let (requests, mut transport) = recording_transport();
    transport.responses.extend([
        Ok(ControlResponse::Settings(lock_snapshot(
            1,
            "taskbarFloatingMargin",
            SettingValue::Integer(32),
        ))),
        Ok(ControlResponse::Settings(lock_snapshot(
            2,
            "taskbarFloatingMargin",
            SettingValue::Integer(32),
        ))),
    ]);
    let mut application = settings_application(transport);
    let mut document = ProbeDocument::default();

    application
        .handle_action(
            &release("settings.taskbar-floating-margin.up"),
            &mut document,
        )
        .expect("margin up at maximum");

    let written = written_values(&requests, "taskbarFloatingMargin");
    assert_eq!(
        written,
        vec![SettingValue::Integer(32)],
        "32 is the maximum, never 33"
    );
    assert_eq!(document.texts["taskbar-floating-margin-value"], "32");
}

#[test]
fn ribbon_width_stepper_moves_by_half_a_pixel_in_tenths() {
    let (requests, mut transport) = recording_transport();
    transport.responses.extend([
        Ok(ControlResponse::Settings(lock_snapshot(
            1,
            "screensaverRibbonWidthTenths",
            SettingValue::Integer(110),
        ))),
        Ok(ControlResponse::Settings(lock_snapshot(
            2,
            "screensaverRibbonWidthTenths",
            SettingValue::Integer(105),
        ))),
        Ok(ControlResponse::Settings(lock_snapshot(
            3,
            "screensaverRibbonWidthTenths",
            SettingValue::Integer(110),
        ))),
    ]);
    let mut application = settings_application(transport);
    let mut document = ProbeDocument::default();

    application
        .handle_action(
            &release("settings.screensaver-ribbon-width.down"),
            &mut document,
        )
        .expect("ribbon width down");
    assert_eq!(document.texts["screensaver-ribbon-width-value"], "10.5 px");
    application
        .handle_action(
            &release("settings.screensaver-ribbon-width.up"),
            &mut document,
        )
        .expect("ribbon width up");

    let written = written_values(&requests, "screensaverRibbonWidthTenths");
    assert_eq!(
        written,
        vec![SettingValue::Integer(105), SettingValue::Integer(110)],
        "each click moves the width by exactly 5 tenths",
    );
}

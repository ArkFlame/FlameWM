use super::*;

#[test]
fn external_decoration_change_moves_the_highlight_without_a_write() {
    let (requests, mut transport) = recording_transport();
    transport.responses.extend([
        Ok(ControlResponse::Settings(decoration_snapshot(1, "rounded"))),
        Ok(ControlResponse::Settings(decoration_snapshot(2, "squared"))),
    ]);
    let mut application = SettingsApplication::new(transport);
    let mut document = ProbeDocument::default();
    application
        .client_mut()
        .refresh_settings()
        .expect("initial snapshot");
    application
        .sync(&mut document)
        .expect("sync initial snapshot");
    assert!(document.highlighted["decoration-rounded"]);
    application
        .client_mut()
        .refresh_settings()
        .expect("external snapshot");
    application
        .sync(&mut document)
        .expect("sync external snapshot");
    assert!(
        document.highlighted["decoration-squared"],
        "external squared snapshot moves the highlight"
    );
    assert!(!document.highlighted["decoration-rounded"]);
    assert!(
        requests
            .borrow()
            .iter()
            .all(|request| !matches!(request, ControlRequest::ApplySettings(_))),
        "external sync must not write"
    );
}

fn animation_snapshot(revision: u64, key: &str, value: &str) -> SettingsSnapshot {
    let mut snapshot = SettingsSnapshot::new(revision);
    snapshot
        .values
        .insert(key.to_owned(), SettingValue::Text(value.to_owned()));
    snapshot
}

fn animations_enabled_snapshot(revision: u64, enabled: bool) -> SettingsSnapshot {
    let mut snapshot = SettingsSnapshot::new(revision);
    snapshot.values.insert(
        "animationsEnabled".to_owned(),
        SettingValue::Boolean(enabled),
    );
    snapshot
}

#[test]
fn global_animation_switch_hides_subordinate_settings_but_stays_visible() {
    let (requests, mut transport) = recording_transport();
    transport.responses.extend([
        Ok(ControlResponse::Settings(animations_enabled_snapshot(
            1, false,
        ))),
        Ok(ControlResponse::Settings(animations_enabled_snapshot(
            2, true,
        ))),
    ]);
    let mut application = settings_application(transport);
    let mut document = ProbeDocument::default();
    let subordinate = [
        "animation-speed-setting",
        "animation-start-settings",
        "animation-window-settings",
        "animation-minmax-settings",
    ];

    application
        .sync(&mut document)
        .expect("sync animations disabled");
    assert!(document.visible["anim-global-off"]);
    assert!(!document.visible["anim-global-on"]);
    for id in subordinate {
        assert!(
            !document.visible[id],
            "{id} is hidden while animations are off"
        );
    }

    application
        .handle_action(&release("settings.toggle.animationsEnabled"), &mut document)
        .expect("enable animations");
    assert!(document.visible["anim-global-on"]);
    assert!(!document.visible["anim-global-off"]);
    for id in subordinate {
        assert!(document.visible[id], "{id} returns when animations are on");
    }
    assert_eq!(
        written_values(&requests, "animationsEnabled"),
        vec![SettingValue::Boolean(true)]
    );
}

#[test]
fn animation_style_segments_write_exact_values_and_highlight_the_snapshot() {
    let (requests, mut transport) = recording_transport();
    let mut revision = 1;
    transport
        .responses
        .push_back(Ok(ControlResponse::Settings(SettingsSnapshot::new(
            revision,
        ))));
    let mut writes = Vec::new();
    for (key, values) in crate::ANIMATION_STYLE_ROWS {
        for value in values {
            revision += 1;
            transport
                .responses
                .push_back(Ok(ControlResponse::Settings(animation_snapshot(
                    revision, key, value,
                ))));
            writes.push((key, *value));
        }
    }
    revision += 1;
    transport
        .responses
        .push_back(Ok(ControlResponse::Settings(animation_snapshot(
            revision,
            "startAnimation",
            "fade",
        ))));
    let mut application = settings_application(transport);
    let mut document = ProbeDocument::default();

    for (key, value) in &writes {
        application
            .handle_action(&release(&format!("settings.{key}.{value}")), &mut document)
            .expect("animation style option");
        let (_, values) = crate::ANIMATION_STYLE_ROWS
            .iter()
            .find(|(row, _)| row == key)
            .expect("animation row");
        for option in *values {
            assert_eq!(
                document.highlighted[&crate::animation_style_node(key, option)],
                option == value,
                "{key}.{option} highlight after writing {value}"
            );
        }
    }

    let written = applied_writes(&requests);
    let expected: Vec<(String, SettingValue)> = writes
        .iter()
        .map(|(key, value)| ((*key).to_owned(), SettingValue::Text((*value).to_owned())))
        .collect();
    assert_eq!(
        written, expected,
        "each segment writes its exact key and value"
    );

    application
        .client_mut()
        .refresh_settings()
        .expect("external snapshot");
    application
        .sync(&mut document)
        .expect("sync external snapshot");
    assert!(document.highlighted[&crate::animation_style_node("startAnimation", "fade")]);
    assert!(!document.highlighted[&crate::animation_style_node("startAnimation", "none")]);
}

#[test]
fn lock_timeout_segments_write_exact_minutes_and_highlight_the_snapshot() {
    let (requests, mut transport) = recording_transport();
    transport
        .responses
        .push_back(Ok(ControlResponse::Settings(SettingsSnapshot::new(1))));
    let minutes_options = [0_i64, 5, 10, 15, 30];
    for (index, minutes) in minutes_options.iter().enumerate() {
        transport
            .responses
            .push_back(Ok(ControlResponse::Settings(lock_snapshot(
                index as u64 + 2,
                "lockTimeoutMinutes",
                SettingValue::Integer(*minutes),
            ))));
    }
    let mut application = settings_application(transport);
    let mut document = ProbeDocument::default();

    for minutes in minutes_options {
        application
            .handle_action(
                &release(&format!("settings.lock.timeout.{minutes}")),
                &mut document,
            )
            .expect("lock timeout option");
        for option in minutes_options {
            assert_eq!(
                document.highlighted[&format!("lock-timeout-{option}")],
                option == minutes,
                "lock-timeout-{option} highlight after writing {minutes}"
            );
        }
    }

    let written = written_values(&requests, "lockTimeoutMinutes");
    let expected: Vec<SettingValue> = minutes_options
        .iter()
        .map(|minutes| SettingValue::Integer(*minutes))
        .collect();
    assert_eq!(
        written, expected,
        "each timeout segment writes its exact minutes"
    );
}

#[test]
fn lock_action_ids_parse_to_typed_writes_and_reject_off_list_values() {
    for (action, ok) in [
        ("settings.lock.timeout.0", true),
        ("settings.lock.timeout.30", true),
        ("settings.lock.timeout.7", false),
        ("settings.lock.timeout.-5", false),
        ("settings.lock.clock.12h", true),
        ("settings.lock.clock.13h", false),
        ("settings.lock.background.choose", true),
        ("settings.lock.background.clear", true),
        ("settings.lock.background.default", true),
        ("settings.wallpaper.default", true),
        ("settings.wallpaper.clear", true),
        ("settings.wallpaper.reset", false),
        ("settings.avatar.choose", true),
        ("settings.avatar.clear", true),
        ("settings.start-icon.choose", true),
        ("settings.start-icon.clear", true),
        ("settings.toggle.lockShowDate", true),
        ("settings.toggle.lockShowUsername", true),
        ("settings.toggle.lockOnSuspend", false),
        ("settings.color.lockbackground.preset.0", true),
        ("settings.color.lockbackground.swatch.0", false),
    ] {
        assert_eq!(crate::parse_action(action).is_ok(), ok, "{action}");
    }
}

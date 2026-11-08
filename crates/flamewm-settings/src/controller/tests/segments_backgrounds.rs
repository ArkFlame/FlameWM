use super::*;

#[test]
fn last_taskbar_or_dock_cannot_be_disabled() {
    for (key, other) in [
        ("taskbar.enabled", "dock.enabled"),
        ("dock.enabled", "taskbar.enabled"),
    ] {
        let (requests, mut transport) = recording_transport();
        let mut snapshot = lock_snapshot(1, key, SettingValue::Boolean(true));
        snapshot
            .values
            .insert(other.to_owned(), SettingValue::Boolean(false));
        transport
            .responses
            .push_back(Ok(ControlResponse::Settings(snapshot)));
        let mut application = settings_application(transport);
        let error = application
            .handle_action(
                &release(&format!("settings.toggle.{key}")),
                &mut ProbeDocument::default(),
            )
            .expect_err("the last navigation surface must stay enabled");
        assert!(error.contains("At least one"), "{error}");
        assert!(written_values(&requests, key).is_empty());
    }
}

#[test]
fn taskbar_style_segment_writes_floating_and_shows_only_its_floating_steppers() {
    let (requests, mut transport) = recording_transport();
    transport.responses.extend([
        Ok(ControlResponse::Settings(lock_snapshot(
            1,
            "taskbarStyle",
            SettingValue::Text("classic".to_owned()),
        ))),
        Ok(ControlResponse::Settings(lock_snapshot(
            2,
            "taskbarStyle",
            SettingValue::Text("floating".to_owned()),
        ))),
    ]);
    let mut application = settings_application(transport);
    let mut document = ProbeDocument::default();
    application
        .sync(&mut document)
        .expect("sync classic snapshot");
    assert!(document.highlighted["taskbar-style-classic"]);
    assert!(
        !document.visible["taskbar-floating-margin-row"],
        "classic hides floating steppers"
    );
    assert!(!document.visible["taskbar-floating-radius-row"]);

    application
        .handle_action(&release("settings.taskbar.style.floating"), &mut document)
        .expect("floating segment");

    assert!(document.highlighted["taskbar-style-floating"]);
    assert!(!document.highlighted["taskbar-style-classic"]);
    assert!(
        document.visible["taskbar-floating-margin-row"],
        "floating shows margin"
    );
    assert!(
        document.visible["taskbar-floating-radius-row"],
        "floating shows radius"
    );
    let written = written_values(&requests, "taskbarStyle");
    assert_eq!(written, vec![SettingValue::Text("floating".to_owned())]);
}

#[test]
fn taskbar_material_selectors_keep_text_ink_in_selected_and_unselected_states() {
    let options = [
        ("blur-only", "taskbar-material-blur-only"),
        ("inherit", "taskbar-material-inherit"),
        ("custom-tint", "taskbar-material-custom-tint"),
    ];
    for (mode, expected) in appearance_text_inks() {
        for (selected, _) in options {
            let mut snapshot = SettingsSnapshot::new(1);
            snapshot
                .values
                .insert("appearance".to_owned(), SettingValue::Text(mode.to_owned()));
            snapshot.values.insert(
                "taskbarMaterialMode".to_owned(),
                SettingValue::Text(selected.to_owned()),
            );
            let (_, mut transport) = recording_transport();
            transport
                .responses
                .push_back(Ok(ControlResponse::Settings(snapshot)));
            let application = settings_application(transport);
            let mut document = ProbeDocument::default();

            application
                .sync(&mut document)
                .expect("sync taskbar material selector text tokens");

            for (value, id) in options {
                assert_eq!(
                    document.highlighted[id],
                    value == selected,
                    "{mode} {id} state"
                );
                assert_selector_inherits_theme_text(&document, id, expected);
            }
        }
    }
}

#[test]
fn cursor_size_segments_write_exact_pixels_and_highlight_the_snapshot() {
    let (requests, mut transport) = recording_transport();
    transport.responses.extend([
        Ok(ControlResponse::Settings(lock_snapshot(
            1,
            "cursorSize",
            SettingValue::Integer(32),
        ))),
        Ok(ControlResponse::Settings(lock_snapshot(
            2,
            "cursorSize",
            SettingValue::Integer(64),
        ))),
    ]);
    let mut application = settings_application(transport);
    let mut document = ProbeDocument::default();
    application
        .sync(&mut document)
        .expect("sync initial snapshot");
    assert!(document.highlighted["cursor-size-32"]);
    assert!(!document.highlighted["cursor-size-64"]);

    application
        .handle_action(&release("settings.cursor.size.64"), &mut document)
        .expect("cursor size 64");
    assert!(document.highlighted["cursor-size-64"]);
    assert!(!document.highlighted["cursor-size-32"]);

    let written = written_values(&requests, "cursorSize");
    assert_eq!(written, vec![SettingValue::Integer(64)]);
}

#[test]
fn background_mode_actions_write_distinct_modes_and_never_touch_saved_paths() {
    let (requests, mut transport) = recording_transport();
    for revision in 1..=5 {
        transport
            .responses
            .push_back(Ok(ControlResponse::Settings(lock_snapshot(
                revision,
                "wallpaperMode",
                SettingValue::Text("default".to_owned()),
            ))));
    }
    let mut application = settings_application(transport);
    let mut document = ProbeDocument::default();
    for action in [
        "settings.wallpaper.default",
        "settings.wallpaper.clear",
        "settings.lock.background.clear",
        "settings.lock.background.default",
    ] {
        application
            .handle_action(&release(action), &mut document)
            .expect("background mode action");
    }

    let written = applied_writes(&requests);
    let text = |value: &str| SettingValue::Text(value.to_owned());
    assert_eq!(
        written,
        vec![
            ("wallpaperMode".to_owned(), text("default")),
            ("wallpaperMode".to_owned(), text("solid")),
            ("lockBackgroundMode".to_owned(), text("solid")),
            ("lockBackgroundMode".to_owned(), text("default")),
        ],
        "Clear must write solid, Default must write default, and no saved path is erased"
    );
}

#[test]
fn wallpaper_fit_selector_writes_the_next_wire_value_and_labels_it() {
    let (requests, mut transport) = recording_transport();
    transport.responses.extend([
        Ok(ControlResponse::Settings(lock_snapshot(
            1,
            "wallpaperFit",
            SettingValue::Text("fill".to_owned()),
        ))),
        Ok(ControlResponse::Settings(lock_snapshot(
            2,
            "wallpaperFit",
            SettingValue::Text("fit".to_owned()),
        ))),
    ]);
    let mut application = settings_application(transport);
    let mut document = ProbeDocument::default();

    application
        .handle_action(&release("settings.wallpaper.fit"), &mut document)
        .expect("wallpaper fit cycle");

    assert_eq!(document.texts["wallpaper-fit-value"], "Fit");
    let written = written_values(&requests, "wallpaperFit");
    assert_eq!(written, vec![SettingValue::Text("fit".to_owned())]);
}

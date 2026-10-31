use super::*;

#[test]
fn decoration_option_buttons_write_exact_values_and_highlight_the_snapshot() {
    let (requests, mut transport) = recording_transport();
    transport.responses.extend([
        Ok(ControlResponse::Settings(decoration_snapshot(1, "rounded"))),
        Ok(ControlResponse::Settings(decoration_snapshot(2, "squared"))),
        Ok(ControlResponse::Settings(decoration_snapshot(3, "rounded"))),
    ]);
    let mut application = settings_application(transport);
    let mut document = ProbeDocument::default();

    application
        .handle_action(&release("settings.decoration.squared"), &mut document)
        .expect("squared option");
    assert!(
        document.highlighted["decoration-squared"],
        "squared is highlighted after its write"
    );
    assert!(
        !document.highlighted["decoration-rounded"],
        "rounded is cleared after squared write"
    );

    application
        .handle_action(&release("settings.decoration.rounded"), &mut document)
        .expect("rounded option");
    assert!(document.highlighted["decoration-rounded"]);
    assert!(!document.highlighted["decoration-squared"]);

    let writes = applied_writes(&requests);
    assert_eq!(
        writes,
        vec![
            (
                "decoration".to_owned(),
                SettingValue::Text("squared".to_owned())
            ),
            (
                "decoration".to_owned(),
                SettingValue::Text("rounded".to_owned())
            ),
        ],
    );
}

fn appearance_snapshot(revision: u64, appearance: Option<&str>) -> SettingsSnapshot {
    let mut snapshot = SettingsSnapshot::new(revision);
    if let Some(appearance) = appearance {
        snapshot.values.insert(
            "appearance".to_owned(),
            SettingValue::Text(appearance.to_owned()),
        );
    }
    snapshot
}

#[test]
fn application_appearance_buttons_write_exact_values_and_highlight_the_snapshot() {
    let (requests, mut transport) = recording_transport();
    transport.responses.extend([
        Ok(ControlResponse::Settings(appearance_snapshot(
            1,
            Some("dark"),
        ))),
        Ok(ControlResponse::Settings(appearance_snapshot(
            2,
            Some("light"),
        ))),
    ]);
    let mut application = settings_application(transport);
    let mut document = ProbeDocument::default();
    application
        .sync(&mut document)
        .expect("sync initial snapshot");
    assert!(document.highlighted["appearance-apps-dark"]);
    assert!(!document.highlighted["appearance-apps-light"]);

    application
        .handle_action(&release("settings.appearance.apps.light"), &mut document)
        .expect("light option");
    assert!(document.highlighted["appearance-apps-light"]);
    assert!(!document.highlighted["appearance-apps-dark"]);

    let writes = applied_writes(&requests);
    assert_eq!(
        writes,
        vec![(
            "appearance".to_owned(),
            SettingValue::Text("light".to_owned())
        )],
    );
}

#[test]
fn application_appearance_highlights_nothing_without_an_authoritative_value() {
    let transport = RecordingTransport {
        responses: VecDeque::from([Ok(ControlResponse::Settings(appearance_snapshot(
            1, None,
        )))]),
        ..RecordingTransport::default()
    };
    let application = settings_application(transport);
    let mut document = ProbeDocument::default();
    application
        .sync(&mut document)
        .expect("sync snapshot without appearance");
    assert!(!document.highlighted["appearance-apps-dark"]);
    assert!(!document.highlighted["appearance-apps-light"]);
}

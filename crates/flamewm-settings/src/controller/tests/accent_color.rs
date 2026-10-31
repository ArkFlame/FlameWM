use super::*;

#[test]
fn accent_reset_is_retired_and_writes_nothing() {
    let (requests, transport) = recording_transport();
    let mut application = SettingsApplication::new(transport);
    assert!(
        application
            .handle_action(
                &release("settings.accent.reset"),
                &mut ProbeDocument::default()
            )
            .is_err()
    );
    assert!(
        requests
            .borrow()
            .iter()
            .all(|request| !matches!(request, ControlRequest::ApplySettings(_))),
        "a retired accent reset must not write any setting"
    );
}

#[test]
fn surface_preset_circle_writes_its_exact_hex_in_one_transaction() {
    let (mut application, requests) = color_session("#191B1D", 1);
    application
        .handle_action(
            &release("settings.color.surface.preset.2"),
            &mut ProbeDocument::default(),
        )
        .expect("preset release");
    assert_eq!(
        requests.borrow().last(),
        Some(&ControlRequest::ApplySettings(SettingsTransaction {
            expected_revision: 7,
            changes: vec![SettingsChange {
                key: "surfaceColor".to_owned(),
                value: Some(SettingValue::Text("#3DAEE9".to_owned())),
            }],
            reset_section: None,
        })),
    );
    assert_eq!(
        requests.borrow().len(),
        2,
        "one GetSettings and one ApplySettings only"
    );
}

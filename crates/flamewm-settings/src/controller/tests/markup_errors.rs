use super::*;

#[test]
fn every_markup_action_is_handled_by_the_controller() {
    let actions = markup_actions();
    assert!(!actions.is_empty(), "settings markup declares actions");
    for action in actions {
        assert!(
            crate::parse_action(action).is_ok(),
            "orphan action in settings markup: {action}"
        );
    }
}

#[test]
fn unknown_action_is_an_inline_error_and_never_syncs_or_calls() {
    let (requests, transport) = recording_transport();
    let mut application = SettingsApplication::new(transport);
    let mut document = ProbeDocument::default();
    let result = application.handle_action(&release("settings.bogus.action"), &mut document);
    assert!(result.is_err());
    assert!(
        requests.borrow().is_empty(),
        "unknown action must not call the service"
    );
    assert!(
        document.texts.is_empty() && document.visible.is_empty(),
        "unknown action must not sync"
    );
    application.report_error(&mut document, result.unwrap_err());
    assert!(document.texts["settings-error"].contains("settings.bogus.action"));
}

#[test]
fn failed_control_request_keeps_previous_authoritative_value() {
    let (_, mut transport) = recording_transport();
    let mut initial = SettingsSnapshot::new(4);
    initial
        .values
        .insert("fontBold".to_owned(), SettingValue::Boolean(false));
    transport
        .responses
        .push_back(Ok(ControlResponse::Settings(initial)));
    transport
        .responses
        .push_back(Err(flamewm_control_core::ControlError {
            name: "com.arkflame.FlameWM1.Error.Unavailable",
            code: flamewm_api::ErrorCode::Unavailable,
            message: "service busy".to_owned(),
        }));
    let mut application = settings_application(transport);
    let mut document = ProbeDocument::default();
    let result = application.handle_action(&release("settings.toggle.fontBold"), &mut document);
    assert!(result.is_err());
    assert_eq!(
        application
            .client()
            .settings()
            .and_then(|s| s.values.get("fontBold")),
        Some(&SettingValue::Boolean(false)),
        "failed write must leave the previous authoritative value"
    );
    assert!(document.texts.is_empty(), "failed write must not sync");
    application.report_error(&mut document, result.unwrap_err());
    assert!(document.texts["settings-error"].contains("service busy"));
}

#[test]
fn empty_snapshots_are_errors_not_panics() {
    let (requests, transport) = recording_transport();
    let mut application = SettingsApplication::new(transport);
    let mut document = ProbeDocument::default();
    for action in [
        "settings.taskbar.position",
        "settings.taskbar.size",
        "settings.surface-opacity.up",
        "settings.font-size.up",
        "settings.display.select.0",
        "settings.display.keep",
        "settings.hotkey.start",
    ] {
        // Each must fail before any service call, because the transport queue is empty.
        let _ = application.handle_action(&release(action), &mut document);
    }
    assert!(
        requests.borrow().is_empty(),
        "empty snapshots must not reach the service"
    );
    // A page change with no snapshots loaded still syncs without panicking.
    application
        .handle_action(&release("settings.page.about"), &mut document)
        .expect("page navigation with empty snapshots");
    assert_eq!(document.visible.get("page-about"), Some(&true));
}

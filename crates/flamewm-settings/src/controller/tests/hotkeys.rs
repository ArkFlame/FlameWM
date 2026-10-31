use super::*;

#[test]
fn duplicate_hotkey_is_rejected_inline_without_a_service_write() {
    let (requests, mut transport) = recording_transport();
    transport.responses.extend([
        Ok(ControlResponse::Shortcuts(shortcut_snapshot_with(&[
            ("WorkspaceLeft", "Ctrl+Super+Left"),
            ("ToggleStartMenu", "Super"),
        ]))),
        Ok(ControlResponse::CaptureLease(12)),
        Ok(ControlResponse::Unit),
    ]);
    let mut application = shortcuts_application(transport);
    let mut document = ProbeDocument::default();
    application
        .handle_action(&release("settings.hotkey.start"), &mut document)
        .expect("begin capture");
    let key = key_release("Left", &["Ctrl", "Super"]);
    let result = application.handle_key_event(&key, &mut document);
    assert!(result.is_err(), "duplicate binding must be rejected");
    assert!(result.unwrap_err().contains("Desktop left"));
    assert_eq!(
        requests.borrow().len(),
        2,
        "capture lease begins, but duplicate sends no shortcut write"
    );
    assert!(
        !requests
            .borrow()
            .iter()
            .any(|request| matches!(request, ControlRequest::SetShortcut { .. }))
    );
    application
        .client_mut()
        .cancel_hotkey_capture()
        .expect("release duplicate capture lease");
}

#[test]
fn backspace_restores_the_default_hotkey_and_ends_capture() {
    let (requests, mut transport) = recording_transport();
    transport.responses.extend([
        Ok(ControlResponse::Shortcuts(shortcut_snapshot_with(&[(
            "ToggleStartMenu",
            "Super",
        )]))),
        Ok(ControlResponse::CaptureLease(7)),
        Ok(ControlResponse::Unit),
        Ok(ControlResponse::Unit),
        Ok(ControlResponse::Shortcuts(shortcut_snapshot_with(&[]))),
        Ok(ControlResponse::Settings(SettingsSnapshot::new(3))),
    ]);
    let mut application = shortcuts_application(transport);
    let mut document = ProbeDocument::default();
    application
        .handle_action(&release("settings.hotkey.start"), &mut document)
        .expect("begin capture");
    let backspace = key_release("Backspace", &[]);
    application
        .handle_key_event(&backspace, &mut document)
        .expect("backspace resets");
    assert!(matches!(
        requests.borrow().get(2),
        Some(ControlRequest::EndHotkeyCapture { lease: 7 })
    ));
    assert!(
        matches!(requests.borrow().get(3), Some(ControlRequest::ResetShortcut { action, .. }) if action == "ToggleStartMenu")
    );
    // Capture is over: a later stray key release must not write anything.
    let stray = key_release("A", &[]);
    application
        .handle_key_event(&stray, &mut document)
        .expect("stray key ignored");
    assert_eq!(
        requests.borrow().len(),
        6,
        "stray key must not reach the service"
    );
}

#[test]
fn hotkey_reset_button_restores_default_through_the_service() {
    let (requests, mut transport) = recording_transport();
    transport.responses.extend([
        Ok(ControlResponse::Unit),
        Ok(ControlResponse::Shortcuts(shortcut_snapshot_with(&[]))),
        Ok(ControlResponse::Settings(SettingsSnapshot::new(3))),
    ]);
    let mut application = SettingsApplication::new(transport);
    application
        .handle_action(
            &release("settings.hotkey.left.reset"),
            &mut ProbeDocument::default(),
        )
        .expect("reset left");
    assert!(matches!(
        requests.borrow().first(),
        Some(ControlRequest::ResetShortcut { action, .. }) if action == "WorkspaceLeft"
    ));
}

#[test]
fn hotkey_row_projects_its_binding_and_capture_prompt() {
    let mut transport = RecordingTransport::default();
    transport.responses.extend([
        Ok(ControlResponse::Shortcuts(shortcut_snapshot_with(&[(
            "WorkspaceUp",
            "Ctrl+Super+Up",
        )]))),
        Ok(ControlResponse::CaptureLease(13)),
        Ok(ControlResponse::Unit),
    ]);
    let mut application = shortcuts_application(transport);
    let mut document = ProbeDocument::default();
    application
        .handle_action(&release("settings.hotkey.up"), &mut document)
        .expect("begin capture");
    assert_eq!(document.texts["hotkey-key-up"], "Press a key...");
    assert!(
        !document.texts.contains_key("hotkey-key-left"),
        "unbound rows without a snapshot entry are untouched"
    );
}

#[test]
fn switching_page_ends_capture_lease_before_disarming_prompt() {
    let (requests, mut transport) = recording_transport();
    transport.responses.extend([
        Ok(ControlResponse::Shortcuts(shortcut_snapshot_with(&[]))),
        Ok(ControlResponse::CaptureLease(31)),
        Ok(ControlResponse::Unit),
    ]);
    let mut application = shortcuts_application(transport);
    let mut document = ProbeDocument::default();
    application
        .handle_action(&release("settings.hotkey.start"), &mut document)
        .expect("begin capture");
    application
        .handle_action(&release("settings.page.about"), &mut document)
        .expect("switch page");
    assert_eq!(application.client().ui().hotkey_capture, None);
    assert_eq!(application.client().ui().hotkey_capture_lease, None);
    assert!(matches!(
        requests.borrow().get(2),
        Some(ControlRequest::EndHotkeyCapture { lease: 31 })
    ));
}

#[test]
fn hotkey_row_markup_ids_parse_to_their_capture_action() {
    use flamewm_api::shortcuts::{
        ACTION_SCREENSHOT_FULL, ACTION_SCREENSHOT_REGION, ACTION_SCREENSHOT_WINDOW,
        ACTION_SESSION_LOCK, ACTION_TOGGLE_START_MENU, ACTION_WORKSPACE_DOWN,
        ACTION_WORKSPACE_LEFT, ACTION_WORKSPACE_RIGHT, ACTION_WORKSPACE_UP,
    };
    let html = include_str!("../../../../../ui/settings.html");
    let rows = [
        ("start", ACTION_TOGGLE_START_MENU),
        ("left", ACTION_WORKSPACE_LEFT),
        ("right", ACTION_WORKSPACE_RIGHT),
        ("up", ACTION_WORKSPACE_UP),
        ("down", ACTION_WORKSPACE_DOWN),
        ("lock", ACTION_SESSION_LOCK),
        ("shot-full", ACTION_SCREENSHOT_FULL),
        ("shot-region", ACTION_SCREENSHOT_REGION),
        ("shot-window", ACTION_SCREENSHOT_WINDOW),
    ];
    for (slot, binding) in rows {
        // The row's key button is the one that carries the capture id.
        let marker = format!("id=\"hotkey-key-{slot}\" data-action=\"");
        let start = html
            .find(&marker)
            .unwrap_or_else(|| panic!("hotkey row {slot} missing from markup"))
            + marker.len();
        let id = html[start..].split('"').next().unwrap_or_default();
        assert_eq!(
            id,
            format!("settings.hotkey.{slot}"),
            "markup id for row {slot}"
        );
        assert!(
            matches!(
                crate::parse_action(id),
                Ok(crate::SettingsAction::Hotkey(action, crate::HotkeyOperation::Capture))
                    if action == binding
            ),
            "{id} must parse to the capture action for {binding}"
        );
    }
}

#[test]
fn native_keyboard_ids_are_not_settings_errors_and_keep_capture_armed() {
    // Regression: Super release emits native `keyboard.start.0` into the
    // action path; it used to fail as "unknown settings action".
    assert!(matches!(
        crate::parse_action("keyboard.start.0"),
        Ok(crate::SettingsAction::NativeKeyEvent)
    ));
    let mut transport = RecordingTransport::default();
    transport.responses.extend([
        Ok(ControlResponse::Shortcuts(shortcut_snapshot_with(&[]))),
        Ok(ControlResponse::CaptureLease(19)),
        Ok(ControlResponse::Unit),
    ]);
    let mut application = shortcuts_application(transport);
    let mut document = ProbeDocument::default();
    application
        .handle_action(&release("settings.hotkey.start"), &mut document)
        .expect("begin capture");
    document.texts.remove("hotkey-key-start");
    application
        .handle_action(&release("keyboard.start.0"), &mut document)
        .expect("native Super release is not a settings error");
    assert_eq!(
        document.texts.get("hotkey-key-start").map(String::as_str),
        Some("Press a key..."),
        "native key id must not disarm an armed capture"
    );
}

fn key_release(key: &str, modifiers: &[&str]) -> flamewm_ui_x11::UiKeyEvent {
    flamewm_ui_x11::UiKeyEvent {
        phase: flamewm_ui_x11::UiKeyPhase::Release,
        key: key.to_owned(),
        modifiers: modifiers
            .iter()
            .map(|modifier| (*modifier).to_owned())
            .collect(),
        text: None,
    }
}

#[test]
fn bare_super_release_commits_super_to_the_start_row() {
    let (requests, mut transport) = recording_transport();
    transport.responses.extend([
        Ok(ControlResponse::Shortcuts(shortcut_snapshot_with(&[]))),
        Ok(ControlResponse::CaptureLease(20)),
        Ok(ControlResponse::Unit),
        Ok(ControlResponse::Unit),
        Ok(ControlResponse::Shortcuts(shortcut_snapshot_with(&[(
            "ToggleStartMenu",
            "Super",
        )]))),
        Ok(ControlResponse::Settings(SettingsSnapshot::new(3))),
    ]);
    let mut application = shortcuts_application(transport);
    let mut document = ProbeDocument::default();
    application
        .handle_action(&release("settings.hotkey.start"), &mut document)
        .expect("begin capture");
    // The Super release carries its own Super bit and nothing else.
    application
        .handle_key_event(&key_release("Super_L", &["Super"]), &mut document)
        .expect("bare Super commits");
    assert!(matches!(
        requests.borrow().get(1),
        Some(ControlRequest::BeginHotkeyCapture { action, .. }) if action == "ToggleStartMenu"
    ));
    assert!(matches!(
        requests.borrow().get(2),
        Some(ControlRequest::EndHotkeyCapture { lease: 20 })
    ));
    assert!(matches!(
        requests.borrow().get(3),
        Some(ControlRequest::SetShortcut { action, binding, .. })
            if action == "ToggleStartMenu" && binding == "Super"
    ));
}

#[test]
fn super_chord_release_does_not_commit_and_keeps_capture_armed() {
    let (requests, mut transport) = recording_transport();
    transport.responses.extend([
        Ok(ControlResponse::Shortcuts(shortcut_snapshot_with(&[]))),
        Ok(ControlResponse::CaptureLease(21)),
        Ok(ControlResponse::Unit),
    ]);
    let mut application = shortcuts_application(transport);
    let mut document = ProbeDocument::default();
    application
        .handle_action(&release("settings.hotkey.start"), &mut document)
        .expect("begin capture");
    application
        .handle_key_event(&key_release("Super_L", &["Ctrl", "Super"]), &mut document)
        .expect("chord release is not a commit");
    let requests = requests.borrow();
    assert_eq!(
        requests.len(),
        2,
        "initial snapshot plus capture lease; no shortcut write for a Ctrl+Super release"
    );
    assert!(matches!(requests[0], ControlRequest::GetShortcuts));
    assert!(
        matches!(&requests[1], ControlRequest::BeginHotkeyCapture { action, .. } if action == "ToggleStartMenu")
    );
    assert!(!requests.iter().any(|request| matches!(
        request,
        ControlRequest::SetShortcut { .. }
            | ControlRequest::ClearShortcut { .. }
            | ControlRequest::ResetShortcut { .. }
    )));
    assert_eq!(
        document.texts.get("hotkey-key-start").map(String::as_str),
        Some("Press a key..."),
        "capture stays armed after a modifier-only chord release"
    );
}

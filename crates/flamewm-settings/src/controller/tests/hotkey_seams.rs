use super::*;

#[test]
fn hotkey_controller_seam_preserves_escape_as_clear_binding() {
    let (requests, mut transport) = recording_transport();
    transport.responses.extend([
        Ok(ControlResponse::Shortcuts(shortcut_snapshot_with(&[(
            "ToggleStartMenu",
            "Super",
        )]))),
        Ok(ControlResponse::CaptureLease(7)),
        Ok(ControlResponse::Unit),
        Ok(ControlResponse::Unit),
        Ok(ControlResponse::Shortcuts(ShortcutSnapshot {
            revision: 2,
            bindings: BTreeMap::new(),
        })),
        Ok(ControlResponse::Settings(SettingsSnapshot::new(3))),
    ]);
    let mut client = SettingsClient::new(transport);
    client
        .refresh_shortcuts()
        .expect("initial shortcut snapshot");
    client
        .begin_hotkey_capture("ToggleStartMenu")
        .expect("begin lease");
    assert!(client.capture_hotkey("Escape"));
    client.commit_hotkey_capture().expect("clear hotkey");
    assert!(matches!(
        requests.borrow().get(1),
        Some(ControlRequest::BeginHotkeyCapture {
            action,
            expected_revision,
        })
            if action == "ToggleStartMenu" && *expected_revision == 7
    ));
    assert!(matches!(
        requests.borrow().get(2),
        Some(ControlRequest::EndHotkeyCapture { lease: 7 })
    ));
    assert!(
        matches!(requests.borrow().get(3), Some(ControlRequest::ClearShortcut { action, .. }) if action == "ToggleStartMenu")
    );
}

#[test]
fn hotkey_reset_buttons_are_present_for_every_hotkey_row() {
    let actions = markup_actions();
    for slot in ["start", "left", "right", "up", "down"] {
        let action = format!("settings.hotkey.{slot}.reset");
        assert!(
            actions.contains(&action.as_str()),
            "missing hotkey reset markup action: {action}"
        );
    }
}

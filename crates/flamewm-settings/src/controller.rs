#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::{BTreeMap, VecDeque};
    use std::rc::Rc;

    use flamewm_api::display::{DisplayMode, DisplaySnapshot, OutputSnapshot};
    use flamewm_api::settings::{
        SettingValue, SettingsChange, SettingsSnapshot, SettingsTransaction,
    };
    use flamewm_api::shortcuts::ShortcutSnapshot;
    use flamewm_api::{ModeId, OutputId, Rect, Size};
    use flamewm_control_core::{ControlRequest, ControlResponse};
    use flamewm_settings_core::{ControlTransport, SettingsClient};
    use flamewm_ui_x11::{RuntimeImage, UiActionEvent, UiActionPhase, UiColor, UiDocumentAccess};

    use crate::SettingsApplication;

    #[derive(Default)]
    struct RecordingTransport {
        responses: VecDeque<Result<ControlResponse, flamewm_control_core::ControlError>>,
        requests: Rc<RefCell<Vec<ControlRequest>>>,
    }

    impl ControlTransport for RecordingTransport {
        fn call(
            &mut self,
            request: ControlRequest,
        ) -> Result<ControlResponse, flamewm_control_core::ControlError> {
            self.requests.borrow_mut().push(request);
            self.responses
                .pop_front()
                .expect("controller test response was not queued")
        }
    }

    #[derive(Default)]
    struct ProbeDocument {
        texts: BTreeMap<String, String>,
        visible: BTreeMap<String, bool>,
        highlighted: BTreeMap<String, bool>,
    }

    impl UiDocumentAccess for ProbeDocument {
        fn text(&mut self, id: &str, text: String) -> Result<(), String> {
            self.texts.insert(id.to_owned(), text);
            Ok(())
        }

        fn image_rgb8(&mut self, _id: &str, _image: RuntimeImage) -> Result<(), String> {
            Ok(())
        }

        fn image_rgba8(&mut self, _id: &str, _image: RuntimeImage) -> Result<(), String> {
            Ok(())
        }

        fn visible(&mut self, id: &str, visible: bool) -> Result<(), String> {
            self.visible.insert(id.to_owned(), visible);
            Ok(())
        }

        fn toggle(
            &mut self,
            _track_id: &str,
            _knob_id: &str,
            _checked: bool,
        ) -> Result<(), String> {
            Ok(())
        }

        fn position(&mut self, _id: &str, _left: f32, _top: f32) -> Result<(), String> {
            Ok(())
        }

        fn size(&mut self, _id: &str, _width: f32, _height: f32) -> Result<(), String> {
            Ok(())
        }

        fn background(&mut self, id: &str, _color: UiColor) -> Result<(), String> {
            self.highlighted.insert(id.to_owned(), true);
            Ok(())
        }

        fn border(&mut self, _id: &str, _color: UiColor) -> Result<(), String> {
            Ok(())
        }

        fn foreground(&mut self, _id: &str, _color: UiColor) -> Result<(), String> {
            Ok(())
        }

        fn background_clear(&mut self, id: &str) -> Result<(), String> {
            self.highlighted.insert(id.to_owned(), false);
            Ok(())
        }

        fn border_clear(&mut self, _id: &str) -> Result<(), String> {
            Ok(())
        }

        fn foreground_clear(&mut self, _id: &str) -> Result<(), String> {
            Ok(())
        }

        fn font_size(&mut self, _id: &str, _size_px: f32) -> Result<(), String> {
            Ok(())
        }

        fn font_weight(&mut self, _id: &str, _weight: u16) -> Result<(), String> {
            Ok(())
        }

        fn overflow(
            &mut self,
            _id: &str,
            _x: flamewm_ui_x11::Overflow,
            _y: flamewm_ui_x11::Overflow,
        ) -> Result<(), String> {
            Ok(())
        }

        fn layer(
            &mut self,
            _id: &str,
            _layer: flamewm_ui_core::style::UiLayer,
        ) -> Result<(), String> {
            Ok(())
        }
    }

    fn release(action: &str) -> UiActionEvent {
        UiActionEvent {
            action: action.to_owned(),
            phase: UiActionPhase::Release,
            inside: true,
            button: 1,
            x: 0.0,
            y: 0.0,
            text: None,
        }
    }

    fn display_snapshot() -> DisplaySnapshot {
        let mode = ModeId(7);
        DisplaySnapshot {
            generation: 3,
            outputs: vec![OutputSnapshot {
                id: OutputId::new("HDMI-1"),
                connector: "HDMI-1".to_owned(),
                edid_identity: "test-edid".to_owned(),
                connected: true,
                primary: true,
                geometry: Rect::new(0, 0, 1920, 1080),
                current_mode: mode,
                modes: vec![DisplayMode {
                    id: mode,
                    resolution: Size::new(1920, 1080),
                    refresh_millihz: 60_000,
                    preferred: true,
                }],
                shell_scale_percent: 100,
            }],
            pending: None,
        }
    }

    fn markup_actions() -> Vec<&'static str> {
        include_str!("../../../ui/settings.html")
            .split("data-action=\"")
            .skip(1)
            .filter_map(|fragment| fragment.split('"').next())
            .collect()
    }

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
        let requests = Rc::new(RefCell::new(Vec::new()));
        let transport = RecordingTransport {
            requests: requests.clone(),
            ..RecordingTransport::default()
        };
        let mut application = SettingsApplication::new(transport);
        let mut document = ProbeDocument::default();
        let result = application.handle_action(&release("settings.bogus.action"), &mut document);
        assert!(result.is_err());
        assert!(requests.borrow().is_empty(), "unknown action must not call the service");
        assert!(document.texts.is_empty() && document.visible.is_empty(), "unknown action must not sync");
        application.report_error(&mut document, result.unwrap_err());
        assert!(document.texts["settings-error"].contains("settings.bogus.action"));
    }

    #[test]
    fn failed_control_request_keeps_previous_authoritative_value() {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let mut initial = SettingsSnapshot::new(4);
        initial
            .values
            .insert("fontBold".to_owned(), SettingValue::Boolean(false));
        let mut transport = RecordingTransport {
            requests: requests.clone(),
            ..RecordingTransport::default()
        };
        transport
            .responses
            .push_back(Ok(ControlResponse::Settings(initial)));
        transport.responses.push_back(Err(flamewm_control_core::ControlError {
            name: "com.arkflame.FlameWM1.Error.Unavailable",
            code: flamewm_api::ErrorCode::Unavailable,
            message: "service busy".to_owned(),
        }));
        let mut application = SettingsApplication::new(transport);
        application.client_mut().refresh_settings().expect("settings snapshot");
        let mut document = ProbeDocument::default();
        let result = application.handle_action(&release("settings.toggle.fontBold"), &mut document);
        assert!(result.is_err());
        assert_eq!(
            application.client().settings().and_then(|s| s.values.get("fontBold")),
            Some(&SettingValue::Boolean(false)),
            "failed write must leave the previous authoritative value"
        );
        assert!(document.texts.is_empty(), "failed write must not sync");
        application.report_error(&mut document, result.unwrap_err());
        assert!(document.texts["settings-error"].contains("service busy"));
    }

    #[test]
    fn empty_snapshots_are_errors_not_panics() {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let transport = RecordingTransport {
            requests: requests.clone(),
            ..RecordingTransport::default()
        };
        let mut application = SettingsApplication::new(transport);
        let mut document = ProbeDocument::default();
        for action in [
            "settings.taskbar.position",
            "settings.taskbar.size",
            "settings.taskbar-opacity.up",
            "settings.font-size.up",
            "settings.display.select.0",
            "settings.display.keep",
            "settings.hotkey.start",
        ] {
            // Each must fail before any service call, because the transport queue is empty.
            let _ = application.handle_action(&release(action), &mut document);
        }
        assert!(requests.borrow().is_empty(), "empty snapshots must not reach the service");
        // A page change with no snapshots loaded still syncs without panicking.
        application
            .handle_action(&release("settings.page.about"), &mut document)
            .expect("page navigation with empty snapshots");
        assert_eq!(document.visible.get("page-about"), Some(&true));
    }

    #[test]
    fn taskbar_output_is_the_shared_selected_output_and_is_labelled() {
        let mut transport = RecordingTransport::default();
        transport
            .responses
            .push_back(Ok(ControlResponse::Displays(two_output_display_snapshot())));
        let mut application = SettingsApplication::new(transport);
        application.client_mut().refresh_displays().expect("display snapshot");
        let mut document = ProbeDocument::default();
        application
            .handle_action(&release("settings.display.select.1"), &mut document)
            .expect("select DP-1 from taskbar");
        assert_eq!(
            application.client().ui().selected_output.as_ref(),
            Some(&OutputId::new("DP-1"))
        );
        assert!(document.texts["taskbar-output-title"].contains("DP-1"));
        assert_eq!(document.visible.get("taskbar-output-1"), Some(&true));
    }

    #[test]
    fn taskbar_size_label_says_it_applies_to_all_outputs() {
        let mut settings = SettingsSnapshot::new(2);
        settings
            .values
            .insert("taskbarHeight".to_owned(), SettingValue::Integer(52));
        let mut transport = RecordingTransport::default();
        transport.responses.push_back(Ok(ControlResponse::Settings(settings)));
        transport.responses.push_back(Ok(ControlResponse::Displays(display_snapshot())));
        transport.responses.push_back(Ok(ControlResponse::Shortcuts(ShortcutSnapshot {
            revision: 1,
            bindings: BTreeMap::new(),
        })));
        transport.responses.push_back(Ok(ControlResponse::Panels(flamewm_api::panels::PanelsSnapshot {
            revision: 1,
            panels: Vec::new(),
            tasks: Vec::new(),
            pinned_apps: Vec::new(),
        })));
        let mut application = SettingsApplication::new(transport);
        let mut document = ProbeDocument::default();
        application.refresh(&mut document).expect("refresh");
        assert_eq!(document.texts["taskbar-size-value"], "52 px (all outputs)");
        assert_eq!(document.texts["about-version"], format!("Version {}", env!("FLAMEWM_SETTINGS_VERSION")));
    }

    #[test]
    fn about_version_text_is_exact_flamewm_version() {
        let version = include_str!("../../../FLAMEWM_VERSION").trim();
        let mut transport = RecordingTransport::default();
        transport.responses.push_back(Ok(ControlResponse::Settings(SettingsSnapshot::new(2))));
        transport.responses.push_back(Ok(ControlResponse::Displays(display_snapshot())));
        transport.responses.push_back(Ok(ControlResponse::Shortcuts(ShortcutSnapshot {
            revision: 1,
            bindings: BTreeMap::new(),
        })));
        transport.responses.push_back(Ok(ControlResponse::Panels(flamewm_api::panels::PanelsSnapshot {
            revision: 1,
            panels: Vec::new(),
            tasks: Vec::new(),
            pinned_apps: Vec::new(),
        })));
        let mut application = SettingsApplication::new(transport);
        let mut document = ProbeDocument::default();
        assert!(!document.texts.contains_key("about-version"), "no runtime about version before refresh");
        application.refresh(&mut document).expect("refresh");
        assert_eq!(document.texts["about-version"], format!("Version {version}"));
        assert_ne!(document.texts["about-version"], version, "bare version must not match");
        assert_ne!(document.texts["about-version"], "Version 0.0.0-wrong");
    }

    #[test]
    fn about_links_map_to_the_fixed_product_uris_only() {
        assert!(matches!(
            crate::parse_action("about.source"),
            Ok(crate::SettingsAction::OpenUri("https://github.com/ArkFlame/flamewm"))
        ));
        assert!(crate::parse_action("about.other").is_err());
    }

    fn shortcut_snapshot_with(bindings: &[(&str, &str)]) -> ShortcutSnapshot {
        ShortcutSnapshot {
            revision: 7,
            bindings: bindings
                .iter()
                .map(|(action, key)| {
                    (
                        (*action).to_owned(),
                        flamewm_api::shortcuts::KeyBinding(Some((*key).to_owned())),
                    )
                })
                .collect(),
        }
    }

    #[test]
    fn duplicate_hotkey_is_rejected_inline_without_a_service_write() {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let mut transport = RecordingTransport {
            requests: requests.clone(),
            ..RecordingTransport::default()
        };
        transport.responses.push_back(Ok(ControlResponse::Shortcuts(shortcut_snapshot_with(&[
            ("WorkspaceLeft", "Ctrl+Super+Left"),
            ("ToggleStartMenu", "Super"),
        ]))));
        transport.responses.push_back(Ok(ControlResponse::CaptureLease(12)));
        transport.responses.push_back(Ok(ControlResponse::Unit));
        let mut application = SettingsApplication::new(transport);
        application.client_mut().refresh_shortcuts().expect("shortcut snapshot");
        let mut document = ProbeDocument::default();
        application
            .handle_action(&release("settings.hotkey.start"), &mut document)
            .expect("begin capture");
        let key = flamewm_ui_x11::UiKeyEvent {
            phase: flamewm_ui_x11::UiKeyPhase::Release,
            key: "Left".to_owned(),
            modifiers: vec!["Ctrl".to_owned(), "Super".to_owned()],
            text: None,
        };
        let result = application.handle_key_event(&key, &mut document);
        assert!(result.is_err(), "duplicate binding must be rejected");
        assert!(result.unwrap_err().contains("Desktop left"));
        assert_eq!(requests.borrow().len(), 2, "capture lease begins, but duplicate sends no shortcut write");
        assert!(!requests.borrow().iter().any(|request| matches!(request, ControlRequest::SetShortcut { .. })));
        application.client_mut().cancel_hotkey_capture().expect("release duplicate capture lease");
    }

    #[test]
    fn backspace_restores_the_default_hotkey_and_ends_capture() {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let mut transport = RecordingTransport {
            requests: requests.clone(),
            ..RecordingTransport::default()
        };
        transport.responses.push_back(Ok(ControlResponse::Shortcuts(shortcut_snapshot_with(&[
            ("ToggleStartMenu", "Super"),
        ]))));
        transport.responses.push_back(Ok(ControlResponse::CaptureLease(7)));
        transport.responses.push_back(Ok(ControlResponse::Unit));
        transport.responses.push_back(Ok(ControlResponse::Unit));
        transport.responses.push_back(Ok(ControlResponse::Shortcuts(shortcut_snapshot_with(&[]))));
        transport.responses.push_back(Ok(ControlResponse::Settings(SettingsSnapshot::new(3))));
        let mut application = SettingsApplication::new(transport);
        application.client_mut().refresh_shortcuts().expect("shortcut snapshot");
        let mut document = ProbeDocument::default();
        application
            .handle_action(&release("settings.hotkey.start"), &mut document)
            .expect("begin capture");
        let backspace = flamewm_ui_x11::UiKeyEvent {
            phase: flamewm_ui_x11::UiKeyPhase::Release,
            key: "Backspace".to_owned(),
            modifiers: Vec::new(),
            text: None,
        };
        application
            .handle_key_event(&backspace, &mut document)
            .expect("backspace resets");
        assert!(matches!(requests.borrow().get(2), Some(ControlRequest::EndHotkeyCapture { lease: 7 })));
        assert!(matches!(requests.borrow().get(3), Some(ControlRequest::ResetShortcut { action, .. }) if action == "ToggleStartMenu"));
        // Capture is over: a later stray key release must not write anything.
        let stray = flamewm_ui_x11::UiKeyEvent {
            phase: flamewm_ui_x11::UiKeyPhase::Release,
            key: "A".to_owned(),
            modifiers: Vec::new(),
            text: None,
        };
        application.handle_key_event(&stray, &mut document).expect("stray key ignored");
        assert_eq!(requests.borrow().len(), 6, "stray key must not reach the service");
    }

    #[test]
    fn hotkey_reset_button_restores_default_through_the_service() {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let mut transport = RecordingTransport {
            requests: requests.clone(),
            ..RecordingTransport::default()
        };
        transport.responses.push_back(Ok(ControlResponse::Unit));
        transport.responses.push_back(Ok(ControlResponse::Shortcuts(shortcut_snapshot_with(&[]))));
        transport.responses.push_back(Ok(ControlResponse::Settings(SettingsSnapshot::new(3))));
        let mut application = SettingsApplication::new(transport);
        application
            .handle_action(&release("settings.hotkey.left.reset"), &mut ProbeDocument::default())
            .expect("reset left");
        assert!(matches!(
            requests.borrow().first(),
            Some(ControlRequest::ResetShortcut { action, .. }) if action == "WorkspaceLeft"
        ));
    }

    #[test]
    fn hotkey_row_projects_its_binding_and_capture_prompt() {
        let mut transport = RecordingTransport::default();
        transport.responses.push_back(Ok(ControlResponse::Shortcuts(shortcut_snapshot_with(&[
            ("WorkspaceUp", "Ctrl+Super+Up"),
        ]))));
        transport.responses.push_back(Ok(ControlResponse::CaptureLease(13)));
        transport.responses.push_back(Ok(ControlResponse::Unit));
        let mut application = SettingsApplication::new(transport);
        application.client_mut().refresh_shortcuts().expect("shortcut snapshot");
        let mut document = ProbeDocument::default();
        application.handle_action(&release("settings.hotkey.up"), &mut document).expect("begin capture");
        assert_eq!(document.texts["hotkey-key-up"], "Press a key...");
        assert!(!document.texts.contains_key("hotkey-key-left"), "unbound rows without a snapshot entry are untouched");
    }

    #[test]
    fn switching_page_ends_capture_lease_before_disarming_prompt() {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let mut transport = RecordingTransport { requests: requests.clone(), ..RecordingTransport::default() };
        transport.responses.extend([
            Ok(ControlResponse::Shortcuts(shortcut_snapshot_with(&[]))),
            Ok(ControlResponse::CaptureLease(31)),
            Ok(ControlResponse::Unit),
        ]);
        let mut application = SettingsApplication::new(transport);
        application.client_mut().refresh_shortcuts().expect("shortcut snapshot");
        let mut document = ProbeDocument::default();
        application.handle_action(&release("settings.hotkey.start"), &mut document).expect("begin capture");
        application.handle_action(&release("settings.page.about"), &mut document).expect("switch page");
        assert_eq!(application.client().ui().hotkey_capture, None);
        assert_eq!(application.client().ui().hotkey_capture_lease, None);
        assert!(matches!(requests.borrow().get(2), Some(ControlRequest::EndHotkeyCapture { lease: 31 })));
    }


    #[test]
    fn hotkey_row_markup_ids_parse_to_their_capture_action() {
        use flamewm_api::shortcuts::{
            ACTION_TOGGLE_START_MENU, ACTION_WORKSPACE_DOWN, ACTION_WORKSPACE_LEFT,
            ACTION_WORKSPACE_RIGHT, ACTION_WORKSPACE_UP,
        };
        let html = include_str!("../../../ui/settings.html");
        let rows = [
            ("start", ACTION_TOGGLE_START_MENU),
            ("left", ACTION_WORKSPACE_LEFT),
            ("right", ACTION_WORKSPACE_RIGHT),
            ("up", ACTION_WORKSPACE_UP),
            ("down", ACTION_WORKSPACE_DOWN),
        ];
        for (slot, binding) in rows {
            // The row's key button is the one that carries the capture id.
            let marker = format!("id=\"hotkey-key-{slot}\" data-action=\"");
            let start = html
                .find(&marker)
                .unwrap_or_else(|| panic!("hotkey row {slot} missing from markup"))
                + marker.len();
            let id = html[start..].split('"').next().unwrap_or_default();
            assert_eq!(id, format!("settings.hotkey.{slot}"), "markup id for row {slot}");
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
        let mut application = SettingsApplication::new(transport);
        application.client_mut().refresh_shortcuts().expect("shortcut snapshot");
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
            modifiers: modifiers.iter().map(|modifier| (*modifier).to_owned()).collect(),
            text: None,
        }
    }

    #[test]
    fn bare_super_release_commits_super_to_the_start_row() {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let mut transport = RecordingTransport {
            requests: requests.clone(),
            ..RecordingTransport::default()
        };
        transport.responses.push_back(Ok(ControlResponse::Shortcuts(shortcut_snapshot_with(&[]))));
        transport.responses.push_back(Ok(ControlResponse::CaptureLease(20)));
        transport.responses.push_back(Ok(ControlResponse::Unit));
        transport.responses.push_back(Ok(ControlResponse::Unit));
        transport.responses.push_back(Ok(ControlResponse::Shortcuts(shortcut_snapshot_with(&[
            ("ToggleStartMenu", "Super"),
        ]))));
        transport.responses.push_back(Ok(ControlResponse::Settings(SettingsSnapshot::new(3))));
        let mut application = SettingsApplication::new(transport);
        application.client_mut().refresh_shortcuts().expect("shortcut snapshot");
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
        let requests = Rc::new(RefCell::new(Vec::new()));
        let mut transport = RecordingTransport {
            requests: requests.clone(),
            ..RecordingTransport::default()
        };
        transport.responses.push_back(Ok(ControlResponse::Shortcuts(shortcut_snapshot_with(&[]))));
        transport.responses.push_back(Ok(ControlResponse::CaptureLease(21)));
        transport.responses.push_back(Ok(ControlResponse::Unit));
        let mut application = SettingsApplication::new(transport);
        application.client_mut().refresh_shortcuts().expect("shortcut snapshot");
        let mut document = ProbeDocument::default();
        application
            .handle_action(&release("settings.hotkey.start"), &mut document)
            .expect("begin capture");
        application
            .handle_key_event(&key_release("Super_L", &["Ctrl", "Super"]), &mut document)
            .expect("chord release is not a commit");
        let requests = requests.borrow();
        assert_eq!(requests.len(), 2, "initial snapshot plus capture lease; no shortcut write for a Ctrl+Super release");
        assert!(matches!(requests[0], ControlRequest::GetShortcuts));
        assert!(matches!(&requests[1], ControlRequest::BeginHotkeyCapture { action, .. } if action == "ToggleStartMenu"));
        assert!(!requests.iter().any(|request| matches!(request,
            ControlRequest::SetShortcut { .. } | ControlRequest::ClearShortcut { .. } | ControlRequest::ResetShortcut { .. }
        )));
        assert_eq!(
            document.texts.get("hotkey-key-start").map(String::as_str),
            Some("Press a key..."),
            "capture stays armed after a modifier-only chord release"
        );
    }

    fn decoration_snapshot(revision: u64, decoration: &str) -> SettingsSnapshot {
        let mut snapshot = SettingsSnapshot::new(revision);
        snapshot
            .values
            .insert("accent".to_owned(), SettingValue::Text("#EF4048".to_owned()));
        snapshot
            .values
            .insert("decoration".to_owned(), SettingValue::Text(decoration.to_owned()));
        snapshot
    }

    #[test]
    fn decoration_option_buttons_write_exact_values_and_highlight_the_snapshot() {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let mut transport = RecordingTransport {
            requests: requests.clone(),
            ..RecordingTransport::default()
        };
        transport.responses.push_back(Ok(ControlResponse::Settings(decoration_snapshot(1, "rounded"))));
        transport.responses.push_back(Ok(ControlResponse::Settings(decoration_snapshot(2, "ios"))));
        transport.responses.push_back(Ok(ControlResponse::Settings(decoration_snapshot(3, "rounded"))));
        let mut application = SettingsApplication::new(transport);
        application.client_mut().refresh_settings().expect("initial snapshot");
        let mut document = ProbeDocument::default();

        application
            .handle_action(&release("settings.decoration.ios"), &mut document)
            .expect("ios option");
        assert!(document.highlighted["decoration-ios"], "ios is highlighted after its write");
        assert!(!document.highlighted["decoration-rounded"], "rounded is cleared after ios write");

        application
            .handle_action(&release("settings.decoration.rounded"), &mut document)
            .expect("rounded option");
        assert!(document.highlighted["decoration-rounded"]);
        assert!(!document.highlighted["decoration-ios"]);

        let writes: Vec<(String, SettingValue)> = requests
            .borrow()
            .iter()
            .filter_map(|request| match request {
                ControlRequest::ApplySettings(transaction) => transaction
                    .changes
                    .first()
                    .and_then(|change| change.value.clone().map(|value| (change.key.clone(), value))),
                _ => None,
            })
            .collect();
        assert_eq!(
            writes,
            vec![
                ("decoration".to_owned(), SettingValue::Text("ios".to_owned())),
                ("decoration".to_owned(), SettingValue::Text("rounded".to_owned())),
            ],
        );
    }

    #[test]
    fn external_decoration_change_moves_the_highlight_without_a_write() {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let mut transport = RecordingTransport {
            requests: requests.clone(),
            ..RecordingTransport::default()
        };
        transport.responses.push_back(Ok(ControlResponse::Settings(decoration_snapshot(1, "rounded"))));
        transport.responses.push_back(Ok(ControlResponse::Settings(decoration_snapshot(2, "ios"))));
        let mut application = SettingsApplication::new(transport);
        let mut document = ProbeDocument::default();
        application.client_mut().refresh_settings().expect("initial snapshot");
        application.sync(&mut document).expect("sync initial snapshot");
        assert!(document.highlighted["decoration-rounded"]);
        application.client_mut().refresh_settings().expect("external snapshot");
        application.sync(&mut document).expect("sync external snapshot");
        assert!(document.highlighted["decoration-ios"], "external ios snapshot moves the highlight");
        assert!(!document.highlighted["decoration-rounded"]);
        assert!(
            requests.borrow().iter().all(|request| !matches!(request, ControlRequest::ApplySettings(_))),
            "external sync must not write"
        );
    }

    fn animation_snapshot(revision: u64, key: &str, value: &str) -> SettingsSnapshot {
        let mut snapshot = SettingsSnapshot::new(revision);
        snapshot.values.insert(key.to_owned(), SettingValue::Text(value.to_owned()));
        snapshot
    }

    #[test]
    fn animation_style_segments_write_exact_values_and_highlight_the_snapshot() {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let mut transport = RecordingTransport {
            requests: requests.clone(),
            ..RecordingTransport::default()
        };
        let mut revision = 1;
        transport.responses.push_back(Ok(ControlResponse::Settings(SettingsSnapshot::new(revision))));
        let mut writes = Vec::new();
        for (key, values) in crate::ANIMATION_STYLE_ROWS {
            for value in values {
                revision += 1;
                transport
                    .responses
                    .push_back(Ok(ControlResponse::Settings(animation_snapshot(revision, key, value))));
                writes.push((key, *value));
            }
        }
        revision += 1;
        transport
            .responses
            .push_back(Ok(ControlResponse::Settings(animation_snapshot(revision, "startAnimation", "fade"))));
        let mut application = SettingsApplication::new(transport);
        application.client_mut().refresh_settings().expect("initial snapshot");
        let mut document = ProbeDocument::default();

        for (key, value) in &writes {
            application
                .handle_action(&release(&format!("settings.{key}.{value}")), &mut document)
                .expect("animation style option");
            let (_, values) = crate::ANIMATION_STYLE_ROWS
                .iter()
                .find(|(row, _)| row == key)
                .expect("animation row");
            for option in values.iter() {
                assert_eq!(
                    document.highlighted[&crate::animation_style_node(key, option)],
                    option == value,
                    "{key}.{option} highlight after writing {value}"
                );
            }
        }

        let written: Vec<(String, SettingValue)> = requests
            .borrow()
            .iter()
            .filter_map(|request| match request {
                ControlRequest::ApplySettings(transaction) => transaction
                    .changes
                    .first()
                    .and_then(|change| change.value.clone().map(|value| (change.key.clone(), value))),
                _ => None,
            })
            .collect();
        let expected: Vec<(String, SettingValue)> = writes
            .iter()
            .map(|(key, value)| ((*key).to_owned(), SettingValue::Text((*value).to_owned())))
            .collect();
        assert_eq!(written, expected, "each segment writes its exact key and value");

        application.client_mut().refresh_settings().expect("external snapshot");
        application.sync(&mut document).expect("sync external snapshot");
        assert!(document.highlighted[&crate::animation_style_node("startAnimation", "fade")]);
        assert!(!document.highlighted[&crate::animation_style_node("startAnimation", "none")]);
    }

    #[test]
    fn visible_settings_buttons_cannot_be_actionless() {
        let html = include_str!("../../../ui/settings.html");
        let orphaned = html
            .split("<button")
            .skip(1)
            .filter_map(|button| button.split("</button>").next())
            .filter(|button| !button.contains("data-action=\""))
            .collect::<Vec<_>>();
        assert!(
            orphaned.is_empty(),
            "RED admission: visible settings controls have no action: {orphaned:?}"
        );
    }

    #[test]
    fn opacity_stepper_controls_are_admitted_before_controller_repair() {
        let desktop = include_str!("../../../ui/settings.html")
            .split("id=\"page-desktop\"")
            .nth(1)
            .and_then(|page| page.split("</section>").next())
            .expect("desktop page exists");
        let opacity = desktop
            .split("Selection rectangle opacity")
            .nth(1)
            .expect("selection opacity control exists");
        assert!(
            opacity.contains("data-action=\""),
            "RED admission: current source has no opacity stepper action; exact action names remain unsupported by source"
        );
    }

    #[test]
    fn opacity_clicks_use_one_apply_settings_transaction_at_each_step_boundary() {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let mut transport = RecordingTransport {
            requests: requests.clone(),
            ..RecordingTransport::default()
        };
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
        let requests = Rc::new(RefCell::new(Vec::new()));
        let mut transport = RecordingTransport {
            requests,
            ..RecordingTransport::default()
        };
        transport
            .responses
            .push_back(Ok(ControlResponse::Displays(display_snapshot())));
        let mut application = SettingsApplication::new(transport);
        application
            .client_mut()
            .refresh_displays()
            .expect("display snapshot");
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
        let requests = Rc::new(RefCell::new(Vec::new()));
        let mut initial = SettingsSnapshot::new(4);
        initial
            .values
            .insert("fontBold".to_owned(), SettingValue::Boolean(false));
        let mut updated = SettingsSnapshot::new(5);
        updated
            .values
            .insert("fontBold".to_owned(), SettingValue::Boolean(true));
        let mut transport = RecordingTransport {
            requests: requests.clone(),
            ..RecordingTransport::default()
        };
        transport
            .responses
            .push_back(Ok(ControlResponse::Settings(initial)));
        transport
            .responses
            .push_back(Ok(ControlResponse::Settings(updated)));
        let mut application = SettingsApplication::new(transport);
        application
            .client_mut()
            .refresh_settings()
            .expect("settings snapshot");
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

    #[test]
    fn accent_reset_release_sends_exact_revisioned_change_and_refreshes_snapshot() {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let mut initial = SettingsSnapshot::new(1);
        initial.values.insert(
            "accent".to_owned(),
            SettingValue::Text("#2185D5".to_owned()),
        );
        let mut reset = SettingsSnapshot::new(2);
        reset.values.insert(
            "accent".to_owned(),
            SettingValue::Text("#EF4048".to_owned()),
        );
        let mut transport = RecordingTransport {
            requests: requests.clone(),
            ..RecordingTransport::default()
        };
        transport.responses.push_back(Ok(ControlResponse::Settings(initial)));
        transport.responses.push_back(Ok(ControlResponse::Settings(reset.clone())));
        let mut application = SettingsApplication::new(transport);
        application.client_mut().refresh_settings().expect("initial snapshot");

        application
            .handle_action(
                &release("settings.accent.reset"),
                &mut ProbeDocument::default(),
            )
            .expect("accent reset release");

        assert_eq!(
            *requests.borrow(),
            vec![
                ControlRequest::GetSettings,
                ControlRequest::ApplySettings(SettingsTransaction {
                    expected_revision: 1,
                    changes: vec![SettingsChange {
                        key: "accent".to_owned(),
                        value: Some(SettingValue::Text("#EF4048".to_owned())),
                    }],
                    reset_section: None,
                }),
            ],
            "reset is an explicit accent change at the snapshot revision, not a broad section reset"
        );
        assert_eq!(application.client().settings(), Some(&reset));
    }

    fn color_step_application(initial_value: &str, action: &str) -> Vec<ControlRequest> {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let mut initial = SettingsSnapshot::new(7);
        initial.values.insert(
            "startMenuColor".to_owned(),
            SettingValue::Text(initial_value.to_owned()),
        );
        initial.values.insert(
            "accent".to_owned(),
            SettingValue::Text(initial_value.to_owned()),
        );
        let mut transport = RecordingTransport {
            requests: requests.clone(),
            ..RecordingTransport::default()
        };
        transport.responses.push_back(Ok(ControlResponse::Settings(initial.clone())));
        transport.responses.push_back(Ok(ControlResponse::Settings(initial)));
        let mut application = SettingsApplication::new(transport);
        application.client_mut().refresh_settings().expect("initial snapshot");
        application
            .handle_action(&release(action), &mut ProbeDocument::default())
            .expect("color step release");
        let recorded = requests.borrow().clone();
        recorded
    }

    #[test]
    fn start_menu_color_step_sends_exact_hex_channel_change_in_one_transaction() {
        let requests = color_step_application("#191B1D", "settings.color.startmenu.r.step.16");
        assert_eq!(
            requests.last(),
            Some(&ControlRequest::ApplySettings(SettingsTransaction {
                expected_revision: 7,
                changes: vec![SettingsChange {
                    key: "startMenuColor".to_owned(),
                    value: Some(SettingValue::Text("#291B1D".to_owned())),
                }],
                reset_section: None,
            })),
        );
        assert_eq!(requests.len(), 2, "one GetSettings and one ApplySettings only");
    }

    #[test]
    fn accent_color_step_clamps_channels_at_255_instead_of_wrapping() {
        let requests = color_step_application("#FFFFFF", "settings.color.accent.g.step.16");
        assert_eq!(
            requests.last(),
            Some(&ControlRequest::ApplySettings(SettingsTransaction {
                expected_revision: 7,
                changes: vec![SettingsChange {
                    key: "accent".to_owned(),
                    value: Some(SettingValue::Text("#FFFFFF".to_owned())),
                }],
                reset_section: None,
            })),
        );
    }

    #[test]
    fn accent_swatches_dispatch_only_release_inside_events_and_apply_selected_value() {
        let requests = Rc::new(RefCell::new(Vec::new()));
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
        let mut transport = RecordingTransport {
            requests: requests.clone(),
            ..RecordingTransport::default()
        };
        transport.responses.push_back(Ok(ControlResponse::Settings(initial)));
        transport.responses.push_back(Ok(ControlResponse::Settings(blue)));
        transport.responses.push_back(Ok(ControlResponse::Settings(teal)));
        let mut application = SettingsApplication::new(transport);
        application.client_mut().refresh_settings().expect("initial snapshot");

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
        assert_eq!(requests.borrow().len(), 1, "only the initial snapshot was requested");

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

    #[test]
    fn hotkey_controller_seam_preserves_escape_as_clear_binding() {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let mut transport = RecordingTransport {
            requests: requests.clone(),
            ..RecordingTransport::default()
        };
        transport.responses.push_back(Ok(ControlResponse::Shortcuts(shortcut_snapshot_with(&[
            ("ToggleStartMenu", "Super"),
        ]))));
        transport.responses.push_back(Ok(ControlResponse::CaptureLease(7)));
        transport.responses.push_back(Ok(ControlResponse::Unit));
        transport.responses.push_back(Ok(ControlResponse::Unit));
        transport
            .responses
            .push_back(Ok(ControlResponse::Shortcuts(ShortcutSnapshot {
                revision: 2,
                bindings: BTreeMap::new(),
            })));
        transport
            .responses
            .push_back(Ok(ControlResponse::Settings(SettingsSnapshot::new(3))));
        let mut client = SettingsClient::new(transport);
        client.refresh_shortcuts().expect("initial shortcut snapshot");
        client.begin_hotkey_capture("ToggleStartMenu").expect("begin lease");
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
        assert!(matches!(requests.borrow().get(2), Some(ControlRequest::EndHotkeyCapture { lease: 7 })));
        assert!(matches!(requests.borrow().get(3), Some(ControlRequest::ClearShortcut { action, .. }) if action == "ToggleStartMenu"));
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

    fn two_mode_display_snapshot() -> DisplaySnapshot {
        let mut snapshot = display_snapshot();
        let output = &mut snapshot.outputs[0];
        output.modes = vec![
            DisplayMode {
                id: ModeId(7),
                resolution: Size::new(1920, 1080),
                refresh_millihz: 60_000,
                preferred: true,
            },
            DisplayMode {
                id: ModeId(8),
                resolution: Size::new(1280, 720),
                refresh_millihz: 50_000,
                preferred: false,
            },
        ];
        snapshot
    }

    fn two_output_display_snapshot() -> DisplaySnapshot {
        let mut snapshot = two_mode_display_snapshot();
        let mut second = snapshot.outputs[0].clone();
        second.id = OutputId::new("DP-1");
        second.connector = "DP-1".to_owned();
        second.primary = false;
        second.current_mode = ModeId(8);
        second.geometry = Rect::new(1920, 0, 1280, 720);
        snapshot.outputs.push(second);
        snapshot
    }

    #[test]
    fn display_mode_slots_show_one_label_per_mode_with_distinct_ids() {
        let mut transport = RecordingTransport::default();
        transport
            .responses
            .push_back(Ok(ControlResponse::Displays(two_mode_display_snapshot())));
        let mut application = SettingsApplication::new(transport);
        application
            .client_mut()
            .refresh_displays()
            .expect("display snapshot");
        let mut document = ProbeDocument::default();
        application
            .handle_action(&release("settings.display.select.0"), &mut document)
            .expect("display select");

        assert_eq!(document.visible.get("display-mode-slot-0"), Some(&true));
        assert_eq!(document.visible.get("display-mode-slot-1"), Some(&true));
        assert_eq!(document.visible.get("display-mode-slot-2"), Some(&false));
        let first = document.texts.get("display-mode-slot-0").expect("slot 0 label");
        let second = document.texts.get("display-mode-slot-1").expect("slot 1 label");
        assert!(first.contains("1920 x 1080"), "slot 0 label: {first}");
        assert!(second.contains("1280 x 720"), "slot 1 label: {second}");
        assert_ne!(first, second);
    }

    #[test]
    fn display_mode_slot_action_begins_the_mapped_mode() {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let mut transport = RecordingTransport {
            requests: requests.clone(),
            ..RecordingTransport::default()
        };
        transport
            .responses
            .push_back(Ok(ControlResponse::Displays(two_mode_display_snapshot())));
        transport
            .responses
            .push_back(Ok(ControlResponse::Transaction(flamewm_api::TransactionId(
                11,
            ))));
        let mut application = SettingsApplication::new(transport);
        application
            .client_mut()
            .refresh_displays()
            .expect("display snapshot");
        let mut document = ProbeDocument::default();
        application
            .handle_action(&release("settings.display.select.0"), &mut document)
            .expect("display select");
        application
            .handle_action(&release("settings.display.slot.1"), &mut document)
            .expect("mode slot begin");

        let begun = requests
            .borrow()
            .iter()
            .filter_map(|request| match request {
                ControlRequest::BeginDisplayMode { mode, .. } => Some(*mode),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(begun, vec![ModeId(8)]);
    }

    #[test]
    fn display_mode_slots_and_actions_follow_the_selected_output() {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let mut transport = RecordingTransport {
            requests: requests.clone(),
            ..RecordingTransport::default()
        };
        transport.responses.push_back(Ok(ControlResponse::Displays(two_output_display_snapshot())));
        transport.responses.push_back(Ok(ControlResponse::Transaction(flamewm_api::TransactionId(12))));
        let mut application = SettingsApplication::new(transport);
        application.client_mut().refresh_displays().expect("display snapshot");
        let mut document = ProbeDocument::default();
        application.handle_action(&release("settings.display.select.1"), &mut document).expect("select DP-1");
        assert!(document.texts["display-mode-slot-0"].contains("1920 x 1080"));
        application.handle_action(&release("settings.display.slot.1"), &mut document).expect("begin DP-1 mode");
        assert!(matches!(requests.borrow().last(), Some(ControlRequest::BeginDisplayMode { output, mode: ModeId(8), .. }) if output == &OutputId::new("DP-1")));
    }

    #[test]
    fn display_confirmation_keep_revert_and_errors_track_pending_state() {
        for (action, response, expected_pending) in [
            ("settings.display.keep", Ok(ControlResponse::Unit), false),
            ("settings.display.revert", Ok(ControlResponse::Unit), false),
            ("settings.display.keep", Err(flamewm_control_core::ControlError { name: "com.arkflame.FlameWM1.Error.StaleRevision", code: flamewm_api::ErrorCode::StaleRevision, message: "stale transaction".to_owned() }), true),
        ] {
            let requests = Rc::new(RefCell::new(Vec::new()));
            let mut transport = RecordingTransport { requests: requests.clone(), ..RecordingTransport::default() };
            let mut pending = two_mode_display_snapshot();
            pending.pending = Some(flamewm_api::display::PendingModeChange {
                transaction: flamewm_api::TransactionId(21),
                output: OutputId::new("HDMI-1"),
                mode: ModeId(8),
                deadline_ms: 5000,
            });
            transport.responses.push_back(Ok(ControlResponse::Displays(pending)));
            if !expected_pending {
                let mut settled = two_mode_display_snapshot();
                settled.outputs[0].current_mode = if action.ends_with("keep") { ModeId(8) } else { ModeId(7) };
                transport.responses.push_back(response);
                transport.responses.push_back(Ok(ControlResponse::Displays(settled)));
            } else {
                transport.responses.push_back(response);
            }
            let mut application = SettingsApplication::new(transport);
            application.client_mut().refresh_displays().expect("pending display snapshot");
            let mut document = ProbeDocument::default();
            let before = crate::monotonic_now_ms();
            let result = application.handle_action(&release(action), &mut document);
            let after = crate::monotonic_now_ms();
            assert_eq!(result.is_err(), expected_pending);
            assert_eq!(application.client().ui().pending_display_transaction.is_some(), expected_pending);
            if !expected_pending {
                assert_eq!(document.visible.get("display-confirm"), Some(&false));
            }
            assert!(matches!(requests.borrow().get(1), Some(ControlRequest::KeepDisplayMode { transaction: flamewm_api::TransactionId(21), now_ms }) if action.ends_with("keep") && *now_ms >= before && *now_ms <= after) || matches!(requests.borrow().get(1), Some(ControlRequest::RevertDisplayMode { transaction: flamewm_api::TransactionId(21) }) if action.ends_with("revert")));
        }
    }

    /// Twelve modes with ids 0..12 on the selected output: page 0 holds 0..8, page 1 holds 8..12.
    fn twelve_mode_display_snapshot() -> DisplaySnapshot {
        let mut snapshot = display_snapshot();
        let output = &mut snapshot.outputs[0];
        output.modes = (0..12u64)
            .map(|id| DisplayMode {
                id: ModeId(id),
                resolution: Size::new(640, 480),
                refresh_millihz: 60_000,
                preferred: id == 0,
            })
            .collect();
        output.current_mode = ModeId(0);
        snapshot
    }

    /// Five outputs OUT-0..OUT-4: page 0 holds 0..4, page 1 holds OUT-4.
    fn five_output_display_snapshot() -> DisplaySnapshot {
        let mut snapshot = display_snapshot();
        let template = snapshot.outputs[0].clone();
        snapshot.outputs = (0..5)
            .map(|index| {
                let mut output = template.clone();
                output.id = OutputId::new(format!("OUT-{index}"));
                output.connector = format!("OUT-{index}");
                output.primary = index == 0;
                output
            })
            .collect();
        snapshot
    }

    fn begun_modes(requests: &Rc<RefCell<Vec<ControlRequest>>>) -> Vec<ModeId> {
        requests
            .borrow()
            .iter()
            .filter_map(|request| match request {
                ControlRequest::BeginDisplayMode { mode, .. } => Some(*mode),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn display_mode_page_one_slot_three_begins_absolute_mode_eleven() {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let mut transport = RecordingTransport {
            requests: requests.clone(),
            ..RecordingTransport::default()
        };
        transport
            .responses
            .push_back(Ok(ControlResponse::Displays(twelve_mode_display_snapshot())));
        transport
            .responses
            .push_back(Ok(ControlResponse::Transaction(flamewm_api::TransactionId(31))));
        let mut application = SettingsApplication::new(transport);
        application.client_mut().refresh_displays().expect("display snapshot");
        let mut document = ProbeDocument::default();
        application
            .handle_action(&release("settings.display.modes.next"), &mut document)
            .expect("mode page 2");
        assert_eq!(document.texts["display-mode-page-label"], "9-12 of 12");
        application
            .handle_action(&release("settings.display.slot.3"), &mut document)
            .expect("page 1 slot 3 begins");
        assert_eq!(begun_modes(&requests), vec![ModeId(11)]);
    }

    #[test]
    fn display_output_page_one_slot_zero_selects_absolute_output_four() {
        let mut transport = RecordingTransport::default();
        transport
            .responses
            .push_back(Ok(ControlResponse::Displays(five_output_display_snapshot())));
        let mut application = SettingsApplication::new(transport);
        application.client_mut().refresh_displays().expect("display snapshot");
        let mut document = ProbeDocument::default();
        application
            .handle_action(&release("settings.display.outputs.next"), &mut document)
            .expect("output page 2");
        assert_eq!(document.texts["display-output-page-label"], "5-5 of 5");
        application
            .handle_action(&release("settings.display.select.0"), &mut document)
            .expect("page 1 slot 0 selects");
        assert_eq!(
            application.client().ui().selected_output.as_ref(),
            Some(&OutputId::new("OUT-4"))
        );
    }

    #[test]
    fn stale_page_generation_is_rejected_without_applying() {
        // Cursor stamped at generation 3, authoritative snapshot now generation 4.
        let error = crate::page_slot_index(3, 4, 0, 4, 0, 5, "display output")
            .expect_err("stale generation must be rejected");
        assert!(error.contains("stale"), "legible stale error: {error}");
        assert!(crate::page_slot_index(4, 4, 0, 4, 0, 5, "display output").is_ok());
    }

    #[test]
    fn out_of_range_page_slots_are_legible_errors_and_never_begin() {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let mut transport = RecordingTransport {
            requests: requests.clone(),
            ..RecordingTransport::default()
        };
        transport
            .responses
            .push_back(Ok(ControlResponse::Displays(twelve_mode_display_snapshot())));
        let mut application = SettingsApplication::new(transport);
        application.client_mut().refresh_displays().expect("display snapshot");
        let mut document = ProbeDocument::default();
        application
            .handle_action(&release("settings.display.modes.next"), &mut document)
            .expect("mode page 2");
        // Page 1 slot 7 is absolute index 15, past the 12 modes.
        let past_end = application
            .handle_action(&release("settings.display.slot.7"), &mut document)
            .expect_err("index 15 has no mode");
        assert!(past_end.contains("index 15"), "legible range error: {past_end}");
        // Slot 9 is outside the 8-slot page.
        let outside = application
            .handle_action(&release("settings.display.slot.9"), &mut document)
            .expect_err("slot 9 is outside the page");
        assert!(outside.contains("outside the page"), "legible slot error: {outside}");
        assert!(begun_modes(&requests).is_empty(), "rejected slots must not begin a mode");
    }

    #[test]
    fn pager_steps_clamp_at_the_first_and_last_page() {
        let mut transport = RecordingTransport::default();
        transport
            .responses
            .push_back(Ok(ControlResponse::Displays(twelve_mode_display_snapshot())));
        let mut application = SettingsApplication::new(transport);
        application.client_mut().refresh_displays().expect("display snapshot");
        let mut document = ProbeDocument::default();
        application
            .handle_action(&release("settings.display.modes.prev"), &mut document)
            .expect("prev on first page is a no-op");
        assert_eq!(document.texts["display-mode-page-label"], "1-8 of 12");
        assert_eq!(document.visible.get("display-mode-prev-off"), Some(&true));
        for _ in 0..2 {
            application
                .handle_action(&release("settings.display.modes.next"), &mut document)
                .expect("next page");
        }
        assert_eq!(document.texts["display-mode-page-label"], "9-12 of 12");
        assert_eq!(document.visible.get("display-mode-next-off"), Some(&true));
        assert_eq!(document.visible.get("display-mode-next"), Some(&false));
    }
}

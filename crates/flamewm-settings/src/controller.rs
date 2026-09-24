#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::{BTreeMap, VecDeque};
    use std::rc::Rc;

    use flamewm_api::display::{DisplayMode, DisplaySnapshot, OutputSnapshot};
    use flamewm_api::settings::{SettingValue, SettingsSnapshot};
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

        fn background(&mut self, _id: &str, _color: UiColor) -> Result<(), String> {
            Ok(())
        }

        fn border(&mut self, _id: &str, _color: UiColor) -> Result<(), String> {
            Ok(())
        }

        fn foreground(&mut self, _id: &str, _color: UiColor) -> Result<(), String> {
            Ok(())
        }

        fn background_clear(&mut self, _id: &str) -> Result<(), String> {
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

    fn recognized_action(action: &str) -> bool {
        matches!(
            action,
            "settings.page.appearance"
                | "settings.page.desktop"
                | "settings.page.taskbar"
                | "settings.page.displays"
                | "settings.page.fonts"
                | "settings.page.hotkeys"
                | "settings.page.about"
                | "settings.accent.red"
                | "settings.accent.blue"
                | "settings.accent.purple"
                | "settings.accent.teal"
                | "settings.accent.reset"
                | "settings.icon-theme.next"
                | "settings.wallpaper.reset"
                | "settings.wallpaper.choose"
                | "settings.selection-opacity.down"
                | "settings.selection-opacity.up"
                | "settings.snap-opacity.down"
                | "settings.snap-opacity.up"
                | "settings.taskbar.position"
                | "settings.taskbar.color"
                | "settings.taskbar.size"
                | "settings.taskbar-opacity.down"
                | "settings.taskbar-opacity.up"
                | "settings.font.family"
                | "settings.font-size.down"
                | "settings.font-size.up"
                | "settings.hotkey.start"
                | "settings.hotkey.left"
                | "settings.hotkey.right"
                | "settings.hotkey.up"
                | "settings.hotkey.down"
                | "settings.toggle.stickyNotesEnabled"
                | "settings.toggle.fontBold"
                | "settings.display.select.0"
                | "settings.display.select.1"
                | "settings.display.select.2"
                | "settings.display.select.3"
                | "settings.display.scale.100"
                | "settings.display.scale.125"
                | "settings.display.scale.150"
                | "settings.display.scale.175"
                | "settings.display.scale.200"
                | "about.donate"
                | "about.source"
                | "about.website"
        )
    }

    #[test]
    fn every_markup_action_is_in_the_controller_action_set() {
        for action in markup_actions() {
            assert!(
                recognized_action(action),
                "orphan action in settings markup: {action}"
            );
        }
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
    fn hotkey_controller_seam_preserves_escape_as_clear_binding() {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let mut transport = RecordingTransport {
            requests: requests.clone(),
            ..RecordingTransport::default()
        };
        transport.responses.push_back(Ok(ControlResponse::Unit));
        transport
            .responses
            .push_back(Ok(ControlResponse::Shortcuts(ShortcutSnapshot {
                revision: 2,
                bindings: BTreeMap::new(),
            })));
        let mut client = SettingsClient::new(transport);
        client.begin_hotkey_capture("ToggleStartMenu");
        assert!(client.capture_hotkey("Escape"));
        client.commit_hotkey_capture().expect("clear hotkey");
        assert!(matches!(
            requests.borrow().first(),
            Some(ControlRequest::ClearShortcut {
                action,
                expected_revision,
            })
                if action == "ToggleStartMenu" && *expected_revision == 0
        ));
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, VecDeque};

    use flamewm_api::display::{DisplayMode, DisplaySnapshot, OutputSnapshot};
    use flamewm_api::panels::PanelsSnapshot;
    use flamewm_api::settings::SettingsSnapshot;
    use flamewm_api::shortcuts::ShortcutSnapshot;
    use flamewm_api::{ModeId, OutputId, Rect, Size};
    use flamewm_control_core::{ControlError, ControlRequest, ControlResponse};
    use flamewm_settings_core::ControlTransport;

    use crate::SettingsApplication;

    #[derive(Default)]
    struct RefreshTransport {
        responses: VecDeque<Result<ControlResponse, ControlError>>,
    }

    impl ControlTransport for RefreshTransport {
        fn call(&mut self, _request: ControlRequest) -> Result<ControlResponse, ControlError> {
            self.responses
                .pop_front()
                .expect("projection test response was not queued")
        }
    }

    fn transport() -> RefreshTransport {
        let mut settings = SettingsSnapshot::new(1);
        settings.values.insert(
            "stickyNotesEnabled".to_owned(),
            flamewm_api::settings::SettingValue::Boolean(false),
        );
        settings.values.insert(
            "fontBold".to_owned(),
            flamewm_api::settings::SettingValue::Boolean(false),
        );
        let mut transport = RefreshTransport::default();
        transport
            .responses
            .push_back(Ok(ControlResponse::Settings(settings)));
        let mode = ModeId(7);
        transport
            .responses
            .push_back(Ok(ControlResponse::Displays(DisplaySnapshot {
                generation: 1,
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
            })));
        transport
            .responses
            .push_back(Ok(ControlResponse::Shortcuts(ShortcutSnapshot {
                revision: 1,
                bindings: BTreeMap::new(),
            })));
        transport
            .responses
            .push_back(Ok(ControlResponse::Panels(PanelsSnapshot {
                revision: 1,
                panels: Vec::new(),
                tasks: Vec::new(),
                pinned_apps: Vec::new(),
            })));
        transport
    }

    #[test]
    fn compiled_settings_projection_accepts_every_authoritative_text_target() {
        let mut application = SettingsApplication::new(transport());
        let mut document = crate::view::document().expect("compiled settings document");
        application
            .refresh(&mut document)
            .expect("settings projection must accept its compiled text targets");
    }
}

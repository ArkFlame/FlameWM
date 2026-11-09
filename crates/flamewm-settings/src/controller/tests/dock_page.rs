use super::*;

// OBSERVABLE: through the real refresh projection, the dock option group follows "Enable dock",
// "Reserve space" additionally needs auto-hide off, and the custom surface rows need the custom switch.
// MUTATION: projecting `dock-reserve-row` from `enabled` alone, or dropping `dock-options`, fails here.
#[test]
fn dock_rows_follow_enable_dock_and_auto_hide_through_refresh() {
    // (enabled, auto_hide, surface_custom) -> (options, reserve_space, surface_body)
    let cases = [
        ((false, false, false), (false, false, false)),
        ((false, true, true), (false, false, false)),
        ((true, true, false), (true, false, false)),
        ((true, false, true), (true, true, true)),
    ];
    for ((enabled, auto_hide, custom), (options, reserve, surface)) in cases {
        let mut settings = SettingsSnapshot::new(2);
        settings
            .values
            .insert("dock.enabled".to_owned(), SettingValue::Boolean(enabled));
        settings
            .values
            .insert("dock.autoHide".to_owned(), SettingValue::Boolean(auto_hide));
        settings.values.insert(
            "dock.surfaceCustom".to_owned(),
            SettingValue::Boolean(custom),
        );
        let mut transport = RecordingTransport::default();
        transport
            .responses
            .push_back(Ok(ControlResponse::Settings(settings)));
        queue_displays_shortcuts_panels(&mut transport);
        let mut application = SettingsApplication::new(transport);
        let mut document = ProbeDocument::default();
        application.refresh(&mut document).expect("refresh");
        let case = format!("enabled={enabled} autoHide={auto_hide} custom={custom}");
        assert_eq!(
            document.visible.get("dock-options"),
            Some(&options),
            "{case}"
        );
        assert_eq!(
            document.visible.get("dock-reserve-row"),
            Some(&reserve),
            "{case}"
        );
        assert_eq!(
            document.visible.get("dock-surface-body"),
            Some(&surface),
            "{case}"
        );
    }
}

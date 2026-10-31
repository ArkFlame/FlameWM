use super::*;

#[test]
fn taskbar_output_is_the_shared_selected_output_and_is_labelled() {
    let mut transport = RecordingTransport::default();
    transport
        .responses
        .push_back(Ok(ControlResponse::Displays(two_output_display_snapshot())));
    let mut application = displays_application(transport);
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
    transport
        .responses
        .push_back(Ok(ControlResponse::Settings(settings)));
    queue_displays_shortcuts_panels(&mut transport);
    let mut application = SettingsApplication::new(transport);
    let mut document = ProbeDocument::default();
    application.refresh(&mut document).expect("refresh");
    assert_eq!(document.texts["taskbar-size-value"], "52 px (all outputs)");
    assert_eq!(
        document.texts["about-version"],
        format!("Version {}", env!("FLAMEWM_SETTINGS_VERSION"))
    );
}

#[test]
fn about_version_text_is_exact_flamewm_version() {
    let version = env!("FLAMEWM_SETTINGS_VERSION");
    let mut transport = RecordingTransport::default();
    transport
        .responses
        .push_back(Ok(ControlResponse::Settings(SettingsSnapshot::new(2))));
    queue_displays_shortcuts_panels(&mut transport);
    let mut application = SettingsApplication::new(transport);
    let mut document = ProbeDocument::default();
    assert!(
        !document.texts.contains_key("about-version"),
        "no runtime about version before refresh"
    );
    application.refresh(&mut document).expect("refresh");
    assert_eq!(
        document.texts["about-version"],
        format!("Version {version}")
    );
    assert_ne!(
        document.texts["about-version"], version,
        "bare version must not match"
    );
    assert_ne!(document.texts["about-version"], "Version 0.0.0-wrong");
}

#[test]
fn about_links_map_to_the_fixed_product_uris_only() {
    assert!(matches!(
        crate::parse_action("about.source"),
        Ok(crate::SettingsAction::OpenUri(
            "https://github.com/ArkFlame/flamewm"
        ))
    ));
    assert!(crate::parse_action("about.other").is_err());
}

#[test]
fn lock_preview_action_parses_to_the_preview_request() {
    assert!(matches!(
        crate::parse_action("settings.lock.preview"),
        Ok(crate::SettingsAction::LockPreview)
    ));
    assert!(crate::parse_action("settings.lock.preview.extra").is_err());
}

#[test]
fn screensaver_preview_action_parses_separately_from_lock_preview() {
    assert!(matches!(
        crate::parse_action("settings.screensaver.preview"),
        Ok(crate::SettingsAction::ScreensaverPreview)
    ));
    assert!(crate::parse_action("settings.screensaver.preview.extra").is_err());
}

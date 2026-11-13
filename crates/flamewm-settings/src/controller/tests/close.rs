use super::*;

fn key(
    phase: flamewm_ui_x11::UiKeyPhase,
    key: &str,
    modifiers: &[&str],
) -> flamewm_ui_x11::UiKeyEvent {
    flamewm_ui_x11::UiKeyEvent {
        phase,
        key: key.to_owned(),
        modifiers: modifiers
            .iter()
            .map(|modifier| (*modifier).to_owned())
            .collect(),
        text: None,
    }
}

/// Settings controller whose refresh is answered by an empty snapshot.
fn closable_application() -> SettingsApplication<RecordingTransport> {
    let (_requests, mut transport) = recording_transport();
    transport
        .responses
        .push_back(Ok(ControlResponse::Settings(SettingsSnapshot::new(7))));
    settings_application(transport)
}

#[test]
fn ctrl_q_press_closes_the_window_once() {
    // Precedent: KDE systemsettings closes on Ctrl+Q (KStandardShortcut::Quit).
    let mut application = closable_application();
    let mut document = ProbeDocument::default();
    let press = key(flamewm_ui_x11::UiKeyPhase::Press, "q", &["Ctrl"]);
    application
        .handle_key_event(&press, &mut document)
        .expect("ctrl+q");
    assert_eq!(document.close_requests, 1);
    let release = key(flamewm_ui_x11::UiKeyPhase::Release, "q", &["Ctrl"]);
    application
        .handle_key_event(&release, &mut document)
        .expect("release");
    assert_eq!(
        document.close_requests, 1,
        "the release does not close again"
    );
}

#[test]
fn plain_q_and_escape_do_not_close_the_window() {
    let mut application = closable_application();
    let mut document = ProbeDocument::default();
    for event in [
        key(flamewm_ui_x11::UiKeyPhase::Press, "q", &[]),
        key(flamewm_ui_x11::UiKeyPhase::Press, "Escape", &[]),
    ] {
        application
            .handle_key_event(&event, &mut document)
            .expect("key");
    }
    assert_eq!(document.close_requests, 0);
}

#[test]
fn escape_clears_the_focused_sidebar_search() {
    let mut application = closable_application();
    let mut document = ProbeDocument::default();
    application.nav_search_focused = true;
    application.nav_query = "ab".to_owned();
    let release = key(flamewm_ui_x11::UiKeyPhase::Release, "Escape", &[]);
    application
        .handle_key_event(&release, &mut document)
        .expect("escape");
    assert!(application.nav_query.is_empty());
    assert!(!application.nav_search_focused);
    assert_eq!(document.close_requests, 0);
}

#[test]
fn escape_clears_the_search_text_the_sidebar_shows() {
    // The filter change must reach the document the window paints: the search text returns to
    // the placeholder and the "no matches" row hides, in the same settle as the state change.
    let mut application = closable_application();
    let mut document = ProbeDocument::default();
    application.nav_search_focused = true;
    application.nav_query = "z".to_owned();
    application
        .handle_key_event(
            &key(flamewm_ui_x11::UiKeyPhase::Release, "Escape", &[]),
            &mut document,
        )
        .expect("escape");
    assert_eq!(
        document.texts.get("nav-search-text").map(String::as_str),
        Some("Search")
    );
    assert_eq!(document.visible.get("nav-empty"), Some(&false));
}

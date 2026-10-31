use super::*;

fn api_key_session(
    initial: &str,
    applies: usize,
) -> (
    SettingsApplication<RecordingTransport>,
    Rc<RefCell<Vec<ControlRequest>>>,
) {
    let (requests, mut transport) = recording_transport();
    let mut snapshot = SettingsSnapshot::new(7);
    snapshot.values.insert(
        "assistant.apiKey".to_owned(),
        SettingValue::Text(initial.to_owned()),
    );
    for _ in 0..=applies {
        transport
            .responses
            .push_back(Ok(ControlResponse::Settings(snapshot.clone())));
    }
    (settings_application(transport), requests)
}

#[test]
fn api_key_field_is_masked_until_focused() {
    let (mut application, _requests) = api_key_session("ark_23bRexample", 2);
    let mut document = ProbeDocument::default();
    application
        .handle_action(&release("settings.assistant.key.focus"), &mut document)
        .expect("focus");
    assert_eq!(
        document.texts.get("assistant-api-key").map(String::as_str),
        Some("ark_23bRexample|"),
        "focus shows the full editable key"
    );
    application
        .handle_action(&typed("\u{1b}"), &mut document)
        .expect("escape");
    assert_eq!(
        document.texts.get("assistant-api-key").map(String::as_str),
        Some("ark_23bR\u{2022}\u{2022}\u{2022}\u{2022}"),
        "unfocused key shows the first 8 characters and dots"
    );
}

#[test]
fn api_key_invalid_draft_is_shown_inline_and_not_saved() {
    let (mut application, requests) = api_key_session("ark_23bRexample", 3);
    let mut document = ProbeDocument::default();
    application
        .handle_action(&release("settings.assistant.key.focus"), &mut document)
        .expect("focus");
    for _ in 0..40 {
        application
            .handle_action(&typed("\u{8}"), &mut document)
            .expect("backspace");
    }
    for text in ["sk-live:1", "\n"] {
        application
            .handle_action(&typed(text), &mut document)
            .expect("typed");
    }
    assert_eq!(
        last_apply(&requests, "assistant.apiKey"),
        None,
        "invalid key writes nothing"
    );
    assert!(
        document
            .texts
            .get("assistant-api-key-error")
            .is_some_and(|text| text.contains("ark_")),
        "invalid key shows the inline message"
    );
}

#[test]
fn api_key_valid_draft_saves_and_escape_discards_and_default_clears() {
    let (mut application, requests) = api_key_session("ark_23bRexample", 4);
    let mut document = ProbeDocument::default();
    application
        .handle_action(&release("settings.assistant.key.focus"), &mut document)
        .expect("focus");
    for _ in 0..40 {
        application
            .handle_action(&typed("\u{8}"), &mut document)
            .expect("backspace");
    }
    for text in "  ark_Key_2:9 ".chars() {
        application
            .handle_action(&typed(&text.to_string()), &mut document)
            .expect("typed");
    }
    application
        .handle_action(&typed("\n"), &mut document)
        .expect("enter");
    assert_eq!(
        last_apply(&requests, "assistant.apiKey").as_deref(),
        Some("ark_Key_2:9"),
        "valid key is trimmed and saved on Enter"
    );

    application
        .handle_action(&release("settings.assistant.key.focus"), &mut document)
        .expect("refocus");
    application
        .handle_action(&typed("x"), &mut document)
        .expect("typed");
    application
        .handle_action(&typed("\u{1b}"), &mut document)
        .expect("escape");
    assert_eq!(
        document.texts.get("assistant-api-key").map(String::as_str),
        Some("ark_23bR\u{2022}\u{2022}\u{2022}\u{2022}"),
        "Escape discards the draft and shows the saved key masked"
    );

    application
        .handle_action(&release("settings.assistant.key.reset"), &mut document)
        .expect("reset");
    assert_eq!(
        last_apply(&requests, "assistant.apiKey").as_deref(),
        Some(""),
        "use default key clears the key"
    );
}

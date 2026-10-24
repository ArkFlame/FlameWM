    use super::*;

    #[test]
    fn desktop_tokens_follow_live_accent_and_fall_back_to_neutral() {
        let mut snapshot = SettingsSnapshot::new(1);
        assert_eq!(accent_color(&snapshot).r, 0x7f);
        snapshot.values.insert(
            "accent".to_owned(),
            SettingValue::Text("#9b59b6".to_owned()),
        );
        let tokens = accent_token_values(&snapshot);
        let value = |name: &str| tokens.iter().find(|(key, _)| *key == name).map(|(_, c)| *c);
        assert_eq!(
            value("--desktop-hover"),
            Some(UiColor {
                r: 0x9b,
                g: 0x59,
                b: 0xb6,
                a: 0x26
            })
        );
        assert_eq!(value("--snap-fill").map(|c| c.a), Some(0x33));
        assert_eq!(value("--accent"), Some(accent_color(&snapshot)));
        assert_eq!(tokens.len(), ACCENT_TOKEN_ALPHAS.len());
    }

    #[test]
    fn embedded_desktop_document_has_all_required_convert_nodes() {
        let document = decode_document(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/flamewm-desktop.rwr"
        )))
        .expect("decode embedded compiled RWR");
        ui_contract::require_desktop_nodes(|id| document.has_node(id))
            .expect("embedded desktop UI must satisfy runtime ID contract");
    }

    #[test]
    fn desktop_compiled_document_declares_every_accent_token() {
        let mut document = decode_document(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/flamewm-desktop.rwr"
        )))
        .expect("decode compiled desktop document");
        project_accent_tokens(&mut document, &SettingsSnapshot::new(1))
            .expect("every accent token is declared in ui/desktop :root");
    }

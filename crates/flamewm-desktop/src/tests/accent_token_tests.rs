use super::*;

#[test]
fn desktop_tokens_follow_live_accent_and_fall_back_to_product_default() {
    let mut snapshot = SettingsSnapshot::new(1);
    assert_eq!(accent_color(&snapshot).r, 0x53);
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
fn preview_outline_uses_the_shared_opaque_accent_and_two_pixel_css_style() {
    let mut snapshot = SettingsSnapshot::new(1);
    snapshot
        .values
        .insert("accent".into(), SettingValue::Text("#123456".into()));
    snapshot
        .values
        .insert("accentOpacity".into(), SettingValue::Percent(10));
    let mut document = flamewm_ui_x11::test_support::RecordingUiDoc::default();
    project_accent_tokens(&mut document, &snapshot).expect("project accent tokens");
    assert_eq!(
        document.variables["--preview-outline"],
        UiColor {
            r: 0x12,
            g: 0x34,
            b: 0x56,
            a: flamewm_ui_core::style::PREVIEW_OUTLINE_ALPHA,
        }
    );

    assert_eq!(
        flamewm_ui_core::style::PREVIEW_OUTLINE_WIDTH_PX,
        2,
        "the canonical outline is two pixels wide"
    );
    let desktop_css = include_str!("../../../../ui/desktop/desktop.css");
    let explorer_css = include_str!("../../../../ui/explorer/explorer.css");
    assert!(desktop_css.contains(
        ".selection { position:absolute; display:block; border:2px solid var(--preview-outline); }"
    ));
    assert!(desktop_css.contains(".sticky-move-preview { position:absolute; left:0px; top:0px; width:0px; height:0px; box-sizing:border-box; background:transparent; border:2px solid var(--preview-outline); border-radius:5px; }"));
    assert!(explorer_css.contains(".selection-rectangle { position:absolute; background:transparent; border:2px solid var(--preview-outline); border-radius:3px; }"));
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

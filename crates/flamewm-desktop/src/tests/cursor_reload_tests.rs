use flamewm_session_core::cursor_theme_reload;

#[test]
fn cursor_reload_only_when_resolved_theme_changes() {
    let root = std::env::temp_dir().join(format!("flamewm-desktop-cursor-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Custom").join("cursors")).expect("theme");
    let search = root.display().to_string();
    let first = cursor_theme_reload(None, "Custom", &search);
    assert_eq!(first.as_deref(), Some("Custom"));
    assert_eq!(
        cursor_theme_reload(first.as_deref(), "Custom", &search),
        None
    );
    // An uninstalled request resolves to the same fallback as the applied theme: no reload.
    let fallback = flamewm_session_core::resolve_cursor_theme("Missing", &search);
    assert_eq!(
        cursor_theme_reload(Some(&fallback), "Missing", &search),
        None
    );
    let _ = std::fs::remove_dir_all(&root);
}

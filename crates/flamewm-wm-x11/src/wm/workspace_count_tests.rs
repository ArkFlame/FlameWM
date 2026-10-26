use super::WmConfig;

#[test]
fn persisted_count_applies_only_without_env_override() {
    let base = WmConfig::default();
    assert_eq!(base.resolve_workspaces(false, Some(7)).workspaces, 7);
    assert_eq!(base.resolve_workspaces(true, Some(7)).workspaces, 4);
    assert_eq!(base.resolve_workspaces(false, None).workspaces, 4);
}

#[test]
fn start_path_reads_persisted_count_from_settings_dir() {
    let dir = std::env::temp_dir().join(format!("flamewm-wm-start-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let settings = dir.join("settings.toml");
    std::fs::write(dir.join("workspaces.conf"), "workspaces.count=6\n").expect("write");
    let config = WmConfig::default().with_persisted_workspaces(&settings);
    assert_eq!(config.workspaces, 6);
    assert_eq!(
        WmConfig::default()
            .resolve_workspaces(true, Some(6))
            .workspaces,
        4
    );
    let _ = std::fs::remove_dir_all(&dir);
}

use super::*;

fn load(text: &str) -> FlameResult<ProductSettings> {
    ConfigStore::default().parse_snapshot(text)
}

#[test]
fn dock_and_taskbar_defaults() {
    let d = ProductSettings::default();
    assert!(!d.dock_enabled);
    assert_eq!(d.dock_position, DockPosition::Bottom);
    assert_eq!(d.dock_icon_size, 44);
    assert!(d.dock_pinned.is_empty());
    assert!(d.dock_show_running);
    assert!(d.dock_auto_hide);
    assert!(d.taskbar_enabled);
    assert_eq!(d.taskbar_fill, TaskbarFill::Solid);
    assert_eq!(d.taskbar_gradient_edge, TaskbarGradientEdge::Auto);
    assert_eq!(d.taskbar_style, TaskbarStyle::Classic);
}

#[test]
fn default_dock_and_taskbar_keys_are_not_written() {
    let text = ConfigStore::serialize(&ProductSettings::default());
    for key in [
        "dock.",
        "taskbar.enabled",
        "taskbar.fill",
        "taskbar.gradientEdge",
    ] {
        assert!(!text.contains(key), "default wrote {key}");
    }
}

#[test]
fn dock_keys_parse_and_round_trip() {
    let text = "revision=2\ndock.enabled=1\ndock.position=left\ndock.iconSize=64\n\
                dock.pinned=org.x.Term,firefox,org.x.Term\ndock.showRunning=0\n\
                dock.autoHide=0\ntaskbar.enabled=0\ntaskbar.fill=gradient\ntaskbar.gradientEdge=top\n";
    let parsed = load(text).expect("valid dock file");
    assert!(parsed.dock_enabled);
    assert_eq!(parsed.dock_position, DockPosition::Left);
    assert_eq!(parsed.dock_icon_size, 64);
    assert_eq!(
        parsed.dock_pinned,
        vec!["org.x.Term".to_owned(), "firefox".to_owned()]
    );
    assert!(!parsed.dock_show_running);
    assert!(!parsed.dock_auto_hide);
    assert!(!parsed.taskbar_enabled);
    assert_eq!(parsed.taskbar_fill, TaskbarFill::Gradient);
    assert_eq!(parsed.taskbar_gradient_edge, TaskbarGradientEdge::Top);
    let reloaded = load(&ConfigStore::serialize(&parsed)).expect("reload");
    assert_eq!(reloaded, parsed);
}

#[test]
fn invalid_dock_values_are_rejected() {
    for text in [
        "revision=2\ndock.position=middle\n",
        "revision=2\ntaskbar.fill=dotted\n",
        "revision=2\ntaskbar.gradientEdge=center\n",
        "revision=2\ndock.iconSize=wide\n",
        "revision=2\ndock.autoHide=maybe\n",
        "revision=2\ndock.pinned=ok,bad/slash\n",
    ] {
        let error = load(text).expect_err(text);
        assert_eq!(error.code, ErrorCode::InvalidArgument, "{text}");
    }
}

#[test]
fn icon_size_out_of_range_fails_validation() {
    let mut d = ProductSettings {
        dock_icon_size: 16,
        ..ProductSettings::default()
    };
    assert!(d.validate().is_err());
    d.dock_icon_size = 96;
    assert!(d.validate().is_ok());
}

#[test]
fn pinned_list_is_capped_on_load() {
    let ids: Vec<String> = (0..40).map(|i| format!("app{i}")).collect();
    let parsed = load(&format!("revision=2\ndock.pinned={}\n", ids.join(","))).expect("capped");
    assert_eq!(parsed.dock_pinned.len(), DOCK_PINNED_MAX);
}

#[test]
fn app_id_validation_rules() {
    assert!(is_valid_dock_app_id("org.gnome.Terminal"));
    assert!(is_valid_dock_app_id("my-app_2"));
    assert!(!is_valid_dock_app_id(""));
    assert!(!is_valid_dock_app_id("a b"));
    assert!(!is_valid_dock_app_id("a=b"));
}

#[test]
fn dock_and_taskbar_keys_are_not_reset_by_the_custom_panel_switch() {
    let settings = ProductSettings {
        panel_custom: false,
        dock_enabled: true,
        dock_position: DockPosition::Left,
        dock_icon_size: 64,
        dock_show_running: false,
        dock_pinned: vec!["firefox".to_owned()],
        taskbar_fill: TaskbarFill::Gradient,
        taskbar_gradient_edge: TaskbarGradientEdge::Top,
        taskbar_enabled: false,
        ..ProductSettings::default()
    };
    let effective = settings.effective();
    assert!(effective.dock_enabled);
    assert_eq!(effective.dock_position, DockPosition::Left);
    assert_eq!(effective.dock_icon_size, 64);
    assert!(!effective.dock_show_running);
    assert_eq!(effective.dock_pinned, vec!["firefox".to_owned()]);
    assert_eq!(effective.taskbar_fill, TaskbarFill::Gradient);
    assert_eq!(effective.taskbar_gradient_edge, TaskbarGradientEdge::Top);
    assert!(!effective.taskbar_enabled);
}

#[test]
fn auto_hide_round_trips_and_defaults_stay_unwritten() {
    let text = ConfigStore::serialize(&ProductSettings::default());
    assert!(
        !text.contains("dock.autoHide"),
        "default wrote dock.autoHide"
    );
    let off = ProductSettings {
        dock_auto_hide: false,
        ..ProductSettings::default()
    };
    let written = ConfigStore::serialize(&off);
    assert!(written.contains("dock.autoHide=false"), "{written}");
    let parsed = load(&written).expect("round trip");
    assert!(!parsed.dock_auto_hide);
    assert!(load("revision=2\n").expect("defaults").dock_auto_hide);
}

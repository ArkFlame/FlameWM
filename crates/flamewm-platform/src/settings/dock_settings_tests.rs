use super::*;

fn load(text: &str) -> FlameResult<ProductSettings> {
    ConfigStore::default().parse_snapshot(text)
}

#[test]
fn dock_and_taskbar_defaults() {
    let d = ProductSettings::default();
    assert!(!d.dock_enabled);
    assert_eq!(d.dock_position, DockPosition::Bottom);
    assert_eq!(d.dock_icon_size, 32);
    assert!(d.dock_pinned.is_empty());
    assert!(d.dock_show_running);
    assert!(!d.dock_auto_hide);
    assert!(!d.dock_reserve_space);
    assert_eq!(d.dock_corner_style, DockCornerStyle::Rounded);
    assert!(!d.dock_surface_custom);
    assert!(d.taskbar_enabled);
    assert_eq!(d.taskbar_fill, TaskbarFill::Solid);
    assert_eq!(d.taskbar_gradient_edge, TaskbarGradientEdge::Auto);
    assert_eq!(d.taskbar_style, TaskbarStyle::Classic);
}

#[test]
fn default_dock_and_taskbar_keys_are_not_written() {
    let text = ConfigStore::serialize(&ProductSettings::default());
    for key in [
        "dock.position",
        "dock.iconSize",
        "dock.pinned",
        "dock.cornerStyle",
        "dock.surfaceCustom",
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
fn auto_hide_and_reserve_are_always_written_and_round_trip() {
    let text = ConfigStore::serialize(&ProductSettings::default());
    assert!(text.contains("dock.autoHide=0\n"), "{text}");
    assert!(text.contains("dock.reserveSpace=0\n"), "{text}");
    let on = ProductSettings {
        dock_auto_hide: true,
        dock_reserve_space: true,
        ..ProductSettings::default()
    };
    let written = ConfigStore::serialize(&on);
    assert!(written.contains("dock.autoHide=1\n"), "{written}");
    let parsed = load(&written).expect("round trip");
    assert!(parsed.dock_auto_hide);
    assert!(parsed.dock_reserve_space);
}

#[test]
fn legacy_file_without_auto_hide_key_keeps_auto_hide_and_no_reserve() {
    // Old default: a missing dock.autoHide meant on, and the strip was reserved only when off.
    let legacy = load("revision=2\ndock.enabled=1\n").expect("legacy");
    assert!(legacy.dock_auto_hide);
    assert!(!legacy.dock_reserve_space);
}

#[test]
fn legacy_explicit_auto_hide_off_keeps_the_reserved_strip() {
    let legacy = load("revision=2\ndock.enabled=1\ndock.autoHide=0\n").expect("legacy off");
    assert!(!legacy.dock_auto_hide);
    assert!(legacy.dock_reserve_space, "legacy off reserved the strip");
}

#[test]
fn explicit_reserve_key_wins_over_the_legacy_rule() {
    let explicit = load("revision=2\ndock.autoHide=0\ndock.reserveSpace=0\n").expect("explicit");
    assert!(!explicit.dock_reserve_space);
    let on = load("revision=2\ndock.reserveSpace=1\n").expect("on");
    assert!(on.dock_reserve_space);
}

#[test]
fn corner_style_parses_round_trips_and_rejects_unknown_values() {
    let squared = load("revision=2\ndock.cornerStyle=squared\n").expect("squared");
    assert_eq!(squared.dock_corner_style, DockCornerStyle::Squared);
    let reloaded = load(&ConfigStore::serialize(&squared)).expect("reload");
    assert_eq!(reloaded.dock_corner_style, DockCornerStyle::Squared);
    assert!(load("revision=2\ndock.cornerStyle=round\n").is_err());
}

#[test]
fn dock_surface_overrides_parse_and_stay_unwritten_by_default() {
    let text = ConfigStore::serialize(&ProductSettings::default());
    assert!(!text.contains("dock.surfaceColor"), "{text}");
    let custom = load(
        "revision=2\ndock.surfaceCustom=1\ndock.surfaceColor=#112233\ndock.surfaceOpacity=40\n\
         dock.surfaceBlurPercent=10\ndock.surfaceOutlineColor=#445566\ndock.surfaceOutlineOpacity=20\n\
         dock.surfaceOutlineWidth=3\n",
    )
    .expect("custom surface");
    assert!(custom.dock_surface_custom);
    assert_eq!(custom.dock_surface_color, "#112233");
    assert_eq!(custom.dock_surface_opacity, 40);
    assert_eq!(custom.dock_surface_blur_percent, 10);
    assert_eq!(custom.dock_surface_outline_color, "#445566");
    assert_eq!(custom.dock_surface_outline_opacity, 20);
    assert_eq!(custom.dock_surface_outline_width, 3);
    let reloaded = load(&ConfigStore::serialize(&custom)).expect("reload");
    assert_eq!(reloaded, custom);
}

#[test]
fn dock_surface_ranges_fail_validation() {
    let base = ProductSettings::default();
    for (edit, label) in [
        (
            ProductSettings {
                dock_surface_opacity: 101,
                ..base.clone()
            },
            "opacity",
        ),
        (
            ProductSettings {
                dock_surface_blur_percent: 101,
                ..base.clone()
            },
            "blur",
        ),
        (
            ProductSettings {
                dock_surface_outline_width: 0,
                ..base.clone()
            },
            "width low",
        ),
        (
            ProductSettings {
                dock_surface_outline_width: 5,
                ..base.clone()
            },
            "width high",
        ),
    ] {
        assert!(edit.validate().is_err(), "{label}");
    }
    assert!(base.validate().is_ok());
}

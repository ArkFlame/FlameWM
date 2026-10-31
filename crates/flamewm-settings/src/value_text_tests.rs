use super::*;

macro_rules! assert_action {
    ($action:expr, $($pat:tt)+) => {
        assert!(matches!(parse_action($action), $($pat)+))
    };
}

#[test]
fn avatar_chooser_targets_avatar_media_and_setting() {
    assert_eq!(ChooserTarget::Avatar.media_kind(), MediaKind::Avatar);
    assert_eq!(
        ChooserTarget::LockBackground.media_kind(),
        MediaKind::LockWallpaper
    );
    assert_eq!(
        ChooserTarget::Wallpaper.media_kind(),
        MediaKind::DesktopWallpaper
    );
    assert_eq!(ChooserTarget::StartIcon.media_kind(), MediaKind::StartIcon);
}

#[test]
fn avatar_actions_parse_to_choose_and_clear() {
    assert!(matches!(
        parse_action("settings.avatar.choose"),
        Ok(SettingsAction::AvatarChoose)
    ));
    assert!(matches!(
        parse_action("settings.avatar.clear"),
        Ok(SettingsAction::AvatarClear)
    ));
    assert!(parse_action("settings.avatar.reset").is_err());
}

#[test]
fn avatar_label_shows_file_name_or_default() {
    assert_eq!(avatar_label(""), "Default");
    assert_eq!(
        avatar_label("/home/u/.local/share/flamewm/wallpapers/avatar-0123456789abcdef.png"),
        "avatar-0123456789abcdef.png"
    );
}

#[test]
fn theme_picker_actions_parse_to_typed_targets() {
    use theme_picker::PickerKind;
    assert_action!(
        "settings.theme.icon.open",
        Ok(SettingsAction::ThemePickerOpen(PickerKind::Icon))
    );
    assert_action!(
        "settings.theme.cursor.next",
        Ok(SettingsAction::ThemePickerPage(
            PickerKind::Cursor,
            PageStep::Next
        ))
    );
    assert_action!(
        "settings.theme.icon.select.5",
        Ok(SettingsAction::ThemePickerSelect(PickerKind::Icon, 5))
    );
    assert!(matches!(
        parse_action("settings.theme.close"),
        Ok(SettingsAction::ThemePickerClose)
    ));
    assert_action!(
        "settings.theme.install",
        Ok(SettingsAction::ThemeInstallChoose { folder: false })
    );
    assert_action!(
        "settings.theme.install-folder",
        Ok(SettingsAction::ThemeInstallChoose { folder: true })
    );
}

#[test]
fn theme_picker_actions_reject_out_of_range_and_retired_cycle() {
    for action in [
        "settings.theme.icon.select.6",
        "settings.theme.icon.select.x",
        "settings.theme.font.open",
        "settings.theme.icon.sideways",
        "settings.icon-theme.next",
    ] {
        assert!(parse_action(action).is_err(), "{action} must be rejected");
    }
}

#[test]
fn selecting_a_theme_writes_its_stable_id_and_closes_the_picker() {
    use flamewm_api::settings::{SettingValue, SettingsChange, SettingsSnapshot};
    use flamewm_control_core::{ControlRequest, ControlResponse};
    use test_transport::ScriptedTransport;
    use theme_picker::{PickerKind, ThemeItem, ThemeRegistry};

    let item = |n: usize| ThemeItem {
        id: format!("theme-{n}"),
        name: format!("Theme {n}"),
        preview: None,
    };
    let transport = ScriptedTransport {
        responses: (0..8)
            .map(|_| Ok(ControlResponse::Settings(SettingsSnapshot::new(1))))
            .collect(),
        ..ScriptedTransport::default()
    };
    let requests = transport.requests.clone();
    let mut app = SettingsApplication::new(transport);
    app.themes = ThemeRegistry {
        icon: (0..8).map(item).collect(),
        cursor: Vec::new(),
    };

    // Selecting without an open picker is rejected before any request.
    assert!(
        app.execute(SettingsAction::ThemePickerSelect(PickerKind::Icon, 0))
            .is_err()
    );
    assert!(requests.borrow().is_empty());

    app.execute(SettingsAction::ThemePickerOpen(PickerKind::Icon))
        .expect("open");
    app.execute(SettingsAction::ThemePickerPage(
        PickerKind::Icon,
        PageStep::Next,
    ))
    .expect("page");
    app.execute(SettingsAction::ThemePickerSelect(PickerKind::Icon, 1))
        .expect("select");

    let requests = requests.borrow();
    let transaction = match requests.last() {
        Some(ControlRequest::ApplySettings(transaction)) => transaction,
        other => panic!("expected one ApplySettings, got {other:?}"),
    };
    assert_eq!(
        transaction.changes,
        vec![SettingsChange {
            key: "iconTheme".to_owned(),
            value: Some(SettingValue::Text("theme-7".to_owned())),
        }]
    );
    assert!(app.theme_picker.is_none(), "selection closes the picker");
}

#[test]
fn value_text_mapping_is_authoritative() {
    assert_eq!(panel_edge_label(PanelEdge::Right), "Right");
    assert_eq!(font_size_label(0), "Default");
    assert_eq!(font_size_label(2), "+2");
    assert_eq!(font_size_label(-1), "-1");
}

#[test]
fn taskbar_size_cycle_wraps_in_both_directions() {
    let sizes = [34_i64, 44, 52, 64, 72];
    assert_eq!(cycle_value(&sizes, 72, 1), 34);
    assert_eq!(cycle_value(&sizes, 34, -1), 72);
    assert_eq!(cycle_value(&sizes, 44, 1), 52);
}

#[test]
fn color_swatches_and_coarse_steps_parse_to_fixed_typed_targets() {
    assert_action!(
        "settings.color.surface.preset.0",
        Ok(SettingsAction::ColorSwatch {
            key: "surfaceColor",
            value: "#191B1D"
        })
    );
    assert!(
        parse_action("settings.color.surface.preset.6").is_err(),
        "only six surface presets exist"
    );
    assert!(parse_action("settings.color.taskbar.preset.0").is_err());
    assert_action!(
        "settings.color.lockbackground.preset.5",
        Ok(SettingsAction::ColorSwatch {
            key: "lockBackgroundColor",
            value: "#9B59B6"
        })
    );
    assert!(
        parse_action("settings.picker.open.titlebar").is_err(),
        "titlebar has no colour target"
    );
    assert!(parse_action("settings.color.titlebar.preset.0").is_err());
    assert_action!("settings.picker.pick.0.0", Ok(SettingsAction::ColorPickerPick(value)) if value == "#F7C9C9");
    assert_action!("settings.picker.pick.grey.5", Ok(SettingsAction::ColorPickerPick(value)) if value == "#FFFFFF");
    for action in [
        "settings.color.taskbar.preset.9",
        "settings.color.taskbar.preset.x",
        "settings.color.desktop.preset.0",
        "settings.color.accent.r.step.32",
        "settings.picker.open.desktop",
        "settings.picker.pick.12.0",
        "settings.picker.pick.grey.6",
    ] {
        assert!(parse_action(action).is_err(), "{action} must be rejected");
    }
    assert_eq!(
        animation_style_node("startAnimation", "magic-lamp"),
        "start-style-magic-lamp"
    );
    assert_eq!(
        animation_style_node("windowOpenAnimation", "scale"),
        "window-open-style-scale"
    );
    assert_eq!(
        animation_style_node("windowCloseAnimation", "fade"),
        "window-close-style-fade"
    );
}

#[test]
fn appearance_actions_parse_to_their_exact_typed_targets() {
    assert!(parse_action("settings.start-menu-opacity.up").is_err());
    assert_action!(
        "settings.accent-opacity.down",
        Ok(SettingsAction::PercentStep("accentOpacity", -1))
    );
    assert_action!(
        "settings.toggle.maximizeAnimationEnabled",
        Ok(SettingsAction::Toggle("maximizeAnimationEnabled"))
    );
    assert_action!(
        "settings.titlebar.button-style.square",
        Ok(SettingsAction::TextSelect {
            key: "titlebarButtonStyle",
            value: "square"
        })
    );
    assert!(parse_action("settings.titlebar.minimize.style").is_err());
    assert!(parse_action("settings.titlebar.button-style.cycle").is_err());
    assert_action!(
        "settings.decoration.rounded",
        Ok(SettingsAction::TextSelect {
            key: "decoration",
            value: "rounded"
        })
    );
    assert_action!(
        "settings.decoration.squared",
        Ok(SettingsAction::TextSelect {
            key: "decoration",
            value: "squared"
        })
    );
    assert_action!(
        "settings.render-acceleration.gpu",
        Ok(SettingsAction::TextSelect {
            key: "renderAcceleration",
            value: "gpu"
        })
    );
    assert_action!(
        "settings.appearance.apps.dark",
        Ok(SettingsAction::TextSelect {
            key: "appearance",
            value: "dark"
        })
    );
    assert_action!(
        "settings.appearance.apps.light",
        Ok(SettingsAction::TextSelect {
            key: "appearance",
            value: "light"
        })
    );
    assert!(parse_action("settings.appearance.apps.system").is_err());
    assert_eq!(
        flamewm_api::settings::AppearanceMode::default().as_str(),
        "dark",
        "application appearance defaults to Dark"
    );
    assert!(parse_action("settings.decoration.ios").is_err());
    assert!(parse_action("settings.decoration.cycle").is_err());
}

#[test]
fn screensaver_preview_action_is_distinct_from_lock_preview() {
    assert_action!(
        "settings.screensaver.preview",
        Ok(SettingsAction::ScreensaverPreview)
    );
    assert_action!("settings.lock.preview", Ok(SettingsAction::LockPreview));
}

#[test]
fn red_swatch_selects_red_not_the_default_blue() {
    // Regression: the red swatch once mapped to DEFAULT_ACCENT, so clicking red saved the blue default.
    assert_action!(
        "settings.accent.red",
        Ok(SettingsAction::AccentSwatch(value)) if value == "#EF4048"
    );
    assert_ne!(accent_for_swatch("red"), Some(DEFAULT_ACCENT));
}

#[test]
fn accent_reset_and_start_icon_actions_parse_to_their_typed_targets() {
    assert!(parse_action("settings.accent.reset").is_err());
    assert_action!(
        "settings.start-icon.clear",
        Ok(SettingsAction::StartIconClear)
    );
    assert_action!(
        "settings.start-icon.choose",
        Ok(SettingsAction::StartIconChoose)
    );
    assert_action!(
        "settings.page.surfaces",
        Ok(SettingsAction::AppearanceTab(AppearanceTab::Surfaces))
    );
    assert_action!(
        "settings.page.desktop",
        Ok(SettingsAction::Page(SettingsPage::Desktop))
    );
}

#[test]
fn start_icon_label_shows_file_name_or_built_in_default() {
    assert_eq!(start_icon_label(""), "Built-in icon");
    assert_eq!(
        start_icon_label("/home/u/.local/share/flamewm/wallpapers/start-0123456789abcdef.webp"),
        "start-0123456789abcdef.webp"
    );
}

#[test]
fn remove_managed_media_ignores_empty_and_unmanaged_paths() {
    assert!(remove_managed_media("").is_ok());
    assert!(remove_managed_media("/etc/hostname").is_ok());
    let unmanaged = std::env::temp_dir().join("flamewm-unmanaged-start-icon.png");
    std::fs::write(&unmanaged, b"x").expect("write fixture");
    assert!(remove_managed_media(unmanaged.to_str().expect("utf-8")).is_ok());
    assert!(unmanaged.exists(), "unmanaged files are never deleted");
    std::fs::remove_file(&unmanaged).expect("cleanup fixture");
}

#[test]
fn lock_background_steppers_parse_to_their_exact_typed_targets() {
    assert_action!(
        "settings.lock-bg-blur.up",
        Ok(SettingsAction::IntegerStep {
            key: "lockBackgroundBlurPercent",
            delta: 5,
            minimum: 0,
            maximum: 100
        })
    );
    assert_action!(
        "settings.lock-bg-blur.down",
        Ok(SettingsAction::IntegerStep {
            key: "lockBackgroundBlurPercent",
            delta: -5,
            minimum: 0,
            maximum: 100
        })
    );
    assert_action!(
        "settings.lock-bg-dim.up",
        Ok(SettingsAction::IntegerStep {
            key: "lockBackgroundDimPercent",
            delta: 5,
            minimum: 0,
            maximum: 80
        })
    );
    assert_action!(
        "settings.lock-bg-dim.down",
        Ok(SettingsAction::IntegerStep {
            key: "lockBackgroundDimPercent",
            delta: -5,
            minimum: 0,
            maximum: 80
        })
    );
    assert!(parse_action("settings.lock-bg-dim.sideways").is_err());
}

#[test]
fn taskbar_and_screensaver_actions_parse_to_their_exact_typed_targets() {
    assert_action!(
        "settings.surface-border-opacity.up",
        Ok(SettingsAction::PercentStep("surfaceBorderOpacity", 1))
    );
    assert_action!(
        "settings.surface-border-opacity.down",
        Ok(SettingsAction::PercentStep("surfaceBorderOpacity", -1))
    );
    assert_action!(
        "settings.surface-blur.up",
        Ok(SettingsAction::PercentStep("surfaceBlurPercent", 1))
    );
    assert_action!(
        "settings.surface-saturation.down",
        Ok(SettingsAction::PercentStep("surfaceSaturationPercent", -1))
    );
    assert_action!(
        "settings.surface-highlight.up",
        Ok(SettingsAction::PercentStep("surfaceHighlightPercent", 1))
    );
    assert_action!(
        "settings.glass.dark",
        Ok(SettingsAction::GlassPreset(glass::DARK))
    );
    assert_action!(
        "settings.glass.light",
        Ok(SettingsAction::GlassPreset(glass::LIGHT))
    );
    assert!(parse_action("settings.glass.neon").is_err());
    assert_action!(
        "settings.surface-opacity.down",
        Ok(SettingsAction::PercentStep("surfaceOpacity", -1))
    );
    assert!(parse_action("settings.taskbar-blur.up").is_err());
    assert!(parse_action("settings.taskbar-opacity.up").is_err());
    assert!(parse_action("settings.start-menu-blur.down").is_err());
    assert_action!(
        "settings.window-margin.down",
        Ok(SettingsAction::IntegerStep {
            key: "windowMargin",
            delta: -1,
            minimum: 0,
            maximum: 32
        })
    );
    assert_action!(
        "settings.taskbar-floating-margin.up",
        Ok(SettingsAction::IntegerStep {
            key: "taskbarFloatingMargin",
            delta: 1,
            minimum: 0,
            maximum: 32
        })
    );
    assert_action!(
        "settings.screensaver-ribbon-width.down",
        Ok(SettingsAction::IntegerStep {
            key: "screensaverRibbonWidthTenths",
            delta: -5,
            minimum: 20,
            maximum: 280
        })
    );
    assert_action!(
        "settings.screensaver-display-off.up",
        Ok(SettingsAction::IntegerStep {
            key: "screensaverDisplayOffMinutes",
            delta: 5,
            minimum: 0,
            maximum: 240
        })
    );
    assert_action!(
        "settings.cursor.size.48",
        Ok(SettingsAction::IntegerSelect {
            key: "cursorSize",
            value: 48
        })
    );
    assert_action!(
        "settings.taskbar.style.floating",
        Ok(SettingsAction::TextSelect {
            key: "taskbarStyle",
            value: "floating"
        })
    );
    assert_action!(
        "settings.taskbar.clock.12h",
        Ok(SettingsAction::TextSelect {
            key: "taskbarClockFormat",
            value: "12h"
        })
    );
    assert_action!(
        "settings.screensaver.mode.none",
        Ok(SettingsAction::TextSelect {
            key: "screensaverMode",
            value: "none"
        })
    );
    assert_action!(
        "settings.wallpaper.fit",
        Ok(SettingsAction::TextCycle {
            key: "wallpaperFit",
            ..
        })
    );
    assert!(parse_action("settings.toggle.screensaverSpanMonitors").is_err());
}

#[test]
fn appearance_actions_reject_unknown_targets_channels_and_steps() {
    for action in [
        "settings.color.desktop.r.step.1",
        "settings.color.accent.x.step.1",
        "settings.color.accent.r.step.2",
        "settings.color.accent.r.step.0",
        "settings.color.accent.r",
        "settings.animation-speed.sideways",
    ] {
        assert!(parse_action(action).is_err(), "{action} must be rejected");
    }
}

#[test]
fn display_scale_slider_maps_all_five_percent_steps() {
    for percent in (100_u16..=200).step_by(5) {
        assert_eq!(
            display_scale_from_slider(&(percent - 100).to_string()),
            Ok(percent)
        );
    }
    assert_eq!(display_scale_from_slider("0"), Ok(100));
    assert_eq!(display_scale_from_slider("100"), Ok(200));
    assert_eq!(display_scale_from_slider("35.1"), Ok(135));
    assert_eq!(display_scale_from_slider("94.9"), Ok(195));
    assert!(display_scale_from_slider("NaN").is_err());
    assert!(display_scale_from_slider("abc").is_err());
}

#[test]
fn animation_speed_slider_maps_pointer_percent_to_snapped_hundredths() {
    assert_eq!(animation_speed_from_slider("0"), Ok(25));
    assert_eq!(animation_speed_from_slider("100"), Ok(200));
    // 50% of 25..200 is 112.5, which snaps to 115 (not 100 or 112).
    assert_eq!(animation_speed_from_slider("50"), Ok(115));
    // Default 1.00x = 100 hundredths sits at 75/175 of the track, snapped to 100.
    assert_eq!(animation_speed_from_slider("42.85714285714286"), Ok(100));
    // 37.5% -> 90.625 -> snaps to 90, not the raw 90.625.
    assert_eq!(animation_speed_from_slider("37.5"), Ok(90));
    assert_eq!(animation_speed_from_slider(" 37.5 "), Ok(90));
}

#[test]
fn glass_sliders_map_track_percent_into_their_inclusive_ranges() {
    let opacity = glass_slider("slider.glass-opacity").expect("opacity slider");
    assert_eq!(glass_slider_value("0", opacity), Ok(4));
    assert_eq!(glass_slider_value("100", opacity), Ok(42));
    assert_eq!(glass_slider_value("50", opacity), Ok(23));
    assert_eq!(glass_slider_value("-5", opacity), Ok(4));
    assert_eq!(glass_slider_value("140", opacity), Ok(42));
    let blur = glass_slider("slider.glass-blur").expect("blur slider");
    assert_eq!(glass_slider_value("0", blur), Ok(0));
    assert_eq!(glass_slider_value("100", blur), Ok(54));
    let taskbar = glass_slider("slider.taskbar-opacity").expect("taskbar slider");
    assert_eq!(glass_slider_value("100", taskbar), Ok(75));
    assert!(glass_slider_value("NaN", taskbar).is_err());
    assert!(glass_slider_value("", taskbar).is_err());
    assert!(glass_slider("slider.glass-neon").is_none());
}

#[test]
fn glass_controls_parse_to_their_actions() {
    assert!(matches!(
        parse_action("slider.glass-blur"),
        Ok(SettingsAction::GlassSlider)
    ));
    assert!(matches!(
        parse_action("settings.glass.reset"),
        Ok(SettingsAction::ResetGlass)
    ));
    assert!(matches!(
        parse_action("settings.taskbar.material.custom-tint"),
        Ok(SettingsAction::TextSelect {
            key: "taskbarMaterialMode",
            value: "custom-tint",
        })
    ));
    assert!(matches!(
        parse_action("settings.toggle.reduceTransparency"),
        Ok(SettingsAction::Toggle("reduceTransparency"))
    ));
    assert!(parse_action("settings.taskbar.material.neon").is_err());
}

#[test]
fn taskbar_tint_uses_the_shared_colour_row_and_picker() {
    assert!(matches!(
        parse_action("settings.color.taskbarTint.preset.2"),
        Ok(SettingsAction::ColorSwatch {
            key: "taskbarCustomTint",
            value: "#3DAEE9",
        })
    ));
    assert!(matches!(
        parse_action("settings.picker.open.taskbarTint"),
        Ok(SettingsAction::ColorPickerOpen("taskbarCustomTint"))
    ));
}

#[test]
fn animation_speed_slider_clamps_and_rejects_non_numeric_values() {
    assert_eq!(animation_speed_from_slider("-40"), Ok(25));
    assert_eq!(animation_speed_from_slider("140"), Ok(200));
    assert!(animation_speed_from_slider("").is_err());
    assert!(animation_speed_from_slider("abc").is_err());
    assert!(animation_speed_from_slider("NaN").is_err());
    assert!(animation_speed_from_slider("inf").is_err());
}

#[test]
fn accent_color_rejects_malformed_values_without_panicking() {
    assert!(accent_color("#EF4048").is_ok());
    assert!(accent_color("#é1234").is_err());
    assert!(accent_color("EF4048").is_err());
    assert!(accent_color("#+F4048").is_err());
    assert!(accent_color("#EF+048").is_err());
}

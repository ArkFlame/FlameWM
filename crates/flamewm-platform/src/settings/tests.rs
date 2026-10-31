use super::*;

fn load(text: &str) -> FlameResult<ProductSettings> {
    ConfigStore::default().parse_snapshot(text)
}

/// Serializes `parsed`, asserts it reloads to the same settings, and returns the text.
fn assert_round_trip(parsed: &ProductSettings) -> String {
    let text = ConfigStore::serialize(parsed);
    assert_eq!(load(&text).expect("round trip"), *parsed);
    text
}

/// Each line, set on an otherwise default file, must be rejected as `InvalidArgument`.
fn assert_rejected<S: AsRef<str>>(lines: &[S]) {
    for line in lines {
        let line = line.as_ref();
        assert_eq!(
            load(&format!("revision=2\n{line}\n")).expect_err(line).code,
            ErrorCode::InvalidArgument,
            "{line} must be rejected"
        );
    }
}

#[test]
fn defaults_match_current_flamewm_product_contract() {
    let settings = ProductSettings::default();
    assert_eq!(settings.revision, 1);
    assert_eq!(settings.accent, DEFAULT_ACCENT);
    assert!(!settings.accent_custom);
    assert_eq!(settings.taskbar_height, 44);
    assert_eq!(settings.desktop_selection_fill_opacity, 20);
    assert_eq!(settings.window_snap_preview_fill_opacity, 20);
    assert_eq!(settings.appearance, AppearanceMode::Dark);
}

#[test]
fn unset_icon_theme_defaults_to_plaguesur_and_writes_nothing() {
    let parsed = load("revision=2\n").expect("valid snapshot");
    assert_eq!(parsed.icon_theme, "MacTahoe");
    assert!(!ConfigStore::serialize(&parsed).contains("iconTheme"));
}

#[test]
fn legacy_explicit_mcmojave_icon_theme_is_kept_verbatim() {
    let parsed = load("revision=2\niconTheme=McMojave-circle\n").expect("legacy icon theme");
    assert_eq!(parsed.icon_theme, "McMojave-circle");
    assert!(ConfigStore::serialize(&parsed).contains("iconTheme=McMojave-circle\n"));
}

#[test]
fn explicit_icon_theme_is_kept_and_written() {
    let parsed = load("revision=2\niconTheme=*:-HighContrast\n").expect("explicit icon theme");
    assert_eq!(parsed.icon_theme, "*:-HighContrast");
    let text = assert_round_trip(&parsed);
    assert!(text.contains("iconTheme=*:-HighContrast\n"));
}

#[test]
fn appearance_snapshot_round_trip_defaults_to_dark() {
    let parsed = load("revision=2\n").expect("valid snapshot");
    assert_eq!(parsed.appearance, AppearanceMode::Dark);
    let text = ConfigStore::serialize(&ProductSettings::default());
    assert!(text.contains("appearance=dark\n"));
    let reparsed = load(&text).expect("round trip");
    assert_eq!(reparsed.appearance, AppearanceMode::Dark);
}

#[test]
fn invalid_appearance_value_is_rejected() {
    assert_eq!(
        load("revision=2\nappearance=neon\n")
            .expect_err("bad appearance")
            .code,
        ErrorCode::InvalidArgument
    );
}

#[test]
fn parser_preserves_unknown_compatible_keys() {
    let parsed = load("revision=2\nfutureFeature=value\n").expect("valid future key");
    assert_eq!(
        parsed.unknown_keys.get("futureFeature"),
        Some(&"value".to_owned())
    );
}

#[test]
fn removed_startmenu_and_titlebar_colour_keys_load_as_unknown_and_survive_a_round_trip() {
    let text = "revision=2\nstartMenuColor=#112233\nstartMenuOpacity=80\nstartMenuBlurPercent=0\n\
        titlebarCloseColor=#FF0000\ntitlebarCloseHoverColor=#FFFFFF\ntitlebarMinimizeColor=#zz\n";
    let parsed = load(text).expect("removed keys still load");
    assert_eq!(parsed.unknown_keys.get("startMenuColor"), Some(&"#112233".to_owned()));
    assert_eq!(parsed.unknown_keys.get("titlebarCloseHoverColor"), Some(&"#FFFFFF".to_owned()));
    assert!(assert_round_trip(&parsed).contains("titlebarCloseColor=#FF0000"));
}

#[test]
fn removed_surface_rim_and_highlight_keys_load_as_unknown_and_survive_a_round_trip() {
    let text = "revision=2\nsurfaceRimFadePercent=25\nsurfaceHighlightPercent=10\n";
    let parsed = load(text).expect("legacy surface keys still load");
    assert_eq!(parsed.unknown_keys.get("surfaceRimFadePercent"), Some(&"25".to_owned()));
    assert_eq!(parsed.unknown_keys.get("surfaceHighlightPercent"), Some(&"10".to_owned()));
    assert!(assert_round_trip(&parsed).contains("surfaceRimFadePercent=25"));
}

#[test]
fn stale_revision_cannot_replace_effective_state() {
    let mut store = ConfigStore::default();
    let candidate = ProductSettings {
        revision: 1,
        ..ProductSettings::default()
    };
    assert_eq!(
        store
            .apply_snapshot(candidate)
            .expect_err("must reject stale")
            .code,
        ErrorCode::StaleRevision
    );
}

#[test]
fn five_percent_scales_persist_without_quantization() {
    for percent in (100..=200).step_by(5) {
        let mut settings = ProductSettings::default();
        settings
            .per_output_scale
            .insert("eDP-1".to_owned(), percent);
        settings.validate().expect("valid five-percent scale");
        let saved = ConfigStore::serialize(&settings);
        assert!(saved.contains(&format!("scale.eDP-1={percent}\n")));
        let parsed = load(&saved).expect("saved scale is readable");
        assert_eq!(parsed.per_output_scale.get("eDP-1"), Some(&percent));
    }
    for percent in [99, 101, 104, 106, 133, 199, 201] {
        let mut settings = ProductSettings::default();
        settings
            .per_output_scale
            .insert("eDP-1".to_owned(), percent);
        assert!(settings.validate().is_err(), "{percent} cannot persist");
    }
}

#[test]
fn removed_cursor_shadow_key_is_kept_as_unknown() {
    let loaded = load("revision=4\ncursorShadow=false\n").expect("legacy cursorShadow");
    assert_eq!(loaded.unknown_keys.get("cursorShadow").map(String::as_str), Some("false"));
    assert!(ConfigStore::serialize(&loaded).contains("cursorShadow=false\n"));
}

#[test]
fn invalid_output_scale_is_rejected() {
    let mut settings = ProductSettings::default();
    settings.per_output_scale.insert("eDP-1".to_owned(), 133);
    assert_eq!(
        settings.validate().expect_err("invalid scale").code,
        ErrorCode::InvalidArgument
    );
}

#[test]
fn default_terminal_defaults_empty_and_round_trips() {
    let defaults = ProductSettings::default();
    assert_eq!(defaults.default_terminal, "");
    assert!(!ConfigStore::serialize(&defaults).contains("defaultTerminal"));
    let parsed = load("revision=2\ndefaultTerminal=kitty\n").expect("bare command");
    assert_eq!(parsed.default_terminal, "kitty");
    assert!(ConfigStore::serialize(&parsed).contains("defaultTerminal=kitty"));
    let absolute =
        load("revision=2\ndefaultTerminal=/usr/bin/alacritty\n").expect("absolute path");
    assert_eq!(absolute.default_terminal, "/usr/bin/alacritty");
}

#[test]
fn invalid_default_terminal_values_are_rejected() {
    for value in ["kit ty", "bin/term", "a\tb", "../term"] {
        assert!(
            load(&format!("revision=2\ndefaultTerminal={value}\n")).is_err(),
            "{value:?} must be rejected"
        );
    }
    let long = "x".repeat(256);
    assert!(load(&format!("revision=2\ndefaultTerminal={long}\n")).is_err());
}

#[test]
fn avatar_image_defaults_empty_and_round_trips() {
    let defaults = ProductSettings::default();
    assert_eq!(defaults.avatar_image, "");
    assert!(!ConfigStore::serialize(&defaults).contains("avatarImage"));
    let parsed = load("revision=2\navatarImage=/home/u/.local/share/flamewm/wallpapers/avatar-0123456789abcdef.png\n")
        .expect("absolute avatar path");
    assert_eq!(
        parsed.avatar_image,
        "/home/u/.local/share/flamewm/wallpapers/avatar-0123456789abcdef.png"
    );
    let text = assert_round_trip(&parsed);
    assert!(text.contains("avatarImage=/home/u/"));
}

#[test]
fn assistant_api_key_accepts_default_and_ark_keys_and_rejects_invalid_values() {
    let mut settings = ProductSettings::default();
    assert!(settings.assistant_api_key.is_empty(), "default key is the built-in key");
    for value in ["", "ark_23bRexample"] {
        value.clone_into(&mut settings.assistant_api_key);
        settings.validate().expect("valid assistant API key");
    }
    for value in ["sk-live-key", "ark_23 bR", "ark_23bR\t"] {
        value.clone_into(&mut settings.assistant_api_key);
        assert!(settings.validate().is_err(), "{value:?} must be rejected");
    }
}

#[test]
fn assistant_taskbar_icon_defaults_on_and_parses_off() {
    assert!(ProductSettings::default().assistant_taskbar_icon);
    let parsed = load("assistant.taskbarIcon=0\n").expect("taskbar icon key parses");
    assert!(!parsed.assistant_taskbar_icon);
    let text = assert_round_trip(&parsed);
    assert!(text.contains("assistant.taskbarIcon=false"));
}

#[test]
fn legacy_hermes_url_key_is_ignored_and_never_written() {
    let parsed = load("assistant.hermesUrl=http://127.0.0.1:8642\nassistant.enabled=1\n")
        .expect("legacy key tolerated");
    assert!(!parsed.unknown_keys.contains_key("assistant.hermesUrl"));
    assert!(parsed.assistant_api_key.is_empty());
    let text = assert_round_trip(&parsed);
    assert!(!text.contains("hermesUrl"));
}

#[test]
fn assistant_api_key_round_trips_when_set() {
    let parsed = load("assistant.apiKey=ark_23bRexample\n").expect("api key parses");
    assert_eq!(parsed.assistant_api_key, "ark_23bRexample");
    let text = assert_round_trip(&parsed);
    assert!(text.contains("assistant.apiKey=ark_23bRexample"));
}

#[test]
fn invalid_avatar_image_paths_are_rejected() {
    let long_path = format!("/{}", "a".repeat(4096));
    assert_rejected(&[
        "avatarImage=face.png".to_owned(),
        "avatarImage=images/face.png".to_owned(),
        "avatarImage=/tmp/a\tb.png".to_owned(),
        format!("avatarImage={long_path}"),
    ]);
}

#[test]
fn background_modes_migrate_from_legacy_values_when_keys_are_absent() {
    let legacy_wallpaper =
        load("revision=2\nwallpaper=/home/u/a.png\n").expect("legacy wallpaper");
    assert_eq!(legacy_wallpaper.wallpaper_mode, BackgroundMode::Custom);
    let empty_wallpaper = load("revision=2\n").expect("empty wallpaper");
    assert_eq!(empty_wallpaper.wallpaper_mode, BackgroundMode::Default);
    let custom_lock =
        load("revision=2\nlockBackgroundImage=/home/u/bg.png\n").expect("custom lock");
    assert_eq!(custom_lock.lock_background_mode, BackgroundMode::Custom);
    let solid_lock = load("revision=2\nlockBackgroundColor=#102030\n").expect("solid lock");
    assert_eq!(solid_lock.lock_background_mode, BackgroundMode::Solid);
    let default_lock = load("revision=2\n").expect("default lock");
    assert_eq!(default_lock.lock_background_mode, BackgroundMode::Default);
}

#[test]
fn explicit_background_mode_wins_over_saved_path_and_keeps_it() {
    let parsed = load(
        "revision=2\nwallpaperMode=default\nwallpaper=/home/u/a.png\n\
             lockBackgroundMode=solid\nlockBackgroundImage=/home/u/bg.png\n",
    )
    .expect("explicit modes");
    assert_eq!(parsed.wallpaper_mode, BackgroundMode::Default);
    assert_eq!(parsed.wallpaper, "/home/u/a.png");
    assert_eq!(parsed.lock_background_mode, BackgroundMode::Solid);
    assert_eq!(parsed.lock_background_image, "/home/u/bg.png");
}

#[test]
fn background_modes_round_trip_and_clear_differs_from_default() {
    for mode in [
        BackgroundMode::Default,
        BackgroundMode::Solid,
        BackgroundMode::Custom,
    ] {
        let snapshot = ProductSettings {
            revision: 9,
            wallpaper: "/home/u/a.png".to_owned(),
            wallpaper_mode: mode,
            lock_background_image: "/home/u/bg.png".to_owned(),
            lock_background_mode: mode,
            ..ProductSettings::default()
        };
        let text = ConfigStore::serialize(&snapshot);
        let parsed = load(&text).expect("round trip");
        assert_eq!(parsed.wallpaper_mode, mode);
        assert_eq!(parsed.lock_background_mode, mode);
        assert_eq!(parsed.wallpaper, "/home/u/a.png");
        assert_eq!(parsed.lock_background_image, "/home/u/bg.png");
    }
    let clear = ProductSettings {
        lock_background_mode: BackgroundMode::Solid,
        ..ProductSettings::default()
    };
    let default = ProductSettings {
        lock_background_mode: BackgroundMode::Default,
        ..ProductSettings::default()
    };
    assert_ne!(
        ConfigStore::serialize(&clear),
        ConfigStore::serialize(&default),
        "Clear (solid) must not persist the same output as Default"
    );
}

#[test]
fn invalid_background_mode_and_wallpaper_color_are_rejected() {
    for text in [
        "wallpaperMode=clear\n",
        "lockBackgroundMode=Custom\n",
        "wallpaperColor=red\n",
    ] {
        assert!(load(text).is_err(), "{text} must be rejected");
    }
}

#[test]
fn lock_background_blur_and_dim_defaults_write_nothing() {
    let defaults = ProductSettings::default();
    assert_eq!(defaults.lock_background_blur_percent, 40);
    assert_eq!(defaults.lock_background_dim_percent, 25);
    let text = ConfigStore::serialize(&defaults);
    assert!(!text.contains("lockBackgroundBlurPercent"));
    assert!(!text.contains("lockBackgroundDimPercent"));
}

#[test]
fn lock_background_blur_and_dim_legacy_file_loads_defaults() {
    let parsed = load("revision=1\nlockBackgroundColor=#102030\n").expect("legacy lock file");
    assert_eq!(parsed.lock_background_blur_percent, 40);
    assert_eq!(parsed.lock_background_dim_percent, 25);
}

#[test]
fn lock_background_blur_and_dim_round_trip_bounds() {
    let parsed =
        load("revision=2\nlockBackgroundBlurPercent=100\nlockBackgroundDimPercent=80\n")
            .expect("bounds are valid");
    assert_eq!(parsed.lock_background_blur_percent, 100);
    assert_eq!(parsed.lock_background_dim_percent, 80);
    let text = assert_round_trip(&parsed);
    assert!(text.contains("lockBackgroundBlurPercent=100\n"));
    assert!(text.contains("lockBackgroundDimPercent=80\n"));

    let zero = load("revision=2\nlockBackgroundBlurPercent=0\nlockBackgroundDimPercent=0\n")
        .expect("zero is valid");
    assert_eq!(zero.lock_background_blur_percent, 0);
    assert_eq!(zero.lock_background_dim_percent, 0);
}

#[test]
fn invalid_lock_background_blur_and_dim_values_are_rejected() {
    assert_rejected(&[
        "lockBackgroundBlurPercent=101",
        "lockBackgroundBlurPercent=-1",
        "lockBackgroundBlurPercent=40.5",
        "lockBackgroundDimPercent=81",
        "lockBackgroundDimPercent=-1",
        "lockBackgroundDimPercent=25%",
    ]);
}

#[test]
fn lock_screen_defaults_match_the_contract_and_write_nothing() {
    let defaults = ProductSettings::default();
    assert_eq!(defaults.lock_background_color, "#232629");
    assert_eq!(defaults.lock_background_image, "");
    assert_eq!(defaults.lock_clock_format, LockClockFormat::TwentyFour);
    assert!(defaults.lock_show_date);
    assert!(defaults.lock_show_username);
    assert_eq!(defaults.lock_timeout_minutes, 0);
    assert!(defaults.lock_on_suspend);
    let text = ConfigStore::serialize(&defaults);
    assert!(!text.contains("lock"));
}

#[test]
fn lock_screen_round_trips_non_default_values() {
    let parsed = load(
        "revision=2\nlockBackgroundColor=#102030\nlockBackgroundImage=/usr/share/bg.png\n\
             lockClockFormat=12h\nlockShowDate=false\nlockShowUsername=0\n\
             lockTimeoutMinutes=240\nlockOnSuspend=false\n",
    )
    .expect("valid lock settings");
    assert_eq!(parsed.lock_background_color, "#102030");
    assert_eq!(parsed.lock_background_image, "/usr/share/bg.png");
    assert_eq!(parsed.lock_clock_format, LockClockFormat::Twelve);
    assert!(!parsed.lock_show_date);
    assert!(!parsed.lock_show_username);
    assert_eq!(parsed.lock_timeout_minutes, 240);
    assert!(!parsed.lock_on_suspend);
    let text = assert_round_trip(&parsed);
    assert!(text.contains("lockClockFormat=12h\n"));
    assert!(text.contains("lockShowDate=false\n"));
}

#[test]
fn invalid_lock_screen_values_are_rejected() {
    let long_path = format!("/{}", "a".repeat(4096));
    assert_rejected(&[
        "lockBackgroundColor=red".to_owned(),
        "lockBackgroundColor=#12345".to_owned(),
        "lockClockFormat=13h".to_owned(),
        "lockClockFormat=24H".to_owned(),
        "lockTimeoutMinutes=241".to_owned(),
        "lockTimeoutMinutes=-5".to_owned(),
        "lockTimeoutMinutes=5.5".to_owned(),
        "lockBackgroundImage=wallpaper.png".to_owned(),
        "lockBackgroundImage=images/bg.png".to_owned(),
        "lockBackgroundImage=/tmp/a\tb.png".to_owned(),
        format!("lockBackgroundImage={long_path}"),
        "lockShowDate=maybe".to_owned(),
    ]);
}

#[test]
fn lock_timeout_zero_means_never_and_is_accepted() {
    let parsed = load("revision=2\nlockTimeoutMinutes=0\n").expect("zero timeout is never");
    assert_eq!(parsed.lock_timeout_minutes, 0);
}

#[test]
fn taskbar_style_and_backdrop_defaults_write_nothing() {
    let defaults = ProductSettings::default();
    assert_eq!(defaults.taskbar_style, TaskbarStyle::Floating);
    assert_eq!(defaults.taskbar_floating_margin, 4);
    assert_eq!(defaults.taskbar_floating_radius, 12);
    assert_eq!(defaults.taskbar_blur_percent, 80);
    assert_eq!(defaults.surface_blur_percent, 70);
    let taskbar = defaults.taskbar_backdrop();
    assert_eq!(taskbar.opacity_percent, 65);
    assert_eq!(taskbar.blur_percent, 70);
    assert!((taskbar.radius_px - 28.0).abs() < 1e-4);
    let text = ConfigStore::serialize(&defaults);
    assert!(!text.contains("taskbarStyle"));
    assert!(!text.contains("taskbarBlurPercent"));
    assert!(!text.contains("taskbarFloating"));
}

#[test]
fn taskbar_style_and_blur_round_trip_and_opaque_has_no_radius() {
    let parsed = load(
        "revision=2\ntaskbarStyle=classic\ntaskbarFloatingMargin=32\n\
             taskbarFloatingRadius=0\nsurfaceBlurPercent=100\nsurfaceOpacity=100\n",
    )
    .expect("valid taskbar settings");
    assert_eq!(parsed.taskbar_style, TaskbarStyle::Classic);
    assert_eq!(parsed.taskbar_floating_margin, 32);
    assert_eq!(parsed.taskbar_floating_radius, 0);
    assert!((parsed.taskbar_backdrop().radius_px - 0.0).abs() < 1e-6);
    let text = assert_round_trip(&parsed);
    assert!(text.contains("taskbarStyle=classic\n"));
    assert!(text.contains("surfaceBlurPercent=100\n"));
}

#[test]
fn invalid_taskbar_style_and_blur_values_are_rejected() {
    assert_rejected(&[
        "taskbarStyle=Floating",
        "taskbarStyle=bar",
        "taskbarFloatingMargin=33",
        "taskbarFloatingMargin=-1",
        "taskbarFloatingRadius=25",
        "taskbarBlurPercent=101",
        "taskbarBlurPercent=80.5",
    ]);
}

#[test]
fn screensaver_defaults_match_contract_and_write_nothing() {
    let defaults = ProductSettings::default();
    assert_eq!(
        defaults.screensaver(),
        ScreensaverSettings {
            mode: ScreensaverMode::Ribbons,
            after_lock: Duration::from_secs(60),
            num_ribbons: 6,
            ribbon_width_px: 11.0,
            span_monitors: false,
            display_off_minutes: 20,
        }
    );
    // lockTimeoutMinutes stays 0 (manual lock) so upgrades never lock unexpectedly.
    assert_eq!(defaults.lock_timeout_minutes, 0);
    let text = ConfigStore::serialize(&defaults);
    assert!(!text.contains("screensaver"));
}

#[test]
fn legacy_file_without_screensaver_keys_loads_defaults() {
    let parsed = load("revision=1\nlockTimeoutMinutes=5\nlockOnSuspend=false\n")
        .expect("legacy lock file");
    assert_eq!(
        parsed.screensaver(),
        ProductSettings::default().screensaver()
    );
    assert_eq!(parsed.lock_timeout_minutes, 5);
}

#[test]
fn screensaver_round_trips_non_default_values() {
    let parsed = load(
        "revision=2\nscreensaverMode=none\nscreensaverAfterLockMinutes=0\n\
             screensaverNumRibbons=12\nscreensaverRibbonWidthTenths=280\n\
             screensaverSpanMonitors=true\nscreensaverDisplayOffMinutes=240\n",
    )
    .expect("valid screensaver settings");
    assert_eq!(
        parsed.screensaver(),
        ScreensaverSettings {
            mode: ScreensaverMode::None,
            after_lock: Duration::ZERO,
            num_ribbons: 12,
            ribbon_width_px: 28.0,
            span_monitors: true,
            display_off_minutes: 240,
        }
    );
    let text = assert_round_trip(&parsed);
    assert!(text.contains("screensaverMode=none\n"));
    assert!(text.contains("screensaverSpanMonitors=true\n"));
}

#[test]
fn invalid_screensaver_values_are_rejected() {
    assert_rejected(&[
        "screensaverMode=Ribbons",
        "screensaverMode=blank",
        "screensaverAfterLockMinutes=61",
        "screensaverAfterLockMinutes=-1",
        "screensaverNumRibbons=0",
        "screensaverNumRibbons=13",
        "screensaverRibbonWidthTenths=19",
        "screensaverRibbonWidthTenths=281",
        "screensaverRibbonWidthTenths=11.0",
        "screensaverDisplayOffMinutes=241",
        "screensaverSpanMonitors=maybe",
    ]);
}

#[test]
fn empty_persistence_path_is_rejected_before_io() {
    assert_eq!(
        ConfigStore::persist(Path::new(""), &ProductSettings::default())
            .expect_err("empty path")
            .code,
        ErrorCode::InvalidArgument
    );
}

#[test]
fn persisted_settings_are_exactly_the_serialized_bytes() {
    let dir = std::env::temp_dir().join(format!("flamewm-settings-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("fixture dir");
    let path = dir.join("settings.conf");
    let mut settings = ProductSettings {
        revision: 9,
        ..ProductSettings::default()
    };
    settings
        .unknown_keys
        .insert("extension.key".into(), "value".into());
    let expected = ConfigStore::serialize(&settings);

    ConfigStore::persist(&path, &settings).expect("persist settings");

    assert_eq!(std::fs::read(path).unwrap(), expected.as_bytes());
}

#[test]
fn older_settings_without_new_fields_keep_defaults() {
    let settings = load("revision=4\naccent=#112233\n").expect("legacy settings");
    assert_eq!(settings.revision, 4);
    assert_eq!(settings.accent, "#112233");
}

#[test]
fn new_appearance_keys_default_and_stay_unwritten_when_unchanged() {
    let defaults = ProductSettings::default();
    assert_eq!(defaults.taskbar_opacity, 50);
    assert_eq!(defaults.accent_opacity, 100);
    assert_eq!(defaults.decoration, "rounded");
    assert_eq!(defaults.titlebar_button_style, "circle");
    assert!(defaults.cursor_theme.is_empty());
    assert_eq!(defaults.animation_speed, 100);
    assert_eq!(defaults.start_animation, AnimationStyle::None);
    assert_eq!(defaults.minimize_animation, AnimationStyle::None);
    assert_eq!(defaults.maximize_animation, AnimationStyle::None);
    assert_eq!(defaults.window_open_animation, AnimationStyle::None);
    assert_eq!(defaults.window_close_animation, AnimationStyle::None);
    assert!(!defaults.maximize_animation_enabled);

    let text = ConfigStore::serialize(&defaults);
    assert!(!text.contains("startMenuColor"));
    assert!(!text.contains("titlebar"));
    assert!(!text.contains("animation"));
}

#[test]
fn old_settings_file_parses_to_defaults_and_serializes_unchanged() {
    let old = "revision=3\naccent=#EF4048\ntaskbarColor=#191b1d\nfutureFeature=kept\n";
    let parsed = load(old).expect("old file");
    assert_eq!(parsed.animation_speed, 100);
    let text = assert_round_trip(&parsed);
    assert!(!text.contains("titlebar"));
    assert!(text.contains("futureFeature=kept\n"));
}

#[test]
fn legacy_titlebar_color_is_tolerated_ignored_and_never_written() {
    // Old files carry titlebarColor; even an invalid value must not error or touch surface*.
    for value in ["#A0B0C0", "#12345"] {
        let legacy = load(&format!(
            "revision=3\ntitlebarColor={value}\nsurfaceColor=#445566\n"
        ))
        .expect("legacy titlebarColor tolerated");
        assert_eq!(legacy.surface_color, "#445566");
        assert_eq!(
            legacy.surface_opacity,
            ProductSettings::default().surface_opacity
        );
        let text = ConfigStore::serialize(&legacy);
        assert!(!text.contains("titlebarColor"), "{text}");
        assert!(
            !legacy.unknown_keys.contains_key("titlebarColor"),
            "legacy key is ignored, not kept as unknown"
        );
    }
    assert!(stable_setting("titlebarColor").is_none());
}

#[test]
fn new_appearance_keys_round_trip_when_set() {
    let text = "revision=2\naccentOpacity=40\n\
        decoration=ios\n\
        cursorTheme=Adwaita\nanimationsEnabled=false\n\
        titlebarButtonStyle=square\n\
        animationSpeed=150\nstartAnimationEnabled=0\nfutureFeature=kept\n";
    let parsed = load(text).expect("valid new keys");
    assert_eq!(parsed.accent_opacity, 40);
    assert_eq!(parsed.decoration, "rounded");
    assert_eq!(parsed.titlebar_button_style, "square");
    assert_eq!(parsed.cursor_theme, "Adwaita");
    assert!(!parsed.animations_enabled);
    assert_eq!(parsed.animation_speed, 150);
    assert!(!parsed.start_animation_enabled);
    assert_eq!(
        parsed.unknown_keys.get("futureFeature"),
        Some(&"kept".to_owned())
    );

    let serialized = ConfigStore::serialize(&parsed);
    assert!(serialized.contains("animationsEnabled=false\n"));
    assert!(serialized.contains("futureFeature=kept\n"));
    assert_eq!(load(&serialized).expect("round trip"), parsed);
}

#[test]
fn surface_defaults_and_legacy_taskbar_surface_promotes() {
    let defaults = ProductSettings::default();
    assert_eq!(defaults.surface_color, "#333333");
    assert_eq!(defaults.surface_opacity, 65);
    assert_eq!(defaults.surface_blur_percent, 70);
    assert!(!ConfigStore::serialize(&defaults).contains("surface"));

    let legacy =
        load("revision=2\ntaskbarColor=#112233\ntaskbarOpacity=70\ntaskbarBlurPercent=30\n")
            .expect("legacy taskbar surface");
    assert_eq!(legacy.surface_color, "#112233");
    assert_eq!(legacy.surface_opacity, 70);
    assert_eq!(legacy.surface_blur_percent, 30);
    assert_eq!(legacy.taskbar_color, "#112233");

    let explicit =
        load("revision=3\nsurfaceColor=#445566\nsurfaceOpacity=10\nsurfaceBlurPercent=5\n")
            .expect("explicit surface");
    assert_eq!(explicit.surface_color, "#445566");
    assert_eq!(explicit.surface_opacity, 10);
    assert_eq!(explicit.surface_blur_percent, 5);

    // Legacy taskbar* keys alongside surface* are tolerated on read and ignored for surface values.
    let tolerated = load(
        "revision=3\ntaskbarColor=#112233\ntaskbarOpacity=70\ntaskbarBlurPercent=30\n\
             surfaceColor=#445566\nsurfaceOpacity=10\nsurfaceBlurPercent=5\n",
    )
    .expect("legacy taskbar keys tolerated");
    assert_eq!(tolerated.surface_color, "#445566");
    assert_eq!(tolerated.surface_opacity, 10);
    assert_eq!(tolerated.surface_blur_percent, 5);

    // Live taskbar backdrop follows surface*, never the legacy taskbar* fields.
    let split = load("revision=5\ntaskbarOpacity=100\ntaskbarBlurPercent=100\nsurfaceOpacity=10\nsurfaceBlurPercent=5\n")
        .expect("surface keys win over legacy taskbar keys");
    let backdrop = split.taskbar_backdrop();
    assert_eq!(backdrop.opacity_percent, 10);
    assert_eq!(backdrop.blur_percent, 5);

    // Legacy keys are read but never written; the surface pair carries the value forward.
    let migrated =
        load("revision=2\ntaskbarColor=#112233\ntaskbarOpacity=70\ntaskbarBlurPercent=30\n")
            .expect("legacy taskbar surface");
    let text = ConfigStore::serialize(&migrated);
    assert!(!text.contains("taskbarColor"));
    assert!(!text.contains("taskbarOpacity"));
    assert!(!text.contains("taskbarBlurPercent"));
    assert!(text.contains("surfaceColor=#112233\n"));
    assert!(text.contains("surfaceOpacity=70\n"));
    assert!(text.contains("surfaceBlurPercent=30\n"));
    assert!(stable_setting("taskbarOpacity").is_none());

    let round = load(&ConfigStore::serialize(&explicit)).expect("round trip");
    assert_eq!(round, explicit);
    assert!(load("revision=4\nsurfaceOpacity=101\n").is_err());
    assert!(load("revision=4\nsurfaceColor=red\n").is_err());
    assert!(load("revision=4\nsurfaceBlurPercent=101\n").is_err());
}

#[test]
fn decoration_ios_parses_as_rounded_and_squared_is_accepted() {
    let ios = load("revision=2\ndecoration=ios\n").expect("legacy ios");
    assert_eq!(ios.decoration, "rounded");
    let squared = load("revision=2\ndecoration=squared\n").expect("squared");
    assert_eq!(squared.decoration, "squared");
    assert!(load("revision=2\ndecoration=neon\n").is_err());
}

#[test]
fn render_acceleration_defaults_to_cpu_and_accepts_only_cpu_or_gpu() {
    assert_eq!(
        load("revision=2\n").expect("unset").render_acceleration,
        "cpu"
    );
    assert_eq!(
        load("revision=2\nrenderAcceleration=gpu\n")
            .expect("gpu")
            .render_acceleration,
        "gpu"
    );
    assert!(load("revision=2\nrenderAcceleration=vulkan\n").is_err());
}

#[test]
fn window_margin_defaults_to_two_and_rejects_values_past_thirty_two() {
    assert_eq!(load("revision=2\n").expect("unset").window_margin, 2);
    assert_eq!(
        load("revision=2\nwindowMargin=0\n")
            .expect("zero")
            .window_margin,
        0
    );
    assert!(load("revision=2\nwindowMargin=33\n").is_err());
}

#[test]
fn floating_margin_defaults_to_four_and_stored_value_is_kept() {
    assert_eq!(
        load("revision=2\n").expect("unset").taskbar_floating_margin,
        4
    );
    let legacy = load("revision=2\ntaskbarFloatingMargin=8\n").expect("stored 8");
    assert_eq!(legacy.taskbar_floating_margin, 8);
}

#[test]
fn legacy_titlebar_button_styles_map_to_one_shape_and_are_never_written() {
    let parse = |body: &str| {
        load(&format!("revision=2\n{body}\n")).expect("legacy titlebar styles parse")
    };
    let all_square = "titlebarMinimizeStyle=square\ntitlebarMaximizeStyle=square\n\
        titlebarCloseStyle=square";
    let parsed = parse(all_square);
    assert_eq!(parsed.titlebar_button_style, "square");
    assert_eq!(
        parse("titlebarMinimizeStyle=default\ntitlebarMaximizeStyle=flat\ntitlebarCloseStyle=circle")
            .titlebar_button_style,
        "circle"
    );
    assert_eq!(
        parse("titlebarCloseStyle=square\ntitlebarMinimizeStyle=circle").titlebar_button_style,
        "circle"
    );
    assert_eq!(
        parse("titlebarCloseStyle=square").titlebar_button_style,
        "square"
    );
    assert_eq!(
        parse("titlebarButtonStyle=circle\ntitlebarCloseStyle=square").titlebar_button_style,
        "circle"
    );
    let text = assert_round_trip(&parsed);
    assert!(text.contains("titlebarButtonStyle=square\n"));
    assert!(!text.contains("titlebarMinimizeStyle"));
    assert!(!text.contains("titlebarCloseStyle"));
    assert!(!text.contains("titlebarMaximizeStyle"));
}

#[test]
fn invalid_new_appearance_values_are_rejected() {
    assert_rejected(&[
        "accentOpacity=101",
        "decoration=neon",
        "titlebarMinimizeStyle=round",
        "titlebarButtonStyle=round",
        "cursorTheme=../evil",
        "cursorTheme=a\\b",
        "animationSpeed=24",
        "animationSpeed=201",
        "startAnimation=bounce",
        "animationsEnabled=maybe",
    ]);
}

#[test]
fn wallpaper_path_with_newline_is_rejected() {
    let settings = ProductSettings {
        wallpaper: "bad\npath".to_owned(),
        ..ProductSettings::default()
    };
    assert_eq!(
        settings.validate().expect_err("control character").code,
        ErrorCode::InvalidArgument
    );
}

#[test]
fn base_flags_write_digits_and_optional_flags_write_words() {
    let settings = ProductSettings {
        font_bold: true,
        sticky_notes_enabled: false,
        animations_enabled: false,
        ..ProductSettings::default()
    };
    let text = ConfigStore::serialize(&settings);
    assert!(text.contains("fontBold=1\n"));
    assert!(text.contains("stickyNotesEnabled=0\n"));
    assert!(text.contains("animationsEnabled=false\n"));
    assert!(text.find("fontBold=") < text.find("animationsEnabled="));
    let reparsed = load(&text).expect("round trip");
    assert_eq!(reparsed, settings);
}

#[test]
fn legacy_slide_up_migrates_to_slide_and_is_written_canonically() {
    let parsed = load("revision=2\nstartAnimation=slide-up\n").expect("legacy alias");
    assert_eq!(parsed.start_animation, AnimationStyle::Slide);
    assert_eq!(
        Slot::to_value(&parsed.start_animation),
        SettingValue::Text("slide".to_owned())
    );
    let serialized = ConfigStore::serialize(&parsed);
    assert!(serialized.contains("startAnimation=slide\n"));
    assert!(!serialized.contains("slide-up"));
}

#[test]
fn animation_style_values_are_validated_per_key() {
    for line in [
        "startAnimation=none",
        "startAnimation=slide",
        "startAnimation=fade",
        "startAnimation=magic-lamp",
        "minimizeAnimation=none",
        "minimizeAnimation=fade",
        "minimizeAnimation=magic-lamp",
        "maximizeAnimation=none",
        "maximizeAnimation=fade",
        "maximizeAnimation=magic-lamp", // legacy spelling is accepted as Scale
        "maximizeAnimation=scale",
        "windowOpenAnimation=none",
        "windowOpenAnimation=scale",
        "windowOpenAnimation=fade",
        "windowCloseAnimation=none",
        "windowCloseAnimation=fade",
    ] {
        load(&format!("revision=2\n{line}\n"))
            .unwrap_or_else(|error| panic!("{line} must be accepted: {error:?}"));
    }
    assert_rejected(&[
        "startAnimation=bounce",
        "startAnimation=scale",
        "startAnimation=slide-up-x",
        "minimizeAnimation=slide",
        "minimizeAnimation=slide-up",
        "minimizeAnimation=scale",
        "maximizeAnimation=slide",
        "maximizeAnimation=",
        "windowOpenAnimation=slide",
        "windowOpenAnimation=magic-lamp",
        "windowOpenAnimation=Scale",
        "windowCloseAnimation=scale",
        "windowCloseAnimation=magic-lamp",
        "windowCloseAnimation=Fade",
        "windowCloseAnimation=slide",
    ]);
}

#[test]
fn animation_style_precedence_treats_disabled_or_none_as_none() {
    let mut settings = ProductSettings::default();
    assert_eq!(settings.start_animation_style(), AnimationStyle::None);
    assert_eq!(settings.minimize_animation_style(), AnimationStyle::None);
    assert_eq!(settings.maximize_animation_style(), AnimationStyle::None);
    assert_eq!(settings.window_open_animation_style(), AnimationStyle::None);

    settings.window_open_animation = AnimationStyle::Scale;
    settings.window_open_animation_enabled = true;
    assert_eq!(
        settings.window_open_animation_style(),
        AnimationStyle::Scale
    );

    settings.start_animation_enabled = false;
    settings.minimize_animation_enabled = false;
    settings.maximize_animation_enabled = false;
    settings.window_open_animation_enabled = false;
    assert_eq!(settings.start_animation_style(), AnimationStyle::None);
    assert_eq!(settings.minimize_animation_style(), AnimationStyle::None);
    assert_eq!(settings.maximize_animation_style(), AnimationStyle::None);
    assert_eq!(settings.window_open_animation_style(), AnimationStyle::None);

    settings.start_animation_enabled = true;
    settings.start_animation = AnimationStyle::None;
    settings.minimize_animation_enabled = true;
    settings.minimize_animation = AnimationStyle::None;
    settings.maximize_animation_enabled = true;
    settings.maximize_animation = AnimationStyle::None;
    settings.window_open_animation_enabled = true;
    settings.window_open_animation = AnimationStyle::None;
    assert_eq!(settings.start_animation_style(), AnimationStyle::None);
    assert_eq!(settings.minimize_animation_style(), AnimationStyle::None);
    assert_eq!(settings.maximize_animation_style(), AnimationStyle::None);
    assert_eq!(settings.window_open_animation_style(), AnimationStyle::None);

    let parsed = load("revision=2\nminimizeAnimation=fade\nmaximizeAnimationEnabled=0\n")
        .expect("style and toggle");
    assert_eq!(parsed.minimize_animation_style(), AnimationStyle::Fade);
    assert_eq!(parsed.maximize_animation_style(), AnimationStyle::None);

    let saved = load("revision=2\nwindowOpenAnimation=scale\nwindowOpenAnimationEnabled=0\n")
        .expect("disabled style remains saved");
    assert_eq!(saved.window_open_animation, AnimationStyle::Scale);
    assert_eq!(saved.window_open_animation_style(), AnimationStyle::None);
    let serialized = ConfigStore::serialize(&saved);
    assert!(!serialized.contains("windowOpenAnimationEnabled="));
    let reloaded = load(&serialized).expect("disabled style round trip");
    assert_eq!(reloaded.window_open_animation, AnimationStyle::Scale);
    assert!(!reloaded.window_open_animation_enabled);
    assert_eq!(reloaded.window_open_animation_style(), AnimationStyle::None);
}

#[test]
fn animation_style_keys_round_trip_when_set() {
    let text = "revision=2\nstartAnimation=fade\nminimizeAnimation=none\n\
        maximizeAnimation=fade\nwindowOpenAnimation=fade\nwindowOpenAnimationEnabled=0\n\
        windowCloseAnimation=none\n";
    let parsed = load(text).expect("valid animation styles");
    assert_eq!(parsed.window_close_animation, AnimationStyle::None);
    assert_eq!(parsed.start_animation, AnimationStyle::Fade);
    assert_eq!(parsed.minimize_animation, AnimationStyle::None);
    assert_eq!(parsed.maximize_animation, AnimationStyle::Fade);
    assert_eq!(parsed.window_open_animation, AnimationStyle::Fade);
    assert_eq!(parsed.window_open_animation_style(), AnimationStyle::None);
    let serialized = ConfigStore::serialize(&parsed);
    assert!(serialized.contains("startAnimation=fade\n"));
    assert!(
        !serialized.contains("minimizeAnimation"),
        "default stays unwritten"
    );
    assert!(!serialized.contains("windowOpenAnimationEnabled="));
    assert!(
        !serialized.contains("windowCloseAnimation"),
        "default stays unwritten"
    );
    assert_eq!(load(&serialized).expect("round trip"), parsed);

    let mut enabled = parsed;
    enabled.window_open_animation_enabled = true;
    assert_eq!(enabled.window_open_animation_style(), AnimationStyle::Fade);
    let serialized = ConfigStore::serialize(&enabled);
    assert!(serialized.contains("windowOpenAnimationEnabled=true\n"));
    assert_eq!(load(&serialized).expect("enabled round trip"), enabled);
}

#[test]
fn new_customization_defaults_match_contract_and_write_nothing() {
    let defaults = ProductSettings::default();
    assert_eq!(defaults.wallpaper_fit, WallpaperFit::Fill);
    assert!(!defaults.share_wallpaper);
    assert_eq!(defaults.cursor_size, 24);
    assert_eq!(defaults.taskbar_clock_format, ClockFormat::TwentyFour);
    assert!(!defaults.taskbar_show_seconds);
    assert!(defaults.taskbar_show_date);
    assert!(defaults.snap_preview_enabled);
    assert!(!defaults.focus_follows_mouse);
    let text = ConfigStore::serialize(&defaults);
    for key in [
        "wallpaperFit",
        "shareWallpaper",
        "cursorSize",
        "taskbarClockFormat",
        "taskbarShowSeconds",
        "taskbarShowDate",
        "snapPreviewEnabled",
        "focusFollowsMouse",
    ] {
        assert!(!text.contains(key), "{key} must not be written at default");
    }
}

#[test]
fn new_customization_keys_round_trip_when_set() {
    let parsed = load(
        "revision=2\nwallpaperFit=tile\ncursorSize=64\ntaskbarClockFormat=12h\n\
             taskbarShowSeconds=true\ntaskbarShowDate=false\nsnapPreviewEnabled=0\n\
             focusFollowsMouse=yes\n",
    )
    .expect("valid customization settings");
    assert_eq!(parsed.wallpaper_fit, WallpaperFit::Tile);
    assert_eq!(parsed.cursor_size, 64);
    assert_eq!(parsed.taskbar_clock_format, ClockFormat::Twelve);
    assert!(parsed.taskbar_show_seconds);
    assert!(!parsed.taskbar_show_date);
    assert!(!parsed.snap_preview_enabled);
    assert!(parsed.focus_follows_mouse);
    let text = assert_round_trip(&parsed);
    assert!(text.contains("wallpaperFit=tile\n"));
    assert!(text.contains("cursorSize=64\n"));
    assert!(text.contains("taskbarClockFormat=12h\n"));
    assert!(text.contains("taskbarShowSeconds=true\n"));
    assert!(text.contains("snapPreviewEnabled=false\n"));
}

#[test]
fn shared_wallpaper_toggle_round_trips_and_reads_legacy_spelling() {
    let enabled = load("revision=2\nshareWallpaper=true\n").expect("canonical setting");
    assert!(enabled.share_wallpaper);
    let serialized = ConfigStore::serialize(&enabled);
    assert!(serialized.contains("shareWallpaper=true\n"));
    assert!(!serialized.contains("wallpaperShare="));
    assert!(load(&serialized).expect("canonical reload").share_wallpaper);

    let legacy = load("revision=2\nwallpaperShare=1\n").expect("legacy setting");
    assert!(legacy.share_wallpaper);
    assert!(ConfigStore::serialize(&legacy).contains("shareWallpaper=true\n"));
}

#[test]
fn invalid_new_customization_values_are_rejected() {
    assert_rejected(&[
        "wallpaperFit=Fill",
        "wallpaperFit=zoom",
        "cursorSize=16",
        "cursorSize=20",
        "cursorSize=65",
        "cursorSize=24.0",
        "cursorSize=-24",
        "taskbarClockFormat=24H",
        "taskbarClockFormat=ampm",
        "taskbarShowSeconds=maybe",
        "taskbarShowDate=2",
        "snapPreviewEnabled=on",
        "focusFollowsMouse=sometimes",
    ]);
}

#[test]
fn time_zone_names_accept_iana_shapes_and_reject_paths() {
    for ok in [
        "UTC",
        "Europe/Berlin",
        "America/Argentina/Buenos_Aires",
        "Etc/GMT+3",
    ] {
        assert!(super::is_valid_time_zone_name(ok), "{ok}");
    }
    for bad in [
        "",
        "/etc/localtime",
        "../etc/passwd",
        "Europe/../UTC",
        "a b",
        "x\ny",
    ] {
        assert!(!super::is_valid_time_zone_name(bad), "{bad:?}");
    }
    assert!(load("revision=2\nautoTimeZone=../../etc\n").is_err());
    assert!(load("revision=2\nautoTimeZone=Asia/Tokyo\n").is_ok());
}

#[test]
fn weather_keys_default_on_without_location_and_write_nothing() {
    let defaults = ProductSettings::default();
    assert!(defaults.weather_enabled);
    assert!(defaults.weather_location.is_empty());
    assert_eq!(defaults.weather_units, "celsius");
    assert!(defaults.auto_location_enabled);
    assert!(defaults.auto_time_zone_enabled);
    let text = ConfigStore::serialize(&defaults);
    for key in [
        "weatherEnabled",
        "weatherLocation",
        "weatherUnits",
        "autoLocationEnabled",
        "autoTimeZoneEnabled",
    ] {
        assert!(!text.contains(key), "{key} must not be written at default");
    }
}

#[test]
fn weather_keys_are_typed_stable_rows_and_round_trip() {
    let location = "51.5\t-0.12\tGB\tLondon";
    let parsed = load(&format!(
        "revision=2\nweatherEnabled=true\nweatherUnits=fahrenheit\nweatherLocation={location}\n"
    ))
    .expect("valid weather settings");
    assert!(parsed.weather_enabled);
    assert_eq!(parsed.weather_units, "fahrenheit");
    assert_eq!(parsed.weather_location, location);
    assert!(
        parsed.unknown_keys.is_empty(),
        "weather keys must not fall into unknown_keys"
    );
    let text = assert_round_trip(&parsed);
    // Default-on weather is not written; an explicit off must survive a round trip.
    assert!(!text.contains("weatherEnabled"));
    assert!(text.contains("weatherUnits=fahrenheit\n"));
    let off = load("revision=2\nweatherEnabled=false\n").expect("explicit off");
    assert!(!off.weather_enabled);
    assert!(ConfigStore::serialize(&off).contains("weatherEnabled=false\n"));

    let stable = |key: &str| super::stable_setting(key).expect("typed weather row");
    let defaults = ProductSettings::default();
    assert_eq!(
        (stable("weatherEnabled").get)(&defaults),
        flamewm_api::settings::SettingValue::Boolean(true)
    );
    let mut settings = parsed.clone();
    (stable("weatherLocation").set)(&mut settings, &defaults, None, "weatherLocation")
        .expect("clearing location");
    assert!(settings.weather_location.is_empty());
}

#[test]
fn weather_units_reject_values_other_than_celsius_or_fahrenheit() {
    assert!(load("revision=2\nweatherUnits=kelvin\n").is_err());
    assert!(load("revision=2\nweatherUnits=celsius\n").is_ok());
}

#[test]
fn taskbar_widget_defaults_match_contract_and_write_nothing() {
    let defaults = ProductSettings::default();
    assert!(defaults.taskbar_show_weather);
    assert!(defaults.taskbar_show_volume);
    assert!(defaults.taskbar_show_network);
    assert!(defaults.taskbar_show_battery);
    assert!(defaults.taskbar_show_clock);
    assert!(defaults.taskbar_show_workspaces);
    assert!(!defaults.taskbar_battery_only_when_relevant);
    let text = ConfigStore::serialize(&defaults);
    for key in [
        "taskbarShowWeather",
        "taskbarShowVolume",
        "taskbarShowNetwork",
        "taskbarShowBattery",
        "taskbarShowClock",
        "taskbarShowWorkspaces",
        "taskbarBatteryOnlyWhenRelevant",
    ] {
        assert!(!text.contains(key), "{key} must not be written at default");
    }
}

#[test]
fn taskbar_widget_keys_round_trip_off_and_on_defaults() {
    let parsed = load(
        "revision=2\ntaskbarShowWeather=false\ntaskbarShowVolume=false\n\
             taskbarShowNetwork=false\ntaskbarShowBattery=false\n\
             taskbarShowClock=false\ntaskbarShowWorkspaces=false\n\
             taskbarBatteryOnlyWhenRelevant=true\n",
    )
    .expect("valid taskbar widget settings");
    assert!(!parsed.taskbar_show_weather);
    assert!(!parsed.taskbar_show_volume);
    assert!(!parsed.taskbar_show_network);
    assert!(!parsed.taskbar_show_battery);
    assert!(!parsed.taskbar_show_clock);
    assert!(!parsed.taskbar_show_workspaces);
    assert!(parsed.taskbar_battery_only_when_relevant);
    let text = assert_round_trip(&parsed);
    assert!(text.contains("taskbarShowWeather=false\n"));
    assert!(text.contains("taskbarShowClock=false\n"));
    assert!(text.contains("taskbarBatteryOnlyWhenRelevant=true\n"));

    let explicit_on = load("revision=2\ntaskbarShowWeather=true\ntaskbarShowClock=true\n")
        .expect("explicit default-on values");
    assert!(explicit_on.taskbar_show_weather && explicit_on.taskbar_show_clock);
    let on_text = ConfigStore::serialize(&explicit_on);
    assert!(!on_text.contains("taskbarShowWeather"));
    assert!(!on_text.contains("taskbarShowClock"));
}

#[test]
fn taskbar_widget_keys_reject_invalid_and_keep_unknown_keys() {
    assert_eq!(
        load("revision=2\ntaskbarShowNetwork=maybe\n")
            .expect_err("invalid bool")
            .code,
        ErrorCode::InvalidArgument
    );
    let parsed =
        load("revision=2\naccent=#EF4048\ntaskbarShowBattery=false\nfutureFeature=kept\n")
            .expect("mixed file");
    assert_eq!(parsed.accent, "#EF4048");
    assert!(!parsed.taskbar_show_battery);
    assert!(parsed.taskbar_show_clock);
    assert_eq!(
        parsed.unknown_keys.get("futureFeature").map(String::as_str),
        Some("kept")
    );
}

#[test]
fn legacy_file_without_taskbar_widget_keys_loads_defaults() {
    let parsed =
        load("revision=1\naccent=#EF4048\ntaskbarHeight=44\n").expect("legacy taskbar file");
    assert!(parsed.taskbar_show_weather);
    assert!(parsed.taskbar_show_volume);
    assert!(parsed.taskbar_show_network);
    assert!(parsed.taskbar_show_battery);
    assert!(parsed.taskbar_show_clock);
    assert!(parsed.taskbar_show_workspaces);
    assert!(!parsed.taskbar_battery_only_when_relevant);
}

#[test]
fn legacy_file_without_customization_keys_loads_defaults() {
    let parsed = load("revision=1\naccent=#EF4048\ntaskbarHeight=44\n").expect("legacy file");
    assert_eq!(parsed.wallpaper_fit, WallpaperFit::Fill);
    assert_eq!(parsed.cursor_size, 24);
    assert_eq!(parsed.taskbar_clock_format, ClockFormat::TwentyFour);
    assert!(parsed.snap_preview_enabled);
    assert!(!parsed.focus_follows_mouse);
}

#[test]
fn focus_follows_mouse_writes_only_true() {
    let settings = ProductSettings {
        focus_follows_mouse: true,
        ..ProductSettings::default()
    };
    let text = ConfigStore::serialize(&settings);
    assert!(text.contains("focusFollowsMouse=true\n"));
    assert!(load(&text).expect("round trip").focus_follows_mouse);
}

#[test]
fn wallpaper_fit_parse_and_as_str_are_inverse() {
    for fit in [
        WallpaperFit::Fill,
        WallpaperFit::Fit,
        WallpaperFit::Stretch,
        WallpaperFit::Center,
        WallpaperFit::Tile,
    ] {
        assert_eq!(WallpaperFit::parse(fit.as_str()), Some(fit));
    }
    assert_eq!(WallpaperFit::parse("Tile"), None);
}

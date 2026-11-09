use super::*;
use flamewm_api::settings::DEFAULT_ACCENT_RGB;
use flamewm_ui_x11::test_support::RecordingUiDoc;
use std::sync::Mutex;

/// Serializes tests that write the process-wide task accent and then read it back.
static TASK_ACCENT_TEST_LOCK: Mutex<()> = Mutex::new(());

fn lock_task_accent() -> std::sync::MutexGuard<'static, ()> {
    TASK_ACCENT_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[test]
fn all_shell_surfaces_share_live_contour_settings() {
    let mut snapshot = SettingsSnapshot::new(1);
    snapshot.values.insert(
        "surfaceOutlineColor".to_owned(),
        SettingValue::Text("#306090".to_owned()),
    );
    snapshot.values.insert(
        "surfaceOutlineOpacity".to_owned(),
        SettingValue::Percent(40),
    );
    let contour = UiColor {
        r: 0x30,
        g: 0x60,
        b: 0x90,
        a: 102,
    };
    let mut doc = RecordingUiDoc::default();
    for (kind, root) in [
        (SurfaceKind::Panel, "taskbar"),
        (SurfaceKind::Start, "start-menu"),
        (SurfaceKind::Launcher, "launcher-popup"),
        (SurfaceKind::Switcher, "switcher-popup"),
        (SurfaceKind::TaskMenu, "task-menu"),
        (SurfaceKind::Media, "media-popup"),
        (SurfaceKind::Osd, "osd"),
    ] {
        project(&mut doc, &snapshot, kind).expect("shared surface projection");
        if kind == SurfaceKind::Panel {
            // Separate mode owns only node borders, not the transparent bar root.
            assert_eq!(doc.borders[root].a, 0, "{root}");
        } else {
            assert_eq!(doc.borders[root], contour, "{root}");
        }
    }
    for popup in [
        "audio-popup",
        "wifi-popup",
        "calendar-popup",
        "weather-popup",
    ] {
        project_menu_background(&mut doc, &snapshot, popup).expect("popup projection");
        assert_eq!(doc.borders[popup], contour, "{popup}");
    }
    snapshot
        .values
        .insert("surfaceOutlineOpacity".to_owned(), SettingValue::Percent(0));
    project(&mut doc, &snapshot, SurfaceKind::Start).expect("contour disabled");
    assert_eq!(doc.borders["start-menu"].a, 0);
}

#[test]
fn osd_projection_applies_surface_fill_and_accent_token() {
    let mut snapshot = SettingsSnapshot::new(1);
    snapshot.values.insert(
        "reduceTransparency".to_owned(),
        SettingValue::Boolean(false),
    );
    snapshot
        .values
        .insert("surfaceOpacity".to_owned(), SettingValue::Percent(40));
    snapshot.values.insert(
        "accent".to_owned(),
        SettingValue::Text("#00ff00".to_owned()),
    );
    let mut doc = RecordingUiDoc::default();
    project(&mut doc, &snapshot, SurfaceKind::Osd).expect("osd projection");
    assert_eq!(doc.backgrounds["osd"], surface_background(&snapshot));
    assert_eq!(doc.backgrounds["osd"].a, 102);
    assert_eq!(doc.variables["--accent"].g, 0xff);
    assert!(!doc.backgrounds.contains_key("start-menu"));
}

#[test]
fn start_projection_applies_background_and_accent_tokens() {
    let mut snapshot = SettingsSnapshot::new(1);
    snapshot.values.insert(
        "reduceTransparency".to_owned(),
        SettingValue::Boolean(false),
    );
    snapshot.values.insert(
        "surfaceColor".to_owned(),
        SettingValue::Text("#102030".to_owned()),
    );
    snapshot
        .values
        .insert("surfaceCustom".to_owned(), SettingValue::Boolean(true));
    snapshot
        .values
        .insert("surfaceOpacity".to_owned(), SettingValue::Percent(50));
    snapshot.values.insert(
        "accent".to_owned(),
        SettingValue::Text("#00ff00".to_owned()),
    );
    snapshot
        .values
        .insert("accentOpacity".to_owned(), SettingValue::Percent(50));
    let mut doc = RecordingUiDoc::default();
    project(&mut doc, &snapshot, SurfaceKind::Start).expect("start projection");
    assert_eq!(
        doc.backgrounds["start-menu"],
        UiColor {
            r: 0x10,
            g: 0x20,
            b: 0x30,
            a: 128
        }
    );
    assert_eq!(
        doc.variables["--accent"],
        UiColor {
            r: 0,
            g: 255,
            b: 0,
            a: 127
        }
    );
    assert_eq!(doc.variables["--accent-soft"].a, 27);
    assert_eq!(doc.variables["--accent-strong"].a, 110);
}

// OBSERVABLE: the Start root requests node backdrop blur equal to the shared surfaceBlurPercent
// when it is > 0; at 0 it requests a zero blur (renderer: no blur), the same as desktop menus.
// MUTATION: dropping the node_backdrop call from the Start projection leaves `backdrops` empty.
#[test]
fn start_root_requests_surface_blur_backdrop_only_when_blur_is_set() {
    for (blur, expected) in [(40_u8, Some(40_u8)), (0, Some(0))] {
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot.values.insert(
            "reduceTransparency".to_owned(),
            SettingValue::Boolean(false),
        );
        snapshot
            .values
            .insert("surfaceBlurPercent".to_owned(), SettingValue::Percent(blur));
        let mut doc = RecordingUiDoc::default();
        project(&mut doc, &snapshot, SurfaceKind::Start).expect("start projection");
        assert_eq!(
            doc.backdrops.get("start-menu"),
            Some(&expected),
            "surfaceBlurPercent={blur}"
        );
    }
}

#[test]
fn start_projection_search_field_does_not_reapply_surface_opacity() {
    let mut snapshot = SettingsSnapshot::new(1);
    snapshot.values.insert(
        "reduceTransparency".to_owned(),
        SettingValue::Boolean(false),
    );
    snapshot
        .values
        .insert("surfaceOpacity".to_owned(), SettingValue::Percent(0));
    let mut doc = RecordingUiDoc::default();
    project(&mut doc, &snapshot, SurfaceKind::Start).expect("start projection");
    assert_eq!(doc.backgrounds["start-menu"].a, 0);
    assert_eq!(doc.backgrounds["start-search-box"].a, 255);
}

// OBSERVABLE: the Start search field contour is the shared Surfaces outline (same colour and
// alpha as the menu root), not the fixed `--rim` overlay token it used before.
// MUTATION: dropping the search-box border projection leaves no `start-search-box` entry.
#[test]
fn start_search_field_outline_matches_menu_outline() {
    let mut snapshot = SettingsSnapshot::new(1);
    snapshot.values.insert(
        "surfaceOutlineColor".to_owned(),
        SettingValue::Text("#fedcba".to_owned()),
    );
    snapshot.values.insert(
        "surfaceOutlineOpacity".to_owned(),
        SettingValue::Percent(30),
    );
    let mut doc = RecordingUiDoc::default();
    project(&mut doc, &snapshot, SurfaceKind::Start).expect("start projection");
    let contour = flamewm_ui_x11::surface_style::surface_outline(&snapshot);
    assert_eq!(doc.borders["start-search-box"], contour);
    assert_eq!(doc.borders["start-menu"], contour);
    assert_eq!(contour.a, 76);
}

#[test]
fn launcher_projection_shares_start_surface_pair() {
    let mut doc = RecordingUiDoc::default();
    project(&mut doc, &SettingsSnapshot::new(1), SurfaceKind::Launcher).expect("launcher");
    project(&mut doc, &SettingsSnapshot::new(1), SurfaceKind::Start).expect("start");
    assert_eq!(
        doc.backgrounds["launcher-popup"],
        doc.backgrounds["start-menu"]
    );
    assert_eq!(doc.radii["launcher-popup"], doc.radii["start-menu"]);
}

#[test]
fn default_projection_uses_the_shared_product_accent() {
    // Glass path: the flat default (`reduceTransparency` on) is covered by
    // `flat_default_projects_opaque_surfaces`.
    let mut glass = SettingsSnapshot::new(1);
    glass.values.insert(
        "reduceTransparency".to_owned(),
        SettingValue::Boolean(false),
    );
    let mut doc = RecordingUiDoc::default();
    project(&mut doc, &glass, SurfaceKind::Start).expect("start projection");
    let accent = UiColor {
        r: (DEFAULT_ACCENT_RGB >> 16) as u8,
        g: (DEFAULT_ACCENT_RGB >> 8) as u8,
        b: DEFAULT_ACCENT_RGB as u8,
        a: 255,
    };
    assert_eq!(doc.variables["--accent"], accent);
    assert_eq!(
        doc.variables["--accent-soft"],
        UiColor { a: 0x38, ..accent }
    );
    assert_eq!(
        doc.variables["--accent-strong"],
        UiColor { a: 0xdd, ..accent }
    );
    // Custom surface switch off: the theme-follow default surface (#000000 at 75%), not a custom tint.
    assert_eq!(
        doc.backgrounds["start-menu"],
        UiColor {
            r: 0x00,
            g: 0x00,
            b: 0x00,
            a: 191
        }
    );
}

#[test]
fn surface_background_uses_surface_keys_and_ignores_legacy_taskbar_keys() {
    let mut snapshot = SettingsSnapshot::new(1);
    snapshot.values.insert(
        "reduceTransparency".to_owned(),
        SettingValue::Boolean(false),
    );
    snapshot.values.insert(
        "surfaceColor".to_owned(),
        SettingValue::Text("#102030".to_owned()),
    );
    snapshot
        .values
        .insert("surfaceCustom".to_owned(), SettingValue::Boolean(true));
    snapshot
        .values
        .insert("surfaceOpacity".to_owned(), SettingValue::Percent(100));
    snapshot.values.insert(
        "taskbarColor".to_owned(),
        SettingValue::Text("#ff0000".to_owned()),
    );
    snapshot
        .values
        .insert("taskbarOpacity".to_owned(), SettingValue::Percent(0));
    assert_eq!(
        surface_background(&snapshot),
        UiColor {
            r: 0x10,
            g: 0x20,
            b: 0x30,
            a: 255
        }
    );
    // Custom surface switch off: the theme-follow default surface (#000000 at 75%), not a custom tint.
    let mut glass_default = SettingsSnapshot::new(1);
    glass_default.values.insert(
        "reduceTransparency".to_owned(),
        SettingValue::Boolean(false),
    );
    assert_eq!(
        surface_background(&glass_default),
        UiColor {
            r: 0x00,
            g: 0x00,
            b: 0x00,
            a: 191
        }
    );
}

#[test]
fn flat_default_projects_opaque_surfaces() {
    // Explicit `reduceTransparency` on: every shared surface is opaque.
    let mut defaults = SettingsSnapshot::new(1);
    defaults
        .values
        .insert("reduceTransparency".into(), SettingValue::Boolean(true));
    assert_eq!(surface_background(&defaults).a, 255);
    let mut doc = RecordingUiDoc::default();
    project(&mut doc, &defaults, SurfaceKind::Start).expect("start projection");
    assert_eq!(doc.backgrounds["start-menu"].a, 255);
}

#[test]
fn popup_and_start_corner_radius_follow_decoration_setting() {
    let _accent = lock_task_accent();
    let mut doc = RecordingUiDoc::default();
    project_menu_background(
        &mut doc,
        &snapshot_with("decoration", "rounded"),
        "audio-popup",
    )
    .expect("rounded popup");
    assert_eq!(doc.radii["audio-popup"], 12.0);
    project_menu_background(
        &mut doc,
        &snapshot_with("decoration", "squared"),
        "audio-popup",
    )
    .expect("squared popup");
    assert_eq!(doc.radii["audio-popup"], 0.0);
    project(
        &mut doc,
        &snapshot_with("decoration", "squared"),
        SurfaceKind::Start,
    )
    .expect("start");
    assert_eq!(doc.radii["start-menu"], 0.0);
}

#[test]
fn start_and_popups_share_surface_pair_and_ignore_legacy_startmenu_keys() {
    let _accent = lock_task_accent();
    let mut snapshot = SettingsSnapshot::new(1);
    snapshot.values.insert(
        "reduceTransparency".to_owned(),
        SettingValue::Boolean(false),
    );
    snapshot.values.insert(
        "surfaceColor".to_owned(),
        SettingValue::Text("#445566".to_owned()),
    );
    snapshot
        .values
        .insert("surfaceCustom".to_owned(), SettingValue::Boolean(true));
    snapshot
        .values
        .insert("surfaceOpacity".to_owned(), SettingValue::Percent(10));
    let mut doc = RecordingUiDoc::default();
    project_menu_background(&mut doc, &snapshot, "audio-popup").expect("popup");
    assert_eq!(
        doc.backgrounds["audio-popup"],
        UiColor {
            r: 0x44,
            g: 0x55,
            b: 0x66,
            a: 26
        }
    );
    project(&mut doc, &snapshot, SurfaceKind::Start).expect("start");
    assert_eq!(
        doc.backgrounds["start-menu"],
        doc.backgrounds["audio-popup"]
    );
}

#[test]
fn taskbar_projection_and_backdrop_follow_surface_keys() {
    let _accent = lock_task_accent();
    let mut snapshot = SettingsSnapshot::new(1);
    snapshot.values.insert(
        "reduceTransparency".to_owned(),
        SettingValue::Boolean(false),
    );
    snapshot.values.insert(
        "surfaceColor".to_owned(),
        SettingValue::Text("#445566".to_owned()),
    );
    snapshot
        .values
        .insert("surfaceOpacity".to_owned(), SettingValue::Percent(20));
    snapshot
        .values
        .insert("surfaceBlurPercent".to_owned(), SettingValue::Percent(15));
    // The glass key is authoritative; the legacy surface keys only seed it when absent.
    snapshot
        .values
        .insert("glassBlurPx".to_owned(), SettingValue::Percent(28));
    // Blur Only is an explicit choice now; the default taskbar mode is Inherit.
    snapshot.values.insert(
        "taskbarMaterialMode".to_owned(),
        SettingValue::Text("blur-only".to_owned()),
    );
    // Taskbar capsules use the Dock material (Blur Only: neutral white at 8%, alpha 20), not the
    // legacy surfaceColor/surfaceOpacity pair, which still drives popups and menus.
    let mut doc = RecordingUiDoc::default();
    project(&mut doc, &snapshot, SurfaceKind::Panel).expect("panel");
    // `taskbarSeparate` now defaults off: the unified root paints the Dock tint and the
    // element capsules keep no contour of their own.
    assert_eq!(
        doc.backgrounds["taskbar"],
        UiColor {
            r: 0xff,
            g: 0xff,
            b: 0xff,
            a: 20
        }
    );
    assert_eq!(
        doc.borders["taskbar"].a, 0,
        "classic unified bar keeps no contour of its own"
    );
    assert_eq!(
        doc.borders["workspace-pager"].a, 0,
        "unified bar clears pager contour"
    );
    assert_eq!(
        doc.backgrounds["workspace-pager"].a, 0,
        "unified bar clears pager fill"
    );
    // An explicit `taskbarSeparate` still switches to one capsule per element: the root
    // stops painting the tint so the wallpaper shows between the capsules.
    snapshot
        .values
        .insert("taskbarSeparate".to_owned(), SettingValue::Boolean(true));
    let mut separate = RecordingUiDoc::default();
    project(&mut separate, &snapshot, SurfaceKind::Panel).expect("separate panel");
    assert_eq!(
        separate.backgrounds["taskbar"],
        UiColor {
            r: 0xff,
            g: 0xff,
            b: 0xff,
            a: 0
        },
        "separated bar root paints no tint"
    );
    assert_eq!(
        separate.borders["workspace-pager"].a,
        // Shared rim alpha: RIM_PERCENT of 255, the same derivation as surface_stroke.
        u8::try_from(u16::from(flamewm_ui_core::glass::RIM_PERCENT) * 255 / 100)
            .expect("rim alpha fits u8"),
        "pager matches other capsules"
    );
    assert_eq!(
        separate.backgrounds["workspace-pager"].a, 20,
        "pager uses the Dock material fill"
    );
    // Dock material is independent of surfaceOpacity/surfaceBlurPercent (Blur Only: 8%, 34px).
    assert_eq!(taskbar_backdrop_percents(&snapshot), (8, 85));
}

// REGRESSION: a classic bar must reach the screen end with one top outline, not close off
// its own border box. The bar root keeps no contour and the viewport-wide outline carries
// the shared surface outline.
// MUTATION: draw the classic contour on the root again, or leave the outline transparent.
#[test]
fn classic_taskbar_draws_one_full_width_top_outline() {
    let _accent = lock_task_accent();
    let snapshot = SettingsSnapshot::new(1);
    assert_eq!(
        text_setting(&snapshot, "taskbarStyle", "classic"),
        "classic",
        "a fresh snapshot is the classic layout"
    );
    let mut doc = RecordingUiDoc::default();
    project(&mut doc, &snapshot, SurfaceKind::Panel).expect("classic panel");
    assert_eq!(
        doc.borders["taskbar"].a, 0,
        "classic bar root owns no contour"
    );
    let contour = flamewm_ui_x11::surface_style::surface_outline(&snapshot);
    assert!(contour.a > 0, "the default surface outline is visible");
    assert_eq!(doc.backgrounds["taskbar-outline"], contour);
}

// REGRESSION: the floating layout keeps the bar's own contour (unified) or the per-capsule
// contours (separate) and must never add the classic top outline.
#[test]
fn floating_taskbar_keeps_the_bar_contour_and_clears_the_outline() {
    let _accent = lock_task_accent();
    let mut snapshot = SettingsSnapshot::new(1);
    snapshot.values.insert(
        "taskbarStyle".to_owned(),
        SettingValue::Text("floating".to_owned()),
    );
    let contour = flamewm_ui_x11::surface_style::surface_outline(&snapshot);
    let mut unified = RecordingUiDoc::default();
    project(&mut unified, &snapshot, SurfaceKind::Panel).expect("floating panel");
    assert_eq!(
        unified.borders["taskbar"], contour,
        "floating unified bar keeps the shared contour"
    );
    assert_eq!(
        unified.backgrounds["taskbar-outline"].a, 0,
        "floating bar draws no top outline"
    );
    snapshot
        .values
        .insert("taskbarSeparate".to_owned(), SettingValue::Boolean(true));
    let mut separate = RecordingUiDoc::default();
    project(&mut separate, &snapshot, SurfaceKind::Panel).expect("floating separate panel");
    assert_eq!(
        separate.borders["taskbar"].a, 0,
        "floating separate bar root keeps no contour"
    );
    assert_eq!(separate.backgrounds["taskbar-outline"].a, 0);
}

#[test]
fn popup_menu_background_projects_live_accent_to_popup_and_task_accent() {
    let _accent = lock_task_accent();
    let mut snapshot = SettingsSnapshot::new(1);
    snapshot.values.insert(
        "accent".to_owned(),
        SettingValue::Text("#2080e0".to_owned()),
    );
    let mut doc = RecordingUiDoc::default();
    project_menu_background(&mut doc, &snapshot, "wifi-popup").expect("popup background");
    let accent = UiColor {
        r: 0x20,
        g: 0x80,
        b: 0xe0,
        a: 0xff,
    };
    assert_eq!(doc.variables["--accent"], accent);
    assert_eq!(crate::projection::task_accent(), accent);
}

fn write_ppm_fixture(name: &str) -> std::path::PathBuf {
    let path =
        std::env::temp_dir().join(format!("flamewm-start-icon-{}-{name}", std::process::id()));
    let mut bytes = b"P6\n2 2\n255\n".to_vec();
    bytes.extend_from_slice(&[200u8; 12]);
    std::fs::write(&path, bytes).expect("write ppm fixture");
    path
}

#[test]
fn configured_start_icon_projects_a_square_raster_into_the_panel_node() {
    let path = write_ppm_fixture("ok.ppm");
    let mut document = RecordingUiDoc::default();
    project_start_icon(&mut document, Some(path.display().to_string())).expect("project");
    let _ = std::fs::remove_file(&path);
    // A 2x2 source must be scaled to the 25px box, not kept at natural size.
    assert_eq!(document.images.get(START_ICON_ID), Some(&(25, 25)));
}

#[test]
fn clearing_custom_start_icon_reverts_live_to_packaged_default() {
    let path = write_ppm_fixture("clear.ppm");
    let mut document = RecordingUiDoc::default();
    project_start_icon(&mut document, Some(path.display().to_string())).expect("apply");
    let _ = std::fs::remove_file(&path);
    assert_eq!(document.images.get(START_ICON_ID), Some(&(25, 25)));
    project_start_icon(&mut document, None).expect("clear");
    assert!(!document.images.contains_key(START_ICON_ID));
}

#[test]
fn missing_none_or_unsupported_start_icon_keeps_the_packaged_default() {
    let mut document = RecordingUiDoc::default();
    project_start_icon(&mut document, None).expect("none");
    project_start_icon(&mut document, Some("/nonexistent/start.png".to_owned())).expect("missing");
    let path = write_ppm_fixture("unsupported.txt");
    let renamed = path.with_extension("txt");
    std::fs::rename(&path, &renamed).expect("rename fixture");
    project_start_icon(&mut document, Some(renamed.display().to_string())).expect("txt");
    let _ = std::fs::remove_file(&renamed);
    assert!(document.images.is_empty());
}

#[test]
fn custom_start_icon_is_none_for_empty_and_blank_values() {
    assert_eq!(custom_start_icon(&SettingsSnapshot::new(1)), None);
    assert_eq!(
        custom_start_icon(&snapshot_with("customStartIcon", "")),
        None
    );
    assert_eq!(
        custom_start_icon(&snapshot_with("customStartIcon", "   ")),
        None
    );
    assert_eq!(
        custom_start_icon(&snapshot_with(
            "customStartIcon",
            "/home/u/.local/share/flamewm/start.png"
        )),
        Some("/home/u/.local/share/flamewm/start.png".to_owned())
    );
}

#[test]
fn start_backdrop_percents_read_flat_keys_with_defaults() {
    let mut snapshot = SettingsSnapshot::new(1);
    assert_eq!(
        start_backdrop_percents(&snapshot),
        (DEFAULT_SURFACE_OPACITY, DEFAULT_SURFACE_BLUR_PERCENT)
    );
    snapshot
        .values
        .insert("surfaceOpacity".to_owned(), SettingValue::Percent(50));
    snapshot
        .values
        .insert("surfaceBlurPercent".to_owned(), SettingValue::Percent(30));
    assert_eq!(start_backdrop_percents(&snapshot), (50, 30));
    assert_eq!(start_backdrop_percents(&snapshot), (50, 30));
}

#[test]
fn dock_pill_follows_the_taskbar_material_and_outline() {
    let blur_only = snapshot_with("taskbarMaterialMode", "blur-only");
    let mut doc = RecordingUiDoc::default();
    project(&mut doc, &blur_only, SurfaceKind::Dock).expect("dock projection");
    let fill = taskbar_fill(&blur_only);
    assert_eq!(fill.a, 20, "Blur Only is the Dock material alpha");
    assert_eq!(doc.backgrounds["dock-bar"], fill);
    let outline = flamewm_ui_x11::surface_style::surface_outline(&blur_only);
    assert_eq!(doc.borders["dock-bar"], outline);
    assert_eq!(doc.backgrounds["dock-highlight"].a, outline.a / 2);

    // The same setting moves the pill: changing the taskbar material re-tints the Dock.
    let inherit = snapshot_with("taskbarMaterialMode", "inherit");
    let mut doc = RecordingUiDoc::default();
    project(&mut doc, &inherit, SurfaceKind::Dock).expect("dock projection");
    assert_eq!(doc.backgrounds["dock-bar"], taskbar_fill(&inherit));
    assert_ne!(taskbar_fill(&inherit), fill);
}

fn snapshot_with(key: &str, value: &str) -> SettingsSnapshot {
    let mut snapshot = SettingsSnapshot::new(1);
    snapshot
        .values
        .insert(key.to_owned(), SettingValue::Text(value.to_owned()));
    snapshot
}

#[test]
fn font_mapping_applies_saved_family_and_size_offset() {
    let mut saved = SettingsSnapshot::new(2);
    saved.values.insert(
        "fontFamily".to_owned(),
        SettingValue::Text("DejaVu Sans".to_owned()),
    );
    saved
        .values
        .insert("fontSizeOffset".to_owned(), SettingValue::Integer(3));
    assert_eq!(
        text_setting(&saved, "fontFamily", DEFAULT_FONT_FAMILY),
        "DejaVu Sans"
    );
    assert_eq!(
        (
            integer_setting(&saved, "fontSizeOffset", 0) as f32,
            bool_setting(&saved, "fontBold", false)
        ),
        (3.0, false)
    );

    let defaults = SettingsSnapshot::new(1);
    assert_eq!(
        text_setting(&defaults, "fontFamily", DEFAULT_FONT_FAMILY),
        DEFAULT_FONT_FAMILY
    );
    assert_eq!(integer_setting(&defaults, "fontSizeOffset", 0), 0);

    let mut tiny = SettingsSnapshot::new(3);
    tiny.values
        .insert("fontSizeOffset".to_owned(), SettingValue::Integer(-40));
    assert_eq!(integer_setting(&tiny, "fontSizeOffset", 0), -40);
}

#[test]
fn opacity_percent_maps_to_alpha_bounds() {
    assert_eq!(opacity_alpha(0), 0);
    assert_eq!(opacity_alpha(100), 255);
    assert_eq!(opacity_alpha(DEFAULT_SURFACE_OPACITY), 191);
}

#[test]
fn accent_change_reprojects_every_accent_derived_token() {
    const TOKEN_ALPHAS: [(&str, u8); 8] = [
        ("--accent", 0xff),
        ("--accent-soft", 0x38),
        ("--accent-strong", 0xdd),
        ("--desktop-hover", 0x26),
        ("--desktop-hover-border", 0x4d),
        ("--desktop-selected", 0x47),
        ("--desktop-selected-border", 0xa6),
        ("--snap-fill", 0x33),
    ];
    for (hex, rgb) in [
        ("#3daee9", 0x003d_aee9_u32),
        ("#1abc9c", 0x001a_bc9c),
        ("#9b59b6", 0x009b_59b6),
    ] {
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot
            .values
            .insert("accent".to_owned(), SettingValue::Text(hex.to_owned()));
        let mut doc = RecordingUiDoc::default();
        project(&mut doc, &snapshot, SurfaceKind::Start).expect("start projection");
        let base = UiColor {
            r: (rgb >> 16) as u8,
            g: (rgb >> 8) as u8,
            b: rgb as u8,
            a: 255,
        };
        for (name, alpha) in TOKEN_ALPHAS {
            assert_eq!(
                doc.variables[name],
                UiColor { a: alpha, ..base },
                "{name} under {hex}"
            );
        }
        assert_eq!(doc.variables["--preview-outline"], base);
    }
}

#[test]
fn neutral_tokens_project_light_palette_and_dark_defaults() {
    let mut light = SettingsSnapshot::new(1);
    light
        .values
        .insert("appearance".into(), SettingValue::Text("light".into()));
    let mut doc = RecordingUiDoc::default();
    project_accent_tokens(&mut doc, &light).expect("project light tokens");
    let palette = flamewm_ui_core::glass::palette_from_settings(&light);
    assert_eq!(doc.variables["--text"], rgb(palette.text.0, 255));
    assert_eq!(doc.variables["--muted"], rgb(palette.muted.0, 255));
    assert_eq!(doc.variables["--surface"], rgb(palette.surface.0, 255));

    let mut dark = RecordingUiDoc::default();
    project_accent_tokens(&mut dark, &SettingsSnapshot::new(1)).expect("project dark tokens");
    let shipped = flamewm_ui_core::glass::palette_from_settings(&SettingsSnapshot::new(1));
    assert_eq!(dark.variables["--text"], rgb(shipped.text.0, 255));
    assert_eq!(dark.variables["--panel"], rgb(shipped.panel.0, 255));
}

#[test]
fn preview_outline_stays_opaque_when_accent_opacity_changes() {
    let mut snapshot = SettingsSnapshot::new(1);
    snapshot
        .values
        .insert("accent".into(), SettingValue::Text("#123456".into()));
    snapshot
        .values
        .insert("accentOpacity".into(), SettingValue::Percent(5));
    let mut doc = RecordingUiDoc::default();
    project_accent_tokens(&mut doc, &snapshot).expect("project accent tokens");
    assert_eq!(doc.variables["--preview-outline"], rgb(0x0012_3456, 255));
    let css = include_str!("../../../../ui/shell/flamewm.css");
    assert!(css.contains(".desktop-selection { position:absolute; left:0px; top:0px; width:0px; height:0px; border:2px solid var(--preview-outline); border-radius:var(--r-md); }"));
    assert!(css.contains(".snap-preview { position:absolute; left:0px; top:0px; width:0px; height:0px; background:var(--snap-fill); border:2px solid var(--preview-outline); }"));
}

fn css_rules(css: &str) -> Vec<(&str, &str)> {
    css.split('}')
        .filter_map(|chunk| chunk.split_once('{'))
        .map(|(selector, body)| (selector.trim(), body))
        .collect()
}

fn border_radius_px(body: &str) -> Option<f32> {
    let rest = body.split("border-radius:").nth(1)?;
    rest.split(';')
        .next()?
        .trim()
        .trim_end_matches("px")
        .parse()
        .ok()
}

#[test]
fn every_compiled_shell_surface_declares_accent_tokens() {
    let artifacts: [(&str, &[u8]); 10] = [
        (
            "launcher",
            include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-launcher.rwr")),
        ),
        (
            "panel",
            include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-panel.rwr")),
        ),
        (
            "start",
            include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-start.rwr")),
        ),
        (
            "task-menu",
            include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-task-menu.rwr")),
        ),
        (
            "media",
            include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-media.rwr")),
        ),
        (
            "audio",
            include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-audio.rwr")),
        ),
        (
            "network",
            include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-network.rwr")),
        ),
        (
            "calendar",
            include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-calendar.rwr")),
        ),
        (
            "osd",
            include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-osd.rwr")),
        ),
        (
            "battery",
            include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-battery.rwr")),
        ),
    ];
    for (surface, bytes) in artifacts {
        let mut document = flamewm_ui_x11::decode_document(bytes)
            .unwrap_or_else(|error| panic!("{surface} artifact must decode: {error}"));
        project_accent_tokens(&mut document, &SettingsSnapshot::new(1))
            .unwrap_or_else(|error| panic!("{surface} must declare accent tokens: {error}"));
    }
}

#[test]
fn shell_css_has_no_hardcoded_default_red() {
    let css = include_str!("../../../../ui/shell/flamewm.css");
    for line in css.lines().filter(|line| line.contains("ef4048")) {
        assert!(
            line.contains(".swatch-red") || line.contains("--c-ef4048:#ef4048"),
            "hardcoded accent outside swatch: {line}"
        );
    }
    assert!(
        !css.contains("e81123"),
        "close hover still hard-codes a red"
    );
}

#[test]
fn shell_rust_projections_have_no_hardcoded_red_fill() {
    let sources = [
        include_str!("../projection.rs"),
        include_str!("../async_projection.rs"),
        include_str!("../async_projection/icon_binding.rs"),
        include_str!("../async_projection/icon_cache.rs"),
        include_str!("../async_projection/metrics.rs"),
        include_str!("../async_projection/panel.rs"),
        include_str!("../async_projection/start_view.rs"),
        include_str!("../async_projection/task_slots.rs"),
        include_str!("../projection/network.rs"),
        include_str!("../projection/audio.rs"),
    ];
    for source in sources {
        assert!(
            !source.contains("r: 239"),
            "hardcoded flame red fill in shell projection"
        );
    }
}

// REGRESSION: a window-holding idle pager cell gets the occupied tint, an empty idle cell keeps the
// idle fill, and the active cell keeps the accent fill whatever its occupancy.
#[test]
fn occupied_pager_cells_are_tinted_and_active_cell_keeps_accent() {
    let mut doc = RecordingUiDoc::default();
    let snapshot = flamewm_api::workspace::WorkspaceSnapshot {
        revision: 1,
        count: 3,
        active_index: 2,
        last_index: None,
        names: Vec::new(),
        occupancy: vec![1, 0, 1],
    };
    crate::projection::project_pager(&mut doc, Some(&snapshot)).expect("pager projects");
    // Cells: index 0 -> cell 1, index 1 -> cell 3 (empty), index 2 -> cell 2 (active).
    assert_eq!(doc.backgrounds["workspace-1"].a, 40, "occupied idle cell");
    assert_eq!(doc.backgrounds["workspace-3"].a, 128, "empty idle cell");
    assert_ne!(
        doc.backgrounds["workspace-2"],
        doc.backgrounds["workspace-1"]
    );
}

#[test]
fn workspace_pager_cells_are_borderless_and_share_one_radius() {
    let css = include_str!("../../../../ui/shell/flamewm.css");
    let rules = css_rules(css);
    for (selector, body) in &rules {
        if selector.contains("workspace-button") {
            if let Some(radius) = border_radius_px(body) {
                assert_eq!(radius, 5.0, "pager radius {radius}px in `{selector}`");
            }
            assert!(
                !body.contains("border-color"),
                "pager cell `{selector}` must not paint an outline"
            );
        }
    }
    for required in [
        ".workspace-button",
        ".workspace-button:hover",
        ".workspace-button.active",
    ] {
        let body = rules
            .iter()
            .find(|(selector, _)| *selector == required)
            .map(|(_, body)| *body)
            .expect(required);
        assert_eq!(border_radius_px(body), Some(5.0), "{required} radius");
    }
}

#[test]
fn accent_carries_accent_opacity_as_alpha() {
    let mut snapshot = SettingsSnapshot::new(1);
    snapshot
        .values
        .insert("accentOpacity".to_owned(), SettingValue::Percent(0));
    let transparent = accent_color(&snapshot);
    assert_eq!(
        (transparent.r, transparent.g, transparent.b, transparent.a),
        (
            (DEFAULT_ACCENT_RGB >> 16) as u8,
            (DEFAULT_ACCENT_RGB >> 8) as u8,
            DEFAULT_ACCENT_RGB as u8,
            0
        )
    );

    snapshot
        .values
        .insert("accentOpacity".to_owned(), SettingValue::Percent(100));
    assert_eq!(accent_color(&snapshot).a, 255);
    assert_eq!(accent_color(&SettingsSnapshot::new(1)).a, 255);
}

fn slide_snapshot(pairs: &[(&str, SettingValue)]) -> SettingsSnapshot {
    let mut snapshot = SettingsSnapshot::new(1);
    for (key, value) in pairs {
        snapshot.values.insert((*key).to_owned(), value.clone());
    }
    snapshot
}

const START_BUTTON: (i32, i32, u32, u32) = (20, 1050, 48, 36);

#[test]
fn start_slide_maps_each_panel_edge() {
    let snapshot = slide_snapshot(&[("startAnimation", SettingValue::Text("slide".to_owned()))]);
    let edge_of = |edge| start_slide(&snapshot, edge, None).map(|slide| slide.edge);
    assert_eq!(edge_of(PanelEdge::Bottom), Some(SlideEdge::Bottom));
    assert_eq!(edge_of(PanelEdge::Top), Some(SlideEdge::Top));
    assert_eq!(edge_of(PanelEdge::Left), Some(SlideEdge::Left));
    assert_eq!(edge_of(PanelEdge::Right), Some(SlideEdge::Right));
}

#[test]
fn start_slide_missing_keys_default_to_immediate_and_slide_uses_base_speed() {
    assert_eq!(
        start_slide(&SettingsSnapshot::new(1), PanelEdge::Bottom, None),
        None
    );
    let snapshot = slide_snapshot(&[("startAnimation", SettingValue::Text("slide".to_owned()))]);
    let slide = start_slide(&snapshot, PanelEdge::Bottom, None).expect("slide");
    assert_eq!(slide, SlideConfig::slide(SlideEdge::Bottom, 100));
}

#[test]
fn start_slide_disabled_by_each_flag_for_every_style() {
    let off = SettingValue::Boolean(false);
    for style in ["slide", "fade", "magic-lamp"] {
        let style = SettingValue::Text(style.to_owned());
        let cases = [
            slide_snapshot(&[
                ("animationsEnabled", off.clone()),
                ("startAnimation", style.clone()),
            ]),
            slide_snapshot(&[
                ("startAnimationEnabled", off.clone()),
                ("startAnimation", style.clone()),
            ]),
        ];
        for snapshot in &cases {
            assert_eq!(
                start_slide(snapshot, PanelEdge::Bottom, Some(START_BUTTON)),
                None
            );
        }
    }
}

#[test]
fn start_slide_none_and_unknown_styles_are_immediate() {
    for style in ["none", "bounce"] {
        let snapshot = slide_snapshot(&[("startAnimation", SettingValue::Text(style.to_owned()))]);
        assert_eq!(
            start_slide(&snapshot, PanelEdge::Bottom, Some(START_BUTTON)),
            None,
            "{style}"
        );
    }
}

#[test]
fn start_slide_slide_and_legacy_slide_up_keep_the_slide() {
    for style in ["slide", "slide-up"] {
        let snapshot = slide_snapshot(&[("startAnimation", SettingValue::Text(style.to_owned()))]);
        let slide = start_slide(&snapshot, PanelEdge::Top, None).expect(style);
        assert_eq!(slide, SlideConfig::slide(SlideEdge::Top, 100), "{style}");
    }
}

#[test]
fn start_slide_fade_has_no_target_and_keeps_the_speed() {
    let snapshot = slide_snapshot(&[
        ("startAnimation", SettingValue::Text("fade".to_owned())),
        ("animationSpeed", SettingValue::Integer(50)),
    ]);
    assert_eq!(
        start_slide(&snapshot, PanelEdge::Left, None),
        Some(SlideConfig::fade(SlideEdge::Left, 50))
    );
}

#[test]
fn start_slide_magic_lamp_folds_into_the_start_button() {
    let snapshot = slide_snapshot(&[(
        "startAnimation",
        SettingValue::Text("magic-lamp".to_owned()),
    )]);
    assert_eq!(
        start_slide(&snapshot, PanelEdge::Bottom, Some(START_BUTTON)),
        Some(SlideConfig::lamp(SlideEdge::Bottom, 100, START_BUTTON))
    );
    // Without button layout the lamp cannot fold anywhere, so the show stays immediate.
    assert_eq!(start_slide(&snapshot, PanelEdge::Bottom, None), None);
}

#[test]
fn start_slide_clamps_speed_to_supported_range() {
    let speed = |value| {
        let snapshot = slide_snapshot(&[
            ("startAnimation", SettingValue::Text("slide".to_owned())),
            ("animationSpeed", SettingValue::Integer(value)),
        ]);
        start_slide(&snapshot, PanelEdge::Top, None)
            .expect("slide")
            .speed
    };
    assert_eq!(speed(10), 25);
    assert_eq!(speed(25), 25);
    assert_eq!(speed(150), 150);
    assert_eq!(speed(999), 200);
}

#[test]
fn icon_theme_name_prefers_snapshot_then_product_default() {
    let set = snapshot_with("iconTheme", "ThemeX");
    assert_eq!(icon_theme_name(Some(&set)), "ThemeX");
    let default = flamewm_platform::settings::icon_theme::DEFAULT_ICON_THEME;
    assert_eq!(icon_theme_name(None), default);
    assert_eq!(icon_theme_name(Some(&SettingsSnapshot::new(1))), default);
}

use std::env;
use std::path::{Path, PathBuf};

use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_ui_core::glass::{self, MaterialRole, RendererCaps};
use flamewm_ui_x11::backdrop::WindowBackdrop;
use flamewm_ui_x11::{UiDocument, decode_document};

const COMPILED_UI: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-settings.rwr"));

pub const SETTINGS_DEFAULT_WIDTH: u32 = 820;
pub const SETTINGS_DEFAULT_HEIGHT: u32 = 540;

pub fn document() -> Result<UiDocument, String> {
    decode_document(COMPILED_UI)
}

/// The settings document with the window-level wallpaper backdrop under its translucent body glass.
/// Opacity is the resolved GlassRegular alpha, so Reduce transparency installs no backdrop.
pub fn document_with_backdrop(
    snapshot: Option<&SettingsSnapshot>,
) -> Result<UiDocument, String> {
    Ok(decode_document(COMPILED_UI)?.with_window_backdrop(window_backdrop(snapshot)))
}

fn window_backdrop(snapshot: Option<&SettingsSnapshot>) -> WindowBackdrop {
    let text = |key: &str| match snapshot.and_then(|value| value.values.get(key)) {
        Some(SettingValue::Text(value)) => value.clone(),
        _ => String::new(),
    };
    let fit = text("wallpaperFit");
    let empty = SettingsSnapshot::new(0);
    let appearance = glass::appearance_from_settings(snapshot.unwrap_or(&empty));
    let body = glass::resolve(
        MaterialRole::GlassRegular,
        appearance,
        100,
        RendererCaps {
            backdrop_blur: true,
        },
    );
    WindowBackdrop {
        wallpaper: text("wallpaper"),
        fit: if fit.is_empty() { "fill".into() } else { fit },
        packaged_root: packaged_root(),
        opacity_percent: u8::try_from(u32::from(body.alpha_permille.min(1000)) / 10).unwrap_or(100),
        blur_percent: glass::blur_percent_from_px(body.blur_px),
    }
}

/// Packaged asset root for the default wallpaper: the install prefix, else the source checkout.
fn packaged_root() -> PathBuf {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let installed = env::current_exe().ok().and_then(|exe| {
        let bin = exe.parent()?;
        let usr = bin.parent()?;
        (bin.file_name()? == "bin" && usr.file_name()? == "usr").then(|| usr.join("share/flamewm"))
    });
    installed
        .filter(|root| root.join("assets").is_dir())
        .unwrap_or(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HTML: &str = include_str!("../../../ui/settings.html");
    const CSS: &str = include_str!("../../../ui/settings.css");
    const DISPATCH: &str = include_str!("lib.rs");

    const PROJECTED_TEXT_IDS: &[&str] = &[
        "display-mode-slot-0",
        "display-mode-slot-1",
        "display-mode-slot-2",
        "display-mode-slot-3",
        "display-mode-overflow",
        "display-output-name-0",
        "display-output-size-0",
        "display-output-name-1",
        "display-output-size-1",
        "display-output-name-2",
        "display-output-size-2",
        "display-output-name-3",
        "display-output-size-3",
        "display-selected-title",
        "display-selected-resolution",
        "display-scale-value",
        "icon-theme-selected-name",
        "selection-opacity-value",
        "snap-opacity-value",
        "desktop-entry-size-value",
        "surface-blur-value",
        "surface-saturation-value",
        "surface-rim-fade-value",
        "surface-shadow-value",
        "surface-highlight-value",
        "surface-opacity-value",
        "start-icon-value",
        "file-manager-value",
        "window-margin-value",
        "taskbar-floating-margin-value",
        "taskbar-floating-radius-value",
        "wallpaper-fit-value",
        "wallpaper-clear",
        "wallpaper-default",
        "lock-background-default",
        "screensaver-after-lock-value",
        "screensaver-ribbons-value",
        "screensaver-ribbon-width-value",
        "screensaver-display-off-value",
        "cursor-theme-value",
        "theme-install-status",
        "theme-icon-page-label",
        "theme-cursor-page-label",
        "theme-icon-name-0",
        "theme-icon-name-1",
        "theme-icon-name-2",
        "theme-icon-name-3",
        "theme-icon-name-4",
        "theme-icon-name-5",
        "theme-cursor-name-0",
        "theme-cursor-name-1",
        "theme-cursor-name-2",
        "theme-cursor-name-3",
        "theme-cursor-name-4",
        "theme-cursor-name-5",
        "accent-opacity-value",
        "animation-speed-value",
        "lock-background-image-value",
        "avatar-value",
        "weather-location-value",
        "lock-bg-blur-value",
        "lock-bg-dim-value",
        "taskbar-size-value",
        "color-picker-hex",
        "color-picker-error",
        "taskbar-edge-value",
        "font-family-value",
        "font-size-value",
        "settings-error",
        "about-version",
        "taskbar-output-title",
        "taskbar-output-0",
        "taskbar-output-1",
        "taskbar-output-2",
        "taskbar-output-3",
        "display-output-page-label",
        "display-mode-page-label",
        "taskbar-output-page-label",
        "hotkey-key-start",
        "hotkey-key-left",
        "hotkey-key-right",
        "hotkey-key-up",
        "hotkey-key-down",
    ];

    const SETTINGS_PAGES: &[&str] = &[
        "theme",
        "start-menu",
        "surfaces",
        "animations",
        "lock-screen",
        "screensaver",
        "desktop",
        "taskbar",
        "displays",
        "fonts",
        "hotkeys",
        "assistant",
        "about",
    ];

    fn css_rule<'a>(css: &'a str, selector: &str) -> &'a str {
        css.split_once(&format!("{selector} {{"))
            .and_then(|(_, rule)| rule.split_once('}').map(|(rule, _)| rule))
            .unwrap_or_else(|| panic!("settings CSS must define {selector}"))
    }

    fn css_value<'a>(rule: &'a str, property: &str) -> Option<&'a str> {
        rule.split(';').find_map(|declaration| {
            let (name, value) = declaration.trim().split_once(':')?;
            (name.trim() == property).then_some(value.trim())
        })
    }

    fn attr_values<'a>(html: &'a str, prefix: &str) -> Vec<&'a str> {
        html.split(prefix)
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .collect()
    }

    fn page_between<'a>(html: &'a str, from: &str, to: &str) -> &'a str {
        html.split_once(&format!("id=\"{from}\""))
            .and_then(|(_, rest)| rest.split_once(&format!("id=\"{to}\"")))
            .map_or_else(
                || panic!("settings page {from} must precede {to}"),
                |(body, _)| body,
            )
    }

    fn seed_text(document: &mut UiDocument, id: &str, text: &str) {
        document
            .text(id, text.to_owned())
            .unwrap_or_else(|error| panic!("compiled text target {id}: {error}"));
    }

    fn assert_action_control(id: &str, action: &str) {
        assert!(
            HTML.contains(&format!("id=\"{id}\" data-action=\"{action}\"")),
            "control {id} must retain its production action {action}"
        );
        assert!(
            DISPATCH.contains(&format!("\"{action}\" =>")),
            "action {action} must have a production dispatch branch"
        );
    }

    #[test]
    fn display_outputs_are_compact_numbered_entries_without_duplicate_previews() {
        assert!(CSS.contains(".display-output-selector { width:100%; display:flex; flex-direction:column;"));
        assert!(!HTML.contains("display-topology-map"));
        assert!(!HTML.contains("monitor-screen"));
        assert!(!CSS.contains(".display-layout"));
        for slot in 0..4 {
            assert!(HTML.contains(&format!(
                "id=\"display-output-{slot}\" data-action=\"settings.display.select.{slot}\""
            )));
            assert!(HTML.contains(&format!("id=\"display-output-name-{slot}\"")));
            assert!(HTML.contains(&format!("id=\"display-output-size-{slot}\"")));
        }
    }

    #[test]
    fn scale_is_a_single_draggable_control_not_five_buckets() {
        assert!(HTML.contains("id=\"display-scale-track\" data-action=\"slider.display-scale\""));
        assert!(HTML.contains("id=\"display-scale-value\""));
        assert!(HTML.contains("role=\"slider\""));
        assert!(HTML.contains("aria-valuemin=\"100\""));
        assert!(HTML.contains("aria-valuemax=\"200\""));
        for old in [100, 125, 150, 175, 200] {
            assert!(!HTML.contains(&format!("id=\"display-scale-{old}\"")));
        }
    }

    #[test]
    fn display_modes_use_one_clipped_current_page_column() {
        let grid = css_rule(CSS, ".mode-grid");
        assert_eq!(css_value(grid, "flex-direction"), Some("column"));
        assert_eq!(css_value(grid, "overflow"), Some("hidden"));
        assert!(!HTML.contains("display-mode-slot-4"));
        assert!(!HTML.contains("mode-column"));
    }

    #[test]
    fn compiled_display_output_name_slots_are_mutable_text_targets() {
        let mut document = document().expect("settings artifact must decode");
        for slot in 0..4 {
            seed_text(
                &mut document,
                &format!("display-output-name-{slot}"),
                &format!("output-{slot}"),
            );
        }
    }

    #[test]
    fn auto_location_toggle_shares_its_row_with_the_description() {
        // Right-aligned like the other switches; the description wraps inside the copy column.
        assert!(HTML.contains(
            "<div class=\"setting-block\"><div class=\"setting-copy\"><div class=\"setting-title\">Automatically detect my location</div>"
        ));
        assert!(HTML.contains("Approximate location from your IP address (ipwho.is, ipinfo.io). Only the city is stored."));
    }

    #[test]
    fn lock_preview_button_is_in_the_lock_screen_card() {
        assert!(HTML.contains(
            "id=\"lock-preview\" data-action=\"settings.lock.preview\">Preview lock screen</button>"
        ));
    }

    #[test]
    fn surface_colour_is_one_control_on_the_surfaces_page() {
        let surfaces = page_between(HTML, "page-surfaces", "page-animations");
        let taskbar = page_between(HTML, "page-taskbar", "page-displays");
        assert!(surfaces.contains("id=\"color-surface-wheel\""));
        assert!(surfaces.contains("data-action=\"settings.picker.open.surface\""));
        assert!(surfaces.contains("id=\"surface-opacity-value\""));
        assert!(surfaces.contains("id=\"surface-blur-value\""));
        assert!(
            !taskbar.contains("color-taskbar-"),
            "taskbar colour must not be exposed"
        );
        assert!(!taskbar.contains("taskbar-opacity-value"));
        assert!(!taskbar.contains("taskbar-blur-value"));
        assert!(!HTML.contains("taskbar-color-value"));
        assert!(!HTML.contains("settings.taskbar.color"));
        assert!(!HTML.contains("settings.taskbar-blur"));
        assert!(!HTML.contains("settings.taskbar-opacity"));
        assert_eq!(HTML.matches("id=\"color-surface-wheel\"").count(), 1);
    }

    #[test]
    fn glass_rows_bind_their_controls_on_the_surfaces_page() {
        let surfaces = page_between(HTML, "page-surfaces", "page-animations");
        for (id, action) in [
            ("reduce-transparency-on", "settings.toggle.reduceTransparency"),
            ("reduce-transparency-off", "settings.toggle.reduceTransparency"),
            ("taskbar-material-blur-only", "settings.taskbar.material.blur-only"),
            ("taskbar-material-inherit", "settings.taskbar.material.inherit"),
            ("taskbar-material-custom-tint", "settings.taskbar.material.custom-tint"),
            ("glass-reset", "settings.glass.reset"),
            ("color-taskbarTint-wheel", "settings.picker.open.taskbarTint"),
        ] {
            assert!(
                surfaces.contains(&format!("id=\"{id}\" data-action=\"{action}\"")),
                "{id} must bind {action} on the surfaces page"
            );
        }
        for (track, slug) in [
            ("slider.glass-opacity", "glass-opacity"),
            ("slider.glass-blur", "glass-blur"),
            ("slider.taskbar-opacity", "taskbar-opacity"),
        ] {
            assert!(surfaces.contains(&format!("data-action=\"{track}\"")));
            for suffix in ["value", "fill", "thumb"] {
                assert!(
                    surfaces.contains(&format!("id=\"{slug}-{suffix}\"")),
                    "{slug}-{suffix} must exist for the slider"
                );
            }
        }
        assert!(surfaces.contains("id=\"color-taskbarTint-current\""));
        assert!(surfaces.contains("id=\"color-taskbarTint-preset-5\""));
        assert!(surfaces.contains("id=\"color-taskbarTint-selected-5\""));
        assert!(surfaces.contains("id=\"taskbar-tint-live\""));
        assert!(surfaces.contains("Reset glass settings"));
    }

    #[test]
    fn taskbar_material_rows_are_hidden_when_not_applicable() {
        let surfaces = page_between(HTML, "page-surfaces", "page-animations");
        // Inherit and custom-tint modes each hide the row that does not apply; no disabled twins are drawn.
        assert!(!surfaces.contains("taskbar-opacity-off"));
        assert!(!surfaces.contains("taskbar-tint-off"));
        assert!(surfaces.contains("id=\"taskbar-opacity-row\""));
        assert!(surfaces.contains("id=\"taskbar-tint-row\""));
        let live = surfaces
            .split("id=\"taskbar-opacity-live\"")
            .nth(1)
            .expect("live taskbar opacity slider");
        assert!(live.contains("data-action=\"slider.taskbar-opacity\""));
    }

    #[test]
    fn glass_rows_draw_no_element_shadows() {
        let rest = HTML
            .split_once("id=\"glass-dark\"")
            .map(|(_, rest)| rest)
            .expect("glass preset row");
        let glass = rest.split_once("Window frame").map_or(rest, |(glass, _)| glass);
        assert!(!glass.contains("shadow"), "glass rows must not use shadows");
    }

    #[test]
    fn appearance_tabs_are_six_pages_in_nav_order_with_unique_ids() {
        let nav = attr_values(HTML, "data-action=\"settings.page.");
        assert_eq!(nav, SETTINGS_PAGES);
        for page in nav {
            let id = format!("id=\"page-{page}\"");
            assert_eq!(HTML.matches(&id).count(), 1, "one section for nav {page}");
        }
        assert!(!HTML.contains("id=\"page-appearance\""));
    }

    #[test]
    fn appearance_sub_pages_have_no_duplicate_controls() {
        let mut ids = attr_values(HTML, "id=\"");
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(
            ids.len(),
            total,
            "every element id must be unique in settings.html"
        );
        assert!(!HTML.contains("id=\"accent-reset\""));
        assert!(!HTML.contains("settings.accent.reset"));
        assert!(!HTML.contains("Accent preset"));
    }

    #[test]
    fn screensaver_preview_is_labelled_and_uses_the_screensaver_action() {
        assert!(HTML.contains(
            "id=\"screensaver-preview\" data-action=\"settings.screensaver.preview\">Preview</button>"
        ));
    }

    #[test]
    fn start_icon_controls_are_on_the_start_menu_page() {
        let start = page_between(HTML, "page-start-menu", "page-surfaces");
        assert!(
            start.contains("id=\"start-icon-choose\" data-action=\"settings.start-icon.choose\"")
        );
        assert!(
            start.contains("id=\"start-icon-clear\" data-action=\"settings.start-icon.clear\"")
        );
        assert!(start.contains("id=\"start-icon-value\""));
    }

    #[test]
    fn settings_source_preserves_native_canvas_and_page_contract() {
        assert!(css_rule(CSS, "body").contains("width:100%"));
        assert!(css_rule(CSS, ".settings-nav").contains("width:187px"));
        assert!(CSS.contains(".nav-selection { position:absolute; left:8px; width:171px;"));
    }

    #[test]
    fn native_settings_window_matches_the_authored_root_geometry() {
        assert_eq!(SETTINGS_DEFAULT_WIDTH, 820);
        assert_eq!(SETTINGS_DEFAULT_HEIGHT, 540);

        let authored_root = css_rule(CSS, "body");
        assert!(
            css_value(authored_root, "width") == Some("100%")
                && css_value(authored_root, "height") == Some("100%"),
            "T01 native/authored geometry mismatch: native default is {SETTINGS_DEFAULT_WIDTH}x{SETTINGS_DEFAULT_HEIGHT}, but authored root is {authored_root}"
        );
    }

    #[test]
    fn settings_root_is_not_fixed_across_the_resize_viewport_matrix() {
        let root = css_rule(CSS, "body");
        let nav = css_rule(CSS, ".settings-nav");
        let main = css_rule(CSS, ".settings-main");
        let page = css_rule(CSS, ".settings-page");

        for (width, height) in [(760, 500), (820, 540), (960, 640)] {
            assert!(
                css_value(root, "width") == Some("100%")
                    && css_value(root, "height") == Some("100%"),
                "T02 root does not match {width}x{height} viewport: {root}"
            );
            assert!(
                css_value(nav, "width") == Some("187px")
                    && css_value(nav, "height") == Some("100%")
            );
            assert!(
                css_value(main, "flex") == Some("1")
                    && css_value(main, "width") == Some("auto")
                    && css_value(main, "min-width") == Some("0")
                    && css_value(main, "height") == Some("100%"),
                "T02 main does not fill remaining viewport at {width}x{height}: {main}"
            );
            assert!(
                css_value(page, "width") == Some("100%")
                    && css_value(page, "top") == Some("0")
                    && css_value(page, "bottom") == Some("0"),
                "page must fill main via top/bottom, not percent height: {page}"
            );
        }
    }

    #[test]
    fn theme_picker_preview_and_row_targets_are_compiled() {
        let mut document = document().expect("settings artifact must decode");
        for node in ["theme-icon", "theme-cursor"] {
            for slot in 0..6 {
                document
                    .image_rgba8(
                        &format!("{node}-preview-{slot}"),
                        flamewm_ui_x11::RuntimeImage {
                            source: "fixture".to_owned(),
                            width: 1,
                            height: 1,
                            pixels: vec![0, 0, 0, 255],
                        },
                    )
                    .unwrap_or_else(|error| panic!("compiled {node} preview {slot}: {error}"));
                document
                    .visible(&format!("{node}-row-{slot}"), true)
                    .unwrap_or_else(|error| panic!("compiled {node} row {slot}: {error}"));
            }
            document
                .visible(&format!("{node}-picker"), true)
                .unwrap_or_else(|error| panic!("compiled {node} picker: {error}"));
        }
    }

    #[test]
    fn every_projection_text_target_is_seeded_for_strict_document_updates() {
        let mut document = document().expect("settings artifact must decode");
        for id in PROJECTED_TEXT_IDS {
            seed_text(&mut document, id, "projection-seed");
        }
    }

    #[test]
    fn compiled_wallpaper_controls_are_addressable_and_use_production_actions() {
        let mut document = document().expect("settings artifact must decode");

        for (id, action, label) in [
            ("wallpaper-choose", "settings.wallpaper.choose", "Choose…"),
            ("wallpaper-clear", "settings.wallpaper.clear", "Clear"),
            ("wallpaper-default", "settings.wallpaper.default", "Default"),
            (
                "lock-background-default",
                "settings.lock.background.default",
                "Default",
            ),
            (
                "lock-background-clear",
                "settings.lock.background.clear",
                "Clear",
            ),
        ] {
            assert_action_control(id, action);
            seed_text(&mut document, id, label);
        }
    }

    #[test]
    fn compiled_application_appearance_selector_is_addressable_and_labelled() {
        let mut document = document().expect("settings artifact must decode");
        assert!(HTML.contains("Applies to newly opened applications."));
        for (id, action, label) in [
            (
                "appearance-apps-dark",
                "settings.appearance.apps.dark",
                "Dark",
            ),
            (
                "appearance-apps-light",
                "settings.appearance.apps.light",
                "Light",
            ),
        ] {
            assert_action_control(id, action);
            seed_text(&mut document, id, label);
            document
                .background(
                    id,
                    flamewm_ui_x11::UiColor {
                        r: 0x3d,
                        g: 0x3d,
                        b: 0x3d,
                        a: 0xff,
                    },
                )
                .unwrap_or_else(|error| panic!("compiled appearance selected state {id}: {error}"));
        }
    }

    #[test]
    fn compiled_accent_swatches_have_action_ancestors_and_measurable_client_points() {
        let document = document().expect("settings artifact must decode");
        let viewport = (820.0, 540.0);

        for (name, action) in [
            ("blue", "settings.accent.blue"),
            ("teal", "settings.accent.teal"),
        ] {
            let selected_dot = format!("accent-selected-{name}");
            let (x, y, width, height) = document
                .layout_box_by_id(&selected_dot, viewport)
                .unwrap_or_else(|| panic!("compiled accent target {selected_dot} is missing"));
            assert!(
                width > 0.0 && height > 0.0,
                "{selected_dot} must be hit-testable"
            );
            let center = (x + width / 2.0, y + height / 2.0);
            assert!(center.0 >= 0.0 && center.0 < viewport.0);
            assert!(center.1 >= 0.0 && center.1 < viewport.1);
            assert!(
                HTML.contains(&format!(
                    "<button class=\"accent-swatch swatch-{name}\" data-action=\"{action}\"><span id=\"{selected_dot}\""
                )),
                "compiled hit target {selected_dot} must be nested under the {action} button"
            );
        }

        assert!(
            !HTML.contains("settings.accent.reset"),
            "the accent preset reset is removed; the saved accent stays"
        );
    }

    /// Frozen W2 contract: paging controls, their stable ids and data-actions.
    /// Slot actions (`settings.display.select.N`, `settings.display.slot.N`)
    /// stay slot-relative; the controller maps slot N to page * width + N.
    const PAGER_CONTROLS: &[(&str, &str)] = &[
        ("display-output-prev", "settings.display.outputs.prev"),
        ("display-output-next", "settings.display.outputs.next"),
        ("display-mode-prev", "settings.display.modes.prev"),
        ("display-mode-next", "settings.display.modes.next"),
        ("taskbar-output-prev", "settings.display.outputs.prev"),
        ("taskbar-output-next", "settings.display.outputs.next"),
    ];

    const PAGER_DISABLED_TWINS: &[&str] = &[
        "display-output-prev-off",
        "display-output-next-off",
        "display-mode-prev-off",
        "display-mode-next-off",
        "taskbar-output-prev-off",
        "taskbar-output-next-off",
    ];

    const PAGER_LABEL_IDS: &[&str] = &[
        "display-output-page-label",
        "display-mode-page-label",
        "taskbar-output-page-label",
    ];

    const STATE_CLASSES: &[&str] = &[
        "pager-row",
        "pager-step",
        "pager-off",
        "pager-label",
        "is-selected",
        "is-current",
        "is-disabled",
        "is-pending",
        "is-error",
    ];

    #[test]
    fn pager_controls_have_stable_ids_and_data_actions() {
        for (id, action) in PAGER_CONTROLS {
            assert!(
                HTML.contains(&format!("id=\"{id}\" data-action=\"{action}\"")),
                "pager control {id} must bind data-action {action}"
            );
        }
    }

    #[test]
    fn pager_action_names_in_markup_match_the_frozen_contract() {
        let markup_actions = attr_values(HTML, "data-action=\"")
            .into_iter()
            .filter(|action| {
                action.starts_with("settings.display.outputs.")
                    || action.starts_with("settings.display.modes.")
            });
        for action in markup_actions {
            assert!(
                PAGER_CONTROLS
                    .iter()
                    .any(|(_, expected)| *expected == action),
                "markup paging action {action} is not in the frozen contract"
            );
        }
    }

    #[test]
    fn pager_disabled_states_are_inert_non_button_twins() {
        for id in PAGER_DISABLED_TWINS {
            assert!(
                HTML.contains(&format!(
                    "<div class=\"pager-off is-disabled\" id=\"{id}\">"
                )),
                "disabled pager twin {id} must be an inert div with no data-action"
            );
        }
    }

    #[test]
    fn pager_label_targets_are_projected_and_compiled() {
        let mut document = document().expect("settings artifact must decode");
        for id in PAGER_LABEL_IDS {
            assert!(
                PROJECTED_TEXT_IDS.contains(id),
                "{id} must be listed as a projected text target"
            );
            seed_text(&mut document, id, "1-4 of 5");
        }
    }

    #[test]
    fn pager_actions_have_settings_dispatch_branches() {
        for (_, action) in PAGER_CONTROLS {
            assert!(
                DISPATCH.contains(&format!("\"{action}\"")),
                "pager action {action} must have a dispatch branch in lib.rs (W2)"
            );
        }
    }

    #[test]
    fn state_classes_are_styled_and_applied_in_the_markup() {
        for class in STATE_CLASSES {
            assert!(
                CSS.contains(&format!(".{class} {{")),
                "state class .{class} must be styled in settings.css"
            );
        }
        assert!(HTML.contains("class=\"settings-inline-error is-error\""));
        assert!(HTML.contains("class=\"setting-block is-pending\" id=\"display-confirm\""));
    }

    #[test]
    fn settings_css_uses_no_unsupported_pseudo_states_or_attribute_selectors() {
        for unsupported in [":focus", ":disabled", ":checked", "["] {
            assert!(
                !CSS.contains(unsupported),
                "settings CSS must not use {unsupported}; the render compiler does not support it"
            );
        }
    }

    #[test]
    fn scroll_pages_have_auto_height_so_content_extent_is_measured() {
        // Render-core measures scroll extent from auto-height content; a
        // percent height resolves against an unbounded probe (~3e35 px) and
        // wheel scrolling never reaches a clamp.
        let page = css_rule(CSS, ".settings-page");
        assert_eq!(
            css_value(page, "height"),
            None,
            "page must not declare height"
        );
        assert_eq!(css_value(page, "bottom"), Some("0"));
    }

    #[test]
    fn scroll_pages_reserve_room_above_the_inline_footer_at_760x500() {
        let footer = css_rule(CSS, ".settings-inline-error");
        let footer_height = css_value(footer, "height")
            .and_then(|value| value.strip_suffix("px"))
            .and_then(|value| value.parse::<u32>().ok())
            .expect("footer must declare a pixel height");
        let scroll = css_rule(CSS, ".page-scroll");
        let reserved = css_value(scroll, "padding-bottom")
            .and_then(|value| value.strip_suffix("px"))
            .and_then(|value| value.parse::<u32>().ok())
            .expect("scroll pages must declare pixel bottom padding");
        assert!(
            reserved >= footer_height,
            "scroll content must clear the {footer_height}px footer at 760x500"
        );
        assert_eq!(css_value(scroll, "overflow-y"), Some("auto"));
        assert!(HTML.contains("id=\"page-displays\" class=\"settings-page page-scroll\""));
    }
}

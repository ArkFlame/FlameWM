use flamewm_ui_x11::{UiDocument, decode_document};

const COMPILED_UI: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-settings.rwr"));

pub const SETTINGS_DEFAULT_WIDTH: u32 = 820;
pub const SETTINGS_DEFAULT_HEIGHT: u32 = 540;

pub fn document() -> Result<UiDocument, String> {
    decode_document(COMPILED_UI)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROJECTED_TEXT_IDS: &[&str] = &[
        "display-mode-slot-0",
        "display-mode-slot-1",
        "display-mode-slot-2",
        "display-mode-slot-3",
        "display-mode-slot-4",
        "display-mode-slot-5",
        "display-mode-slot-6",
        "display-mode-slot-7",
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
        "display-selected-scale",
        "selection-opacity-value",
        "snap-opacity-value",
        "icon-theme-value",
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
        "color-value-taskbar",
        "color-value-startmenu",
        "color-value-accent",
        "color-value-titlebar",
        "start-menu-opacity-value",
        "accent-opacity-value",
        "animation-speed-value",
        "color-value-lockbackground",
        "lock-background-image-value",
        "titlebar-minimize-style-value",
        "titlebar-maximize-style-value",
        "titlebar-close-style-value",
        "taskbar-opacity-value",
        "taskbar-size-value",
        "taskbar-color-value",
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

    #[test]
    fn compiled_display_output_name_slots_are_mutable_text_targets() {
        let mut document = document().expect("settings artifact must decode");
        for slot in 0..4 {
            document
                .text(
                    &format!("display-output-name-{slot}"),
                    format!("output-{slot}"),
                )
                .unwrap_or_else(|error| panic!("compiled display name slot {slot}: {error}"));
        }
    }

    #[test]
    fn settings_source_preserves_native_canvas_and_page_contract() {
        let html = include_str!("../../../ui/settings.html");
        let css = include_str!("../../../ui/settings.css");
        for page in [
            "appearance",
            "desktop",
            "taskbar",
            "displays",
            "fonts",
            "hotkeys",
            "about",
        ] {
            assert!(html.contains(&format!("id=\"page-{page}\"")));
        }
        assert!(css_rule(css, "body").contains("width:100%"));
        assert!(css_rule(css, ".settings-nav").contains("width:208px"));
        assert!(css.contains(".nav-selection { position:absolute; left:8px; width:192px;"));
    }

    #[test]
    fn native_settings_window_matches_the_authored_root_geometry() {
        let css = include_str!("../../../ui/settings.css");
        assert_eq!(SETTINGS_DEFAULT_WIDTH, 820);
        assert_eq!(SETTINGS_DEFAULT_HEIGHT, 540);

        let authored_root = css_rule(css, "body");
        assert!(
            css_value(authored_root, "width") == Some("100%")
                && css_value(authored_root, "height") == Some("100%"),
            "T01 native/authored geometry mismatch: native default is {}x{}, but authored root is {authored_root}",
            SETTINGS_DEFAULT_WIDTH,
            SETTINGS_DEFAULT_HEIGHT
        );
    }

    #[test]
    fn settings_root_is_not_fixed_across_the_resize_viewport_matrix() {
        let css = include_str!("../../../ui/settings.css");
        let root = css_rule(css, "body");
        let nav = css_rule(css, ".settings-nav");
        let main = css_rule(css, ".settings-main");
        let page = css_rule(css, ".settings-page");

        for (width, height) in [(760, 500), (820, 540), (960, 640)] {
            assert!(
                css_value(root, "width") == Some("100%")
                    && css_value(root, "height") == Some("100%"),
                "T02 root does not match {width}x{height} viewport: {root}"
            );
            assert!(
                css_value(nav, "width") == Some("208px")
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
            document
                .text(id, "projection-seed".to_owned())
                .unwrap_or_else(|error| panic!("compiled projection target {id}: {error}"));
        }
    }

    #[test]
    fn compiled_wallpaper_controls_are_addressable_and_use_production_actions() {
        let mut document = document().expect("settings artifact must decode");
        let html = include_str!("../../../ui/settings.html");
        let dispatch = include_str!("lib.rs");

        for (id, action, label) in [
            ("wallpaper-choose", "settings.wallpaper.choose", "Choose…"),
            (
                "wallpaper-reset",
                "settings.wallpaper.reset",
                "Restore default",
            ),
        ] {
            assert!(
                html.contains(&format!("id=\"{id}\" data-action=\"{action}\"")),
                "wallpaper control {id} must retain its production action"
            );
            assert!(
                dispatch.contains(&format!("\"{action}\" =>")),
                "wallpaper action {action} must have a production dispatch branch"
            );
            document
                .text(id, label.to_owned())
                .unwrap_or_else(|error| panic!("compiled wallpaper action target {id}: {error}"));
        }
    }

    #[test]
    fn compiled_accent_swatches_have_action_ancestors_and_measurable_client_points() {
        let document = document().expect("settings artifact must decode");
        let html = include_str!("../../../ui/settings.html");
        let viewport = (820.0, 540.0);

        for (name, action) in [
            ("blue", "settings.accent.blue"),
            ("teal", "settings.accent.teal"),
        ] {
            let selected_dot = format!("accent-selected-{name}");
            let (x, y, width, height) = document
                .layout_box_by_id(&selected_dot, viewport)
                .unwrap_or_else(|| panic!("compiled accent target {selected_dot} is missing"));
            assert!(width > 0.0 && height > 0.0, "{selected_dot} must be hit-testable");
            let center = (x + width / 2.0, y + height / 2.0);
            assert!(center.0 >= 0.0 && center.0 < viewport.0);
            assert!(center.1 >= 0.0 && center.1 < viewport.1);
            assert!(
                html.contains(&format!(
                    "<button class=\"accent-swatch swatch-{name}\" data-action=\"{action}\"><span id=\"{selected_dot}\""
                )),
                "compiled hit target {selected_dot} must be nested under the {action} button"
            );
        }

        assert!(html.contains("<button class=\"settings-button\" data-action=\"settings.accent.reset\">Reset</button>"));
        assert!(
            !html.contains("id=\"accent-reset\""),
            "reset currently has no compiled id, so UiDocument's public layout query cannot independently measure its hit box"
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
        let html = include_str!("../../../ui/settings.html");
        for (id, action) in PAGER_CONTROLS {
            assert!(
                html.contains(&format!("id=\"{id}\" data-action=\"{action}\"")),
                "pager control {id} must bind data-action {action}"
            );
        }
    }

    #[test]
    fn pager_action_names_in_markup_match_the_frozen_contract() {
        let html = include_str!("../../../ui/settings.html");
        let markup_actions = html
            .split("data-action=\"")
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .filter(|action| {
                action.starts_with("settings.display.outputs.")
                    || action.starts_with("settings.display.modes.")
            });
        for action in markup_actions {
            assert!(
                PAGER_CONTROLS.iter().any(|(_, expected)| *expected == action),
                "markup paging action {action} is not in the frozen contract"
            );
        }
    }

    #[test]
    fn pager_disabled_states_are_inert_non_button_twins() {
        let html = include_str!("../../../ui/settings.html");
        for id in PAGER_DISABLED_TWINS {
            assert!(
                html.contains(&format!(
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
            document
                .text(id, "1-4 of 5".to_owned())
                .unwrap_or_else(|error| panic!("compiled pager label {id}: {error}"));
        }
    }

    #[test]
    fn pager_actions_have_settings_dispatch_branches() {
        let dispatch = include_str!("lib.rs");
        for (_, action) in PAGER_CONTROLS {
            assert!(
                dispatch.contains(&format!("\"{action}\"")),
                "pager action {action} must have a dispatch branch in lib.rs (W2)"
            );
        }
    }

    #[test]
    fn state_classes_are_styled_and_applied_in_the_markup() {
        let css = include_str!("../../../ui/settings.css");
        let html = include_str!("../../../ui/settings.html");
        for class in STATE_CLASSES {
            assert!(
                css.contains(&format!(".{class} {{")),
                "state class .{class} must be styled in settings.css"
            );
        }
        assert!(html.contains("class=\"settings-inline-error is-error\""));
        assert!(html.contains("class=\"setting-block is-pending\" id=\"display-confirm\""));
    }

    #[test]
    fn settings_css_uses_no_unsupported_pseudo_states_or_attribute_selectors() {
        let css = include_str!("../../../ui/settings.css");
        for unsupported in [":focus", ":disabled", ":checked", "["] {
            assert!(
                !css.contains(unsupported),
                "settings CSS must not use {unsupported}; the render compiler does not support it"
            );
        }
    }

    #[test]
    fn scroll_pages_have_auto_height_so_content_extent_is_measured() {
        // Render-core measures scroll extent from auto-height content; a
        // percent height resolves against an unbounded probe (~3e35 px) and
        // wheel scrolling never reaches a clamp.
        let css = include_str!("../../../ui/settings.css");
        let page = css_rule(css, ".settings-page");
        assert_eq!(
            css_value(page, "height"),
            None,
            "page must not declare height"
        );
        assert_eq!(css_value(page, "bottom"), Some("0"));
    }

    #[test]
    fn scroll_pages_reserve_room_above_the_inline_footer_at_760x500() {
        let css = include_str!("../../../ui/settings.css");
        let html = include_str!("../../../ui/settings.html");
        let footer = css_rule(css, ".settings-inline-error");
        let footer_height = css_value(footer, "height")
            .and_then(|value| value.strip_suffix("px"))
            .and_then(|value| value.parse::<u32>().ok())
            .expect("footer must declare a pixel height");
        let scroll = css_rule(css, ".page-scroll");
        let reserved = css_value(scroll, "padding-bottom")
            .and_then(|value| value.strip_suffix("px"))
            .and_then(|value| value.parse::<u32>().ok())
            .expect("scroll pages must declare pixel bottom padding");
        assert!(
            reserved >= footer_height,
            "scroll content must clear the {footer_height}px footer at 760x500"
        );
        assert_eq!(css_value(scroll, "overflow-y"), Some("auto"));
        assert!(html.contains("id=\"page-displays\" class=\"settings-page page-scroll\""));
    }
}

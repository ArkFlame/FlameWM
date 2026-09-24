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

    const PROJECTED_TEXT_IDS: [&str; 11] = [
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
        let runtime = include_str!("runtime.rs");
        let css = include_str!("../../../ui/settings.css");
        assert_eq!(SETTINGS_DEFAULT_WIDTH, 820);
        assert_eq!(SETTINGS_DEFAULT_HEIGHT, 540);
        assert!(runtime.contains(
            "normal_window_config(view::SETTINGS_DEFAULT_WIDTH, view::SETTINGS_DEFAULT_HEIGHT)"
        ));

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
                    && css_value(page, "height") == Some("100%")
            );
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
                dispatch.contains(&format!("action == \"{action}\"")),
                "wallpaper action {action} must have a production dispatch branch"
            );
            document
                .text(id, label.to_owned())
                .unwrap_or_else(|error| panic!("compiled wallpaper action target {id}: {error}"));
        }
    }
}

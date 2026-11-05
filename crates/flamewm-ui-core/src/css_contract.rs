//! Stylesheet contract helpers shared by every owner that ships CSS.
//!
//! A colour value is written once, in a `:root` token declaration. Every rule outside `:root`
//! reads `var(--token)`, so `appearance` can reach each surface through projected tokens.

/// Colour literals in `css` outside `:root` blocks and comments: `#hex` runs of 3/4/6/8 digits,
/// CSS colour functions, and the `white`/`black` keywords.
#[must_use]
pub fn raw_colour_literals(css: &str) -> Vec<String> {
    let body = strip_root_blocks(&strip_comments(css));
    let mut found = Vec::new();
    for (index, _) in body.match_indices('#') {
        let digits: String = body[index + 1..]
            .chars()
            .take_while(|c| c.is_ascii_hexdigit())
            .collect();
        if matches!(digits.len(), 3 | 4 | 6 | 8) {
            found.push(format!("#{digits}"));
        }
    }
    for function in ["rgb(", "rgba(", "hsl(", "hsla("] {
        if body.contains(function) {
            found.push(function.to_string());
        }
    }
    for word in body.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
        if word == "white" || word == "black" {
            found.push(word.to_string());
        }
    }
    found
}

fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        rest = match rest[start..].find("*/") {
            Some(end) => &rest[start + end + 2..],
            None => "",
        };
    }
    out.push_str(rest);
    out
}

/// Removes every `:root { ... }` block. Token blocks hold no nested braces.
fn strip_root_blocks(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find(":root") {
        out.push_str(&rest[..start]);
        rest = match rest[start..].find('}') {
            Some(end) => &rest[start + end + 1..],
            None => "",
        };
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::{raw_colour_literals, strip_comments};

    const TASK_MANAGER_CSS: &str = include_str!("../../../ui/task-manager/task-manager.css");
    const EXPLORER_CSS: &str = include_str!("../../../ui/explorer/explorer.css");
    const DESKTOP_CSS: &str = include_str!("../../../ui/desktop/desktop.css");
    const MENU_CSS: &str = include_str!("../../../ui/shared/menu.css");

    #[test]
    fn detects_literals_outside_root_only() {
        let css = ":root { --a:#ffffff40; }\n/* #123456 and white in a comment */\n.x { color:#abcdef; background:var(--a); }";
        assert_eq!(raw_colour_literals(css), vec!["#abcdef".to_string()]);
    }

    #[test]
    fn detects_keywords_and_functions() {
        let css =
            ".x { color:white; border-color:rgba(0,0,0,.5); } .white-space { white-space:nowrap; }";
        assert_eq!(
            raw_colour_literals(css),
            vec!["rgba(".to_string(), "white".to_string()]
        );
    }

    #[test]
    fn task_manager_sheet_has_no_raw_colour_outside_root() {
        assert_eq!(raw_colour_literals(TASK_MANAGER_CSS), Vec::<String>::new());
    }

    #[test]
    fn explorer_sheet_has_no_raw_colour_outside_root() {
        assert_eq!(raw_colour_literals(EXPLORER_CSS), Vec::<String>::new());
    }

    #[test]
    fn desktop_sheet_has_no_raw_colour_outside_root() {
        assert_eq!(raw_colour_literals(DESKTOP_CSS), Vec::<String>::new());
    }

    #[test]
    fn shared_menu_sheet_has_no_raw_colour_outside_root() {
        assert_eq!(raw_colour_literals(MENU_CSS), Vec::<String>::new());
    }

    /// Appearance tokens a sheet reads must be declared in its own `:root`, so the Dark defaults
    /// exist before the first projection.
    fn undeclared_appearance_tokens(css: &str) -> Vec<String> {
        let root = css
            .lines()
            .find(|line| line.starts_with(":root"))
            .unwrap_or("");
        ["--rim", "--hover-fill", "--pressed-fill", "--text"]
            .into_iter()
            .filter(|token| css.contains(&format!("var({token})")))
            .filter(|token| !root.contains(&format!("{token}:")))
            .map(str::to_string)
            .collect()
    }

    /// Named single-line labels must ellipsize rather than clip mid-glyph.
    #[test]
    fn named_nowrap_labels_declare_text_overflow_ellipsis() {
        let required: [(&str, &str, &[&str]); 2] = [
            (
                "finder",
                include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../ui/finder/finder.css")),
                &[".name", ".parent", ".tile-name", ".tile-meta", ".dup-name", ".dup-path"],
            ),
            (
                "explorer",
                EXPLORER_CSS,
                &[".name-text", ".field-text", ".place-label", ".status-message"],
            ),
        ];
        for (sheet, css, selectors) in required {
            let css = strip_comments(css);
            for selector in selectors {
                let body = css
                    .split('}')
                    .filter_map(|block| block.split_once('{'))
                    .filter(|(head, _)| head.trim() == *selector)
                    .map(|(_, body)| body.to_string())
                    .collect::<String>();
                assert!(!body.is_empty(), "{sheet}: no rule for `{selector}`");
                let compact: String = body.chars().filter(|c| !c.is_whitespace()).collect();
                assert!(
                    compact.contains("text-overflow:ellipsis"),
                    "{sheet}: `{selector}` must declare text-overflow:ellipsis"
                );
            }
        }
    }

    #[test]
    fn sheets_declare_the_appearance_tokens_they_read() {
        for (name, css) in [
            ("task-manager", TASK_MANAGER_CSS),
            ("explorer", EXPLORER_CSS),
            ("desktop", DESKTOP_CSS),
            ("menu", MENU_CSS),
        ] {
            assert_eq!(
                undeclared_appearance_tokens(css),
                Vec::<String>::new(),
                "{name}"
            );
        }
    }

    /// Surfaces that receive `project_appearance_tokens` must declare every token it writes: an
    /// undeclared name rejects the whole projection, so the sheet keeps Dark text on Light glass.
    #[test]
    fn projecting_sheets_declare_every_text_and_overlay_token() {
        const SETTINGS_CSS: &str = include_str!("../../../ui/settings.css");
        const PROJECTED: [&str; 6] = [
            "--text",
            "--text-secondary",
            "--muted",
            "--rim",
            "--hover-fill",
            "--pressed-fill",
        ];
        for (name, css) in [
            ("task-manager", TASK_MANAGER_CSS),
            ("desktop", DESKTOP_CSS),
            ("settings", SETTINGS_CSS),
        ] {
            let root = css.lines().find(|line| line.starts_with(":root")).unwrap_or("");
            let missing: Vec<&str> = PROJECTED
                .into_iter()
                .filter(|token| !root.contains(&format!("{token}:")))
                .collect();
            assert_eq!(missing, Vec::<&str>::new(), "{name}");
        }
    }

    #[test]
    fn hover_and_pressed_rules_read_overlay_tokens() {
        // Hover and pressed fills must come from the appearance overlays, never a white literal.
        const SETTINGS_CSS: &str = include_str!("../../../ui/settings.css");
        for css in [TASK_MANAGER_CSS, EXPLORER_CSS, MENU_CSS, SETTINGS_CSS] {
            for line in css.lines().filter(|line| line.contains(":active {")) {
                // Settings nav rows use `--nav-pressed`, which `project_nav_row_tokens` also inverts.
                assert!(
                    line.contains("var(--pressed-fill)") || line.contains("var(--nav-pressed)"),
                    "pressed rule not overlay-driven: {line}"
                );
            }
        }
    }
}

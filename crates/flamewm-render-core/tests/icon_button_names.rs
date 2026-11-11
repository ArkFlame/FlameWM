//! Icon-only buttons in ui/index.html carry accessible names (cmp-a11y slice 4). Static markup check only.

use std::{fs, path::Path};

fn repo_file(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// Returns the opening tag of the button that carries `needle` (e.g. `id="task-start"`).
fn button_tag<'a>(html: &'a str, needle: &str) -> &'a str {
    let at = html
        .find(needle)
        .unwrap_or_else(|| panic!("{needle} missing"));
    let start = html[..at].rfind('<').expect("tag start");
    let end = at + html[at..].find('>').expect("tag end");
    &html[start..=end]
}

#[test]
fn index_icon_only_buttons_have_accessible_names() {
    let html = repo_file("ui/index.html");
    for (needle, name) in [
        (r#"data-action="settings.minimize""#, "Minimize"),
        (r#"data-action="settings.maximize""#, "Maximize"),
        (r#"data-action="settings.close""#, "Close"),
        (r#"id="task-start""#, "Start"),
        (r#"id="task-browser""#, "Browser"),
        (r#"id="task-files""#, "Files"),
        (r#"id="task-terminal""#, "Terminal"),
        (r#"id="task-code""#, "Code"),
        (r#"id="task-settings""#, "Settings"),
        (r#"id="task-music""#, "Music"),
        (r#"id="task-generic""#, "Application"),
        (r#"id="tray-media""#, "Play or pause media"),
        (r#"id="tray-volume""#, "Volume"),
        (r#"id="tray-network""#, "Network"),
        (r#"id="watermark-toggle""#, "Show FlameWM mark"),
        (r#"id="sticky-toggle""#, "Sticky notes"),
        (r#"id="font-bold-toggle""#, "Global bold text"),
    ] {
        let tag = button_tag(&html, needle);
        assert!(tag.contains(&format!(r#"aria-label="{name}""#)), "{tag}");
    }
}

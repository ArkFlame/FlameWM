//! Icon-only explorer toolbar buttons carry accessible names (cmp-a11y slice 3). Static markup check only.

use std::{fs, path::Path};

fn repo_file(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
fn explorer_icon_only_buttons_have_accessible_names() {
    let html = repo_file("ui/explorer/explorer.html");
    for (id, name) in [
        ("nav-back", "Back"),
        ("nav-forward", "Forward"),
        ("nav-up", "Up"),
        ("nav-refresh", "Refresh"),
        ("menu-toggle", "Menu"),
        ("search-clear", "Clear search"),
    ] {
        let tag = html
            .lines()
            .find(|line| line.contains(&format!(r#"id="{id}""#)))
            .unwrap_or_else(|| panic!("button {id} missing"));
        assert!(tag.contains(&format!(r#"aria-label="{name}""#)), "{tag}");
    }
    for (selector, name) in [
        (r#"class="tab-add""#, "New tab"),
        (r#"class="tab-close""#, "Close tab"),
    ] {
        let tag = html
            .lines()
            .find(|line| line.contains(selector))
            .unwrap_or_else(|| panic!("{selector} missing"));
        assert!(tag.contains(&format!(r#"aria-label="{name}""#)), "{tag}");
    }
}

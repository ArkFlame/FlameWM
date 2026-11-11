//! Icon-only composer buttons carry accessible names (cmp-a11y slice 2). Static markup check only.

use std::{fs, path::Path};

fn repo_file(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
fn assistant_icon_only_buttons_have_accessible_names() {
    let html = repo_file("ui/assistant/assistant.html");
    for (id, name) in [
        ("record", "Record"),
        ("tts", "Read replies aloud"),
        ("mic", "Microphone"),
        ("send", "Send"),
        ("stop", "Stop"),
    ] {
        let tag = html
            .lines()
            .find(|line| line.contains(&format!(r#"id="{id}""#)))
            .unwrap_or_else(|| panic!("button {id} missing"));
        assert!(tag.contains(&format!(r#"aria-label="{name}""#)), "{tag}");
    }
}

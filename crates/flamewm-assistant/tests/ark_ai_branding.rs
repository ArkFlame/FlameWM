//! User-visible ARK AI labels: the window title, header, and desktop entry name.

use std::{fs, path::Path};

fn repo_file(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
fn assistant_window_title_and_header_are_ark_ai() {
    let html = repo_file("ui/assistant/assistant.html");
    assert!(html.contains("<title>ARK AI</title>"));
    assert!(html.contains(r#"<span class="title">ARK AI</span>"#));
}

#[test]
fn assistant_desktop_entry_is_named_ark_ai() {
    let entry = repo_file("packaging/ark-ai.desktop");
    assert!(entry.lines().any(|line| line == "Name=ARK AI"));
    assert!(entry.lines().any(|line| line == "Exec=flamewm-assistant"));
}

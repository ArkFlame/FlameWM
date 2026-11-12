//! Shell-page accent swatches in ui/index.html carry KDE-style colour names (cmp-a11y D-swatch),
//! reusing the settings name table (`nodes::kde_accent_name`). Static markup check only: it
//! does not prove what a screen reader announces at runtime.

use std::{fs, path::Path};

fn repo_file(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
fn index_accent_swatches_carry_kde_names() {
    let html = repo_file("ui/index.html");
    let expected = [
        ("red", "Red"),
        ("blue", "Blue"),
        ("purple", "Purple"),
        ("teal", "Teal"),
        ("green", "Green"),
        ("amber", "Orange"),
    ];
    let mut seen = 0;
    for (index, _) in html.match_indices(r#"<button class="accent-swatch"#) {
        let rest = &html[index..];
        let tag = &rest[..=rest.find('>').expect("swatch tag closes")];
        seen += 1;
        let key = tag
            .split("data-action=\"accent.")
            .nth(1)
            .and_then(|tail| tail.split('"').next())
            .expect("swatch has accent action");
        let want = expected
            .iter()
            .find(|(k, _)| *k == key)
            .unwrap_or_else(|| panic!("unexpected swatch {key}"))
            .1;
        assert!(
            tag.contains(&format!(r#"aria-label="{want}""#)),
            "swatch {key} must be named {want}: {tag}"
        );
    }
    assert_eq!(seen, expected.len(), "index.html accent swatch count");
}

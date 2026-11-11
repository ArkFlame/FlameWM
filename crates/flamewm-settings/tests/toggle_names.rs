//! Settings on/off toggles carry accessible names (cmp-a11y slice 5). Static markup check only:
//! it proves the wiring in ui/settings.html, not what a screen reader announces at runtime.

use std::{fs, path::Path};

fn repo_file(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let start = tag.find(&key)? + key.len();
    let end = tag[start..].find('"')? + start;
    Some(&tag[start..end])
}

fn is_toggle(tag: &str) -> bool {
    tag.contains(r#"class="toggle toggle-on""#)
        || tag.contains(r#"class="toggle toggle-off""#)
        || tag.contains(r#"id="startup-toggle-"#)
}

fn named(html: &str, tag: &str) -> bool {
    if let Some(label) = attr(tag, "aria-label") {
        return !label.trim().is_empty();
    }
    match attr(tag, "aria-labelledby") {
        Some(ids) => {
            let mut any = false;
            for id in ids.split_whitespace() {
                any = true;
                if !html.contains(&format!(r#"id="{id}""#)) {
                    return false;
                }
            }
            any
        }
        None => false,
    }
}

#[test]
fn settings_toggles_have_accessible_names() {
    let html = repo_file("ui/settings.html");
    let mut toggles = 0;
    let mut offenders = Vec::new();
    for (index, _) in html.match_indices("<button") {
        let rest = &html[index..];
        let end = rest
            .find('>')
            .unwrap_or_else(|| panic!("unterminated button tag"));
        let tag = &rest[..=end];
        if !is_toggle(tag) {
            continue;
        }
        toggles += 1;
        if !named(&html, tag) {
            offenders.push(tag.to_owned());
        }
    }
    assert!(
        toggles >= 144,
        "expected at least 144 settings toggle buttons, found {toggles}"
    );
    assert!(
        offenders.is_empty(),
        "{} of {toggles} settings toggles lack an accessible name (aria-label, or aria-labelledby naming existing ids):\n{}",
        offenders.len(),
        offenders.join("\n")
    );
}

/// On/off toggles are `role="switch"` whose `aria-checked` matches the twin's class. The
/// controller shows exactly one twin via `document.visible`, so the exposed state tracks the setting.
#[test]
fn settings_toggles_expose_switch_role_and_state() {
    let html = repo_file("ui/settings.html");
    let mut switches = 0;
    let mut offenders = Vec::new();
    for (index, _) in html.match_indices("<button") {
        let rest = &html[index..];
        let tag = &rest[..=rest.find('>').expect("button tag closes")];
        let expected = if tag.contains(r#"class="toggle toggle-on""#) {
            "true"
        } else if tag.contains(r#"class="toggle toggle-off""#) {
            "false"
        } else {
            continue;
        };
        switches += 1;
        if attr(tag, "role") != Some("switch") || attr(tag, "aria-checked") != Some(expected) {
            offenders.push(tag.to_owned());
        }
        if expected == "true" {
            let action = attr(tag, "data-action").expect("toggle has data-action");
            let twins = html.matches(&format!(r#"data-action="{action}""#)).count();
            assert_eq!(twins, 2, "{action} must have one on and one off twin");
        }
    }
    assert!(
        switches >= 136,
        "expected at least 136 switch toggles, found {switches}"
    );
    assert!(
        offenders.is_empty(),
        "{} of {switches} switch toggles lack role=switch or a matching aria-checked:\n{}",
        offenders.len(),
        offenders.join("\n")
    );
}

/// Accent swatches carry KDE-style colour names; the accent wheel names its current value
/// through a text node the controller updates (nearest KDE name).
#[test]
fn accent_swatches_and_wheel_carry_colour_names() {
    let html = repo_file("ui/settings.html");
    let mut swatches = 0;
    for (index, _) in html.match_indices(r#"<button class="accent-swatch"#) {
        let rest = &html[index..];
        let tag = &rest[..=rest.find('>').expect("swatch tag closes")];
        swatches += 1;
        let label = attr(tag, "aria-label").unwrap_or_default();
        assert!(
            ["Graphite", "White", "Red", "Blue", "Purple", "Teal"].contains(&label),
            "unexpected swatch name {label:?} in {tag}"
        );
    }
    assert_eq!(swatches, 6, "accent swatch count");
    let wheel = html
        .find(r#"<button class="wheel-swatch" id="color-accent-wheel""#)
        .expect("accent wheel button");
    let wheel_end = wheel + html[wheel..].find("</button>").expect("wheel closes");
    assert!(
        html[wheel..wheel_end].contains(r#"id="color-accent-name""#),
        "accent wheel must hold the colour-name text node"
    );
}

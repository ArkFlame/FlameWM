use super::{NAV_BRAND_HEIGHT_PX, NAV_ROW_PITCH_PX, NAV_SEARCH_BLOCK_PX, nav_top};

const CSS: &str = include_str!("../../../ui/settings.css");
const HTML: &str = include_str!("../../../ui/settings.html");

fn px_in_rule(selector: &str, property: &str) -> f32 {
    let start = CSS.find(selector).expect("selector in settings.css");
    let body = &CSS[start..];
    let body = &body[..body.find('}').expect("rule end")];
    let at = body
        .find(&format!("{property}:"))
        .expect("property in rule")
        + property.len()
        + 1;
    let value: String = body[at..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    value.parse().expect("px value")
}

#[test]
fn indicator_top_matches_css_row_layout_for_every_page() {
    let rows = HTML.matches("data-action=\x22settings.page.").count();
    assert_eq!(rows, 17, "sidebar page rows in settings.html");
    assert_eq!(
        px_in_rule(".settings-brand {", "height"),
        NAV_BRAND_HEIGHT_PX
    );
    assert_eq!(px_in_rule(".nav-row {", "height"), NAV_ROW_PITCH_PX);
    for index in 0..rows {
        let expected = NAV_BRAND_HEIGHT_PX
            + NAV_SEARCH_BLOCK_PX
            + index as f32 * px_in_rule(".nav-row {", "height");
        assert_eq!(nav_top(index), expected, "indicator for row {index}");
    }
}

/// Body of the first CSS rule whose selector starts with `selector` (e.g. `.nav-row {`).
fn rule_body(selector: &str) -> &'static str {
    let start = CSS.find(selector).expect("selector in settings.css");
    let body = &CSS[start..];
    &body[..body.find('}').expect("rule end")]
}

// REGRESSION: the sidebar overflowed its window with no scroll container, so the last rows
// (ARK AI, About) were unreachable. The nav must scroll and its rows must not shrink.
#[test]
fn sidebar_scrolls_and_rows_keep_their_height() {
    assert!(rule_body(".nav-scroll {").contains("overflow-y:auto"));
    assert!(rule_body(".settings-brand {").contains("height:72px"));
}

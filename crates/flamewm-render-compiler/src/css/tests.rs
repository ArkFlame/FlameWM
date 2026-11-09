use super::*;
use crate::html::parse_html;

#[test]
fn descendant_and_child_selectors_match() {
    let html =
        parse_html("<body><div class='panel'><button id='start'>Start</button></div></body>")
            .unwrap();
    let rules = parse_stylesheet(".panel > #start:hover { background: #f00; }").unwrap();
    let button = html
        .nodes
        .iter()
        .position(|node| node.attrs.get("id").map(String::as_str) == Some("start"))
        .unwrap();
    assert!(rules[0].selector.matches(&html, button));
}

fn selector_count(css: &str) -> usize {
    parse_stylesheet(css).unwrap().len()
}

#[test]
fn nested_rules_flatten_with_descendant_and_ampersand_parents() {
    let html = parse_html(
        "<body><div class='panel'><button id='go' class='primary'>Go</button></div></body>",
    )
    .unwrap();
    let rules = parse_stylesheet(
            ".panel { width: 1px; button { height: 2px; } &.wide { height: 3px; } & > .x { color: red; } }",
        )
        .unwrap();
    // .panel, .panel button, .panel.wide, .panel > .x
    assert_eq!(rules.len(), 4);
    assert_eq!(rules[0].declarations.len(), 1);
    let button = html
        .nodes
        .iter()
        .position(|node| node.attrs.get("id").map(String::as_str) == Some("go"))
        .unwrap();
    assert!(rules[1].selector.matches(&html, button));
    assert_eq!(rules[1].declarations[0].name, "height");
    assert!(!rules[2].selector.matches(&html, button));
}

#[test]
fn nested_hover_state_stays_on_the_rightmost_compound() {
    // `.row` (empty), `.row button`, `.row button:hover`
    let rules = parse_stylesheet(".row { button { &:hover { color: red; } } }").unwrap();
    assert_eq!(rules.len(), 3);
    assert_eq!(rules[2].selector.state, PseudoState::Hover);
    assert!(parse_stylesheet(".row { &:hover button { color: red; } }").is_err());
}

#[test]
fn nesting_error_cases_are_rejected() {
    assert!(
        parse_stylesheet("& { color: red; }").is_err(),
        "& needs a parent"
    );
    assert!(
        parse_stylesheet("color: red;").is_err(),
        "top-level declaration"
    );
    assert!(parse_stylesheet(".a { @media x { color: red; } }").is_err());
    assert!(
        parse_stylesheet(".a { .b { color: red; }").is_err(),
        "unclosed block"
    );
    assert!(
        parse_stylesheet(".a, { color: red; }").is_err(),
        "empty selector"
    );
}

#[test]
fn is_and_where_expand_into_alternatives() {
    assert_eq!(selector_count(".a :is(.b, .c) { width: 1px; }"), 2);
    assert_eq!(selector_count(":is(.a, .b):is(.c, .d) { width: 1px; }"), 4);
    assert_eq!(selector_count(":where(.a, .b) button { width: 1px; }"), 2);
    assert_eq!(
        selector_count("button:is(:hover, :focus) { width: 1px; }"),
        2
    );
}

#[test]
fn is_takes_argument_specificity_and_where_contributes_none() {
    let is_rules = parse_stylesheet(":is(#a, .b) { width: 1px; }").unwrap();
    assert_eq!(is_rules[0].selector.specificity, (1, 0, 0));
    assert_eq!(is_rules[1].selector.specificity, (0, 1, 0));
    let where_rules = parse_stylesheet("div:where(#a, .b) { width: 1px; }").unwrap();
    assert_eq!(where_rules[0].selector.specificity, (0, 0, 1));
    assert_eq!(where_rules[1].selector.specificity, (0, 0, 1));
    let bare = parse_stylesheet(":where(.a) { width: 1px; }").unwrap();
    assert_eq!(bare[0].selector.specificity, (0, 0, 0));
}

#[test]
fn is_where_match_like_their_expanded_selectors() {
    let html =
        parse_html("<body><div class='panel'><button id='start'>Start</button></div></body>")
            .unwrap();
    let button = html
        .nodes
        .iter()
        .position(|node| node.attrs.get("id").map(String::as_str) == Some("start"))
        .unwrap();
    let rules =
        parse_stylesheet(":where(.panel, .other) > :is(button, span) { width: 1px; }").unwrap();
    assert!(rules
        .iter()
        .any(|rule| rule.selector.matches(&html, button)));
}

#[test]
fn is_where_error_cases_are_rejected() {
    for bad in [
        ":is() { width: 1px; }",
        ":is(.a, ) { width: 1px; }",
        ":where(.a { width: 1px; }",
        ":is(.a { width: 1px; }",
        ".a:is(.b) .c { width: 1px; } :is(:hover .x) { width: 1px; }",
    ] {
        assert!(parse_stylesheet(bad).is_err(), "{bad} must be rejected");
    }
    let many: Vec<String> = (0..9).map(|i| format!(".c{i}")).collect();
    let long = format!(
        ":is({}) :is({}) {{ width: 1px; }}",
        many.join(","),
        many.join(",")
    );
    assert!(
        parse_stylesheet(&long).is_err(),
        "81 alternatives exceed the bound"
    );
}

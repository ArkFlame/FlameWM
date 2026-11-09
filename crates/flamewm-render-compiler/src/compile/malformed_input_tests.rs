use super::*;

fn page(style: &str, body: &str) -> String {
    format!("<body><style>{style}</style>{body}</body>")
}

#[test]
fn unterminated_block_is_an_error() {
    let source = page("#a { width: 1px;", "<div id=\"a\"></div>");
    assert!(compile_str(&source, "", CompileOptions::default()).is_err());
    let source = page("#a { width: 1px; } #b {", "<div id=\"a\"></div>");
    assert!(compile_str(&source, "", CompileOptions::default()).is_err());
}

#[test]
fn unknown_at_rule_is_an_error() {
    let source = page(
        "@media (min-width: 1px) { #a { width: 1px; } }",
        "<div id=\"a\"></div>",
    );
    assert!(compile_str(&source, "", CompileOptions::default()).is_err());
    let source = page("@font-face { src: none; }", "");
    assert!(compile_str(&source, "", CompileOptions::default()).is_err());
}

#[test]
fn variable_cycle_is_an_error_not_a_loop() {
    let source = page(":root { --a: var(--b); --b: var(--a); }", "");
    assert!(compile_str(&source, "", CompileOptions::default()).is_err());
    let source = page(
        ":root { --gap: 2px; } #a { width: var(--a); }",
        "<div id=\"a\"></div>",
    );
    assert!(compile_str(&source, "", CompileOptions::default()).is_err());
}

#[test]
fn huge_or_hostile_calc_is_an_error() {
    let deep_open = format!("calc({}1px{})", "(".repeat(100_000), ")".repeat(100_000));
    assert!(parse_length(&deep_open).is_err(), "deep nesting");
    let unclosed = format!("calc({}1px", "(".repeat(100_000));
    assert!(parse_length(&unclosed).is_err(), "unclosed nesting");
    let overflow = format!("calc({}px)", "9".repeat(400));
    assert!(parse_length(&overflow).is_err(), "non-finite result");
    let long_chain = format!("calc(1px{})", " + 1px".repeat(20_000));
    assert_eq!(parse_length(&long_chain).unwrap(), Length::Px(20_001.0));
}

#[test]
fn deeply_nested_stylesheet_and_dom_are_errors() {
    let blocks = format!(
        "{}#a{{width:1px;}}{}",
        "x{".repeat(100_000),
        "}".repeat(100_000)
    );
    assert!(parse_stylesheet(&blocks).is_err(), "nested blocks");
    let source = format!(
        "<body>{}{}</body>",
        "<div>".repeat(5_000),
        "</div>".repeat(5_000)
    );
    assert!(
        compile_str(&source, "", CompileOptions::default()).is_err(),
        "deep DOM"
    );
}

#[test]
fn hostile_selectors_never_panic() {
    for selector in [
        ":", "#", ".", "a:", "#a#b", "a..b", "a:hover:", "::before", "[x]", "a+b", "*:root", "é{",
        "#é", ".a.", "a > > b",
    ] {
        let source = page(
            &format!("{selector} {{ width: 1px; }}"),
            "<div id=\"a\"></div>",
        );
        let _ = compile_str(&source, "", CompileOptions::default());
    }
}

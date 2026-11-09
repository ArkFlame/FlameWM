use super::*;
use flamewm_render_core::encode;

fn fixture_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("flamewm-compile-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("sheets")).expect("create fixture dir");
    dir
}

fn style_of_id(output: &CompileOutput, id: &str) -> Style {
    output
        .document
        .nodes
        .iter()
        .find(|node| node.id == id)
        .unwrap_or_else(|| panic!("no node with id {id}"))
        .style
        .as_ref()
        .clone()
}

#[test]
fn calc_resolves_px_percent_and_number_values_in_length_contexts() {
    assert_eq!(
        parse_length("calc(50% * 0.5)").unwrap(),
        Length::Percent(0.25)
    );
    assert_eq!(
        parse_length("calc(100% / 4)").unwrap(),
        Length::Percent(0.25)
    );
    assert_eq!(parse_length("calc(4px * 3)").unwrap(), Length::Px(12.0));
    assert_eq!(parse_px("calc((4px + 2px) / 2)").unwrap(), 3.0);
    assert_eq!(parse_number("calc(1 / 4)").unwrap(), 0.25);
    assert_eq!(parse_edges("calc(2px * 3) 1px").unwrap().top, 6.0);
}

#[test]
fn calc_rejects_wrong_units_and_mixed_units_per_property() {
    assert!(parse_length("calc(10px + 5%)").is_err(), "mixed units");
    assert!(parse_length("calc(2)").is_err(), "unitless is not a length");
    assert!(parse_px("calc(50%)").is_err(), "percent is not px");
    assert!(parse_px("calc(2px + 1)").is_err(), "mixed px and number");
    assert!(parse_number("calc(1px)").is_err(), "length is not a number");
    assert!(
        parse_edges("calc(2px * 2px) 1px").is_err(),
        "length squared"
    );
}

#[test]
fn calc_in_declarations_compiles_and_bad_calc_fails_the_compile() {
    let source = "<body><style>:root { --gap: 3px; } #a { padding: calc(var(--gap) * 2); width: calc(100% / 2); gap: calc((4px + 2px) / 2); }</style><div id=\"a\"></div></body>";
    let output = compile_str(source, "", CompileOptions::default()).unwrap();
    assert!(output.warnings.is_empty());
    let style = style_of_id(&output, "a");
    assert_eq!(style.padding.top, 6.0, "var() inside calc()");
    assert_eq!(style.width, Length::Percent(0.5));
    assert_eq!(style.gap, 3.0);
    for bad in [
        "width: calc(10px + 5%)",
        "padding: calc(2px * 2px)",
        "gap: calc(4px + 2)",
        "width: calc(1px",
    ] {
        let source = format!("<body><style>#a {{ {bad}; }}</style><div id=\"a\"></div></body>");
        assert!(
            compile_str(&source, "", CompileOptions::default()).is_err(),
            "{bad}"
        );
    }
}

#[test]
fn inset_shorthand_expands_to_four_sides_and_rejects_bad_counts() {
    let inset = parse_inset("calc(1px + 1px) 3px").unwrap();
    assert_eq!(
        inset,
        [
            Length::Px(2.0),
            Length::Px(3.0),
            Length::Px(2.0),
            Length::Px(3.0)
        ]
    );
    assert_eq!(parse_inset("auto").unwrap(), [Length::Auto; 4]);
    assert!(parse_inset("1px 2px 3px 4px 5px").is_err());
    assert!(parse_inset("").is_err());
}

#[test]
fn at_import_in_inline_style_is_rejected_without_a_base_directory() {
    let source = "<body><style>@import \"x.css\";</style></body>";
    assert!(compile_str(source, "", CompileOptions::default()).is_err());
}

#[test]
fn linked_sheet_imports_are_expanded_relative_to_the_sheet() {
    let dir = fixture_dir("import");
    fs::write(
        dir.join("page.html"),
        "<body><link rel=\"stylesheet\" href=\"sheets/main.css\"><div id=\"a\"></div></body>",
    )
    .unwrap();
    fs::write(dir.join("sheets/main.css"), "@import \"partial.css\";").unwrap();
    fs::write(dir.join("sheets/partial.css"), "#a { width: 7px; }").unwrap();
    let output = compile_file_in(&dir.join("page.html"), &[], CompileOptions::default(), None)
        .expect("imports compile");
    assert_eq!(style_of_id(&output, "a").width, Length::Px(7.0));
}

fn count_entries(dir: &Path) -> usize {
    fs::read_dir(dir).map(Iterator::count).unwrap_or(0)
}

#[test]
fn cache_hit_matches_fresh_compile_and_corrupt_entries_recompile() {
    let dir = fixture_dir("cache");
    let cache_dir = dir.join("cache");
    let page = dir.join("page.html");
    let source = "<body><style>#a { width: 9px; }</style><div id=\"a\"></div></body>";
    fs::write(&page, source).unwrap();
    let fresh = compile_file_in(&page, &[], CompileOptions::default(), None).unwrap();
    let stored = compile_file_in(&page, &[], CompileOptions::default(), Some(&cache_dir)).unwrap();
    assert_eq!(count_entries(&cache_dir), 1);
    let hit = compile_file_in(&page, &[], CompileOptions::default(), Some(&cache_dir)).unwrap();
    let fresh_bytes = encode(&fresh.document).unwrap();
    assert_eq!(encode(&stored.document).unwrap(), fresh_bytes);
    assert_eq!(encode(&hit.document).unwrap(), fresh_bytes);

    for entry in fs::read_dir(&cache_dir).unwrap() {
        fs::write(entry.unwrap().path(), b"corrupt").unwrap();
    }
    let recovered =
        compile_file_in(&page, &[], CompileOptions::default(), Some(&cache_dir)).unwrap();
    assert_eq!(encode(&recovered.document).unwrap(), fresh_bytes);

    fs::write(&page, source.replace("9px", "11px")).unwrap();
    let edited = compile_file_in(&page, &[], CompileOptions::default(), Some(&cache_dir)).unwrap();
    assert_eq!(style_of_id(&edited, "a").width, Length::Px(11.0));
    assert_eq!(
        count_entries(&cache_dir),
        2,
        "edited source gets its own key"
    );
}

#[test]
fn compile_errors_are_never_cached() {
    let dir = fixture_dir("cache-error");
    let cache_dir = dir.join("cache");
    let page = dir.join("page.html");
    fs::write(
        &page,
        "<body><style>#a { width: calc(1px + 5%); }</style><div id=\"a\"></div></body>",
    )
    .unwrap();
    assert!(compile_file_in(&page, &[], CompileOptions::default(), Some(&cache_dir)).is_err());
    assert_eq!(count_entries(&cache_dir), 0);
}

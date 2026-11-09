use super::*;

#[test]
fn css_color_hex_and_variable_compile() {
    assert_eq!(
        parse_color_literal("#ef4444"),
        Some(Color::rgb(239, 68, 68))
    );
}

pub(super) fn compile_body(html: &str) -> CompileOutput {
    compile_str(html, "", CompileOptions::default()).unwrap()
}

pub(super) fn style_of(output: &CompileOutput, id: &str) -> Style {
    output
        .document
        .nodes
        .iter()
        .find(|node| node.id == id)
        .unwrap()
        .style
        .as_ref()
        .clone()
}

#[test]
fn css_border_reset_shorthands_preserve_uniform_painter_semantics() {
    let html = r#"<body><style>
            .base { border: 2px solid #ef4444; }
            .none { border: 2px solid #ef4444; border: none; }
            .zero { border: 2px solid #ef4444; border: 0; }
            .pixel-zero { border: 2px solid #ef4444; border: 0px; }
            .restore { border: none; border: 1px solid #ffffff40; }
        </style>
        <div id="base" class="base">b</div>
        <div id="none" class="none">n</div>
        <div id="zero" class="zero">z</div>
        <div id="pixel-zero" class="pixel-zero">p</div>
        <div id="restore" class="restore">r</div></body>"#;
    let output = compile_body(html);
    assert_eq!(style_of(&output, "base").border_width, 2.0);
    for id in ["none", "zero", "pixel-zero"] {
        assert_eq!(style_of(&output, id).border_width, 0.0, "{id}");
        assert_eq!(
            style_of(&output, id).border_color,
            ColorValue::Literal(Color::TRANSPARENT)
        );
    }
    assert_eq!(style_of(&output, "restore").border_width, 1.0);
    assert_eq!(
        style_of(&output, "restore").border_color,
        ColorValue::Literal(Color {
            r: 255,
            g: 255,
            b: 255,
            a: 64
        })
    );
    let encoded = flamewm_render_core::encode(&output.document).unwrap();
    assert_eq!(
        flamewm_render_core::decode(&encoded).unwrap(),
        output.document
    );
}

#[test]
fn css_border_reset_rejects_unsupported_values_without_partial_mutation() {
    for value in [
        "hidden",
        "dashed",
        "2px dashed red",
        "1px",
        "10",
        "auto",
        "2px solid",
    ] {
        assert!(
            compile_str(
                &format!("<body><style>div {{ border: {value}; }}</style><div>ok</div></body>"),
                "",
                CompileOptions::default(),
            )
            .is_err(),
            "bad border value {value} must fail"
        );
    }
}

#[test]
fn scrollbar_width_none_hides_scrollbars_per_node() {
    let output = compile_str(
        r#"<body><div class="list" id="list">x</div><div id="plain">x</div></body>"#,
        ".list { overflow-y: auto; scrollbar-width: none; }",
        CompileOptions::default(),
    )
    .unwrap();
    assert!(style_of(&output, "list").hide_scrollbars);
    assert_eq!(
        style_of(&output, "list").overflow_y,
        flamewm_render_core::Overflow::Auto
    );
    assert!(!style_of(&output, "plain").hide_scrollbars);
}

pub(super) fn temp_fixture_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "flamewm-{tag}-{}-{}",
        std::process::id(),
        flamewm_api::clock::unix_nanos()
    ))
}

pub(super) fn interactive_fixture() -> CompileOutput {
    compile_body(
        r#"<body><style>:root { --accent: #ef4444; } body { display:flex; } button { background: var(--accent); width: 100px; } button:hover { background:#ffffff; }</style><button id="start" data-action="start.toggle">Start</button></body>"#,
    )
}

#[test]
fn linked_shared_stylesheet_matches_inline_sheet() {
    let dir = temp_fixture_dir("shared-link");
    let base_css = "* { box-sizing:border-box; } html { width:100%; height:100%; }";
    let app_css = "#probe { background:#102030; width:37px; }";
    fs::create_dir_all(dir.join("base")).unwrap();
    fs::create_dir_all(dir.join("app")).unwrap();
    fs::write(dir.join("base/base.css"), base_css).unwrap();
    fs::write(dir.join("app/app.css"), app_css).unwrap();
    fs::write(
            dir.join("app/linked.html"),
            r#"<!doctype html><html><head><link rel="stylesheet" href="../base/base.css"><link rel="stylesheet" href="app.css"></head><body><div id="probe">x</div></body></html>"#,
        )
        .unwrap();
    fs::write(
            dir.join("app/inline.html"),
            format!(
                r#"<!doctype html><html><head><style>{base_css}{app_css}</style></head><body><div id="probe">x</div></body></html>"#
            ),
        )
        .unwrap();
    let linked = compile_file(&dir.join("app/linked.html"), &[], CompileOptions::default());
    let inline = compile_file(&dir.join("app/inline.html"), &[], CompileOptions::default());
    let _ = fs::remove_dir_all(&dir);
    let mut linked = linked.unwrap().document;
    let inline = inline.unwrap().document;
    // The fingerprint hashes the source text, which differs by construction; compare the compiled tree.
    linked.source_fingerprint = inline.source_fingerprint;
    assert_eq!(
        flamewm_render_core::encode(&linked).unwrap(),
        flamewm_render_core::encode(&inline).unwrap()
    );
}

#[test]
fn var_color_uses_fallback_only_when_variable_is_unknown() {
    let variables: HashMap<String, u16> = HashMap::from([("--accent".to_string(), 3)]);
    assert_eq!(
        parse_color_value("var(--accent, #ff0000)", &variables),
        Ok(ColorValue::Variable(3))
    );
    assert_eq!(
        parse_color_value("var(--missing, #ff0000)", &variables),
        Ok(ColorValue::Literal(Color::rgb(255, 0, 0)))
    );
    assert!(parse_color_value("var(--missing, nope)", &variables).is_err());
    assert!(parse_color_value("var(--missing)", &variables).is_err());
}

#[test]
fn compiles_color_variable() {
    let variables = interactive_fixture().document.variables;
    // Built-in tokens are always present; the sheet's `--accent` must not duplicate the built-in.
    assert_eq!(variables.iter().filter(|v| v.name == "--accent").count(), 1);
    assert_eq!(
        variables
            .iter()
            .find(|v| v.name == "--accent")
            .unwrap()
            .default,
        Color::rgb(0xef, 0x44, 0x44)
    );
}

#[test]
fn length_custom_properties_resolve_with_fallback() {
    let css = ":root { --gap: 6px; --w: 40px; } #a { padding: var(--gap) 4px; gap: var(--gap); border-radius: var(--missing, 3px); width: var(--w); max-width: var(--nope, 90%); }";
    let output = compile_str(
        r#"<body><div id="a">x</div></body>"#,
        css,
        CompileOptions::default(),
    )
    .unwrap();
    let style = style_of(&output, "a");
    assert_eq!(style.padding.top, 6.0);
    assert_eq!(style.padding.right, 4.0);
    assert_eq!(style.gap, 6.0);
    assert_eq!(style.border_radius, 3.0);
    assert_eq!(style.width, Length::Px(40.0));
    assert_eq!(style.max_width, Length::Percent(0.9));
    assert!(output.document.variables.iter().all(|v| v.name != "--gap"));
}

#[test]
fn length_var_rejects_unknown_without_fallback_and_color_tokens() {
    let lengths = HashMap::from([("--gap".to_string(), "6px".to_string())]);
    assert_eq!(
        resolve_length_vars("var(--gap) var(--gap)", &lengths).unwrap(),
        "6px 6px"
    );
    assert!(resolve_length_vars("var(--accent)", &lengths).is_err());
    assert!(resolve_length_vars("var(--gap", &lengths).is_err());
    let err = compile_str(
        r#"<body><div id="a">x</div></body>"#,
        "#a { width: var(--accent); }",
        CompileOptions::default(),
    );
    assert!(err.is_err());
    let color_ok = compile_str(
        r#"<body><div id="a">x</div></body>"#,
        "#a { background: var(--accent, red); }",
        CompileOptions::default(),
    );
    assert!(color_ok.is_ok());
}

#[test]
fn compiles_native_action() {
    assert!(interactive_fixture()
        .document
        .nodes
        .iter()
        .any(|node| node.action == "start.toggle"));
}

#[test]
fn compiles_pointer_cursor() {
    let output = compile_body(
        r#"<body><style>button { cursor: pointer; }</style><button id="open">Open</button></body>"#,
    );
    let node = output
        .document
        .nodes
        .iter()
        .find(|node| node.id == "open")
        .unwrap();
    assert_eq!(node.style.cursor, CursorKind::Pointer);
}

#[test]
fn compiles_wrap_and_break_anywhere() {
    use flamewm_render_core::TextWrap;
    let output = compile_body(
        r#"<body><style>.a { white-space: normal; } .b { white-space: nowrap; } .c { overflow-wrap: anywhere; }</style><div class="a" id="a">x</div><div class="b" id="b">x</div><div class="c" id="c">x</div></body>"#,
    );
    let find = |id: &str| style_of(&output, id);
    assert_eq!(find("a").text_wrap, TextWrap::Wrap);
    assert!(!find("a").break_anywhere);
    assert_eq!(find("b").text_wrap, TextWrap::NoWrap);
    assert_eq!(find("c").text_wrap, TextWrap::Wrap);
    assert!(find("c").break_anywhere);
}

#[test]
fn compiles_text_overflow_ellipsis_and_inherits_it() {
    use flamewm_render_core::TextOverflow;
    let output = compile_body(
        r#"<body><style>.d { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; } .e { white-space: nowrap; }</style><div class="d" id="d">x</div><div class="e" id="e">x</div></body>"#,
    );
    assert_eq!(style_of(&output, "d").text_overflow, TextOverflow::Ellipsis);
    assert_eq!(style_of(&output, "e").text_overflow, TextOverflow::Clip);
}

#[test]
fn shared_text_shadow_token_is_inherited_and_opt_out_clears_it() {
    let css = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../ui/desktop/desktop.css"
    ));
    let result = compile_str(
            r#"<body><div class="desktop-label-line" id="label">Test label</div><div id="menu">Menu</div></body>"#,
            css,
            CompileOptions::default(),
        ).expect("desktop CSS must compile under strict mode");
    let label_id = result
        .document
        .nodes
        .iter()
        .position(|n| n.id == "label")
        .unwrap();
    let label = &result.document.nodes[label_id];
    let shadow = label
        .style
        .text_shadow
        .expect("desktop labels inherit the shared shadow token");
    assert_eq!(
        (shadow.offset_x, shadow.offset_y, shadow.blur_radius),
        (0.0, 1.0, 2.0)
    );
    let child = &result.document.nodes[label.first_child.unwrap() as usize];
    assert_eq!(
        child.style.text_shadow,
        Some(shadow),
        "text inherits its label's shadow"
    );
    assert_eq!(style_of(&result, "menu").text_shadow, Some(shadow));
    let binary = flamewm_render_core::encode(&result.document).expect("encode shadowed document");
    assert_eq!(
        flamewm_render_core::decode(&binary).expect("decode shadowed document"),
        result.document
    );
}

#[test]
fn root_text_shadow_token_overrides_default_and_none_opts_out() {
    let result = compile_str(
        r#"<body><div id="plain">x</div><div id="off">y</div></body>"#,
        ":root { --text-shadow: 1px 2px #000000ff; } #off { text-shadow: none; }",
        CompileOptions::default(),
    )
    .expect("token must compile");
    let plain = style_of(&result, "plain")
        .text_shadow
        .expect("token applies");
    assert_eq!(
        (plain.offset_x, plain.offset_y, plain.blur_radius),
        (1.0, 2.0, 0.0)
    );
    assert_eq!(style_of(&result, "off").text_shadow, None);
}

#[test]
fn builtin_accent_tokens_resolve_without_root_and_root_overrides_them() {
    let accent = flamewm_api::settings::DEFAULT_ACCENT_RGB;
    let (r, g, b) = ((accent >> 16) as u8, (accent >> 8) as u8, accent as u8);
    // Resolve a node's background to the default value of the token it references.
    let token_default = |result: &CompileOutput, id: &str| {
        let ColorValue::Variable(index) = style_of(result, id).background else {
            panic!("#{id} must reference a token");
        };
        result.document.variables[index as usize].default
    };
    let css = "#a { background: var(--accent); } #b { background: var(--accent-soft); }";
    let result = compile_str(
        r#"<body><div id="a">x</div><div id="b">y</div></body>"#,
        css,
        CompileOptions::default(),
    )
    .expect("builtin tokens must compile without a :root declaration");
    assert_eq!(token_default(&result, "a"), Color { r, g, b, a: 255 });
    assert_eq!(token_default(&result, "b"), Color { r, g, b, a: 0x38 });
    // A sheet `:root` declaration overrides the built-in default by name.
    let overridden = compile_str(
        r#"<body><div id="a">x</div></body>"#,
        &format!("{css} :root {{ --accent: #ef4444; }}"),
        CompileOptions::default(),
    )
    .expect("sheet override must compile");
    assert_eq!(
        token_default(&overridden, "a"),
        Color::rgb(0xef, 0x44, 0x44)
    );
}

#[test]
fn text_shadow_blur_is_bounded_and_legacy_syntax_is_pixel_equivalent() {
    let output = compile_body(
        r#"<body><style>
            .soft { text-shadow: 0px 1px 3px #000000b8; }
            .legacy { text-shadow: 0px 1px #000000b8; }
            .none { text-shadow: none; }
        </style><div id="soft" class="soft">two lines</div><div id="legacy" class="legacy">old</div><div id="none" class="none">none</div></body>"#,
    );
    assert_eq!(
        style_of(&output, "soft").text_shadow.unwrap().blur_radius,
        3.0
    );
    let old = style_of(&output, "legacy").text_shadow.unwrap();
    assert_eq!(
        (old.blur_radius, old.offset_x, old.offset_y),
        (0.0, 0.0, 1.0)
    );
    assert_eq!(style_of(&output, "none").text_shadow, None);
    let invalid = compile_str(
        r#"<body><style>#bad { text-shadow: 0px 1px 5px #000000; }</style><div id="bad">x</div></body>"#,
        "",
        CompileOptions::default(),
    );
    assert!(invalid.is_err(), "unbounded blur must be rejected");
}

#[test]
fn desktop_sticky_text_wraps_while_other_ui_stays_nowrap() {
    use flamewm_render_core::TextWrap;
    let css = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../ui/desktop/desktop.css"
    ))
    .expect("read desktop UI CSS");
    assert!(
        css.contains(".sticky-text")
            && css.contains("white-space:normal")
            && css.contains("overflow-wrap:anywhere"),
        "desktop CSS must enable wrap only on .sticky-text"
    );
    let output = compile_str(
            r#"<body><div class="desktop-label-line" id="label">x</div><div class="desktop-menu-row" id="row">x</div><div class="sticky-text" id="sticky">x</div></body>"#,
            &css,
            CompileOptions::default(),
        )
        .unwrap();
    let find = |id: &str| style_of(&output, id);
    let sticky = find("sticky");
    assert_eq!(sticky.text_wrap, TextWrap::Wrap);
    assert!(sticky.break_anywhere);
    assert_eq!(sticky.overflow_x, flamewm_render_core::Overflow::Hidden);
    assert_eq!(find("label").text_wrap, TextWrap::NoWrap);
    assert!(!find("label").break_anywhere);
    assert_eq!(find("row").text_wrap, TextWrap::NoWrap);
    assert!(!find("row").break_anywhere);
}

#[test]
fn desktop_convert_wheel_css_compiles_with_native_flex_label_centering() {
    // Regression: the strict RWR compiler rejects `text-align`, which
    // previously panicked the desktop build script before Xephyr started.
    // Compile the actual desktop and shared menu styles, not a mock rule.
    let desktop_css = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../ui/desktop/desktop.css"
    ));
    let shared_menu_css = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../ui/shared/menu.css"
    ));
    let css = format!("{desktop_css}\n{shared_menu_css}");
    let output = compile_str(
            r#"<html><body><section class="context-menu convert-wheel"><button id="convert-option" class="convert-wheel-option convert-wheel-pos-0">WEBP</button></section></body></html>"#,
            &css,
            CompileOptions::default(),
        )
        .expect("desktop conversion wheel CSS must compile in strict mode");
    let style = style_of(&output, "convert-option");
    assert_eq!(style.display, Display::Flex);
    assert_eq!(style.justify_content, JustifyContent::Center);
    assert_eq!(style.align_items, AlignItems::Center);
}

#[test]
fn ordinary_ui_css_uses_default_arrow_cursor() {
    // C-CURSOR-ARROW: ordinary buttons/menu rows/desktop entries/titlebar
    // buttons -> default arrow; text/password entry -> text; explicit
    // resize edges only -> resize. Deferred follow-up lines (owned by
    // parallel jobs this wave): ui/desktop/desktop.css `.desktop-item
    // cursor:pointer`, ui/shell/flamewm.css `.desktop-icon cursor:move`.
    let files = [
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../ui/settings.css"),
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../ui/desktop/desktop.css"),
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../ui/shell/flamewm.css"),
    ];
    for path in files {
        let css = std::fs::read_to_string(path).expect("read active UI CSS");
        assert!(
            !css.contains("cursor:pointer"),
            "ordinary UI CSS must not use cursor:pointer: {path}"
        );
    }
    // Cascade-safe: legitimate text/move roles must still exist in the
    // active cascade (checked here so the guard fails closed if a
    // future edit strips them globally).
    let shell = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../ui/shell/flamewm.css"
    ))
    .expect("read shell UI CSS");
    assert!(
        shell.contains("cursor:text") && shell.contains("cursor:move"),
        "active cascade must keep legitimate text/move cursor roles"
    );
}

#[test]
fn zero_width_seed_keeps_runtime_text_slot_mutable() {
    let output = compile_body(r#"<body><div id="slot">&#8203;</div></body>"#);
    let slot = output
        .document
        .nodes
        .iter()
        .position(|node| node.id == "slot")
        .expect("slot node");
    let child = output.document.nodes[slot]
        .first_child
        .expect("zero-width seed must compile to a text child");
    assert_eq!(output.document.nodes[child as usize].kind, NodeKind::Text);
}

#[test]
fn cache_key_changes_with_utilities_revision() {
    let source = "<p class=\"flex\"></p>";
    let html = crate::html::parse_html(source).unwrap();
    let key = |revision: u64| {
        cache_key(
            source,
            "",
            std::path::Path::new("."),
            &html,
            CompileOptions::default(),
            revision,
        )
    };
    assert!(key(1).is_some());
    assert_eq!(key(1), key(1));
    assert_ne!(key(1), key(2));
    assert_ne!(key(crate::utilities::UTILITIES_REVISION), key(0));
}

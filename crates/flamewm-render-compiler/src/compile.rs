use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use flamewm_render_core::{
    fnv1a64, AlignItems, Color, ColorValue, ColorVariable, CompiledDocument, CompiledNode,
    CursorKind, Display, Edges, FlexDirection, ImageAsset, ImageTreatment, JustifyContent, Length,
    NodeKind, Overflow, Position, Style, TextOverflow, TextShadow, TextWrap,
};

use crate::cache;
use crate::calc::{eval_calc, CalcUnit};
use crate::css::{parse_declarations, parse_stylesheet, Declaration, PseudoState, Rule};
use crate::html::{parse_html, HtmlNodeKind, ParsedHtml};
use crate::imports::expand_imports;
use crate::rule_index::RuleIndex;

mod build;
mod declarations;
mod node;
mod shorthands;
mod values;
mod vars;

use self::build::Compiler;
use self::node::*;
use self::shorthands::*;
use self::values::*;
use self::vars::*;

#[derive(Clone, Copy, Debug)]
pub struct CompileOptions {
    pub strict: bool,
}

impl Default for CompileOptions {
    fn default() -> Self {
        Self { strict: true }
    }
}

#[derive(Clone, Debug)]
pub struct CompileOutput {
    pub document: CompiledDocument,
    pub warnings: Vec<String>,
}

pub fn compile_str(
    html_source: &str,
    extra_css: &str,
    options: CompileOptions,
) -> Result<CompileOutput, String> {
    let html = parse_html(html_source)?;
    if !html.linked_css.is_empty() {
        return Err("compile_str cannot resolve <link rel=\"stylesheet\">; use compile_file or pass CSS explicitly".to_string());
    }
    let css = format!("{}\n{extra_css}", html.embedded_css);
    compile_parsed(html_source, &css, html, options, None)
}

pub fn compile_file(
    html_path: &Path,
    extra_css_paths: &[PathBuf],
    options: CompileOptions,
) -> Result<CompileOutput, String> {
    compile_file_in(
        html_path,
        extra_css_paths,
        options,
        cache::default_dir().as_deref(),
    )
}

/// Compiles a page from disk. With `cache_dir`, a previous compile of identical inputs is reused.
/// Outputs with warnings are never cached, so a hit always has an empty warning list.
fn compile_file_in(
    html_path: &Path,
    extra_css_paths: &[PathBuf],
    options: CompileOptions,
    cache_dir: Option<&Path>,
) -> Result<CompileOutput, String> {
    let html_source = fs::read_to_string(html_path)
        .map_err(|error| format!("failed to read {}: {error}", html_path.display()))?;
    let html = parse_html(&html_source)?;
    let base = html_path.parent().unwrap_or_else(|| Path::new("."));
    let mut css = expand_imports(html_path, &html.embedded_css)?;
    let mut sheets = Vec::with_capacity(html.linked_css.len() + extra_css_paths.len());
    for href in &html.linked_css {
        if href.contains("://") || href.starts_with("data:") {
            return Err(format!("runtime/network stylesheet URL '{href}' is forbidden; FlameWM Render stylesheets must be local build inputs"));
        }
        sheets.push(base.join(href));
    }
    sheets.extend_from_slice(extra_css_paths);
    for path in &sheets {
        css.push('\n');
        let source = fs::read_to_string(path)
            .map_err(|error| format!("failed to read stylesheet {}: {error}", path.display()))?;
        css.push_str(&expand_imports(path, &source)?);
    }
    let key = cache_dir.and_then(|_| cache_key(&html_source, &css, base, &html, options));
    if let (Some(dir), Some(key)) = (cache_dir, key) {
        if let Some(document) = cache::load(dir, key) {
            return Ok(CompileOutput {
                document,
                warnings: Vec::new(),
            });
        }
    }
    let output = compile_parsed(&html_source, &css, html, options, Some(base))?;
    if let (Some(dir), Some(key)) = (cache_dir, key) {
        if output.warnings.is_empty() {
            cache::store(dir, key, &output.document);
        }
    }
    Ok(output)
}

/// Hashes every input that changes the compiled output: compiler version, options, page HTML,
/// expanded CSS, and the bytes of each `src` image. `None` when an image cannot be read, so the
/// compile reports that error rather than caching around it.
fn cache_key(
    html_source: &str,
    css: &str,
    base: &Path,
    html: &ParsedHtml,
    options: CompileOptions,
) -> Option<u64> {
    let mut input = Vec::with_capacity(html_source.len() + css.len() + 64);
    input.extend_from_slice(env!("CARGO_PKG_VERSION").as_bytes());
    input.push(0);
    input.push(options.strict as u8);
    input.push(0);
    input.extend_from_slice(html_source.as_bytes());
    input.push(0);
    input.extend_from_slice(css.as_bytes());
    input.push(0);
    for node in &html.nodes {
        if let Some(src) = node.attrs.get("src") {
            let bytes = fs::read(base.join(src)).ok()?;
            input.extend_from_slice(src.as_bytes());
            input.push(0);
            input.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            input.extend_from_slice(&bytes);
        }
    }
    Some(fnv1a64(&input))
}

fn compile_parsed(
    html_source: &str,
    css_source: &str,
    html: ParsedHtml,
    options: CompileOptions,
    asset_base: Option<&Path>,
) -> Result<CompileOutput, String> {
    let rules = parse_stylesheet(css_source)?;
    let (variables, variable_map, length_map) = collect_variables(&rules)?;
    let mut fingerprint_input = Vec::with_capacity(html_source.len() + css_source.len() + 1);
    fingerprint_input.extend_from_slice(html_source.as_bytes());
    fingerprint_input.push(0);
    fingerprint_input.extend_from_slice(css_source.as_bytes());

    let rule_index = RuleIndex::new(&rules);
    let mut context = Compiler {
        html: &html,
        rules: &rules,
        rule_index: &rule_index,
        variable_map: &variable_map,
        length_map: &length_map,
        options,
        asset_base,
        warnings: Vec::new(),
        assets: Vec::new(),
        asset_index: HashMap::new(),
        nodes: Vec::new(),
    };

    let inherited = Style {
        text_shadow: root_text_shadow(&rules, &variable_map)?,
        ..Style::default()
    };
    let root = context.compile_subtree(html.root, None, &inherited, 0)?;
    for asset in &context.assets {
        fingerprint_input.push(0xff);
        fingerprint_input.extend_from_slice(asset.source.as_bytes());
        fingerprint_input.extend_from_slice(&asset.width.to_le_bytes());
        fingerprint_input.extend_from_slice(&asset.height.to_le_bytes());
        fingerprint_input.extend_from_slice(&asset.pixels);
    }
    let document = CompiledDocument {
        source_fingerprint: fnv1a64(&fingerprint_input),
        root,
        variables,
        assets: context.assets,
        nodes: context.nodes,
    };
    document.validate()?;
    Ok(CompileOutput {
        document,
        warnings: context.warnings,
    })
}

/// Shared text shadow token: every text run inherits it from the document root.
/// Override with `:root { --text-shadow: <shadow> }`; opt out per element with `text-shadow: none`.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_color_hex_and_variable_compile() {
        assert_eq!(
            parse_color_literal("#ef4444"),
            Some(Color::rgb(239, 68, 68))
        );
    }

    fn compile_body(html: &str) -> CompileOutput {
        compile_str(html, "", CompileOptions::default()).unwrap()
    }

    fn style_of(output: &CompileOutput, id: &str) -> Style {
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

    fn temp_fixture_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "flamewm-{tag}-{}-{}",
            std::process::id(),
            flamewm_api::clock::unix_nanos()
        ))
    }

    fn interactive_fixture() -> CompileOutput {
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
        let binary =
            flamewm_render_core::encode(&result.document).expect("encode shadowed document");
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
    fn parses_binary_ppm_asset_payload() {
        let mut bytes = b"P6\n1 1\n255\n".to_vec();
        bytes.extend_from_slice(&[0x12, 0x34, 0x56]);
        let parsed = flamewm_image_core::ppm::decode(&bytes).unwrap();
        assert_eq!(parsed.width, 1);
        assert_eq!(parsed.height, 1);
        assert_eq!(parsed.pixels, vec![0x12, 0x34, 0x56, 255]);
    }

    #[test]
    fn opaque_ppm_asset_compiles_to_fully_opaque_rgba8() {
        use std::io::Write;
        let dir = temp_fixture_dir("j05-ppm");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let ppm = dir.join("icon.ppm");
        let mut bytes = b"P6\n1 1\n255\n".to_vec();
        bytes.extend_from_slice(&[0x11, 0x22, 0x33]);
        std::fs::write(&ppm, &bytes).expect("temp ppm");
        let html = dir.join("doc.html");
        let mut file = std::fs::File::create(&html).expect("temp html");
        write!(
            file,
            "<html><body><img id=\"i\" src=\"icon.ppm\"></body></html>"
        )
        .expect("write html");
        let output = compile_file(&html, &[], CompileOptions::default()).expect("compile");
        assert_eq!(output.document.assets.len(), 1);
        assert_eq!(
            output.document.assets[0].pixels,
            vec![0x11, 0x22, 0x33, 255]
        );
        assert!(output.document.assets[0].is_fully_opaque());
    }

    fn compile_svg_html(name: &str, body: &str) -> CompileOutput {
        use std::io::Write;
        let breeze =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/web/breeze");
        let bytes = std::fs::read(breeze.join(name)).expect("read breeze svg");
        let dir = temp_fixture_dir(&format!("j03-svg-{name}"));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join(name), &bytes).expect("copy svg");
        let html = dir.join("doc.html");
        let mut file = std::fs::File::create(&html).expect("temp html");
        write!(file, "<html><body>{body}</body></html>").expect("write html");
        compile_file(&html, &[], CompileOptions::default()).expect("compile")
    }

    fn compile_svg_fixture(name: &str) -> CompileOutput {
        compile_svg_html(name, &format!("<img id=\"i\" src=\"{name}\">"))
    }

    fn mean_rgb(pixels: &[u8]) -> (f32, f32, f32) {
        let mut sum = [0u64; 3];
        let mut count = 0u64;
        for pixel in pixels.chunks_exact(4) {
            if pixel[3] > 8 {
                sum[0] += u64::from(pixel[0]);
                sum[1] += u64::from(pixel[1]);
                sum[2] += u64::from(pixel[2]);
                count += 1;
            }
        }
        assert!(count > 0, "expected visible pixels");
        (
            sum[0] as f32 / count as f32,
            sum[1] as f32 / count as f32,
            sum[2] as f32 / count as f32,
        )
    }

    fn count_near(pixels: &[u8], target: [u8; 3], tol: i16) -> usize {
        pixels
            .chunks_exact(4)
            .filter(|pixel| {
                pixel[3] > 8
                    && (i16::from(pixel[0]) - i16::from(target[0])).abs() <= tol
                    && (i16::from(pixel[1]) - i16::from(target[1])).abs() <= tol
                    && (i16::from(pixel[2]) - i16::from(target[2])).abs() <= tol
            })
            .count()
    }

    #[test]
    fn semantic_svg_text_fallback_compiles_to_light_text() {
        let output = compile_svg_fixture("network-wireless.svg");
        assert_eq!(output.document.assets.len(), 1);
        let (r, g, b) = mean_rgb(&output.document.assets[0].pixels);
        assert!(
            (r - 0xf1 as f32).abs() < 20.0
                && (g - 0xf2 as f32).abs() < 20.0
                && (b - 0xf3 as f32).abs() < 20.0,
            "mean=({r},{g},{b}) expected near #f1f2f3"
        );
    }

    #[test]
    fn semantic_svg_accent_compiles_to_flame_red_preserving_alpha() {
        let output = compile_svg_fixture("folder.svg");
        assert_eq!(output.document.assets.len(), 1);
        let asset = &output.document.assets[0];
        assert!(asset.pixels.chunks_exact(4).any(|p| p[3] > 8 && p[3] < 255));
        assert!(count_near(&asset.pixels, [0xef, 0x40, 0x48], 32) > 200);
        // Dedup: same src twice in one document yields one asset, both nodes share it.
        let dup = compile_svg_html(
            "folder.svg",
            r#"<img id="a" src="folder.svg"><img id="b" src="folder.svg">"#,
        );
        assert_eq!(dup.document.assets.len(), 1);
        let image_of = |id: &str| {
            dup.document
                .nodes
                .iter()
                .find(|n| n.id == id)
                .unwrap()
                .image
                .unwrap()
        };
        assert_eq!(image_of("a"), image_of("b"));
    }

    #[test]
    fn full_color_svg_compiles_unchanged() {
        let output = compile_svg_fixture("applications-games.svg");
        assert_eq!(output.document.assets.len(), 1);
        let asset = &output.document.assets[0];
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets/web/breeze/applications-games.svg"),
        )
        .expect("read games svg");
        let (w, h) = flamewm_image_core::svg::natural_size(&bytes).unwrap();
        let plain = flamewm_image_core::svg::render(&bytes, w, h).unwrap();
        assert_eq!(asset.width, w);
        assert_eq!(asset.height, h);
        assert_eq!(asset.pixels, plain.pixels);
        // Multi-color: distinct blue, green, red, yellow populations.
        assert!(count_near(&asset.pixels, [0x0c, 0x85, 0xdc], 64) > 0);
        assert!(count_near(&asset.pixels, [0x27, 0xab, 0x5f], 64) > 0);
        assert!(count_near(&asset.pixels, [0xda, 0x44, 0x53], 64) > 0);
        assert!(count_near(&asset.pixels, [0xfd, 0xae, 0x23], 64) > 0);
    }

    #[test]
    fn internet_svg_compiles_readable() {
        let output = compile_svg_fixture("internet-web-browser.svg");
        assert_eq!(output.document.assets.len(), 1);
        let asset = &output.document.assets[0];
        let visible: usize = asset.pixels.chunks_exact(4).filter(|p| p[3] > 8).count();
        assert!(visible > 100, "expected readable icon, visible={visible}");
        assert!(count_near(&asset.pixels, [0x19, 0x7c, 0xf1], 64) > 20);
    }

    #[test]
    fn focus_visible_and_focus_compile_to_focus_style() {
        let html = r#"<body><button id="a">A</button><button id="b">B</button><button id="c">C</button></body>"#;
        let css = "#a:focus-visible { border-color: #ff0000; } #b:focus { border-color: #00ff00; }";
        let output = compile_str(html, css, CompileOptions::default()).unwrap();
        let focus_of = |id: &str| {
            output
                .document
                .nodes
                .iter()
                .find(|node| node.id == id)
                .unwrap()
                .focus_style
                .is_some()
        };
        assert!(focus_of("a"));
        assert!(focus_of("b"));
        assert!(!focus_of("c"));
    }

    #[test]
    fn focus_ring_token_is_builtin_accent_derived() {
        let output = compile_body("<body><button id='a'>A</button></body>");
        assert!(output
            .document
            .variables
            .iter()
            .any(|variable| variable.name == "--focus-ring"));
    }
}

#[cfg(test)]
mod ui_pages_tests {
    use super::*;
    use flamewm_render_core::encode;

    fn html_pages(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("read ui dir") {
            let path = entry.expect("ui entry").path();
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name != "assets") {
                    html_pages(&path, out);
                }
            } else if path.extension().is_some_and(|ext| ext == "html") {
                out.push(path);
            }
        }
    }

    fn css_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("read ui dir") {
            let path = entry.expect("ui entry").path();
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name != "assets") {
                    css_files(&path, out);
                }
            } else if path.extension().is_some_and(|ext| ext == "css") {
                out.push(path);
            }
        }
    }

    #[test]
    fn every_ui_stylesheet_resolves_length_variables() {
        let ui = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../ui"));
        let mut sheets = Vec::new();
        css_files(ui, &mut sheets);
        assert!(!sheets.is_empty());
        // Shared tokens live in base.css, which pages load alongside their own sheet.
        let base = ui.join("base/base.css");
        for sheet in &sheets {
            let mut source =
                expand_imports(&base, &fs::read_to_string(&base).expect("read base CSS"))
                    .expect("expand base CSS");
            source.push_str(
                &expand_imports(sheet, &fs::read_to_string(sheet).expect("read ui CSS"))
                    .unwrap_or_else(|error| panic!("{}: {error}", sheet.display())),
            );
            let rules = parse_stylesheet(&source)
                .unwrap_or_else(|error| panic!("{}: {error}", sheet.display()));
            let (_, _, lengths) = collect_variables(&rules)
                .unwrap_or_else(|error| panic!("{}: {error}", sheet.display()));
            for declaration in rules.iter().flat_map(|rule| &rule.declarations) {
                if takes_length_var(&declaration.name) && declaration.value.contains("var(") {
                    resolve_length_vars(declaration.value.trim(), &lengths).unwrap_or_else(
                        |error| panic!("{}: {}: {error}", sheet.display(), declaration.name),
                    );
                }
            }
        }
    }

    /// Replaces each `var(--name)` whose `name` is a length token with the token's literal text.
    fn inline_length_tokens(css: &str, lengths: &HashMap<String, String>) -> String {
        let mut output = String::with_capacity(css.len());
        let mut rest = css;
        while let Some(start) = rest.find("var(--") {
            output.push_str(&rest[..start]);
            let after = &rest[start + "var(".len()..];
            let end = after.find(')').expect("closed var()");
            match lengths.get(&after[..end]) {
                Some(literal) => output.push_str(literal),
                None => output.push_str(&rest[start..start + "var(".len() + end + 1]),
            }
            rest = &after[end + 1..];
        }
        output.push_str(rest);
        output
    }

    /// Encoded document with the source fingerprint (header bytes 8..16) zeroed, since the
    /// fingerprint hashes source text and the token rewrite changes that text on purpose.
    fn encoded_without_fingerprint(document: &CompiledDocument) -> Vec<u8> {
        let mut bytes = encode(document).expect("encode document");
        bytes[8..16].fill(0);
        bytes
    }

    #[test]
    fn token_references_compile_to_the_same_document_as_their_literals() {
        let ui = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../ui"));
        let tokens = expand_imports(
            &ui.join("base/tokens.css"),
            &fs::read_to_string(ui.join("base/tokens.css")).expect("read tokens"),
        )
        .expect("expand tokens");
        let (_, _, lengths) = collect_variables(&parse_stylesheet(&tokens).unwrap()).unwrap();
        assert!(lengths.len() >= 15, "token sheet defines the length tokens");

        let mut pages = Vec::new();
        html_pages(ui, &mut pages);
        let (mut compared, mut skipped, mut found) = (0, 0, 0);
        for page in pages {
            let source = fs::read_to_string(&page).expect("read page");
            if !source.contains("tokens.css") {
                continue;
            }
            found += 1;
            // Same inputs as compile_file: embedded CSS, then each linked sheet with its imports.
            let html = parse_html(&source).expect("parse page");
            let base = page.parent().expect("page parent");
            let mut css = expand_imports(&page, &html.embedded_css).expect("embedded CSS");
            for href in &html.linked_css {
                let path = base.join(href);
                css.push('\n');
                let text = fs::read_to_string(&path).expect("read linked sheet");
                css.push_str(&expand_imports(&path, &text).expect("expand linked sheet"));
            }
            let literal_css = inline_length_tokens(&css, &lengths);
            let compile = |css: &str| {
                compile_parsed(
                    &source,
                    css,
                    parse_html(&source).unwrap(),
                    CompileOptions::default(),
                    Some(base),
                )
            };
            let (with_tokens, with_literals) = match (compile(&css), compile(&literal_css)) {
                (Ok(a), Ok(b)) => (a, b),
                // Missing image files are checked out separately; the other test reports them.
                (Err(error), _) | (_, Err(error)) if error.starts_with("failed to read image") => {
                    skipped += 1;
                    continue;
                }
                (Err(error), _) | (_, Err(error)) => panic!("{}: {error}", page.display()),
            };
            assert_eq!(
                encoded_without_fingerprint(&with_tokens.document),
                encoded_without_fingerprint(&with_literals.document),
                "{} differs between tokens and literals",
                page.display()
            );
            compared += 1;
        }
        assert!(found > 0, "no page links the token sheet");
        eprintln!("token compile-diff: {compared} compared, {skipped} skipped (missing images)");
    }

    #[test]
    fn every_ui_page_compiles_with_its_linked_css() {
        let ui = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../ui"));
        let mut pages = Vec::new();
        html_pages(ui, &mut pages);
        assert!(
            !pages.is_empty(),
            "no UI pages found under {}",
            ui.display()
        );
        let mut failures = Vec::new();
        for page in &pages {
            if let Err(error) = compile_file(page, &[], CompileOptions::default()) {
                // Image bytes are not checked out in every workspace; CSS errors still fail.
                if error.starts_with("failed to read image") {
                    eprintln!("skipped (missing image asset): {error}");
                } else {
                    failures.push(format!("{}: {error}", page.display()));
                }
            }
        }
        assert!(
            failures.is_empty(),
            "UI pages failed to compile:\n{}",
            failures.join("\n")
        );
    }
}

#[cfg(test)]
mod css_feature_tests {
    use super::*;
    use flamewm_render_core::encode;

    fn fixture_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("flamewm-compile-{tag}-{}", std::process::id()));
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
        let stored =
            compile_file_in(&page, &[], CompileOptions::default(), Some(&cache_dir)).unwrap();
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
        let edited =
            compile_file_in(&page, &[], CompileOptions::default(), Some(&cache_dir)).unwrap();
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
}

#[cfg(test)]
mod malformed_input_tests {
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
            ":", "#", ".", "a:", "#a#b", "a..b", "a:hover:", "::before", "[x]", "a+b", "*:root",
            "é{", "#é", ".a.", "a > > b",
        ] {
            let source = page(
                &format!("{selector} {{ width: 1px; }}"),
                "<div id=\"a\"></div>",
            );
            let _ = compile_str(&source, "", CompileOptions::default());
        }
    }
}

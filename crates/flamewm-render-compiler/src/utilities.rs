//! Used-only utility classes (`flex`, `p-4`, `w-[240px]`, `hover:bg-accent-soft`), lowered at compile time.
//! Spec: `docs/UI_UTILITIES_SPEC.md`. A token that resolves to nothing here is an ordinary component class.

use crate::compile::values::{parse_length, parse_px};
use crate::css::{class_rule, Declaration, PseudoState, Rule};
use crate::html::ParsedHtml;

/// Fixed utilities: name, then the declarations it lowers to. Kept sorted by name (checked by a test).
// table:begin
const UTILITIES: &[(&str, &[(&str, &str)])] = &[
    ("absolute", &[("position", "absolute")]),
    ("bg-accent", &[("background", "var(--accent)")]),
    ("bg-accent-soft", &[("background", "var(--accent-soft)")]),
    ("bg-black", &[("background", "#000000")]),
    ("bg-transparent", &[("background", "transparent")]),
    ("bg-white", &[("background", "#ffffff")]),
    ("block", &[("display", "block")]),
    ("border", &[("border-width", "1px")]),
    ("border-rim", &[("border-color", "var(--rim)")]),
    ("bottom-0", &[("bottom", "0")]),
    ("box-border", &[("box-sizing", "border-box")]),
    ("cursor-default", &[("cursor", "default")]),
    ("cursor-pointer", &[("cursor", "pointer")]),
    ("flex", &[("display", "flex")]),
    ("flex-1", &[("flex-grow", "1")]),
    ("flex-col", &[("flex-direction", "column")]),
    ("flex-row", &[("flex-direction", "row")]),
    ("font-bold", &[("font-weight", "700")]),
    ("font-medium", &[("font-weight", "500")]),
    ("font-semibold", &[("font-weight", "600")]),
    ("grow", &[("flex-grow", "1")]),
    ("grow-0", &[("flex-grow", "0")]),
    ("h-full", &[("height", "100%")]),
    ("hidden", &[("display", "none")]),
    ("inset-0", &[("inset", "0")]),
    ("items-center", &[("align-items", "center")]),
    ("items-end", &[("align-items", "flex-end")]),
    ("items-start", &[("align-items", "flex-start")]),
    ("items-stretch", &[("align-items", "stretch")]),
    ("justify-between", &[("justify-content", "space-between")]),
    ("justify-center", &[("justify-content", "center")]),
    ("justify-end", &[("justify-content", "flex-end")]),
    ("justify-start", &[("justify-content", "flex-start")]),
    ("left-0", &[("left", "0")]),
    ("min-w-0", &[("min-width", "0")]),
    ("opacity-0", &[("opacity", "0")]),
    ("opacity-100", &[("opacity", "1")]),
    ("opacity-25", &[("opacity", "0.25")]),
    ("opacity-50", &[("opacity", "0.5")]),
    ("opacity-75", &[("opacity", "0.75")]),
    ("overflow-hidden", &[("overflow", "hidden")]),
    ("relative", &[("position", "relative")]),
    ("right-0", &[("right", "0")]),
    ("rounded-full", &[("border-radius", "9999px")]),
    ("rounded-lg", &[("border-radius", "var(--radius-lg)")]),
    ("rounded-md", &[("border-radius", "var(--radius-md)")]),
    ("rounded-sm", &[("border-radius", "var(--radius-sm)")]),
    ("rounded-xl", &[("border-radius", "var(--radius-xl)")]),
    ("select-none", &[("user-select", "none")]),
    ("text-accent", &[("color", "var(--accent)")]),
    ("text-base", &[("font-size", "var(--font-base)")]),
    ("text-lg", &[("font-size", "var(--font-lg)")]),
    ("text-md", &[("font-size", "var(--font-md)")]),
    ("text-muted", &[("color", "var(--muted)")]),
    ("text-sm", &[("font-size", "var(--font-sm)")]),
    ("text-text", &[("color", "var(--text)")]),
    ("text-xl", &[("font-size", "var(--font-xl)")]),
    ("text-xs", &[("font-size", "var(--font-xs)")]),
    ("top-0", &[("top", "0")]),
    (
        "truncate",
        &[
            ("white-space", "nowrap"),
            ("overflow", "hidden"),
            ("text-overflow", "ellipsis"),
        ],
    ),
    ("w-1/2", &[("width", "50%")]),
    ("w-1/3", &[("width", "33.333333%")]),
    ("w-1/4", &[("width", "25%")]),
    ("w-2/3", &[("width", "66.666667%")]),
    ("w-full", &[("width", "100%")]),
    ("whitespace-nowrap", &[("white-space", "nowrap")]),
];
// table:end

/// Scaled families `<prefix>-<step>` (step N = N x 4px, `px` = 1px) and `<prefix>-[<length>]`.
/// The bool says whether the target properties accept percentages in arbitrary values.
const SCALED: &[(&str, &[&str], bool)] = &[
    ("gap", &["gap"], false),
    ("h", &["height"], true),
    ("m", &["margin"], false),
    ("mb", &["margin-bottom"], false),
    ("ml", &["margin-left"], false),
    ("mr", &["margin-right"], false),
    ("mt", &["margin-top"], false),
    ("mx", &["margin-left", "margin-right"], false),
    ("my", &["margin-top", "margin-bottom"], false),
    ("p", &["padding"], false),
    ("pb", &["padding-bottom"], false),
    ("pl", &["padding-left"], false),
    ("pr", &["padding-right"], false),
    ("pt", &["padding-top"], false),
    ("px", &["padding-left", "padding-right"], false),
    ("py", &["padding-top", "padding-bottom"], false),
    ("w", &["width"], true),
];

/// A resolved token: its state (`hover:` etc., `Base` when none) and what it lowers to.
pub(crate) struct Resolved<'a> {
    state: PseudoState,
    body: Body<'a>,
}

enum Body<'a> {
    Fixed(&'static [(&'static str, &'static str)]),
    Scaled(&'static [&'static str], Value<'a>),
}

enum Value<'a> {
    Px(u32),
    Raw(&'a str),
}

impl Resolved<'_> {
    fn declarations(&self) -> Vec<Declaration> {
        let declaration = |name: &str, value: &str| Declaration {
            name: name.to_string(),
            value: value.to_string(),
        };
        match &self.body {
            Body::Fixed(pairs) => pairs
                .iter()
                .map(|(name, value)| declaration(name, value))
                .collect(),
            Body::Scaled(properties, value) => {
                let text = match value {
                    Value::Px(px) => format!("{px}px"),
                    Value::Raw(raw) => raw.to_string(),
                };
                properties
                    .iter()
                    .map(|name| declaration(name, &text))
                    .collect()
            }
        }
    }
}

/// One class token. `None` for anything that is not a utility, including invalid values and unknown variants.
fn lookup(token: &str) -> Option<Resolved<'_>> {
    let (state, base) = match token.split_once(':') {
        None => (PseudoState::Base, token),
        Some(("hover", base)) => (PseudoState::Hover, base),
        Some(("focus", base)) => (PseudoState::Focus, base),
        Some(("active", base)) => (PseudoState::Active, base),
        Some(_) => return None,
    };
    Some(Resolved {
        state,
        body: lookup_base(base)?,
    })
}

fn lookup_base(name: &str) -> Option<Body<'_>> {
    if let Ok(index) = UTILITIES.binary_search_by_key(&name, |&(entry, _)| entry) {
        return Some(Body::Fixed(UTILITIES[index].1));
    }
    let (prefix, rest) = name.split_once('-')?;
    let &(_, properties, percent) = SCALED.iter().find(|(family, _, _)| *family == prefix)?;
    let value = match rest
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
    {
        Some(inner) => {
            if !valid_length(inner, percent) {
                return None;
            }
            Value::Raw(inner)
        }
        None => Value::Px(scale_px(rest)?),
    };
    Some(Body::Scaled(properties, value))
}

/// Spacing step: `px` is 1px, `N` is N x 4px, `N.5` adds 2px, N in 0..=12.
fn scale_px(step: &str) -> Option<u32> {
    if step == "px" {
        return Some(1);
    }
    let (whole, half) = match step.strip_suffix(".5") {
        Some(whole) => (whole, 2),
        None => (step, 0),
    };
    if whole.is_empty() || !whole.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let steps: u32 = whole.parse().ok()?;
    (steps <= 12).then_some(steps * 4 + half)
}

/// Arbitrary length: a `var(--token)` (resolved and checked later by the declaration path) or a
/// length the target property's existing parser accepts.
fn valid_length(inner: &str, percent: bool) -> bool {
    if inner.starts_with("var(--") && inner.ends_with(')') {
        return true;
    }
    if percent {
        parse_length(inner).is_ok()
    } else {
        parse_px(inner).is_ok()
    }
}

/// Synthetic rules for the utility tokens one document uses: one per distinct token, in name order.
/// Rules start at `first_order`, after every linked sheet, so a utility overrides a component class.
pub(crate) fn rules_for(html: &ParsedHtml, first_order: u32) -> Vec<Rule> {
    let mut used: Vec<&str> = html
        .nodes
        .iter()
        .filter_map(|node| node.attrs.get("class"))
        .flat_map(|class| class.split_ascii_whitespace())
        .filter(|token| lookup(token).is_some())
        .collect();
    used.sort_unstable();
    used.dedup();
    used.iter()
        .zip(first_order..)
        .filter_map(|(token, order)| {
            let resolved = lookup(token)?;
            Some(class_rule(
                token.to_string(),
                resolved.state,
                order,
                resolved.declarations(),
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::{compile_parsed, compile_str, CompileOptions, CompileOutput};
    use crate::html::parse_html;
    use flamewm_render_core::encode;
    use std::fmt::Write as _;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    /// Markdown rows for the spec's generated table.
    fn generated_table() -> String {
        let mut table = String::from("| Class | Declarations |\n|---|---|\n");
        for (name, pairs) in UTILITIES {
            let declarations: Vec<String> = pairs
                .iter()
                .map(|(property, value)| format!("`{property}: {value}`"))
                .collect();
            writeln!(table, "| `{name}` | {} |", declarations.join("; ")).unwrap();
        }
        table
    }

    fn ui_dir() -> PathBuf {
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../ui")).to_path_buf()
    }

    /// Token lengths plus the colour tokens the fixtures use (`--text`, `--muted`, `--rim` are per-sheet).
    fn fixture_css() -> String {
        let tokens = fs::read_to_string(ui_dir().join("base/tokens.css")).expect("read tokens");
        format!("{tokens}\n:root {{ --text: #f5f5f7; --muted: #a1a1a6; --rim: #3a3a3c; }}")
    }

    fn compile_body(body: &str, css: &str) -> Result<CompileOutput, String> {
        compile_str(
            &format!("<body>{body}</body>"),
            css,
            CompileOptions::default(),
        )
    }

    /// Encoded document with the source fingerprint zeroed (it hashes the source text, which differs by design).
    fn encoded_without_fingerprint(output: &CompileOutput) -> Vec<u8> {
        let mut bytes = encode(&output.document).expect("encode document");
        bytes[8..16].fill(0);
        bytes
    }

    fn token_count(body: &str) -> usize {
        let document = parse_html(&format!("<body>{body}</body>")).expect("parse fixture");
        rules_for(&document, 0).len()
    }

    #[test]
    fn table_is_sorted_and_unique() {
        assert!(UTILITIES.windows(2).all(|pair| pair[0].0 < pair[1].0));
    }

    #[test]
    fn every_entry_resolves_and_compiles_in_every_state() {
        let mut body = String::new();
        for (name, pairs) in UTILITIES {
            let resolved = lookup(name).unwrap_or_else(|| panic!("{name} does not resolve"));
            assert_eq!(resolved.state, PseudoState::Base);
            assert_eq!(resolved.declarations().len(), pairs.len(), "{name}");
            for prefix in ["", "hover:", "focus:", "active:"] {
                write!(body, r#"<div class="{prefix}{name}"></div>"#).unwrap();
            }
        }
        for (family, _, _) in SCALED {
            for step in [
                "0",
                "1",
                "12",
                "1.5",
                "px",
                "[10px]",
                "[50%]",
                "[var(--space-md)]",
            ] {
                write!(body, r#"<div class="{family}-{step}"></div>"#).unwrap();
            }
        }
        compile_body(&body, &fixture_css())
            .unwrap_or_else(|error| panic!("utility table rejected by compiler: {error}"));
    }

    #[test]
    fn used_tokens_only_emit_rules() {
        assert_eq!(token_count(r#"<div class="flex p-4"></div>"#), 2);
        assert_eq!(
            token_count(r#"<div class="flex flex"><i class="flex"></i></div>"#),
            1
        );
        assert_eq!(token_count(r#"<div class="btn panel"></div>"#), 0);
        assert_eq!(token_count("<div>text</div>"), 0);
    }

    #[test]
    fn utility_document_matches_hand_written_css() {
        let css = fixture_css();
        let utilities = compile_body(
            r#"<div class="flex items-center gap-2 p-4 w-[240px] text-md bg-accent-soft hover:bg-accent">x</div>"#,
            &css,
        )
        .expect("utilities compile");
        let hand_written = compile_body(
            r#"<div class="card">x</div>"#,
            &format!(
                "{css}\n.card {{ display: flex; align-items: center; gap: 8px; padding: 16px; width: 240px; font-size: var(--font-md); background: var(--accent-soft); }}\n.card:hover {{ background: var(--accent); }}"
            ),
        )
        .expect("hand-written CSS compiles");
        assert_eq!(
            encoded_without_fingerprint(&utilities),
            encoded_without_fingerprint(&hand_written)
        );
    }

    #[test]
    fn collisions_with_component_classes_are_reported() {
        let mut offenders = Vec::new();
        let mut sheets = Vec::new();
        collect_css(&ui_dir(), &mut sheets);
        assert!(!sheets.is_empty(), "no UI stylesheets found");
        for sheet in &sheets {
            let text = fs::read_to_string(sheet).expect("read sheet");
            for class in selector_classes(&text) {
                if lookup(&class).is_some() {
                    offenders.push(format!("{}: .{class}", sheet.display()));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "UI selectors define utility class names (rename the component class, do not change the utility):\n{}",
            offenders.join("\n")
        );
    }

    fn collect_css(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("read ui dir") {
            let path = entry.expect("ui entry").path();
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name != "assets") {
                    collect_css(&path, out);
                }
            } else if path.extension().is_some_and(|ext| ext == "css") {
                out.push(path);
            }
        }
    }

    /// Class names in selector preludes: text before each `{` since the last `}` or `;`.
    fn selector_classes(css: &str) -> Vec<String> {
        let mut names = Vec::new();
        let mut prelude_start = 0;
        for (index, ch) in css.char_indices() {
            match ch {
                '{' => {
                    for part in css[prelude_start..index].split('.').skip(1) {
                        let name = part
                            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
                            .next()
                            .unwrap_or("");
                        if !name.is_empty() {
                            names.push(name.to_string());
                        }
                    }
                }
                '}' | ';' => prelude_start = index + 1,
                _ => {}
            }
        }
        names
    }

    #[test]
    fn spec_table_matches_the_const_table() {
        let spec =
            fs::read_to_string(ui_dir().join("../docs/UI_UTILITIES_SPEC.md")).expect("read spec");
        let (begin, end) = ("<!-- utilities:begin -->", "<!-- utilities:end -->");
        let start = spec.find(begin).expect("begin marker") + begin.len();
        let stop = spec.find(end).expect("end marker");
        assert_eq!(spec[start..stop].trim(), generated_table().trim());
    }

    #[test]
    fn unknown_and_invalid_tokens_are_ignored_without_panic() {
        for token in [
            "",
            "-",
            "-4",
            "p-",
            "p-13",
            "p-1.25",
            "p-[50%]",
            "w-[banana]",
            "w-[]",
            "w-[",
            "p-[10px",
            "disabled:flex",
            "hover:",
            "hover:hover:flex",
            "flex:hover",
            "w-1/5",
            "foo-bar",
            "[",
            "é-4",
        ] {
            assert!(lookup(token).is_none(), "{token:?} should be ignored");
        }
        assert!(lookup("w-[240px]").is_some());
        assert!(lookup("w-[50%]").is_some());
        assert!(lookup("gap-[var(--space-md)]").is_some());
        assert!(lookup("hover:bg-accent-soft").is_some());
    }

    /// Test-only timing harness: median compile time per UI page with the same inputs as `compile_file`.
    /// Run: `cargo test -p flamewm-render-compiler ui_compile_timing -- --ignored --nocapture`.
    #[test]
    #[ignore = "timing harness, run explicitly"]
    fn ui_compile_timing() {
        let mut pages = Vec::new();
        html_pages(&ui_dir(), &mut pages);
        for page in pages {
            let source = fs::read_to_string(&page).expect("read page");
            let html = parse_html(&source).expect("parse page");
            let base = page.parent().expect("page parent");
            let mut css = crate::imports::expand_imports(&page, &html.embedded_css).expect("css");
            for href in &html.linked_css {
                let path = base.join(href);
                css.push('\n');
                let text = fs::read_to_string(&path).expect("read sheet");
                css.push_str(&crate::imports::expand_imports(&path, &text).expect("expand sheet"));
            }
            let uses = html.nodes.iter().any(|node| {
                node.attrs.get("class").is_some_and(|class| {
                    class.split_ascii_whitespace().any(|t| lookup(t).is_some())
                })
            });
            // Interleaved per iteration, so machine load hits the compile and the hook alike.
            let (mut compile, mut hook) = (Vec::new(), Vec::new());
            for _ in 0..200 {
                let start = Instant::now();
                let parsed = parse_html(&source).expect("parse page");
                let result =
                    compile_parsed(&source, &css, parsed, CompileOptions::default(), Some(base));
                compile.push(start.elapsed().as_nanos());
                if let Err(error) = result {
                    if !error.starts_with("failed to read image") {
                        panic!("{}: {error}", page.display());
                    }
                }
                let parsed = parse_html(&source).expect("parse page");
                let start = Instant::now();
                std::hint::black_box(rules_for(&parsed, 0));
                hook.push(start.elapsed().as_nanos());
            }
            compile.sort_unstable();
            hook.sort_unstable();
            println!(
                "TIMING\t{}\t{}\t{}\t{}",
                page.display(),
                compile[100],
                hook[100],
                uses
            );
        }
    }

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
}

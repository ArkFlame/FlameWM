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

/// Longest token considered a utility. Longer tokens are component classes and are never looked up.
const MAX_TOKEN_LEN: usize = 128;

/// Variant prefixes accepted by `lookup`. Kept in step with its match arms (checked by a test).
const VARIANTS: [&str; 3] = ["hover", "focus", "active"];

/// Hashed three-byte prefixes (16-bit key, 65536 bits): every prefix a utility token can start with.
/// Fixed names and variants contribute their first three bytes. A one-letter scaled family `h` contributes
/// `h-` followed by each step, bracket or `px` start; a longer family contributes its first three bytes.
const TRIGRAMS: [u64; 1024] = trigrams();

/// Tokens whose first three bytes are not marked here cannot be utilities.
const SCALED_NEXT: &[u8] = b"0123456789[p";

const fn trigram_key(a: u8, b: u8, c: u8) -> usize {
    let mixed = ((a as u32) << 16 | (b as u32) << 8 | c as u32).wrapping_mul(0x9E37_79B1);
    (mixed >> 16) as usize
}

const fn set_trigram(mask: &mut [u64; 1024], key: usize) {
    mask[key >> 6] |= 1u64 << (key & 63);
}

const fn trigrams() -> [u64; 1024] {
    let mut mask = [0u64; 1024];
    let mut i = 0;
    while i < UTILITIES.len() {
        let bytes = UTILITIES[i].0.as_bytes();
        assert!(bytes.len() >= 3, "utility names are at least three bytes");
        set_trigram(&mut mask, trigram_key(bytes[0], bytes[1], bytes[2]));
        i += 1;
    }
    i = 0;
    while i < VARIANTS.len() {
        let bytes = VARIANTS[i].as_bytes();
        set_trigram(&mut mask, trigram_key(bytes[0], bytes[1], bytes[2]));
        i += 1;
    }
    i = 0;
    while i < SCALED.len() {
        let bytes = SCALED[i].0.as_bytes();
        if bytes.len() == 1 {
            let mut next = 0;
            while next < SCALED_NEXT.len() {
                set_trigram(&mut mask, trigram_key(bytes[0], b'-', SCALED_NEXT[next]));
                next += 1;
            }
        } else if bytes.len() == 2 {
            set_trigram(&mut mask, trigram_key(bytes[0], bytes[1], b'-'));
        } else {
            set_trigram(&mut mask, trigram_key(bytes[0], bytes[1], bytes[2]));
        }
        i += 1;
    }
    mask
}

/// Cheap reject before `lookup`: length bound, then the hashed three-byte mask. Never rejects a token `lookup` accepts.
fn is_candidate(token: &str) -> bool {
    let bytes = token.as_bytes();
    if bytes.len() < 3 || bytes.len() > MAX_TOKEN_LEN {
        return false;
    }
    let key = trigram_key(bytes[0], bytes[1], bytes[2]);
    TRIGRAMS[key >> 6] & (1u64 << (key & 63)) != 0
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0100_0000_01b3;

const fn fnv_bytes(mut hash: u64, bytes: &[u8]) -> u64 {
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
        i += 1;
    }
    hash
}

/// FNV-1a over every name, declaration and scaled-family field, with separators so field edits cannot collide.
const fn table_hash(fixed: &[(&str, &[(&str, &str)])], scaled: &[(&str, &[&str], bool)]) -> u64 {
    let mut hash = FNV_OFFSET;
    let mut i = 0;
    while i < fixed.len() {
        let (name, pairs) = fixed[i];
        hash = fnv_bytes(hash, name.as_bytes());
        hash = fnv_bytes(hash, &[0u8]);
        let mut j = 0;
        while j < pairs.len() {
            let (property, value) = pairs[j];
            hash = fnv_bytes(hash, property.as_bytes());
            hash = fnv_bytes(hash, &[0u8]);
            hash = fnv_bytes(hash, value.as_bytes());
            hash = fnv_bytes(hash, &[0u8]);
            j += 1;
        }
        hash = fnv_bytes(hash, &[1u8]);
        i += 1;
    }
    i = 0;
    while i < scaled.len() {
        let (family, properties, percent) = scaled[i];
        hash = fnv_bytes(hash, family.as_bytes());
        hash = fnv_bytes(hash, &[0u8]);
        let mut j = 0;
        while j < properties.len() {
            hash = fnv_bytes(hash, properties[j].as_bytes());
            hash = fnv_bytes(hash, &[0u8]);
            j += 1;
        }
        hash = fnv_bytes(hash, &[percent as u8, 1u8]);
        i += 1;
    }
    hash
}

/// Changes whenever the table changes. Part of the compile cache key, so an edit cannot serve stale output.
pub(crate) const UTILITIES_REVISION: u64 = table_hash(UTILITIES, SCALED);

/// Synthetic rules for the utility tokens one document uses: one per distinct token, in name order.
/// Rules start at `first_order`, after every linked sheet, so a utility overrides a component class.
/// Candidates are filtered cheaply, then deduplicated, so `lookup` runs once per distinct token.
pub(crate) fn rules_for(html: &ParsedHtml, first_order: u32) -> Vec<Rule> {
    // Most tokens are component classes: the two-byte mask rejects them with no lookup. Survivors are
    // looked up in place; only the (few) matches are sorted, so the common page never sorts its tokens.
    let mut used: Vec<&str> = Vec::new();
    for node in &html.nodes {
        let Some(class) = node.attrs.get("class") else {
            continue;
        };
        for token in class.split_ascii_whitespace() {
            if is_candidate(token) && lookup(token).is_some() {
                used.push(token);
            }
        }
    }
    if used.is_empty() {
        return Vec::new();
    }
    used.sort_unstable();
    used.dedup();
    let mut rules = Vec::new();
    let mut order = first_order;
    for token in used {
        if let Some(resolved) = lookup(token) {
            rules.push(class_rule(
                token.to_string(),
                resolved.state,
                order,
                resolved.declarations(),
            ));
            order += 1;
        }
    }
    rules
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

    /// FNV-1a 64 over the encoded document (source fingerprint zeroed), so the golden is one number.
    fn fnv1a(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
        })
    }

    /// Encoded length and FNV-1a of one UI page compiled the way `compile_file` links it.
    /// `without_images` drops `<img>` tags: the branding assets are not in the source tree, and images do not
    /// take part in the class migration being checked (both sides of the check strip them the same way).
    fn compiled_page_digest(page: &Path, without_images: bool) -> (usize, u64) {
        let mut source = fs::read_to_string(page).expect("read page");
        if without_images {
            while let Some(start) = source.find("<img") {
                let end = start + source[start..].find('>').expect("img end") + 1;
                source.replace_range(start..end, "");
            }
        }
        let html = parse_html(&source).expect("parse page");
        let base = page.parent().expect("page parent");
        let mut css = crate::imports::expand_imports(page, &html.embedded_css).expect("css");
        for href in &html.linked_css {
            let path = base.join(href);
            css.push('\n');
            let text = fs::read_to_string(&path).expect("read sheet");
            css.push_str(&crate::imports::expand_imports(&path, &text).expect("expand sheet"));
        }
        let output = compile_parsed(
            &source,
            &css,
            parse_html(&source).expect("parse page"),
            CompileOptions::default(),
            Some(base),
        )
        .expect("compile page");
        let bytes = encoded_without_fingerprint(&output);
        (bytes.len(), fnv1a(&bytes))
    }

    /// Migrating dock slot classes to utilities must not change the compiled dock document.
    /// Golden recorded from the pre-migration dock (dock slot classes reverted after 2e41bbe4 regressed compile
    /// time). Re-pinned from 284001 bytes once the shared shell styles (a0a0034b) changed the compiled output.
    /// Update only for an intended dock change.
    #[test]
    fn dock_document_is_unchanged_by_utility_migration() {
        let page = ui_dir().join("shell/dock.html");
        assert_eq!(
            compiled_page_digest(&page, false),
            (284_218, 0xc81f_3439_2812_9276)
        );
    }

    /// Settings hotkeys section: row and key classes moved to utilities; the compiled page must not change.
    /// Re-pinned from 785_753 bytes: ARK AI branding (b8f6e15f) renamed the Assistant page labels (785_726 bytes),
    /// then the Dock page and sidebar scroll wrapper (58f66676) added its markup and styles (827_242 bytes).
    /// Update only for an intended settings page change.
    /// Re-pinned 0xdf93_9260_dd67_2338 -> 0xc575_3a91_ac46_57f8 for the --muted unification: settings.css
    /// text colour #aeb9cc -> #aeb4bb, now from ui/base/base.css. Byte count unchanged.
    /// Re-pinned 827_242 / 0xc575_3a91_ac46_57f8 -> 828_052 / 0x4784_cecd_88bd_f6ea for the intended
    /// settings slider-track focus ring (`.slider-track:focus-visible` in ui/base/base.css).
    /// Re-pinned 828_052 / 0x4784_cecd_88bd_f6ea -> 828_456 / 0xb788_7d9f_bc6a_259a (+404 bytes) for the
    /// Re-pinned 828_456 -> 838_838 for the settings usage rows added by d98649e5 (settings.html).
    /// Re-pinned 838_838 -> 845_055 for the settings primary and rotation rows added by 00c067b1 (settings.html).
    /// intended sidebar nav-row focus ring (`.settings-nav .nav-row:focus-visible` in ui/base/base.css).
    #[test]
    fn settings_hotkeys_document_is_unchanged_by_utility_migration() {
        let page = ui_dir().join("settings.html");
        assert_eq!(
            compiled_page_digest(&page, true),
            (845_055, 0x81e8_93fd_cc87_db4b)
        );
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

#[cfg(test)]
mod prefilter_and_revision_tests {
    use super::*;

    #[test]
    fn prefilter_never_rejects_a_token_lookup_accepts() {
        let mut samples: Vec<String> = Vec::new();
        for (name, _) in UTILITIES {
            samples.push(name.to_string());
            for variant in VARIANTS {
                samples.push(format!("{variant}:{name}"));
            }
        }
        for (family, _, _) in SCALED {
            for step in ["0", "12", "2.5", "px", "[10px]", "[var(--x)]"] {
                samples.push(format!("{family}-{step}"));
                samples.push(format!("hover:{family}-{step}"));
            }
        }
        let accepted: Vec<&String> = samples
            .iter()
            .filter(|token| lookup(token).is_some())
            .collect();
        assert!(accepted.len() > UTILITIES.len());
        for token in accepted {
            assert!(
                is_candidate(token),
                "{token} is accepted by lookup but rejected by the prefilter"
            );
        }
    }

    #[test]
    fn variants_are_exactly_the_lookup_prefixes() {
        for variant in VARIANTS {
            assert!(lookup(&format!("{variant}:flex")).is_some(), "{variant}");
        }
        assert!(lookup("active:flex").is_some());
        assert!(lookup("visited:flex").is_none());
    }

    #[test]
    fn prefilter_rejects_empty_long_and_unmasked_tokens() {
        assert!(!is_candidate(""));
        assert!(!is_candidate("x"));
        assert!(UTILITIES.iter().all(|(name, _)| name.len() >= 3));
        assert!(!is_candidate(&format!("w-[{}]", "1".repeat(MAX_TOKEN_LEN))));
        assert!(!is_candidate("Zebra"));
        assert!(is_candidate("flex"));
    }

    #[test]
    fn revision_matches_the_table_and_changes_with_each_field() {
        let base = table_hash(UTILITIES, SCALED);
        assert_eq!(base, UTILITIES_REVISION);
        assert_ne!(base, table_hash(&UTILITIES[1..], SCALED));
        assert_ne!(base, table_hash(UTILITIES, &SCALED[1..]));
        let renamed: &[(&str, &[(&str, &str)])] = &[("flex", &[("display", "grid")])];
        assert_ne!(
            table_hash(renamed, &[]),
            table_hash(&[("flex", &[("display", "flex")])], &[])
        );
        let percent: &[(&str, &[&str], bool)] = &[("w", &["width"], false)];
        assert_ne!(
            table_hash(&[], percent),
            table_hash(&[], &[("w", &["width"], true)])
        );
    }
}

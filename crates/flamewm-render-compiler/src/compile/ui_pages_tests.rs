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
        let mut source = expand_imports(&base, &fs::read_to_string(&base).expect("read base CSS"))
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
                resolve_length_vars(declaration.value.trim(), &lengths).unwrap_or_else(|error| {
                    panic!("{}: {}: {error}", sheet.display(), declaration.name)
                });
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

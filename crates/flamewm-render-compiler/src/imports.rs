//! Compile-time `@import "partial.css";` for FlameWM Render stylesheets. A partial is inlined in
//! place of its statement. Paths resolve relative to the importing file and must be relative local
//! `.css` paths. The import graph must be acyclic. Each partial is inlined once per top-level sheet.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::css::strip_comments;

const IMPORT_KEYWORD: &str = "@import";

/// Expands every `@import` in `source`, the text of `file`, into one flat stylesheet.
pub fn expand_imports(file: &Path, source: &str) -> Result<String, String> {
    let root = canonical(file)?;
    let mut expander = Expander {
        stack: vec![root.clone()],
        seen: HashSet::from([root]),
        output: String::with_capacity(source.len()),
    };
    expander.expand(file, source)?;
    Ok(expander.output)
}

struct Expander {
    /// Files currently being expanded, outermost first. Re-entering one is a cycle.
    stack: Vec<PathBuf>,
    /// Every file already inlined into this sheet. Repeats are skipped.
    seen: HashSet<PathBuf>,
    output: String,
}

impl Expander {
    fn expand(&mut self, file: &Path, source: &str) -> Result<(), String> {
        let text = strip_comments(source)?;
        let mut cursor = 0usize;
        while let Some(relative) = text[cursor..].find(IMPORT_KEYWORD) {
            let at = cursor + relative;
            self.output.push_str(&text[cursor..at]);
            let (target, end) = parse_import(&text, at)?;
            cursor = end;
            let path = file.parent().unwrap_or(Path::new(".")).join(target);
            let resolved = fs::canonicalize(&path).map_err(|error| {
                format!(
                    "@import '{target}' in {} failed: {}: {error}",
                    file.display(),
                    path.display()
                )
            })?;
            if self.stack.contains(&resolved) {
                return Err(format!(
                    "@import cycle: '{target}' in {} imports one of its own ancestors",
                    file.display()
                ));
            }
            if !self.seen.insert(resolved.clone()) {
                continue;
            }
            let partial = fs::read_to_string(&resolved).map_err(|error| {
                format!("failed to read @import {}: {error}", resolved.display())
            })?;
            self.stack.push(resolved.clone());
            self.expand(&resolved, &partial)?;
            self.stack.pop();
        }
        self.output.push_str(&text[cursor..]);
        Ok(())
    }
}

fn canonical(path: &Path) -> Result<PathBuf, String> {
    fs::canonicalize(path).map_err(|error| format!("failed to read {}: {error}", path.display()))
}

/// Parses `@import "<path>";` starting at `at`. Returns the path and the index after the `;`.
fn parse_import(text: &str, at: usize) -> Result<(&str, usize), String> {
    let after_keyword = at + IMPORT_KEYWORD.len();
    let start = skip_ws(text, after_keyword);
    let quote = text[start..].chars().next();
    let (Some(quote @ ('"' | '\'')), true) = (quote, start > after_keyword) else {
        return Err("@import expects a quoted path, e.g. @import \"partial.css\";".to_string());
    };
    let name_start = start + 1;
    let name_end = text[name_start..]
        .find(quote)
        .map(|offset| name_start + offset)
        .ok_or_else(|| "unterminated @import path".to_string())?;
    let target = &text[name_start..name_end];
    validate_target(target)?;
    let semicolon = skip_ws(text, name_end + 1);
    if !text[semicolon..].starts_with(';') {
        return Err(format!("@import '{target}' must end with ';'"));
    }
    Ok((target, semicolon + 1))
}

fn validate_target(target: &str) -> Result<(), String> {
    if target.is_empty() {
        return Err("@import path is empty".to_string());
    }
    if Path::new(target).is_absolute() || target.contains("://") || target.starts_with("data:") {
        return Err(format!(
            "@import '{target}' must be a relative local path, not an absolute or network URL"
        ));
    }
    if !target.ends_with(".css") {
        return Err(format!("@import '{target}' must name a .css partial"));
    }
    Ok(())
}

fn skip_ws(text: &str, mut cursor: usize) -> usize {
    while cursor < text.len() && text.as_bytes()[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    cursor
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fresh per-test directory under the system temp dir; tests never share a path.
    fn fixture_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("flamewm-imports-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("parts")).expect("create fixture dir");
        dir
    }

    fn write(path: &Path, text: &str) {
        fs::write(path, text).expect("write fixture");
    }

    #[test]
    fn inlines_partial_relative_to_importing_file_and_nests() {
        let dir = fixture_dir("nest");
        write(
            &dir.join("main.css"),
            "@import \"parts/a.css\";\n.main { color: red; }",
        );
        write(
            &dir.join("parts/a.css"),
            "@import \"b.css\";\n.a { width: 1px; }",
        );
        write(&dir.join("parts/b.css"), ".b { height: 2px; }");
        let main = dir.join("main.css");
        let out = expand_imports(&main, &fs::read_to_string(&main).unwrap()).unwrap();
        assert!(out.contains(".b { height: 2px; }"));
        assert!(out.contains(".a { width: 1px; }"));
        assert!(out.contains(".main { color: red; }"));
        assert!(!out.contains("@import"));
    }

    #[test]
    fn repeated_partial_is_inlined_once() {
        let dir = fixture_dir("dedupe");
        write(
            &dir.join("main.css"),
            "@import \"parts/p.css\";\n@import \"parts/p.css\";",
        );
        write(&dir.join("parts/p.css"), ":root { --marker: 1px; }");
        let main = dir.join("main.css");
        let out = expand_imports(&main, &fs::read_to_string(&main).unwrap()).unwrap();
        assert_eq!(out.matches("--marker").count(), 1);
    }

    #[test]
    fn cycle_is_rejected_including_self_import() {
        let dir = fixture_dir("cycle");
        write(&dir.join("main.css"), "@import \"parts/x.css\";");
        write(&dir.join("parts/x.css"), "@import \"../main.css\";");
        let main = dir.join("main.css");
        let error = expand_imports(&main, &fs::read_to_string(&main).unwrap()).unwrap_err();
        assert!(error.contains("cycle"), "{error}");

        write(&dir.join("self.css"), "@import \"self.css\";");
        let this = dir.join("self.css");
        let error = expand_imports(&this, &fs::read_to_string(&this).unwrap()).unwrap_err();
        assert!(error.contains("cycle"), "{error}");
    }

    #[test]
    fn missing_partial_and_malformed_statements_are_rejected() {
        let dir = fixture_dir("errors");
        let main = dir.join("main.css");
        for source in [
            "@import \"missing.css\";",
            "@import parts/p.css;",
            "@import \"parts/p.css\"",
            "@import \"parts/p.css;",
            "@import \"/etc/passwd.css\";",
            "@import \"https://example.com/x.css\";",
            "@import \"data:text/css,x\";",
            "@import \"parts/p.txt\";",
            "@import \"\";",
        ] {
            write(&main, source);
            assert!(
                expand_imports(&main, source).is_err(),
                "{source} must be rejected"
            );
        }
    }

    #[test]
    fn import_inside_comment_is_not_expanded() {
        let dir = fixture_dir("comment");
        let main = dir.join("main.css");
        write(&main, "");
        let source = "/* @import \"missing.css\"; */ .a { width: 1px; }";
        let out = expand_imports(&main, source).unwrap();
        assert!(out.contains(".a { width: 1px; }"));
    }
}

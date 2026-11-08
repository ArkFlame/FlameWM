//! Recently used documents from the desktop's `recently-used.xbel`, newest use first. The file is
//! read once per launch; a document only appears when it is also in the file index.

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

/// Most local paths kept from the file.
const MAX_DOCS: usize = 200;

/// `$XDG_DATA_HOME/recently-used.xbel`, or the default under `$HOME/.local/share`.
#[must_use]
pub fn path(data_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    non_empty(data_home)
        .map(|dir| PathBuf::from(dir).join("recently-used.xbel"))
        .or_else(|| {
            non_empty(home).map(|dir| PathBuf::from(dir).join(".local/share/recently-used.xbel"))
        })
}

fn non_empty(value: Option<OsString>) -> Option<OsString> {
    value.filter(|value| !value.is_empty())
}

/// Local paths listed in `path`, newest use first. A missing or unreadable file lists nothing.
#[must_use]
pub fn load(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .map(|text| parse(&text))
        .unwrap_or_default()
}

/// Local paths from the bookmarks in `text`, newest use first, without duplicates.
#[must_use]
pub fn parse(text: &str) -> Vec<String> {
    let mut found: Vec<(String, String)> = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("<bookmark ") {
        rest = &rest[start..];
        let Some(end) = rest.find('>') else {
            break;
        };
        let tag = &rest[..end];
        let modified = attribute(tag, "modified").unwrap_or_default();
        if let Some(path) = attribute(tag, "href").and_then(|uri| file_path(&uri)) {
            found.push((modified, path));
        }
        rest = &rest[end..];
    }
    // ISO 8601 timestamps order correctly as text, so the newest use sorts first.
    found.sort_by(|a, b| b.0.cmp(&a.0));
    let mut seen = HashSet::new();
    found
        .into_iter()
        .map(|(_, path)| path)
        .filter(|path| seen.insert(path.clone()))
        .take(MAX_DOCS)
        .collect()
}

/// The value of `name="..."` inside one tag, with the XML entities the file uses decoded.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let key = format!(" {name}=\"");
    let start = tag.find(&key)? + key.len();
    let value = &tag[start..];
    let raw = &value[..value.find('"')?];
    Some(
        raw.replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&"),
    )
}

/// The local path of a `file:///...` URI. Other schemes and remote hosts give `None`.
fn file_path(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    if !rest.starts_with('/') {
        return None;
    }
    String::from_utf8(percent_decode(rest)?).ok()
}

fn percent_decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xbel version="1.0">
  <bookmark href="file:///home/u/Docs/old%20notes.txt" added="2026-01-01T00:00:00Z" modified="2026-01-02T10:00:00Z" visited="2026-01-02T10:00:00Z"/>
  <bookmark href="https://example.com/page" modified="2026-03-01T00:00:00Z"/>
  <bookmark href="file:///home/u/report.pdf" modified="2026-05-01T08:30:00Z"/>
  <bookmark href="file:///home/u/report.pdf" modified="2026-02-01T00:00:00Z"/>
  <bookmark href="file://remote-host/share/x.txt" modified="2026-06-01T00:00:00Z"/>
  <bookmark href="file:///home/u/a&amp;b.md" modified="2026-04-01T00:00:00Z"/>
</xbel>
"#;

    #[test]
    fn lists_local_paths_newest_use_first_without_duplicates() {
        assert_eq!(
            parse(SAMPLE),
            vec![
                "/home/u/report.pdf".to_owned(),
                "/home/u/a&b.md".to_owned(),
                "/home/u/Docs/old notes.txt".to_owned(),
            ]
        );
    }

    #[test]
    fn missing_or_malformed_input_lists_nothing() {
        assert!(parse("").is_empty());
        assert!(parse("<bookmark href=\"file:///x").is_empty());
        assert!(load(Path::new("/nonexistent/recently-used.xbel")).is_empty());
    }

    #[test]
    fn bad_percent_escapes_drop_only_that_entry() {
        let text = "<bookmark href=\"file:///ok.txt\" modified=\"2026-01-01\"/>\
                    <bookmark href=\"file:///bad%ZZ.txt\" modified=\"2026-02-01\"/>";
        assert_eq!(parse(text), vec!["/ok.txt".to_owned()]);
    }

    #[test]
    fn path_follows_xdg_data_home_then_home() {
        assert_eq!(
            path(Some("/data".into()), Some("/home/u".into())),
            Some(PathBuf::from("/data/recently-used.xbel"))
        );
        assert_eq!(
            path(Some("".into()), Some("/home/u".into())),
            Some(PathBuf::from("/home/u/.local/share/recently-used.xbel"))
        );
        assert_eq!(path(None, None), None);
    }
}

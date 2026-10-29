//! Quick-look content for the preview overlay. Only the first bytes of a text file are read, so a
//! preview costs the same for a 1 KB note and a 10 GB log.

use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Lines shown in the preview.
pub const PREVIEW_LINES: usize = 60;
const READ_LIMIT: u64 = 16 * 1024;
const TEXT_EXTENSIONS: [&str; 24] = [
    "txt", "md", "log", "csv", "json", "toml", "yaml", "yml", "xml", "html", "css", "js", "jsx",
    "ts", "tsx", "rs", "py", "go", "sh", "sql", "conf", "ini", "env", "rst",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Preview {
    Text(Vec<String>),
    /// Nothing readable to show; the overlay shows the type mark and the name.
    Unavailable,
}

/// True when the name's extension is one the preview reads as text.
#[must_use]
pub fn is_text_name(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty() && TEXT_EXTENSIONS.contains(&ext.to_lowercase().as_str())
    })
}

/// Loads the preview for `path`. Directories and binary or unreadable files are unavailable.
#[must_use]
pub fn load(path: &Path, is_dir: bool) -> Preview {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if is_dir || !is_text_name(name) {
        return Preview::Unavailable;
    }
    let Ok(file) = File::open(path) else {
        return Preview::Unavailable;
    };
    let mut bytes = Vec::new();
    if file.take(READ_LIMIT).read_to_end(&mut bytes).is_err() {
        return Preview::Unavailable;
    }
    text_lines(&bytes).map_or(Preview::Unavailable, Preview::Text)
}

/// Lines of `bytes` when it is text without NUL bytes. A read that ended inside a multi-byte
/// character keeps the valid prefix; invalid bytes in the middle reject the file.
#[must_use]
pub fn text_lines(bytes: &[u8]) -> Option<Vec<String>> {
    if bytes.contains(&0) {
        return None;
    }
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => {
            // Keep the valid prefix; the read may have ended inside a character.
            if error.error_len().is_some() {
                return None;
            }
            std::str::from_utf8(&bytes[..error.valid_up_to()]).ok()?
        }
    };
    Some(
        text.lines()
            .take(PREVIEW_LINES)
            .map(|line| line.chars().take(160).collect())
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_text_extensions_case_insensitively() {
        assert!(is_text_name("Notes.MD"));
        assert!(!is_text_name("image.png"));
        assert!(!is_text_name(".md"));
    }

    #[test]
    fn text_lines_limits_lines_and_rejects_binary() {
        let body: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let lines = text_lines(body.as_bytes()).unwrap();
        assert_eq!(lines.len(), PREVIEW_LINES);
        assert_eq!(lines[0], "line 0");
        assert!(text_lines(b"ab\0cd").is_none());
    }

    #[test]
    fn text_lines_keeps_valid_prefix_of_cut_utf8() {
        let mut bytes = "héllo".as_bytes().to_vec();
        bytes.truncate(2);
        assert_eq!(text_lines(&bytes), Some(vec!["h".to_owned()]));
    }

    #[test]
    fn load_reads_text_and_skips_directories() {
        let dir = std::env::temp_dir().join(format!("finder-preview-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("todo.txt");
        std::fs::write(&file, "first\nsecond\n").unwrap();
        assert_eq!(
            load(&file, false),
            Preview::Text(vec!["first".to_owned(), "second".to_owned()])
        );
        assert_eq!(load(&dir, true), Preview::Unavailable);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

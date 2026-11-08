//! URL detection on one screen row and opening a URL with `xdg-open`.
//!
//! Only `http`, `https` and `file` links are recognised or opened. A link is a run of non-space cells
//! that starts at a scheme boundary. Trailing sentence punctuation is not part of the link, and a closing
//! bracket is kept only when the link has its matching opener.

use std::io;
use std::process::{Command, Stdio};

const SCHEMES: [&str; 3] = ["https://", "http://", "file://"];

/// Half-open char range `start..end` of one link within a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Link {
    pub start: usize,
    pub end: usize,
}

fn is_link_char(c: char) -> bool {
    !c.is_whitespace() && !matches!(c, '\0' | '<' | '>' | '"' | '`')
}

/// Trims sentence punctuation from the end, and any closing bracket that has no opener in the link.
fn trim_end(chars: &[char], start: usize, mut end: usize) -> usize {
    while end > start {
        let last = chars[end - 1];
        let unbalanced = match last {
            ')' | ']' | '}' => {
                let (open, close) = match last {
                    ')' => ('(', ')'),
                    ']' => ('[', ']'),
                    _ => ('{', '}'),
                };
                let body = &chars[start..end];
                body.iter().filter(|&&c| c == close).count()
                    > body.iter().filter(|&&c| c == open).count()
            }
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' => true,
            _ => false,
        };
        if !unbalanced {
            break;
        }
        end -= 1;
    }
    end
}

/// Every link on one row, in order. A scheme must not follow a letter, digit or `_`.
#[must_use]
pub fn links_in(row: &[char]) -> Vec<Link> {
    let lower: Vec<char> = row.iter().map(|c| c.to_ascii_lowercase()).collect();
    let mut links = Vec::new();
    let mut col = 0;
    while col < row.len() {
        let boundary = col == 0 || !(row[col - 1].is_alphanumeric() || row[col - 1] == '_');
        let scheme = boundary
            .then(|| SCHEMES.iter().find(|s| starts_with(&lower, col, s)))
            .flatten();
        let Some(scheme) = scheme else {
            col += 1;
            continue;
        };
        let mut end = col + scheme.len();
        while end < row.len() && is_link_char(row[end]) {
            end += 1;
        }
        let end = trim_end(row, col, end);
        if end > col + scheme.len() {
            links.push(Link { start: col, end });
            col = end;
        } else {
            col += scheme.len();
        }
    }
    links
}

fn starts_with(haystack: &[char], at: usize, needle: &str) -> bool {
    let mut chars = needle.chars();
    haystack[at..]
        .iter()
        .take(needle.len())
        .zip(chars.by_ref())
        .all(|(&h, n)| h == n)
        && haystack.len() - at >= needle.len()
}

/// The link covering `col`, as text.
#[must_use]
pub fn link_at(row: &[char], col: usize) -> Option<String> {
    links_in(row)
        .into_iter()
        .find(|link| (link.start..link.end).contains(&col))
        .map(|link| row[link.start..link.end].iter().collect())
}

/// Only these schemes are opened. The check runs before any process is started.
#[must_use]
pub fn is_openable(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    SCHEMES
        .iter()
        .any(|s| lower.starts_with(s) && lower.len() > s.len())
        && !url.chars().any(char::is_whitespace)
}

/// Opens `url` with `xdg-open`, detached. The URL is one argument, never passed through a shell, and the
/// child is reaped on a helper thread.
pub fn open(url: &str) -> io::Result<()> {
    if !is_openable(url) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not an openable link",
        ));
    }
    let mut child = Command::new("xdg-open")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(text: &str) -> Vec<char> {
        text.chars().collect()
    }

    fn found(text: &str) -> Vec<String> {
        let chars = row(text);
        links_in(&chars)
            .into_iter()
            .map(|l| chars[l.start..l.end].iter().collect())
            .collect()
    }

    #[test]
    fn detects_http_https_and_file_links() {
        assert_eq!(
            found("see https://example.com/a?b=1 now"),
            vec!["https://example.com/a?b=1"]
        );
        assert_eq!(found("http://x.org"), vec!["http://x.org"]);
        assert_eq!(found("open file:///tmp/a.txt"), vec!["file:///tmp/a.txt"]);
    }

    #[test]
    fn trailing_punctuation_is_not_part_of_the_link() {
        assert_eq!(found("Go to https://a.b/c."), vec!["https://a.b/c"]);
        assert_eq!(found("(https://a.b/c), next"), vec!["https://a.b/c"]);
        assert_eq!(found("really https://a.b?"), vec!["https://a.b"]);
    }

    #[test]
    fn balanced_brackets_stay_inside_the_link() {
        assert_eq!(
            found("see https://en.wikipedia.org/wiki/Rust_(programming_language)"),
            vec!["https://en.wikipedia.org/wiki/Rust_(programming_language)"]
        );
        assert_eq!(found("[https://a.b/x]"), vec!["https://a.b/x"]);
    }

    #[test]
    fn schemes_inside_words_and_bare_schemes_are_not_links() {
        assert!(found("xhttps://a.b").is_empty(), "letter before the scheme");
        assert!(found("https://").is_empty(), "scheme with no host");
        assert!(
            found("ftp://a.b mailto:x@y").is_empty(),
            "other schemes are not detected"
        );
    }

    #[test]
    fn several_links_on_one_row() {
        assert_eq!(
            found("a https://one.x b http://two.y"),
            vec!["https://one.x", "http://two.y"]
        );
    }

    #[test]
    fn link_at_maps_a_clicked_column_to_its_link() {
        let chars = row("go https://a.b now");
        assert_eq!(link_at(&chars, 3), Some("https://a.b".to_owned()));
        assert_eq!(link_at(&chars, 10), Some("https://a.b".to_owned()));
        assert_eq!(link_at(&chars, 0), None, "outside the link");
        assert_eq!(link_at(&chars, 14), None, "the space after the link");
        assert_eq!(
            link_at(&chars, 13),
            Some("https://a.b".to_owned()),
            "last link column"
        );
    }

    #[test]
    fn only_web_and_file_links_are_openable() {
        assert!(is_openable("https://a.b"));
        assert!(is_openable("HTTP://a.b"));
        assert!(is_openable("file:///etc/hosts"));
        assert!(!is_openable("javascript:alert(1)"));
        assert!(!is_openable("--help"));
        assert!(!is_openable("https://"));
        assert!(!is_openable("https://a b"));
        assert!(
            open("--option").is_err(),
            "rejected before any process starts"
        );
    }
}

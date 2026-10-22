//! Pure clipboard payload encoding and parsing. No X11 access happens here.

use std::fmt::Write as _;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use crate::FileOp;

pub const URI_LIST: &str = "text/uri-list";
pub const GNOME_FILES: &str = "x-special/gnome-copied-files";
pub const KDE_CUT: &str = "application/x-kde-cutselection";
pub const TEXT_PLAIN: &str = "text/plain";

/// `file://` URI for an absolute path. Every byte outside the unreserved set is
/// percent-encoded, so spaces, `#`, `%`, newlines and UTF-8 bytes all survive.
pub fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            uri.push(char::from(byte));
        } else {
            write!(uri, "%{byte:02X}").expect("writing to a String cannot fail");
        }
    }
    uri
}

/// Parses a local `file://` URI into an absolute path. Other schemes, remote hosts
/// and malformed percent escapes yield `None`.
pub fn parse_file_uri(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let slash = rest.find('/')?;
    let host = &rest[..slash];
    if !host.is_empty() && host != "localhost" {
        return None;
    }
    let bytes = percent_decode(rest[slash..].as_bytes())?;
    Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

fn percent_decode(input: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%' {
            let hex = input.get(i + 1..i + 3)?;
            if !hex.iter().all(u8::is_ascii_hexdigit) {
                return None;
            }
            let digits = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(digits, 16).ok()?);
            i += 3;
        } else {
            out.push(input[i]);
            i += 1;
        }
    }
    Some(out)
}

/// `text/uri-list` per RFC 2483: CRLF-terminated URIs.
pub fn uri_list(paths: &[PathBuf]) -> Vec<u8> {
    paths
        .iter()
        .map(|path| file_uri(path) + "\r\n")
        .collect::<String>()
        .into_bytes()
}

/// Reads `text/uri-list`, skipping blanks and `#` comments and ignoring non-file URIs.
pub fn parse_uri_list(bytes: &[u8]) -> Vec<PathBuf> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(parse_file_uri)
        .collect()
}

/// `x-special/gnome-copied-files`: an operation line followed by one URI per line.
pub fn gnome_files(op: FileOp, paths: &[PathBuf]) -> Vec<u8> {
    let mut text = String::from(op_word(op));
    for path in paths {
        text.push('\n');
        text.push_str(&file_uri(path));
    }
    text.into_bytes()
}

pub fn parse_gnome_files(bytes: &[u8]) -> Option<(FileOp, Vec<PathBuf>)> {
    let text = String::from_utf8_lossy(bytes);
    let mut lines = text.lines().map(|line| line.trim_end_matches('\0'));
    let op = match lines.next()?.trim() {
        "copy" => FileOp::Copy,
        "cut" => FileOp::Cut,
        _ => return None,
    };
    let paths = lines
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(parse_file_uri)
        .collect();
    Some((op, paths))
}

fn op_word(op: FileOp) -> &'static str {
    match op {
        FileOp::Copy => "copy",
        FileOp::Cut => "cut",
    }
}

/// `application/x-kde-cutselection`: "1" marks a cut, "0" a copy.
pub fn kde_cut(op: FileOp) -> &'static [u8] {
    match op {
        FileOp::Copy => b"0",
        FileOp::Cut => b"1",
    }
}

/// Anything other than "1" (including empty) is a copy.
pub fn parse_kde_cut(bytes: &[u8]) -> FileOp {
    if bytes.trim_ascii() == b"1" {
        FileOp::Cut
    } else {
        FileOp::Copy
    }
}

/// `text/plain` fallback: one raw path per line.
pub fn text_plain(paths: &[PathBuf]) -> Vec<u8> {
    let mut out = Vec::new();
    for path in paths {
        out.extend_from_slice(path.as_os_str().as_bytes());
        out.push(b'\n');
    }
    out
}

/// Every file-clipboard target the owner advertises, in publication order.
pub fn payloads(paths: &[PathBuf], op: FileOp) -> Vec<(&'static str, Vec<u8>)> {
    vec![
        (URI_LIST, uri_list(paths)),
        (GNOME_FILES, gnome_files(op, paths)),
        (KDE_CUT, kde_cut(op).to_vec()),
        (TEXT_PLAIN, text_plain(paths)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn encodes_spaces_hash_percent_unicode_and_newline() {
        assert_eq!(
            file_uri(&path("/tmp/a b#%ü\nx")),
            "file:///tmp/a%20b%23%25%C3%BC%0Ax"
        );
        assert_eq!(file_uri(&path("/a-._~/Z9")), "file:///a-._~/Z9");
    }

    #[test]
    fn decodes_round_trip_and_hex_case() {
        for raw in ["/tmp/a b#%ü\nx", "/", "/x/ünï cödé/€.txt", "/a%2Fb"] {
            assert_eq!(parse_file_uri(&file_uri(&path(raw))), Some(path(raw)));
        }
        assert_eq!(parse_file_uri("file:///%c3%bc"), Some(path("/ü")));
        assert_eq!(parse_file_uri("file://localhost/x"), Some(path("/x")));
    }

    #[test]
    fn rejects_non_file_and_malformed_uris() {
        assert_eq!(parse_file_uri("http://host/x"), None);
        assert_eq!(parse_file_uri("file://remote/x"), None);
        assert_eq!(parse_file_uri("file:relative"), None);
        assert_eq!(parse_file_uri("file://"), None);
        assert_eq!(parse_file_uri("file:///%zz"), None);
        assert_eq!(parse_file_uri("file:///%2"), None);
        assert_eq!(parse_file_uri("file:///%+1"), None);
    }

    #[test]
    fn uri_list_is_crlf_terminated_and_empty_for_no_paths() {
        assert_eq!(uri_list(&[]), b"");
        assert_eq!(
            uri_list(&[path("/a"), path("/b c")]),
            b"file:///a\r\nfile:///b%20c\r\n"
        );
    }

    #[test]
    fn uri_list_parses_crlf_lf_comments_blanks_and_foreign_uris() {
        let input = b"# comment\r\nfile:///a\r\n\r\nhttp://x/y\nfile:///b%20c\n\nfile:///d";
        assert_eq!(
            parse_uri_list(input),
            vec![path("/a"), path("/b c"), path("/d")]
        );
        assert!(parse_uri_list(b"").is_empty());
        assert!(parse_uri_list(b"# only\r\n").is_empty());
    }

    #[test]
    fn uri_list_round_trips_awkward_names() {
        let paths = vec![path("/tmp/a b#%ü\nx"), path("/tmp/crlf\r")];
        assert_eq!(parse_uri_list(&uri_list(&paths)), paths);
    }

    #[test]
    fn gnome_payload_round_trips_copy_and_cut() {
        let paths = vec![path("/a b"), path("/ü#")];
        assert_eq!(
            gnome_files(FileOp::Cut, &paths),
            b"cut\nfile:///a%20b\nfile:///%C3%BC%23"
        );
        assert_eq!(
            parse_gnome_files(&gnome_files(FileOp::Copy, &paths)),
            Some((FileOp::Copy, paths.clone()))
        );
        assert_eq!(
            parse_gnome_files(&gnome_files(FileOp::Cut, &paths)),
            Some((FileOp::Cut, paths))
        );
    }

    #[test]
    fn gnome_parse_tolerates_crlf_nul_and_skips_foreign_lines() {
        assert_eq!(
            parse_gnome_files(b"copy\r\nfile:///a\r\nhttp://x\0\n"),
            Some((FileOp::Copy, vec![path("/a")]))
        );
        assert_eq!(parse_gnome_files(b"copy"), Some((FileOp::Copy, vec![])));
    }

    #[test]
    fn gnome_parse_rejects_unknown_operation_and_empty_input() {
        assert_eq!(parse_gnome_files(b"paste\nfile:///a"), None);
        assert_eq!(parse_gnome_files(b""), None);
    }

    #[test]
    fn kde_cutselection_detects_cut_and_defaults_to_copy() {
        assert_eq!(kde_cut(FileOp::Cut), b"1");
        assert_eq!(kde_cut(FileOp::Copy), b"0");
        assert_eq!(parse_kde_cut(b"1"), FileOp::Cut);
        assert_eq!(parse_kde_cut(b" 1\n"), FileOp::Cut);
        assert_eq!(parse_kde_cut(b"0"), FileOp::Copy);
        assert_eq!(parse_kde_cut(b""), FileOp::Copy);
        assert_eq!(parse_kde_cut(b"yes"), FileOp::Copy);
    }

    #[test]
    fn text_plain_is_one_raw_path_per_line() {
        assert_eq!(
            text_plain(&[path("/a b"), path("/ü")]),
            "/a b\n/ü\n".as_bytes()
        );
        assert!(text_plain(&[]).is_empty());
    }

    #[test]
    fn payloads_cover_every_advertised_target() {
        let names: Vec<_> = payloads(&[path("/a")], FileOp::Cut)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(names, [URI_LIST, GNOME_FILES, KDE_CUT, TEXT_PLAIN]);
    }
}

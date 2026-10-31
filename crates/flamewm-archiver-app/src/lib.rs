//! `FlameWM` archive manager: argument handling and entry presentation.

use std::path::{Path, PathBuf};

use flamewm_archiver::{Entry, Kind, list};

/// What the binary was asked to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// Open an archive and show its entries.
    Open(PathBuf),
    /// No usable arguments.
    Usage,
}

/// Parses argv (without the program name).
#[must_use]
pub fn parse_args(args: &[String]) -> Command {
    match args.first() {
        Some(path) if !path.is_empty() => Command::Open(PathBuf::from(path)),
        _ => Command::Usage,
    }
}

/// One line per entry: kind, size, path. Directories show no size.
#[must_use]
pub fn format_entry(entry: &Entry) -> String {
    let kind = match entry.kind {
        Kind::File => "file",
        Kind::Dir => "dir ",
        Kind::Symlink => "link",
        Kind::Other => "other",
    };
    format!("{kind} {:>12}  {}", entry.size, entry.path.display())
}

/// Lists `archive` as formatted lines.
pub fn listing(archive: &Path) -> flamewm_archiver::Result<Vec<String>> {
    Ok(list(archive)?.iter().map(format_entry).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_open_and_usage() {
        assert_eq!(parse_args(&[]), Command::Usage);
        assert_eq!(parse_args(&[String::new()]), Command::Usage);
        assert_eq!(
            parse_args(&["a.zip".into()]),
            Command::Open(PathBuf::from("a.zip"))
        );
    }

    #[test]
    fn formats_entry_kind_and_size() {
        let entry = Entry {
            path: PathBuf::from("docs/a.txt"),
            size: 42,
            kind: Kind::File,
            mode: None,
        };
        assert_eq!(format_entry(&entry), "file           42  docs/a.txt");
    }
}

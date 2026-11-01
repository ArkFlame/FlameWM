//! `FlameWM` archive manager: argument handling, entry presentation and the window.

pub mod window;

use std::path::{Path, PathBuf};

use flamewm_archiver::{Entry, Kind, list};

/// What the binary was asked to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// Open an archive and show its entries.
    Open(PathBuf),
    /// Create `out` from `inputs` (`--create OUT PATH...`).
    Create { out: PathBuf, inputs: Vec<PathBuf> },
    /// No usable arguments.
    Usage,
}

/// Parses argv (without the program name).
#[must_use]
pub fn parse_args(args: &[String]) -> Command {
    match args {
        [flag, out, inputs @ ..] if flag == "--create" && !inputs.is_empty() => Command::Create {
            out: PathBuf::from(out),
            inputs: inputs.iter().map(PathBuf::from).collect(),
        },
        [flag, ..] if flag == "--create" => Command::Usage,
        [path, ..] if !path.is_empty() => Command::Open(PathBuf::from(path)),
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
    fn parses_create_with_inputs() {
        let args = ["--create".into(), "out.tar.gz".into(), "src".into()];
        assert_eq!(
            parse_args(&args),
            Command::Create {
                out: PathBuf::from("out.tar.gz"),
                inputs: vec![PathBuf::from("src")],
            }
        );
        assert_eq!(
            parse_args(&["--create".into(), "out.zip".into()]),
            Command::Usage
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

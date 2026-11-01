//! Archive format selection by file name.
//!
//! This module is the only place that knows which extensions `FlameWM` accepts, so the UI never
//! keeps a second list.

use std::path::Path;

use crate::error::{ArchiveError, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Zip,
    Tar,
    TarGz,
    TarXz,
    TarBz2,
}

/// Accepted file-name endings with the format each one selects.
///
/// A suffix that is itself the tail of a longer one is listed after it (`.tar.gz` before `.tar`,
/// `.tbz2` before `.tbz` is not needed because neither is a tail of the other), so the first match
/// is always the most specific one.
const SUFFIXES: [(&str, Format); 9] = [
    (".tar.gz", Format::TarGz),
    (".tgz", Format::TarGz),
    (".tar.xz", Format::TarXz),
    (".txz", Format::TarXz),
    (".tar.bz2", Format::TarBz2),
    (".tbz2", Format::TarBz2),
    (".tbz", Format::TarBz2),
    (".tar", Format::Tar),
    (".zip", Format::Zip),
];

impl Format {
    /// Picks the format from the file name. Content sniffing is not used, so the name is the contract.
    pub fn detect(path: &Path) -> Result<Self> {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        Self::from_name(&name).ok_or_else(|| ArchiveError::Unsupported(path.display().to_string()))
    }

    /// `name` without its archive suffix, when the name carries one.
    #[must_use]
    pub fn strip_suffix(name: &str) -> Option<&str> {
        SUFFIXES
            .iter()
            .find_map(|(suffix, _)| name.strip_suffix(suffix))
    }

    fn from_name(name: &str) -> Option<Self> {
        let (_, format) = SUFFIXES.iter().find(|(suffix, _)| name.ends_with(suffix))?;
        Some(*format)
    }
}

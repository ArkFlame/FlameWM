//! Archive format selection by file name.

use std::path::Path;

use crate::error::{ArchiveError, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Zip,
    Tar,
    TarGz,
}

impl Format {
    /// Picks the format from the file name. Content sniffing is not used, so the name is the contract.
    pub fn detect(path: &Path) -> Result<Self> {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if name.ends_with(".zip") {
            Ok(Self::Zip)
        } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
            Ok(Self::TarGz)
        } else if name.ends_with(".tar") {
            Ok(Self::Tar)
        } else {
            Err(ArchiveError::Unsupported(path.display().to_string()))
        }
    }
}

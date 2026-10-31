//! Error type shared by every archive operation.

use std::fmt;
use std::io;
use std::path::PathBuf;

#[derive(Debug)]
pub enum ArchiveError {
    Io(io::Error),
    Zip(zip::result::ZipError),
    /// Entry name escapes the destination (`..`, absolute path) or would be written through a symlink.
    UnsafePath(PathBuf),
    /// Archive path has no supported extension, or the name is not representable.
    Unsupported(String),
    /// The operation was cancelled through its `CancelToken`.
    Cancelled,
}

pub type Result<T> = std::result::Result<T, ArchiveError>;

impl fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "I/O error: {err}"),
            Self::Zip(err) => write!(f, "zip error: {err}"),
            Self::UnsafePath(path) => write!(f, "unsafe archive path rejected: {}", path.display()),
            Self::Unsupported(what) => write!(f, "unsupported archive or name: {what}"),
            Self::Cancelled => f.write_str("operation cancelled"),
        }
    }
}

impl std::error::Error for ArchiveError {}

impl From<io::Error> for ArchiveError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<zip::result::ZipError> for ArchiveError {
    fn from(err: zip::result::ZipError) -> Self {
        Self::Zip(err)
    }
}

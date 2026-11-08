//! `FlameWM` archive core: list, extract, create, add and remove zip and tar archives.
//!
//! Tar is supported raw and compressed with gzip, xz and bzip2; the compressed variants share one
//! streaming tar path and differ only by the decoder/encoder placed under `tar::Builder`. Path
//! safety, cancellation and progress live here, so the UI layer only forwards requests. Long
//! operations run on the caller's thread and accept a `CancelToken` and a progress sink.

mod create;
mod edit;
mod error;
mod extract;
mod format;
mod progress;
mod safe_path;
mod sevenz;
mod visit;
mod writer;

#[cfg(test)]
mod tests;

pub use create::{Source, collect, create, create_encrypted};
pub use edit::{add, remove};
pub use error::{ArchiveError, Result};
pub use extract::{extract, extract_with_password};
pub use format::Format;
pub use progress::{CancelToken, Progress, Summary};
pub use safe_path::sanitize;
pub use sevenz::parse_listing;
pub use visit::{Entry, Kind, list, visit, visit_with_password};

//! `FlameWM` archive core: list, extract, create, add and remove zip and tar archives.
//!
//! Path safety, cancellation and progress live here, so the UI layer only forwards requests.
//! Long operations run on the caller's thread and accept a `CancelToken` and a progress sink.

mod create;
mod edit;
mod error;
mod extract;
mod format;
mod progress;
mod safe_path;
mod visit;
mod writer;

#[cfg(test)]
mod tests;

pub use create::{Source, collect, create};
pub use edit::{add, remove};
pub use error::{ArchiveError, Result};
pub use extract::extract;
pub use format::Format;
pub use progress::{CancelToken, Progress, Summary};
pub use safe_path::sanitize;
pub use visit::{Entry, Kind, list, visit};

//! Crash-isolated filesystem desktop model and file-operation policy.
//!
//! Native X11 surfaces and the inotify file descriptor adapter stay outside this crate. The model,
//! watcher reconciliation, filesystem actions and Freedesktop Trash semantics are real Rust code
//! and can be exercised without a running window manager.

mod atomic_file;
pub mod conversion;
pub mod file_actions;
pub mod layout;
pub mod model;
pub mod persistence;
pub mod presentation;
pub mod selection;
pub mod sticky;
pub mod sticky_persistence;
pub mod trash;
pub mod watcher;

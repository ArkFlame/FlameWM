//! FlameWM desktop entry, shared by its binary and the multi-call executable.

/// The application entry point, compiled into this library so the multi-call `flamewm-apps`
/// executable can run it; `src/main.rs` is a one-line wrapper around it.
#[doc(hidden)]
#[path = "main.rs"]
pub mod entry;

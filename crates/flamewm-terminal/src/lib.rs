//! FlameWM Terminal: PTY, VT parser, cell grid, themes and the retained-UI projection. The binary in `main.rs`
//! wires these to the FlameWM window loop.
pub mod app;
pub mod glyphs;
pub mod keys;
pub mod parser;
pub mod pool;
pub mod pty;
pub mod screen;
pub mod session;
pub mod settings;
pub mod store;
pub mod suggest;
pub mod theme;
pub mod themes;
pub mod ui;
pub mod view;
pub mod wake;

extern crate self as flamewm_terminal;

/// The application entry point, compiled into this library so the multi-call `flamewm-apps`
/// executable can run it; `src/main.rs` is a one-line wrapper around it.
#[doc(hidden)]
#[path = "main.rs"]
pub mod entry;

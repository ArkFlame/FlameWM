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
pub mod suggest;
pub mod settings;
pub mod store;
pub mod theme;
pub mod themes;
pub mod ui;
pub mod view;

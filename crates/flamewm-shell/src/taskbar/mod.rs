//! Renderer-neutral taskbar feature composition.

pub mod context_menu;
pub mod intent;
pub mod status;
pub mod tasks;
pub mod workspaces;

pub use intent::{StatusIntent, TaskbarIntent};

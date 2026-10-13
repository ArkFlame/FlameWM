//! FlameWM product policy ported from the current native C++ implementation.
//!
//! This crate owns product semantics, not window-manager protocol state. The native FlameWM desktop backend
//! engine remains authoritative for windows, focus, stacking, work areas, EWMH/ICCCM and native
//! X11 resources.

pub mod chrome;
pub mod display;
pub mod host;
pub mod identity;
pub mod panel;
pub mod product;
pub mod scale;
pub mod search;
pub mod services;
pub mod settings;
pub mod shortcuts;
pub mod system;
pub mod task;
pub mod workspace;

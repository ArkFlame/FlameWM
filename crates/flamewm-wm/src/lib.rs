//! `FlameWM` window-manager facade.
//!
//! Native X11 authority lives in `flamewm-wm-x11`.

pub use flamewm_wm_x11::{
    SettingsObserver, SettingsWake, WmConfig, apply_cursor_shadow, apply_cursor_theme,
    persisted_appearance, persisted_cursor_theme, run, run_with_settings_observer,
};

extern crate self as flamewm_wm;

/// The application entry point, compiled into this library so the multi-call `flamewm-apps`
/// executable can run it; `src/main.rs` is a one-line wrapper around it.
#[doc(hidden)]
#[path = "main.rs"]
pub mod entry;

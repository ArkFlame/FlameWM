//! FlameWM window-manager facade.
//!
//! Native X11 authority lives in `flamewm-wm-x11`.

pub use flamewm_wm_x11::{
    SettingsObserver, SettingsWake, WmConfig, apply_cursor_theme, persisted_appearance,
    persisted_cursor_theme, run, run_with_settings_observer,
};

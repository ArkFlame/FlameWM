//! FlameWM window-manager facade.
//!
//! Native X11 authority lives in `flamewm-wm-x11`.

pub use flamewm_wm_x11::{SettingsObserver, SettingsWake, WmConfig, run, run_with_settings_observer};

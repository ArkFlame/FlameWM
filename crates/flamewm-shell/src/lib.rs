//! Runtime shell adapter. Product state arrives as snapshots; UI actions leave as controls.

pub mod assistant_popover;
pub mod async_projection;
pub mod bus_offline;
pub mod calendar;
pub mod control_actions;
pub mod cursor_theme;
pub mod dock;
pub mod icon_loader;
pub mod launcher;
#[cfg(test)]
pub(crate) mod test_recorder;
use flamewm_debug::log_limit;
pub mod bluetooth_dialog;
pub mod notification_center;
pub mod osd;
pub mod panel;
pub mod polkit_answer;
pub mod polkit_dialog;
pub mod popovers;
pub mod popup_controller;
pub mod popup_role;
pub mod projection;
pub mod quick_controls;
pub mod runtime;
pub mod settings_projection;
pub mod start;
pub mod status;
pub mod surface_backdrop;
pub mod switcher;
pub mod taskbar_scale;
pub mod tray_host;
pub mod weather_auto;
pub mod weather_host;
mod weather_provider;

/// The active taskbar status projection is compiled without activating stale
/// taskbar controllers that target an older control contract.
pub mod taskbar {
    #[path = "context_menu.rs"]
    pub mod context_menu;
    #[path = "intent.rs"]
    pub mod intent;
    #[path = "show_desktop.rs"]
    pub mod show_desktop;
    #[path = "status/mod.rs"]
    pub mod status;
    #[path = "tasks/mod.rs"]
    pub mod tasks;
    #[path = "workspaces/mod.rs"]
    pub mod workspaces;
}

pub use runtime::{
    ChangedDomains, ContextMenuKind, ContextMenuState, ProjectionKind, ShellControl, ShellDirty,
    ShellRuntime, ShellSnapshot, ShellSurfaces, TaskSlotTarget,
};

extern crate self as flamewm_shell;

/// The application entry point, compiled into this library so the multi-call `flamewm-apps`
/// executable can run it; `src/main.rs` is a one-line wrapper around it.
#[doc(hidden)]
#[path = "main.rs"]
pub mod entry;

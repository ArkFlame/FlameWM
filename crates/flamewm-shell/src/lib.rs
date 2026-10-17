//! Runtime shell adapter. Product state arrives as snapshots; UI actions leave as controls.

pub mod async_projection;
pub mod calendar;
pub mod control_actions;
pub mod cursor_theme;
pub mod icon_loader;
pub mod osd;
pub mod panel;
pub mod popovers;
pub mod popup_controller;
pub mod popup_role;
pub mod projection;
pub mod quick_controls;
pub mod runtime;
#[path = "settings_projection.rs"]
pub mod settings_projection;
pub mod start;
pub mod surface_backdrop;
pub mod status;
pub mod weather_host;
mod weather_provider;
pub mod weather_auto;

/// The active taskbar status projection is compiled without activating stale
/// taskbar controllers that target an older control contract.
pub mod taskbar {
    #[path = "context_menu.rs"]
    pub mod context_menu;
    #[path = "intent.rs"]
    pub mod intent;
    #[path = "status/mod.rs"]
    pub mod status;
    #[path = "tasks/mod.rs"]
    pub mod tasks;
    #[path = "workspaces/mod.rs"]
    pub mod workspaces;
}

pub use runtime::{
    ChangedDomains, ContextMenuKind, ContextMenuState, ProjectionKind, ShellControl, ShellDirty,
    ShellRuntime, ShellSnapshot, ShellSurfaces,
};

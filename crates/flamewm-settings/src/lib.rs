//! `FlameWM` Settings application library: markup binding and control transport.
//! Split: `consts`, `pager`, `hotkeys`, `actions`, `input`, `sliders`, `tables`,
//! `nav`, `nodes`, `location`, `labels`, and `app_impl_*` (the `SettingsApplication` impl).

mod file_manager;
use std::path::Path;
use std::process::Command;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Instant;

pub mod control;
mod display_layout;
pub mod file_chooser;
pub mod runtime;
pub mod theme_picker;
pub mod view;

#[cfg(test)]
mod controller;
#[cfg(test)]
mod projection;
#[cfg(test)]
mod ui_contract;

/// Source of the split application module tree, for source-scanning tests.
#[cfg(test)]
pub(crate) const APP_SOURCE: &str = concat!(
    include_str!("lib.rs"),
    include_str!("consts.rs"),
    include_str!("actions.rs"),
    include_str!("nav.rs"),
    include_str!("app_impl_1.rs"),
    include_str!("app_impl_2.rs"),
    include_str!("app_impl_3.rs"),
);

mod actions;
mod app_impl_1;
mod app_impl_2;
mod app_impl_3;
mod consts;
mod hotkeys;
mod input;
mod labels;
mod location;
mod nav;
mod nodes;
mod pager;
mod sliders;
mod tables;

use actions::*;
use consts::*;
use hotkeys::*;
use input::*;
use labels::*;
use location::*;
use nav::*;
use nodes::*;
use pager::*;
use sliders::*;
use tables::*;

pub use labels::display_mode_label;
pub use labels::font_size_label;
pub use labels::panel_edge_label;
pub use pager::normal_window_config;

use flamewm_api::panels::PanelEdge;
use flamewm_api::session::SessionAction;
use flamewm_api::settings::SettingValue;
use flamewm_api::{ModeId, OutputId};
use flamewm_control_core::{ControlError, ControlRequest};
use flamewm_integrations_linux::media_store::{MediaKind, MediaStore, import_for_setting};
use flamewm_settings_core::{ControlTransport, DISPLAY_MODE_SLOTS, SettingsClient, SettingsPage};
use flamewm_ui_core::glass;
use flamewm_ui_x11::{
    UiActionEvent, UiActionPhase, UiColor, UiDocumentAccess, UiKeyEvent, UiKeyPhase, UiWindowConfig,
};

pub struct SettingsApplication<T: ControlTransport> {
    client: SettingsClient<T>,
    wallpaper_chooser: Option<file_chooser::WallpaperChooserWorker>,
    /// The setting the open file chooser writes when it completes.
    chooser_target: ChooserTarget,
    pending_open: Option<Receiver<Result<(), String>>>,
    /// True only between a hotkey row capture and its committing key release.
    /// Gates key events so a stale capture armed by the core cannot commit later.
    hotkey_armed: bool,
    error: Option<String>,
    pages: DisplayPages,
    themes: theme_picker::ThemeRegistry,
    /// The open theme picker and its current page.
    theme_picker: Option<(theme_picker::PickerKind, usize)>,
    color_picker: Option<ColorPicker>,
    theme_scan: Option<Receiver<theme_picker::ThemeRegistry>>,
    theme_install: Option<Receiver<theme_picker::InstallOutcome>>,
    theme_status: Option<String>,
    appearance_tab: AppearanceTab,
    /// Resolved cursor theme last loaded into this window's live cursor owners.
    applied_cursor_theme: Option<String>,
    /// Pointer origin for an output card; motion remains local, only release
    /// causes a native `RandR` request.
    monitor_drag: Option<(usize, f32, u64, OutputId)>,
    /// Non-authoritative drag preview, bound to the original output and generation.
    scale_drag: Option<(OutputId, u64, u16)>,
    /// Arrow keys affect scale only after the slider is focused by pointer.
    scale_slider_focused: bool,
    /// Sidebar search text and whether the search field takes typed text.
    nav_query: String,
    nav_search_focused: bool,
    /// Edit buffer for `assistant.apiKey`; `Some` while the field takes typed text.
    api_key_draft: Option<ApiKeyDraft>,
    file_manager_label: String,
}

/// Typed API key text not yet saved, with the inline message for the last Enter.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ApiKeyDraft {
    buffer: String,
    error: Option<String>,
}

#[cfg(test)]
mod accent_node_tests;
#[cfg(test)]
mod card_fill_tests;
#[cfg(test)]
mod cursor_reload_tests;
#[cfg(test)]
mod display_pager_fixture_tests;
#[cfg(test)]
mod nav_indicator_layout_tests;
#[cfg(test)]
mod test_transport;
#[cfg(test)]
mod time_zone_label_tests;
#[cfg(test)]
mod value_text_tests;

extern crate self as flamewm_settings;

/// The application entry point, compiled into this library so the multi-call `flamewm-apps`
/// executable can run it; `src/main.rs` is a one-line wrapper around it.
#[doc(hidden)]
#[path = "main.rs"]
pub mod entry;

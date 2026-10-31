//! Canonical Flame Control wire schema.
//!
//! This crate is intentionally D-Bus-library-free. It maps the native
//! `com.arkflame.FlameWM1` object ABI to typed `ControlRequest`/`ControlResponse` values. A
//! libdbus adapter only has to translate D-Bus primitive/container values to/from `WireValue`.
//! Product validation remains in Platform services.

use std::collections::BTreeMap;

use flamewm_api::applications::{ApplicationLaunchOptions, DesktopApplication};
use flamewm_api::display::{DisplayMode, DisplaySnapshot, OutputSnapshot, PendingModeChange};
use flamewm_api::panels::{PanelSnapshot, PanelsSnapshot, TaskEntry, TaskEntryKind};
use flamewm_api::session::{SessionAction, SessionCapabilities};
use flamewm_api::settings::{SettingValue, SettingsChange, SettingsSnapshot, SettingsTransaction};
use flamewm_api::shell_bootstrap::ShellBootstrapSnapshot;
use flamewm_api::shortcuts::{KeyBinding, ShortcutSnapshot};
use flamewm_api::system::{
    AudioEndpointKind, AudioEndpointSnapshot, AudioMuteAction, AudioSnapshot, AudioStreamSnapshot,
    AudioTarget, AudioVolumeAction, MediaSnapshot, NetworkAccessPointSnapshot, NetworkKind,
    NetworkSecretRequestSnapshot, NetworkSnapshot, PlaybackState, PowerSnapshot, PowerState,
    ServiceAvailability, SystemAction, SystemSnapshot,
};
use flamewm_api::window::{WindowSnapshot, WindowState};
use flamewm_api::workspace::WorkspaceSnapshot;
use flamewm_api::{
    DesktopAppId, ErrorCode, ModeId, OutputId, PanelEdge, Rect, Size, TaskEntryId, TransactionId,
    WindowRef,
};
use flamewm_control_core::{
    ControlError, ControlRequest, ControlResponse, IFACE_APPLICATIONS, IFACE_DISPLAYS,
    IFACE_PANELS, IFACE_ROOT, IFACE_SESSION, IFACE_SETTINGS, IFACE_SHORTCUTS, IFACE_SYSTEM,
    IFACE_WEATHER, IFACE_WINDOWS, IFACE_WORKSPACES, Version,
};

mod wire;
pub use wire::*;
mod call;
pub use call::*;
mod reply;
pub use reply::*;
mod decode;
use decode::*;
mod signal;
pub use signal::*;
mod encode;
pub use encode::*;
mod system;
use system::*;
mod action;
pub use action::*;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod hotkey_capture_lease_wire_tests;

//! Transport-neutral Flame Control protocol and dispatcher.
//!
//! Transport adapters map these typed requests/responses onto
//! `com.arkflame.FlameWM1` without putting D-Bus into product policy crates. The native WM path
//! uses the external-reactor `dbus` adapter so FlameWM retains one desktop event loop.

use core::fmt;
use std::collections::BTreeMap;

use flamewm_api::applications::{ApplicationLaunchOptions, DesktopApplication};
use flamewm_api::display::DisplaySnapshot;
use flamewm_api::panels::PanelsSnapshot;
use flamewm_api::ports::EnginePorts;
use flamewm_api::session::{SessionAction, SessionCapabilities};
use flamewm_api::settings::{SettingsSnapshot, SettingsTransaction};
use flamewm_api::shell_bootstrap::ShellBootstrapSnapshot;
use flamewm_api::shortcuts::{KeyBinding, ShortcutSnapshot};
use flamewm_api::system::{SystemAction, SystemSnapshot};
use flamewm_api::window::WindowSnapshot;
use flamewm_api::workspace::WorkspaceSnapshot;
use flamewm_api::{
    DesktopAppId, ErrorCode, FlameError, ModeId, OutputId, PanelEdge, TransactionId, WindowRef,
};
use flamewm_platform::host::PlatformHost;

pub mod mutations;

pub use mutations::{ControlMutation, ControlMutationQueue, ControlMutationResult, MutationLane};

pub const BUS_NAME: &str = "com.arkflame.FlameWM1";
pub const OBJECT_PATH: &str = "/com/arkflame/FlameWM1";
pub const IFACE_ROOT: &str = "com.arkflame.FlameWM1";
pub const IFACE_SETTINGS: &str = "com.arkflame.FlameWM1.Settings";
pub const IFACE_WORKSPACES: &str = "com.arkflame.FlameWM1.Workspaces";
pub const IFACE_DISPLAYS: &str = "com.arkflame.FlameWM1.Displays";
pub const IFACE_SHORTCUTS: &str = "com.arkflame.FlameWM1.Shortcuts";
pub const IFACE_PANELS: &str = "com.arkflame.FlameWM1.Panels";
pub const IFACE_SESSION: &str = "com.arkflame.FlameWM1.Session";
pub const IFACE_WINDOWS: &str = "com.arkflame.FlameWM1.Windows";
pub const IFACE_APPLICATIONS: &str = "com.arkflame.FlameWM1.Applications";
pub const IFACE_SYSTEM: &str = "com.arkflame.FlameWM1.System";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    #[must_use]
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let mut parts = value.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        let parsed = Self::new(major, minor, patch);
        (parsed.to_string() == value).then_some(parsed)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[must_use]
pub const fn current_version() -> Version {
    Version::new(0, 1, 4)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlRequest {
    Ping,
    GetVersion,
    GetCapabilities,
    GetShellBootstrap,
    GetWindows,
    GetApplications,
    LaunchApplication {
        app: DesktopAppId,
        options: ApplicationLaunchOptions,
    },
    ActivateWindow(WindowRef),
    MinimizeWindow(WindowRef),
    RestoreWindow(WindowRef),
    CloseWindow(WindowRef),
    GetSettings,
    ApplySettings(SettingsTransaction),
    GetWorkspaces,
    ActivateWorkspace {
        index: usize,
        expected_revision: u64,
    },
    InsertWorkspaceAfter {
        index: usize,
        expected_revision: u64,
    },
    RemoveWorkspace {
        index: usize,
        expected_revision: u64,
    },
    GetDisplays,
    BeginDisplayMode {
        output: OutputId,
        mode: ModeId,
        expected_generation: u64,
        now_ms: u64,
    },
    KeepDisplayMode {
        transaction: TransactionId,
        now_ms: u64,
    },
    RevertDisplayMode {
        transaction: TransactionId,
    },
    SetShellScale {
        output: OutputId,
        percent: u16,
        expected_revision: u64,
    },
    GetShortcuts,
    SetShortcut {
        action: String,
        binding: String,
        expected_revision: u64,
    },
    ClearShortcut {
        action: String,
        expected_revision: u64,
    },
    ResetShortcut {
        action: String,
        expected_revision: u64,
    },
    ApplyShortcuts {
        bindings: BTreeMap<String, KeyBinding>,
        expected_revision: u64,
    },
    /// Suspends every WM shortcut grab so Settings receives raw keys (bare Super included).
    /// Refused with `Busy` while another lease is held. Restored by `EndHotkeyCapture`.
    BeginHotkeyCapture {
        action: String,
        expected_revision: u64,
    },
    /// Ends the lease returned by `BeginHotkeyCapture` and restores the grab set.
    EndHotkeyCapture {
        lease: u64,
    },
    GetPanels,
    SetPanelEdge {
        output: OutputId,
        edge: PanelEdge,
        expected_revision: u64,
    },
    SetPanelSize {
        output: OutputId,
        logical_size: u16,
        expected_revision: u64,
    },
    PinApp {
        app: DesktopAppId,
        expected_revision: u64,
    },
    UnpinApp {
        app: DesktopAppId,
        expected_revision: u64,
    },
    ReorderTask {
        entry_id: flamewm_api::TaskEntryId,
        index: usize,
        expected_revision: u64,
    },
    GetSessionCapabilities,
    SessionAction(SessionAction),
    GetSystem,
    SystemAction {
        action: SystemAction,
        expected_revision: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlResponse {
    Pong,
    Version(Version),
    Capabilities(Vec<String>),
    ShellBootstrap(ShellBootstrapSnapshot),
    Windows(Vec<WindowSnapshot>),
    Applications(Vec<DesktopApplication>),
    Settings(SettingsSnapshot),
    Workspaces(WorkspaceSnapshot),
    Displays(DisplaySnapshot),
    Transaction(TransactionId),
    Shortcuts(ShortcutSnapshot),
    CaptureLease(u64),
    Panels(PanelsSnapshot),
    SessionCapabilities(SessionCapabilities),
    Changed(bool),
    System(SystemSnapshot),
    Unit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlError {
    pub name: &'static str,
    pub code: ErrorCode,
    pub message: String,
}

impl From<FlameError> for ControlError {
    fn from(error: FlameError) -> Self {
        Self {
            name: error_name(error.code),
            code: error.code,
            message: error.message,
        }
    }
}

#[must_use]
pub const fn error_name(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::InvalidArgument => "com.arkflame.FlameWM1.Error.InvalidArgument",
        ErrorCode::NotFound => "com.arkflame.FlameWM1.Error.NotFound",
        ErrorCode::StaleRevision => "com.arkflame.FlameWM1.Error.StaleRevision",
        ErrorCode::Conflict => "com.arkflame.FlameWM1.Error.Conflict",
        ErrorCode::Busy => "com.arkflame.FlameWM1.Error.Busy",
        ErrorCode::Unsupported => "com.arkflame.FlameWM1.Error.Unsupported",
        ErrorCode::Unavailable => "com.arkflame.FlameWM1.Error.Unavailable",
        ErrorCode::PermissionDenied => "com.arkflame.FlameWM1.Error.PermissionDenied",
        ErrorCode::Timeout => "com.arkflame.FlameWM1.Error.Unavailable",
        ErrorCode::IoFailure => "com.arkflame.FlameWM1.Error.InternalFailure",
        ErrorCode::EngineRejected => "com.arkflame.FlameWM1.Error.InternalFailure",
        ErrorCode::InternalFailure => "com.arkflame.FlameWM1.Error.InternalFailure",
    }
}

pub struct Dispatcher;

impl Dispatcher {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    pub fn dispatch<E: EnginePorts>(
        &self,
        host: &mut PlatformHost<E>,
        request: ControlRequest,
    ) -> Result<ControlResponse, ControlError> {
        let response = match request {
            ControlRequest::Ping => ControlResponse::Pong,
            ControlRequest::GetVersion => ControlResponse::Version(current_version()),
            ControlRequest::GetCapabilities => {
                ControlResponse::Capabilities(host.capabilities().items().to_vec())
            }
            ControlRequest::GetShellBootstrap => {
                host.refresh_windows()?;
                let windows = host.engine().snapshot()?;
                let windows_revision = windows
                    .iter()
                    .map(|window| window.state_generation)
                    .max()
                    .unwrap_or(0);
                ControlResponse::ShellBootstrap(ShellBootstrapSnapshot {
                    windows_revision,
                    windows,
                    workspaces: host.workspace_snapshot()?,
                    panels: host.panels_snapshot(),
                    applications: host.search_applications(""),
                    session: host.session_capabilities(),
                    displays: host.display_snapshot()?,
                    system: host.system_snapshot().clone(),
                })
            }
            ControlRequest::GetWindows => {
                host.refresh_windows()?;
                ControlResponse::Windows(host.engine().snapshot()?)
            }
            ControlRequest::GetApplications => {
                ControlResponse::Applications(host.search_applications(""))
            }
            ControlRequest::LaunchApplication { app, options } => {
                host.launch_application(&app, &options)?;
                ControlResponse::Unit
            }
            ControlRequest::ActivateWindow(window) => {
                host.activate_window(window)?;
                ControlResponse::Unit
            }
            ControlRequest::MinimizeWindow(window) => {
                host.minimize_window(window)?;
                ControlResponse::Unit
            }
            ControlRequest::RestoreWindow(window) => {
                host.restore_window(window)?;
                ControlResponse::Unit
            }
            ControlRequest::CloseWindow(window) => {
                host.close_window(window)?;
                ControlResponse::Unit
            }
            ControlRequest::GetSettings => ControlResponse::Settings(host.settings_snapshot()),
            ControlRequest::ApplySettings(transaction) => {
                ControlResponse::Settings(host.apply_settings(&transaction)?)
            }
            ControlRequest::GetWorkspaces => {
                ControlResponse::Workspaces(host.workspace_snapshot()?)
            }
            ControlRequest::ActivateWorkspace {
                index,
                expected_revision,
            } => {
                host.activate_workspace(index, expected_revision)?;
                ControlResponse::Unit
            }
            ControlRequest::InsertWorkspaceAfter {
                index,
                expected_revision,
            } => {
                host.insert_workspace_after(index, expected_revision)?;
                ControlResponse::Unit
            }
            ControlRequest::RemoveWorkspace {
                index,
                expected_revision,
            } => {
                host.remove_workspace(index, expected_revision)?;
                ControlResponse::Unit
            }
            ControlRequest::GetDisplays => ControlResponse::Displays(host.display_snapshot()?),
            ControlRequest::BeginDisplayMode {
                output,
                mode,
                expected_generation,
                now_ms,
            } => ControlResponse::Transaction(host.begin_display_mode(
                &output,
                mode,
                expected_generation,
                now_ms,
            )?),
            ControlRequest::KeepDisplayMode {
                transaction,
                now_ms,
            } => {
                host.keep_display_mode(transaction, now_ms)?;
                ControlResponse::Unit
            }
            ControlRequest::RevertDisplayMode { transaction } => {
                host.revert_display_mode(transaction)?;
                ControlResponse::Unit
            }
            ControlRequest::SetShellScale {
                output,
                percent,
                expected_revision,
            } => ControlResponse::Changed(host.set_shell_scale(
                &output,
                percent,
                expected_revision,
            )?),
            ControlRequest::GetShortcuts => ControlResponse::Shortcuts(host.shortcut_snapshot()),
            ControlRequest::SetShortcut {
                action,
                binding,
                expected_revision,
            } => {
                host.set_shortcut(&action, &binding, expected_revision)?;
                ControlResponse::Unit
            }
            ControlRequest::ClearShortcut {
                action,
                expected_revision,
            } => {
                host.clear_shortcut(&action, expected_revision)?;
                ControlResponse::Unit
            }
            ControlRequest::ResetShortcut {
                action,
                expected_revision,
            } => {
                host.reset_shortcut(&action, expected_revision)?;
                ControlResponse::Unit
            }
            ControlRequest::ApplyShortcuts {
                bindings,
                expected_revision,
            } => {
                host.apply_shortcuts(&bindings, expected_revision)?;
                ControlResponse::Unit
            }
            ControlRequest::BeginHotkeyCapture {
                action,
                expected_revision,
            } => ControlResponse::CaptureLease(host.begin_hotkey_capture(
                &action,
                expected_revision,
            )?),
            ControlRequest::EndHotkeyCapture { lease } => {
                host.end_hotkey_capture(lease)?;
                ControlResponse::Unit
            }
            ControlRequest::GetPanels => ControlResponse::Panels(host.panels_snapshot()),
            ControlRequest::SetPanelEdge {
                output,
                edge,
                expected_revision,
            } => {
                host.set_panel_edge(&output, edge, expected_revision)?;
                ControlResponse::Unit
            }
            ControlRequest::SetPanelSize {
                output,
                logical_size,
                expected_revision,
            } => {
                host.set_panel_size(&output, logical_size, expected_revision)?;
                ControlResponse::Unit
            }
            ControlRequest::PinApp {
                app,
                expected_revision,
            } => ControlResponse::Changed(host.pin_app(app, expected_revision)?),
            ControlRequest::UnpinApp {
                app,
                expected_revision,
            } => ControlResponse::Changed(host.unpin_app(&app, expected_revision)?),
            ControlRequest::ReorderTask {
                entry_id,
                index,
                expected_revision,
            } => ControlResponse::Changed(host.reorder_task_entry(
                &entry_id,
                index,
                expected_revision,
            )?),
            ControlRequest::GetSessionCapabilities => {
                ControlResponse::SessionCapabilities(host.session_capabilities())
            }
            ControlRequest::SessionAction(action) => {
                host.session_action(action)?;
                ControlResponse::Unit
            }
            ControlRequest::GetSystem => ControlResponse::System(host.system_snapshot().clone()),
            ControlRequest::SystemAction {
                action,
                expected_revision,
            } => {
                host.system_action(action, expected_revision)?;
                ControlResponse::Unit
            }
        };
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parser_rejects_noncanonical_and_accepts_current() {
        assert_eq!(Version::parse("0.1.4"), Some(current_version()));
        assert_eq!(Version::parse("00.0.3"), None);
    }

    #[test]
    fn error_mapping_preserves_public_protocol_name() {
        assert_eq!(
            error_name(ErrorCode::StaleRevision),
            "com.arkflame.FlameWM1.Error.StaleRevision"
        );
    }
}

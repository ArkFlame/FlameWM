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
    IFACE_PANELS, IFACE_ROOT, IFACE_SESSION, IFACE_SETTINGS, IFACE_SHORTCUTS, IFACE_SYSTEM, IFACE_WEATHER,
    IFACE_WINDOWS, IFACE_WORKSPACES, Version,
};

pub type WireDict = BTreeMap<String, WireValue>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireValue {
    Bool(bool),
    U32(u32),
    U64(u64),
    I32(i32),
    I64(i64),
    String(String),
    StringArray(Vec<String>),
    Array(Vec<WireValue>),
    Dict(WireDict),
}

impl WireValue {
    fn as_u64(&self) -> Option<u64> {
        match self {
            Self::U64(value) => Some(*value),
            Self::U32(value) => Some(u64::from(*value)),
            _ => None,
        }
    }

    fn as_u32(&self) -> Option<u32> {
        match self {
            Self::U32(value) => Some(*value),
            Self::U64(value) => u32::try_from(*value).ok(),
            _ => None,
        }
    }

    fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    fn as_i32(&self) -> Option<i32> {
        match self {
            Self::I32(value) => Some(*value),
            Self::I64(value) => i32::try_from(*value).ok(),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    fn as_array(&self) -> Option<&[WireValue]> {
        match self {
            Self::Array(value) => Some(value),
            _ => None,
        }
    }

    fn as_string_array(&self) -> Option<&[String]> {
        match self {
            Self::StringArray(value) => Some(value),
            _ => None,
        }
    }

    fn as_dict(&self) -> Option<&WireDict> {
        match self {
            Self::Dict(value) => Some(value),
            _ => None,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct WireCall {
    pub interface: String,
    pub member: String,
    pub args: Vec<WireValue>,
    /// Monotonic clock supplied by the WM reactor; not part of the D-Bus ABI.
    pub now_ms: u64,
}

impl core::fmt::Debug for WireCall {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let args: Vec<_> = if self.interface == IFACE_SYSTEM && self.member == "SubmitNetworkSecret"
        {
            self.args
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    if index == 2 {
                        WireValue::String("<redacted>".to_owned())
                    } else {
                        value.clone()
                    }
                })
                .collect()
        } else {
            self.args.clone()
        };
        f.debug_struct("WireCall")
            .field("interface", &self.interface)
            .field("member", &self.member)
            .field("args", &args)
            .field("now_ms", &self.now_ms)
            .finish()
    }
}

impl WireCall {
    #[must_use]
    pub fn new(
        interface: impl Into<String>,
        member: impl Into<String>,
        args: Vec<WireValue>,
    ) -> Self {
        Self {
            interface: interface.into(),
            member: member.into(),
            args,
            now_ms: 0,
        }
    }

    #[must_use]
    pub fn with_now_ms(mut self, now_ms: u64) -> Self {
        self.now_ms = now_ms;
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WireReply {
    pub values: Vec<WireValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireError {
    pub name: String,
    pub message: String,
}

impl From<ControlError> for WireError {
    fn from(value: ControlError) -> Self {
        Self {
            name: value.name.to_owned(),
            message: value.message,
        }
    }
}

pub fn decode_call(call: &WireCall) -> Result<ControlRequest, ControlError> {
    match (call.interface.as_str(), call.member.as_str()) {
        (IFACE_ROOT, "Ping") => nullary(call, ControlRequest::Ping),
        (IFACE_ROOT, "GetVersion") => nullary(call, ControlRequest::GetVersion),
        (IFACE_ROOT, "GetCapabilities") => nullary(call, ControlRequest::GetCapabilities),
        (IFACE_ROOT, "GetShellBootstrap") => nullary(call, ControlRequest::GetShellBootstrap),
        (IFACE_WINDOWS, "GetWindows") => nullary(call, ControlRequest::GetWindows),
        (IFACE_APPLICATIONS, "GetApplications") => nullary(call, ControlRequest::GetApplications),
        (IFACE_APPLICATIONS, "LaunchApplication") => {
            expect_arity(call, 3)?;
            Ok(ControlRequest::LaunchApplication {
                app: DesktopAppId::new(required_string(&call.args[0], "appId")?),
                options: ApplicationLaunchOptions {
                    extra_args: required_string_array(&call.args[1], "extraArgs")?.to_vec(),
                    uris: required_string_array(&call.args[2], "uris")?.to_vec(),
                },
            })
        }
        (
            IFACE_WINDOWS,
            member @ ("ActivateWindow" | "MinimizeWindow" | "RestoreWindow" | "CloseWindow"),
        ) => {
            expect_arity(call, 2)?;
            let window = WindowRef::new(
                required_u64(&call.args[0], "windowId")?,
                required_u64(&call.args[1], "generation")?,
            );
            Ok(match member {
                "ActivateWindow" => ControlRequest::ActivateWindow(window),
                "MinimizeWindow" => ControlRequest::MinimizeWindow(window),
                "RestoreWindow" => ControlRequest::RestoreWindow(window),
                _ => ControlRequest::CloseWindow(window),
            })
        }
        (IFACE_SETTINGS, "GetSnapshot") => nullary(call, ControlRequest::GetSettings),
        (IFACE_SETTINGS, "Apply") => {
            expect_arity(call, 2)?;
            let expected_revision = required_u64(&call.args[0], "expectedRevision")?;
            let changes = required_dict(&call.args[1], "changes")?;
            let changes = changes
                .iter()
                .map(|(key, value)| {
                    setting_value_for_key(key, value).map(|value| SettingsChange {
                        key: key.clone(),
                        value: Some(value),
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ControlRequest::ApplySettings(SettingsTransaction {
                expected_revision,
                changes,
                reset_section: None,
            }))
        }
        (IFACE_SETTINGS, "ResetSection") => {
            expect_arity(call, 2)?;
            Ok(ControlRequest::ApplySettings(SettingsTransaction {
                expected_revision: required_u64(&call.args[0], "expectedRevision")?,
                changes: Vec::new(),
                reset_section: Some(required_string(&call.args[1], "section")?.to_owned()),
            }))
        }
        (IFACE_WORKSPACES, "GetSnapshot") => nullary(call, ControlRequest::GetWorkspaces),
        (IFACE_WORKSPACES, "Activate") => {
            workspace_index_request(call, |index, expected_revision| {
                ControlRequest::ActivateWorkspace {
                    index,
                    expected_revision,
                }
            })
        }
        (IFACE_WORKSPACES, "InsertAfter") => {
            workspace_index_request(call, |index, expected_revision| {
                ControlRequest::InsertWorkspaceAfter {
                    index,
                    expected_revision,
                }
            })
        }
        (IFACE_WORKSPACES, "Remove") => {
            workspace_index_request(call, |index, expected_revision| {
                ControlRequest::RemoveWorkspace {
                    index,
                    expected_revision,
                }
            })
        }
        (IFACE_DISPLAYS, "GetSnapshot") => nullary(call, ControlRequest::GetDisplays),
        (IFACE_DISPLAYS, "BeginModeChange") => {
            expect_arity(call, 3)?;
            Ok(ControlRequest::BeginDisplayMode {
                output: OutputId::new(required_string(&call.args[0], "outputKey")?),
                mode: ModeId(required_u64(&call.args[1], "modeId")?),
                expected_generation: required_u64(&call.args[2], "generation")?,
                now_ms: call.now_ms,
            })
        }
        (IFACE_DISPLAYS, "Keep") => {
            expect_arity(call, 1)?;
            Ok(ControlRequest::KeepDisplayMode {
                transaction: TransactionId(required_u64(&call.args[0], "transaction")?),
                now_ms: call.now_ms,
            })
        }
        (IFACE_DISPLAYS, "Revert") => {
            expect_arity(call, 1)?;
            Ok(ControlRequest::RevertDisplayMode {
                transaction: TransactionId(required_u64(&call.args[0], "transaction")?),
            })
        }
        (IFACE_DISPLAYS, "SetShellScale") => {
            expect_arity(call, 3)?;
            let percent = required_u32(&call.args[1], "percent")?;
            Ok(ControlRequest::SetShellScale {
                output: OutputId::new(required_string(&call.args[0], "outputKey")?),
                percent: u16::try_from(percent).map_err(|_| invalid("percent exceeds u16"))?,
                expected_revision: required_u64(&call.args[2], "expectedRevision")?,
            })
        }
        (IFACE_SHORTCUTS, "GetSnapshot") => nullary(call, ControlRequest::GetShortcuts),
        (IFACE_SHORTCUTS, "BeginCapture") => {
            shortcut_action_request(call, |action, expected_revision| {
                ControlRequest::BeginHotkeyCapture {
                    action,
                    expected_revision,
                }
            })
        }
        (IFACE_SHORTCUTS, "EndCapture") => {
            expect_arity(call, 1)?;
            Ok(ControlRequest::EndHotkeyCapture {
                lease: required_u64(&call.args[0], "lease")?,
            })
        }
        (IFACE_SHORTCUTS, "SetBinding") => {
            expect_arity(call, 3)?;
            Ok(ControlRequest::SetShortcut {
                action: required_string(&call.args[0], "action")?.to_owned(),
                binding: required_string(&call.args[1], "binding")?.to_owned(),
                expected_revision: required_u64(&call.args[2], "expectedRevision")?,
            })
        }
        (IFACE_SHORTCUTS, "ClearBinding") => {
            shortcut_action_request(call, |action, expected_revision| {
                ControlRequest::ClearShortcut {
                    action,
                    expected_revision,
                }
            })
        }
        (IFACE_SHORTCUTS, "ResetBinding") => {
            shortcut_action_request(call, |action, expected_revision| {
                ControlRequest::ResetShortcut {
                    action,
                    expected_revision,
                }
            })
        }
        (IFACE_PANELS, "GetSnapshot") => nullary(call, ControlRequest::GetPanels),
        (IFACE_WEATHER, "GetSnapshot") => nullary(call, ControlRequest::GetWeatherFeed),
        (IFACE_WEATHER, "Publish") => {
            expect_arity(call, 1)?;
            Ok(ControlRequest::PublishWeatherFeed {
                payload: required_string(&call.args[0], "payload")?.to_owned(),
            })
        }
        (IFACE_PANELS, "SetEdge") => {
            expect_arity(call, 3)?;
            Ok(ControlRequest::SetPanelEdge {
                output: OutputId::new(required_string(&call.args[0], "outputKey")?),
                edge: parse_edge(required_string(&call.args[1], "edge")?)?,
                expected_revision: required_u64(&call.args[2], "expectedRevision")?,
            })
        }
        (IFACE_PANELS, "SetSize") => {
            expect_arity(call, 3)?;
            Ok(ControlRequest::SetPanelSize {
                output: OutputId::new(required_string(&call.args[0], "outputKey")?),
                logical_size: u16::try_from(required_u32(&call.args[1], "size")?)
                    .map_err(|_| invalid("size exceeds u16"))?,
                expected_revision: required_u64(&call.args[2], "expectedRevision")?,
            })
        }
        (IFACE_PANELS, "Pin") => {
            panel_app_request(call, |app, expected_revision| ControlRequest::PinApp {
                app,
                expected_revision,
            })
        }
        (IFACE_PANELS, "Unpin") => {
            panel_app_request(call, |app, expected_revision| ControlRequest::UnpinApp {
                app,
                expected_revision,
            })
        }
        (IFACE_PANELS, "Reorder") => {
            expect_arity(call, 3)?;
            Ok(ControlRequest::ReorderTask {
                entry_id: TaskEntryId::new(required_string(&call.args[0], "entryId")?),
                index: usize::try_from(required_u32(&call.args[1], "index")?)
                    .map_err(|_| invalid("index does not fit usize"))?,
                expected_revision: required_u64(&call.args[2], "expectedRevision")?,
            })
        }
        (IFACE_SESSION, "GetCapabilities") => nullary(call, ControlRequest::GetSessionCapabilities),
        (IFACE_SESSION, "Lock") => session_action(call, SessionAction::Lock),
        (IFACE_SESSION, "LockPreview") => session_action(call, SessionAction::LockPreview),
        (IFACE_SESSION, "Logout") => session_action(call, SessionAction::Logout),
        (IFACE_SESSION, "Suspend") => session_action(call, SessionAction::Suspend),
        (IFACE_SESSION, "Reboot") => session_action(call, SessionAction::Reboot),
        (IFACE_SESSION, "Shutdown") => session_action(call, SessionAction::Shutdown),
        (IFACE_SESSION, "ScreensaverPreview") => {
            session_action(call, SessionAction::ScreensaverPreview)
        }
        (IFACE_SYSTEM, "GetSnapshot") => nullary(call, ControlRequest::GetSystem),
        (IFACE_SYSTEM, member) => system_action_request(call, member),
        _ => Err(ControlError {
            name: flamewm_control_core::error_name(ErrorCode::NotFound),
            code: ErrorCode::NotFound,
            message: format!(
                "unknown Flame Control method {}.{}",
                call.interface, call.member
            ),
        }),
    }
}

/// Encode typed client request arguments for the D-Bus adapter.
pub fn encode_call(request: &ControlRequest) -> Result<WireCall, ControlError> {
    let call = match request {
        ControlRequest::Ping => nullary_call(IFACE_ROOT, "Ping"),
        ControlRequest::GetVersion => nullary_call(IFACE_ROOT, "GetVersion"),
        ControlRequest::GetCapabilities => nullary_call(IFACE_ROOT, "GetCapabilities"),
        ControlRequest::GetShellBootstrap => nullary_call(IFACE_ROOT, "GetShellBootstrap"),
        ControlRequest::GetWindows => nullary_call(IFACE_WINDOWS, "GetWindows"),
        ControlRequest::GetApplications => nullary_call(IFACE_APPLICATIONS, "GetApplications"),
        ControlRequest::LaunchApplication { app, options } => WireCall::new(
            IFACE_APPLICATIONS,
            "LaunchApplication",
            vec![
                WireValue::String(app.as_str().to_owned()),
                WireValue::StringArray(options.extra_args.clone()),
                WireValue::StringArray(options.uris.clone()),
            ],
        ),
        ControlRequest::ActivateWindow(window) => window_call("ActivateWindow", *window),
        ControlRequest::MinimizeWindow(window) => window_call("MinimizeWindow", *window),
        ControlRequest::RestoreWindow(window) => window_call("RestoreWindow", *window),
        ControlRequest::CloseWindow(window) => window_call("CloseWindow", *window),
        ControlRequest::GetSettings => nullary_call(IFACE_SETTINGS, "GetSnapshot"),
        ControlRequest::ApplySettings(transaction) => match &transaction.reset_section {
            Some(section) => WireCall::new(
                IFACE_SETTINGS,
                "ResetSection",
                vec![
                    WireValue::U64(transaction.expected_revision),
                    WireValue::String(section.clone()),
                ],
            ),
            None => WireCall::new(
                IFACE_SETTINGS,
                "Apply",
                vec![
                    WireValue::U64(transaction.expected_revision),
                    WireValue::Dict(
                        transaction
                            .changes
                            .iter()
                            .filter_map(|change| {
                                change
                                    .value
                                    .as_ref()
                                    .map(|value| (change.key.clone(), setting_wire_value(value)))
                            })
                            .collect(),
                    ),
                ],
            ),
        },
        ControlRequest::GetWorkspaces => nullary_call(IFACE_WORKSPACES, "GetSnapshot"),
        ControlRequest::ActivateWorkspace {
            index,
            expected_revision,
        } => workspace_call("Activate", *index, *expected_revision),
        ControlRequest::InsertWorkspaceAfter {
            index,
            expected_revision,
        } => workspace_call("InsertAfter", *index, *expected_revision),
        ControlRequest::RemoveWorkspace {
            index,
            expected_revision,
        } => workspace_call("Remove", *index, *expected_revision),
        ControlRequest::GetDisplays => nullary_call(IFACE_DISPLAYS, "GetSnapshot"),
        ControlRequest::BeginDisplayMode {
            output,
            mode,
            expected_generation,
            ..
        } => WireCall::new(
            IFACE_DISPLAYS,
            "BeginModeChange",
            vec![
                WireValue::String(output.as_str().to_owned()),
                WireValue::U64(mode.0),
                WireValue::U64(*expected_generation),
            ],
        ),
        ControlRequest::KeepDisplayMode { transaction, .. } => {
            WireCall::new(IFACE_DISPLAYS, "Keep", vec![WireValue::U64(transaction.0)])
        }
        ControlRequest::RevertDisplayMode { transaction } => WireCall::new(
            IFACE_DISPLAYS,
            "Revert",
            vec![WireValue::U64(transaction.0)],
        ),
        ControlRequest::SetShellScale {
            output,
            percent,
            expected_revision,
        } => WireCall::new(
            IFACE_DISPLAYS,
            "SetShellScale",
            vec![
                WireValue::String(output.as_str().to_owned()),
                WireValue::U32(u32::from(*percent)),
                WireValue::U64(*expected_revision),
            ],
        ),
        ControlRequest::GetShortcuts => nullary_call(IFACE_SHORTCUTS, "GetSnapshot"),
        ControlRequest::BeginHotkeyCapture {
            action,
            expected_revision,
        } => named_call(IFACE_SHORTCUTS, "BeginCapture", action, *expected_revision),
        ControlRequest::EndHotkeyCapture { lease } => {
            WireCall::new(IFACE_SHORTCUTS, "EndCapture", vec![WireValue::U64(*lease)])
        }
        ControlRequest::SetShortcut {
            action,
            binding,
            expected_revision,
        } => WireCall::new(
            IFACE_SHORTCUTS,
            "SetBinding",
            vec![
                WireValue::String(action.clone()),
                WireValue::String(binding.clone()),
                WireValue::U64(*expected_revision),
            ],
        ),
        ControlRequest::ClearShortcut {
            action,
            expected_revision,
        } => named_call(IFACE_SHORTCUTS, "ClearBinding", action, *expected_revision),
        ControlRequest::ResetShortcut {
            action,
            expected_revision,
        } => named_call(IFACE_SHORTCUTS, "ResetBinding", action, *expected_revision),
        ControlRequest::GetPanels => nullary_call(IFACE_PANELS, "GetSnapshot"),
        ControlRequest::GetWeatherFeed => nullary_call(IFACE_WEATHER, "GetSnapshot"),
        ControlRequest::PublishWeatherFeed { payload } => WireCall::new(
            IFACE_WEATHER,
            "Publish",
            vec![WireValue::String(payload.clone())],
        ),
        ControlRequest::SetPanelEdge {
            output,
            edge,
            expected_revision,
        } => WireCall::new(
            IFACE_PANELS,
            "SetEdge",
            vec![
                WireValue::String(output.as_str().to_owned()),
                WireValue::String(edge_name(*edge).to_owned()),
                WireValue::U64(*expected_revision),
            ],
        ),
        ControlRequest::SetPanelSize {
            output,
            logical_size,
            expected_revision,
        } => WireCall::new(
            IFACE_PANELS,
            "SetSize",
            vec![
                WireValue::String(output.as_str().to_owned()),
                WireValue::U32(u32::from(*logical_size)),
                WireValue::U64(*expected_revision),
            ],
        ),
        ControlRequest::PinApp {
            app,
            expected_revision,
        } => named_call(IFACE_PANELS, "Pin", app.as_str(), *expected_revision),
        ControlRequest::UnpinApp {
            app,
            expected_revision,
        } => named_call(IFACE_PANELS, "Unpin", app.as_str(), *expected_revision),
        ControlRequest::ReorderTask {
            entry_id,
            index,
            expected_revision,
        } => WireCall::new(
            IFACE_PANELS,
            "Reorder",
            vec![
                WireValue::String(entry_id.as_str().to_owned()),
                WireValue::U32(u32_saturating(*index)),
                WireValue::U64(*expected_revision),
            ],
        ),
        ControlRequest::GetSessionCapabilities => nullary_call(IFACE_SESSION, "GetCapabilities"),
        ControlRequest::GetSystem => nullary_call(IFACE_SYSTEM, "GetSnapshot"),
        ControlRequest::SystemAction {
            action,
            expected_revision,
        } => system_action_call(action, *expected_revision),
        ControlRequest::SessionAction(action) => {
            WireCall::new(IFACE_SESSION, session_method(*action), Vec::new())
        }
        ControlRequest::ApplyShortcuts { .. } => {
            return Err(invalid("ApplyShortcuts has no D-Bus method"));
        }
    };
    Ok(call)
}

fn workspace_call(member: &str, index: usize, revision: u64) -> WireCall {
    WireCall::new(
        IFACE_WORKSPACES,
        member,
        vec![
            WireValue::U32(u32_saturating(index)),
            WireValue::U64(revision),
        ],
    )
}

fn named_call(interface: &str, member: &str, name: &str, revision: u64) -> WireCall {
    WireCall::new(
        interface,
        member,
        vec![WireValue::String(name.to_owned()), WireValue::U64(revision)],
    )
}

fn window_call(member: &str, window: WindowRef) -> WireCall {
    WireCall::new(
        IFACE_WINDOWS,
        member,
        vec![WireValue::U64(window.id), WireValue::U64(window.generation)],
    )
}

const fn session_method(action: SessionAction) -> &'static str {
    match action {
        SessionAction::Lock => "Lock",
        SessionAction::LockPreview => "LockPreview",
        SessionAction::Logout => "Logout",
        SessionAction::Suspend => "Suspend",
        SessionAction::Reboot => "Reboot",
        SessionAction::Shutdown => "Shutdown",
        SessionAction::ScreensaverPreview => "ScreensaverPreview",
    }
}

pub fn encode_reply(
    call: &WireCall,
    response: &ControlResponse,
) -> Result<WireReply, ControlError> {
    let values = match (call.interface.as_str(), call.member.as_str(), response) {
        (IFACE_ROOT, "Ping", ControlResponse::Pong) => vec![WireValue::String("pong".to_owned())],
        (IFACE_ROOT, "GetVersion", ControlResponse::Version(version)) => {
            vec![WireValue::String(version.to_string())]
        }
        (IFACE_ROOT, "GetCapabilities", ControlResponse::Capabilities(items)) => {
            vec![WireValue::StringArray(items.clone())]
        }
        (IFACE_ROOT, "GetShellBootstrap", ControlResponse::ShellBootstrap(snapshot)) => {
            vec![WireValue::Dict(shell_bootstrap_dict(snapshot))]
        }
        (IFACE_WINDOWS, "GetWindows", ControlResponse::Windows(windows)) => {
            vec![WireValue::Array(
                windows.iter().map(window_wire_value).collect(),
            )]
        }
        (IFACE_APPLICATIONS, "GetApplications", ControlResponse::Applications(applications)) => {
            vec![WireValue::Array(
                applications.iter().map(application_wire_value).collect(),
            )]
        }
        (IFACE_SETTINGS, "GetSnapshot", ControlResponse::Settings(snapshot)) => {
            vec![WireValue::Dict(settings_dict(snapshot))]
        }
        (IFACE_SETTINGS, "Apply" | "ResetSection", ControlResponse::Settings(snapshot)) => {
            vec![WireValue::Dict(settings_dict(snapshot))]
        }
        (IFACE_WORKSPACES, "GetSnapshot", ControlResponse::Workspaces(snapshot)) => {
            vec![WireValue::Dict(workspace_dict(snapshot))]
        }
        (IFACE_DISPLAYS, "GetSnapshot", ControlResponse::Displays(snapshot)) => {
            vec![WireValue::Dict(display_dict(snapshot))]
        }
        (IFACE_DISPLAYS, "BeginModeChange", ControlResponse::Transaction(transaction)) => {
            vec![WireValue::U64(transaction.0)]
        }
        (IFACE_SHORTCUTS, "GetSnapshot", ControlResponse::Shortcuts(snapshot)) => {
            vec![WireValue::Dict(shortcut_dict(snapshot))]
        }
        (IFACE_SHORTCUTS, "BeginCapture", ControlResponse::CaptureLease(lease)) => {
            vec![WireValue::U64(*lease)]
        }
        (IFACE_PANELS, "GetSnapshot", ControlResponse::Panels(snapshot)) => {
            vec![WireValue::Dict(panels_dict(snapshot))]
        }
        (IFACE_SESSION, "GetCapabilities", ControlResponse::SessionCapabilities(capabilities)) => {
            vec![WireValue::StringArray(session_capability_names(
                *capabilities,
            ))]
        }
        (IFACE_WEATHER, "GetSnapshot", ControlResponse::WeatherFeed(snapshot)) => {
            vec![WireValue::Dict(weather_feed_dict(snapshot))]
        }
        (IFACE_SYSTEM, "GetSnapshot", ControlResponse::System(snapshot)) => {
            vec![WireValue::Dict(system_dict(snapshot))]
        }
        (_, _, ControlResponse::Unit | ControlResponse::Changed(_)) => Vec::new(),
        _ => {
            return Err(ControlError {
                name: flamewm_control_core::error_name(ErrorCode::InternalFailure),
                code: ErrorCode::InternalFailure,
                message: format!("response does not match {}.{}", call.interface, call.member),
            });
        }
    };
    Ok(WireReply { values })
}

/// Decode the reply for the exact request call. This is the client-side inverse of `encode_reply`.
pub fn decode_reply(call: &WireCall, reply: &WireReply) -> Result<ControlResponse, ControlError> {
    match (call.interface.as_str(), call.member.as_str()) {
        (IFACE_ROOT, "Ping") => {
            let pong = required_string(single_reply(reply)?, "reply")?;
            if pong != "pong" {
                return Err(invalid("Ping reply must be pong"));
            }
            Ok(ControlResponse::Pong)
        }
        (IFACE_ROOT, "GetVersion") => {
            let version = Version::parse(required_string(single_reply(reply)?, "version")?)
                .ok_or_else(|| invalid("version is not canonical major.minor.patch"))?;
            Ok(ControlResponse::Version(version))
        }
        (IFACE_ROOT, "GetCapabilities") => Ok(ControlResponse::Capabilities(
            required_string_array(single_reply(reply)?, "capabilities")?.to_vec(),
        )),
        (IFACE_ROOT, "GetShellBootstrap") => Ok(ControlResponse::ShellBootstrap(
            shell_bootstrap_snapshot(required_dict(single_reply(reply)?, "snapshot")?)?,
        )),
        (IFACE_WINDOWS, "GetWindows") => Ok(ControlResponse::Windows(
            required_array(single_reply(reply)?, "windows")?
                .iter()
                .map(window_snapshot)
                .collect::<Result<Vec<_>, _>>()?,
        )),
        (IFACE_APPLICATIONS, "GetApplications") => Ok(ControlResponse::Applications(
            required_array(single_reply(reply)?, "applications")?
                .iter()
                .map(application_snapshot)
                .collect::<Result<Vec<_>, _>>()?,
        )),
        (IFACE_SETTINGS, "GetSnapshot" | "Apply" | "ResetSection") => {
            Ok(ControlResponse::Settings(settings_snapshot(
                required_dict(single_reply(reply)?, "snapshot")?,
            )?))
        }
        (IFACE_WORKSPACES, "GetSnapshot") => Ok(ControlResponse::Workspaces(workspace_snapshot(
            required_dict(single_reply(reply)?, "snapshot")?,
        )?)),
        (IFACE_DISPLAYS, "GetSnapshot") => Ok(ControlResponse::Displays(display_snapshot(
            required_dict(single_reply(reply)?, "snapshot")?,
        )?)),
        (IFACE_DISPLAYS, "BeginModeChange") => Ok(ControlResponse::Transaction(TransactionId(
            required_u64(single_reply(reply)?, "transaction")?,
        ))),
        (IFACE_SHORTCUTS, "BeginCapture") => Ok(ControlResponse::CaptureLease(required_u64(
            single_reply(reply)?,
            "lease",
        )?)),
        (IFACE_SHORTCUTS, "GetSnapshot") => Ok(ControlResponse::Shortcuts(shortcut_snapshot(
            required_dict(single_reply(reply)?, "snapshot")?,
        )?)),
        (IFACE_PANELS, "GetSnapshot") => Ok(ControlResponse::Panels(panels_snapshot(
            required_dict(single_reply(reply)?, "snapshot")?,
        )?)),
        (IFACE_SESSION, "GetCapabilities") => Ok(ControlResponse::SessionCapabilities(
            session_capabilities(required_string_array(single_reply(reply)?, "capabilities")?),
        )),
        (IFACE_WEATHER, "GetSnapshot") => Ok(ControlResponse::WeatherFeed(weather_feed_snapshot(
            required_dict(single_reply(reply)?, "snapshot")?,
        )?)),
        (IFACE_SYSTEM, "GetSnapshot") => Ok(ControlResponse::System(system_snapshot(
            required_dict(single_reply(reply)?, "snapshot")?,
        )?)),
        (
            IFACE_SYSTEM,
            "SetWifiEnabled"
            | "ConnectKnown"
            | "ConnectWifi"
            | "SubmitNetworkSecret"
            | "CancelNetworkSecret"
            | "Disconnect"
            | "Scan"
            | "Play"
            | "Pause"
            | "PlayPause"
            | "Next"
            | "Previous"
            | "SetVolume"
            | "SetMute"
            | "BatteryMenuOpened"
            | "BatteryMenuClosed",
        )
        | (IFACE_WORKSPACES, "Activate" | "InsertAfter" | "Remove")
        | (IFACE_DISPLAYS, "Keep" | "Revert" | "SetShellScale")
        | (IFACE_WEATHER, "Publish")
        | (IFACE_SHORTCUTS, "SetBinding" | "ClearBinding" | "ResetBinding" | "EndCapture")
        | (IFACE_PANELS, "SetEdge" | "SetSize" | "Pin" | "Unpin" | "Reorder")
        | (
            IFACE_SESSION,
            "Lock" | "LockPreview" | "Logout" | "Suspend" | "Reboot" | "Shutdown"
            | "ScreensaverPreview",
        )
        | (IFACE_WINDOWS, "ActivateWindow" | "MinimizeWindow" | "RestoreWindow" | "CloseWindow")
        | (IFACE_APPLICATIONS, "LaunchApplication") => {
            expect_reply_arity(reply, 0)?;
            Ok(ControlResponse::Unit)
        }
        _ => Err(ControlError {
            name: flamewm_control_core::error_name(ErrorCode::NotFound),
            code: ErrorCode::NotFound,
            message: format!(
                "unknown Flame Control reply {}.{}",
                call.interface, call.member
            ),
        }),
    }
}

fn settings_snapshot(dict: &WireDict) -> Result<SettingsSnapshot, ControlError> {
    let revision = dict_u64(dict, "revision")?;
    let mut snapshot = SettingsSnapshot::new(revision);
    for (key, value) in dict {
        if key == "revision" {
            continue;
        }
        let typed = setting_value_for_key(key, value)?;
        snapshot.values.insert(key.clone(), typed);
    }
    Ok(snapshot)
}

fn window_snapshot(value: &WireValue) -> Result<WindowSnapshot, ControlError> {
    let dict = required_dict(value, "window")?;
    let state = match dict_string(dict, "state")? {
        "normal" => WindowState::Normal,
        "minimized" => WindowState::Minimized,
        "maximized" => WindowState::Maximized,
        "fullscreen" => WindowState::Fullscreen,
        "hidden" => WindowState::Hidden,
        _ => return Err(invalid("window state is invalid")),
    };
    Ok(WindowSnapshot {
        reference: WindowRef::new(dict_u64(dict, "windowId")?, dict_u64(dict, "generation")?),
        title: dict_string(dict, "title")?.to_owned(),
        app_id: DesktopAppId::new(dict_string(dict, "appId")?),
        outer_geometry: rect_dict(dict, "outer")?,
        restore_geometry: rect_dict(dict, "restore")?,
        state,
        sticky: dict_bool(dict, "sticky")?,
        focused: dict_bool(dict, "focused")?,
        workspace: flamewm_api::WorkspaceRef::new(
            dict_i32(dict, "workspaceIndex")?,
            dict_u64(dict, "workspaceRevision")?,
        ),
        output: OutputId::new(dict_string(dict, "output")?),
        state_generation: dict_u64(dict, "stateGeneration")?,
    })
}

fn application_snapshot(value: &WireValue) -> Result<DesktopApplication, ControlError> {
    let dict = required_dict(value, "application")?;
    Ok(DesktopApplication {
        id: DesktopAppId::new(dict_string(dict, "id")?),
        name: dict_string(dict, "name")?.to_owned(),
        generic_name: dict_string(dict, "genericName")?.to_owned(),
        comment: dict_string(dict, "comment")?.to_owned(),
        startup_wm_class: dict_string(dict, "startupWmClass")?.to_owned(),
        argv: dict_string_array(dict, "argv")?.to_vec(),
        keywords: dict_string_array(dict, "keywords")?.to_vec(),
        icon_name: dict_string(dict, "iconName")?.to_owned(),
        categories: dict_string_array(dict, "categories")?.to_vec(),
    })
}

fn rect_dict(dict: &WireDict, prefix: &str) -> Result<Rect, ControlError> {
    Ok(Rect::new(
        dict_i32(dict, &format!("{prefix}X"))?,
        dict_i32(dict, &format!("{prefix}Y"))?,
        dict_i32(dict, &format!("{prefix}Width"))?,
        dict_i32(dict, &format!("{prefix}Height"))?,
    ))
}

fn workspace_snapshot(dict: &WireDict) -> Result<WorkspaceSnapshot, ControlError> {
    let count = usize::try_from(dict_u32(dict, "count")?)
        .map_err(|_| invalid("workspace count does not fit usize"))?;
    let active = usize::try_from(dict_i32(dict, "activeIndex")?)
        .map_err(|_| invalid("active workspace must be nonnegative"))?;
    let last_raw = dict_i32(dict, "lastIndex")?;
    let last_index = if last_raw < 0 {
        None
    } else {
        Some(usize::try_from(last_raw).map_err(|_| invalid("last workspace does not fit usize"))?)
    };
    let names = dict_string_array(dict, "names")?.to_vec();
    let snapshot = WorkspaceSnapshot {
        revision: dict_u64(dict, "revision")?,
        count,
        active_index: active,
        last_index,
        names,
    };
    snapshot.validate().map_err(ControlError::from)?;
    Ok(snapshot)
}

fn shell_bootstrap_snapshot(dict: &WireDict) -> Result<ShellBootstrapSnapshot, ControlError> {
    Ok(ShellBootstrapSnapshot {
        windows_revision: dict_u64(dict, "windowsRevision")?,
        windows: required_array(dict_value(dict, "windows")?, "windows")?
            .iter()
            .map(window_snapshot)
            .collect::<Result<Vec<_>, _>>()?,
        workspaces: workspace_snapshot(required_dict(
            dict_value(dict, "workspaces")?,
            "workspaces",
        )?)?,
        panels: panels_snapshot(required_dict(dict_value(dict, "panels")?, "panels")?)?,
        applications: required_array(dict_value(dict, "applications")?, "applications")?
            .iter()
            .map(application_snapshot)
            .collect::<Result<Vec<_>, _>>()?,
        session: session_capabilities(dict_string_array(dict, "sessionCapabilities")?),
        displays: display_snapshot(required_dict(dict_value(dict, "displays")?, "displays")?)?,
        system: system_snapshot(required_dict(dict_value(dict, "system")?, "system")?)?,
    })
}

fn display_snapshot(dict: &WireDict) -> Result<DisplaySnapshot, ControlError> {
    let generation = dict_u64(dict, "generation")?;
    let outputs = dict.get("outputs").map_or(Ok(Vec::new()), |value| {
        required_array(value, "outputs")?
            .iter()
            .map(output_snapshot)
            .collect::<Result<Vec<_>, _>>()
    })?;
    let pending = if dict
        .get("hasPending")
        .and_then(WireValue::as_bool)
        .unwrap_or(false)
    {
        Some(PendingModeChange {
            transaction: TransactionId(dict_u64(dict, "pendingTx")?),
            output: OutputId::new(dict_string(dict, "pendingOutput")?),
            mode: ModeId(dict_u64(dict, "pendingMode")?),
            deadline_ms: dict_u64(dict, "pendingDeadlineMs")?,
        })
    } else {
        None
    };
    Ok(DisplaySnapshot {
        generation,
        outputs,
        pending,
    })
}

fn output_snapshot(value: &WireValue) -> Result<OutputSnapshot, ControlError> {
    let dict = required_dict(value, "output")?;
    let modes = dict.get("modes").map_or(Ok(Vec::new()), |modes| {
        required_array(modes, "modes")?
            .iter()
            .map(display_mode)
            .collect::<Result<Vec<_>, _>>()
    })?;
    let scale = dict_u32(dict, "scale")?;
    Ok(OutputSnapshot {
        id: OutputId::new(dict_string(dict, "id")?),
        connector: dict_string(dict, "connector")?.to_owned(),
        edid_identity: dict_string(dict, "edid")?.to_owned(),
        connected: dict_bool(dict, "connected")?,
        primary: dict_bool(dict, "primary")?,
        geometry: Rect::new(
            dict_i32(dict, "x")?,
            dict_i32(dict, "y")?,
            dict_i32(dict, "width")?,
            dict_i32(dict, "height")?,
        ),
        current_mode: ModeId(dict_u64(dict, "mode")?),
        modes,
        shell_scale_percent: u16::try_from(scale).map_err(|_| invalid("scale exceeds u16"))?,
    })
}

fn display_mode(value: &WireValue) -> Result<DisplayMode, ControlError> {
    let dict = required_dict(value, "mode")?;
    Ok(DisplayMode {
        id: ModeId(dict_u64(dict, "id")?),
        resolution: Size::new(dict_i32(dict, "width")?, dict_i32(dict, "height")?),
        refresh_millihz: dict_i32(dict, "refreshMillihz")?,
        preferred: dict_bool(dict, "preferred")?,
    })
}

fn shortcut_snapshot(dict: &WireDict) -> Result<ShortcutSnapshot, ControlError> {
    let revision = dict_u64(dict, "revision")?;
    let bindings = dict
        .iter()
        .filter(|(key, _)| key.as_str() != "revision")
        .map(|(key, value)| {
            Ok((
                key.clone(),
                KeyBinding::assigned(required_string(value, key)?),
            ))
        })
        .collect::<Result<BTreeMap<_, _>, ControlError>>()?;
    Ok(ShortcutSnapshot { revision, bindings })
}

fn panels_snapshot(dict: &WireDict) -> Result<PanelsSnapshot, ControlError> {
    let panels = dict.get("panels").map_or(Ok(Vec::new()), |value| {
        required_array(value, "panels")?
            .iter()
            .map(panel_snapshot)
            .collect::<Result<Vec<_>, _>>()
    })?;
    let tasks = dict.get("tasks").map_or(Ok(Vec::new()), |value| {
        required_array(value, "tasks")?
            .iter()
            .map(task_entry)
            .collect::<Result<Vec<_>, _>>()
    })?;
    let pinned_apps = match dict.get("pinnedApps") {
        Some(value) => required_string_array(value, "pinnedApps")?
            .iter()
            .map(DesktopAppId::new)
            .collect(),
        None => Vec::new(),
    };
    Ok(PanelsSnapshot {
        revision: dict_u64(dict, "revision")?,
        panels,
        tasks,
        pinned_apps,
    })
}

fn panel_snapshot(value: &WireValue) -> Result<PanelSnapshot, ControlError> {
    let dict = required_dict(value, "panel")?;
    let size = dict_u32(dict, "size")?;
    Ok(PanelSnapshot {
        output: OutputId::new(dict_string(dict, "output")?),
        edge: parse_edge(dict_string(dict, "edge")?)?,
        logical_size: u16::try_from(size).map_err(|_| invalid("panel size exceeds u16"))?,
        visible: dict_bool(dict, "visible")?,
        geometry: Rect::new(
            dict_i32(dict, "x")?,
            dict_i32(dict, "y")?,
            dict_i32(dict, "width")?,
            dict_i32(dict, "height")?,
        ),
    })
}

fn task_entry(value: &WireValue) -> Result<TaskEntry, ControlError> {
    let dict = required_dict(value, "task")?;
    let kind = match dict_string(dict, "kind")? {
        "pin" => {
            let window = if dict.contains_key("windowId") {
                Some(WindowRef::new(
                    dict_u64(dict, "windowId")?,
                    dict_u64(dict, "windowGeneration")?,
                ))
            } else {
                None
            };
            TaskEntryKind::PinnedSlot { window }
        }
        "window" => TaskEntryKind::Window {
            window: WindowRef::new(
                dict_u64(dict, "windowId")?,
                dict_u64(dict, "windowGeneration")?,
            ),
        },
        _ => return Err(invalid("task kind must be pin or window")),
    };
    Ok(TaskEntry {
        id: TaskEntryId::new(dict_string(dict, "id")?),
        app_id: DesktopAppId::new(dict_string(dict, "appId")?),
        kind,
        order_index: usize::try_from(dict_u32(dict, "order")?)
            .map_err(|_| invalid("task order does not fit usize"))?,
    })
}

fn session_capabilities(names: &[String]) -> SessionCapabilities {
    SessionCapabilities {
        lock: names.iter().any(|name| name == "lock"),
        logout: names.iter().any(|name| name == "logout"),
        suspend: names.iter().any(|name| name == "suspend"),
        reboot: names.iter().any(|name| name == "reboot"),
        shutdown: names.iter().any(|name| name == "shutdown"),
    }
}

/// Build a wire dict from literal entries, so each key is inserted exactly once.
fn dict_of<const N: usize>(entries: [(&str, WireValue); N]) -> WireDict {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

/// Single-value reply: enforces arity 1 and returns the only value.
fn single_reply(reply: &WireReply) -> Result<&WireValue, ControlError> {
    expect_reply_arity(reply, 1)?;
    Ok(&reply.values[0])
}

fn expect_reply_arity(reply: &WireReply, expected: usize) -> Result<(), ControlError> {
    if reply.values.len() == expected {
        Ok(())
    } else {
        Err(invalid(&format!("reply expects {expected} values")))
    }
}

fn required_array<'a>(value: &'a WireValue, name: &str) -> Result<&'a [WireValue], ControlError> {
    value
        .as_array()
        .ok_or_else(|| invalid(&format!("{name} must be array")))
}

fn required_string_array<'a>(
    value: &'a WireValue,
    name: &str,
) -> Result<&'a [String], ControlError> {
    value
        .as_string_array()
        .ok_or_else(|| invalid(&format!("{name} must be string array")))
}

fn dict_value<'a>(dict: &'a WireDict, key: &str) -> Result<&'a WireValue, ControlError> {
    dict.get(key)
        .ok_or_else(|| invalid(&format!("snapshot missing {key}")))
}

fn dict_u64(dict: &WireDict, key: &str) -> Result<u64, ControlError> {
    required_u64(dict_value(dict, key)?, key)
}
fn dict_u32(dict: &WireDict, key: &str) -> Result<u32, ControlError> {
    required_u32(dict_value(dict, key)?, key)
}
fn dict_i32(dict: &WireDict, key: &str) -> Result<i32, ControlError> {
    dict_value(dict, key)?
        .as_i32()
        .ok_or_else(|| invalid(&format!("{key} must be int32")))
}
fn dict_bool(dict: &WireDict, key: &str) -> Result<bool, ControlError> {
    dict_value(dict, key)?
        .as_bool()
        .ok_or_else(|| invalid(&format!("{key} must be bool")))
}
fn dict_string<'a>(dict: &'a WireDict, key: &str) -> Result<&'a str, ControlError> {
    required_string(dict_value(dict, key)?, key)
}
fn dict_string_array<'a>(dict: &'a WireDict, key: &str) -> Result<&'a [String], ControlError> {
    required_string_array(dict_value(dict, key)?, key)
}

/// One row of the Alt+Tab switcher as the shell renders it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitcherEntry {
    pub title: String,
    pub app_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlSignal {
    WindowsChanged {
        revision: u64,
    },
    WindowsSnapshotChanged {
        revision: u64,
        windows: Vec<WindowSnapshot>,
    },
    SettingsChanged {
        revision: u64,
        keys: Vec<String>,
    },
    SettingsSnapshotChanged {
        snapshot: SettingsSnapshot,
    },
    WorkspacesChanged {
        revision: u64,
    },
    WorkspacesSnapshotChanged {
        snapshot: WorkspaceSnapshot,
    },
    TopologyChanged {
        generation: u64,
    },
    DisplaysSnapshotChanged {
        snapshot: DisplaySnapshot,
    },
    DisplayTransactionChanged {
        transaction: u64,
        state: String,
        deadline_ms: u64,
    },
    StartMenuToggleRequested,
    LauncherToggleRequested,
    /// Alt+Tab switcher state, pushed by the WM on open, selection change and close. `entries`
    /// are in MRU order; `selected` indexes them. `immediate` asks for the overlay to show
    /// without the hold delay (a second Tab press arrived). The shell only renders this.
    WindowSwitcherChanged {
        open: bool,
        immediate: bool,
        selected: u32,
        entries: Vec<SwitcherEntry>,
    },
    ShortcutsChanged {
        revision: u64,
    },
    PanelsChanged {
        revision: u64,
    },
    PanelsSnapshotChanged {
        snapshot: PanelsSnapshot,
    },
    SystemSnapshotChanged {
        snapshot: SystemSnapshot,
    },
    SystemChanged {
        revision: u64,
    },
    WeatherFeedChanged {
        snapshot: flamewm_api::weather::WeatherFeedSnapshot,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireSignal {
    pub interface: String,
    pub member: String,
    pub args: Vec<WireValue>,
}

#[must_use]
pub fn encode_signal(signal: &ControlSignal) -> WireSignal {
    match signal {
        ControlSignal::WindowsChanged { revision } => {
            revision_signal(IFACE_WINDOWS, "WindowsChanged", *revision)
        }
        ControlSignal::WindowsSnapshotChanged { revision, windows } => WireSignal {
            interface: IFACE_WINDOWS.to_owned(),
            member: "WindowsSnapshotChanged".to_owned(),
            args: vec![
                WireValue::U64(*revision),
                WireValue::Array(windows.iter().map(window_wire_value).collect()),
            ],
        },
        ControlSignal::WeatherFeedChanged { snapshot } => WireSignal {
            interface: IFACE_WEATHER.to_owned(),
            member: "FeedChanged".to_owned(),
            args: vec![WireValue::Dict(weather_feed_dict(snapshot))],
        },
        ControlSignal::SystemChanged { revision } => WireSignal {
            interface: IFACE_SYSTEM.to_owned(),
            member: "SystemChanged".to_owned(),
            args: vec![WireValue::U64(*revision)],
        },
        ControlSignal::SystemSnapshotChanged { snapshot } => WireSignal {
            interface: IFACE_SYSTEM.to_owned(),
            member: "SystemSnapshotChanged".to_owned(),
            args: vec![WireValue::Dict(system_dict(snapshot))],
        },
        ControlSignal::SettingsChanged { revision, keys } => WireSignal {
            interface: IFACE_SETTINGS.to_owned(),
            member: "SettingsChanged".to_owned(),
            args: vec![
                WireValue::U64(*revision),
                WireValue::StringArray(keys.clone()),
            ],
        },
        ControlSignal::SettingsSnapshotChanged { snapshot } => WireSignal {
            interface: IFACE_SETTINGS.to_owned(),
            member: "SettingsSnapshotChanged".to_owned(),
            args: vec![WireValue::Dict(settings_dict(snapshot))],
        },
        ControlSignal::WorkspacesChanged { revision } => {
            revision_signal(IFACE_WORKSPACES, "WorkspacesChanged", *revision)
        }
        ControlSignal::WorkspacesSnapshotChanged { snapshot } => WireSignal {
            interface: IFACE_WORKSPACES.to_owned(),
            member: "WorkspacesSnapshotChanged".to_owned(),
            args: vec![WireValue::Dict(workspace_dict(snapshot))],
        },
        ControlSignal::TopologyChanged { generation } => {
            revision_signal(IFACE_DISPLAYS, "TopologyChanged", *generation)
        }
        ControlSignal::DisplaysSnapshotChanged { snapshot } => WireSignal {
            interface: IFACE_DISPLAYS.to_owned(),
            member: "DisplaysSnapshotChanged".to_owned(),
            args: vec![WireValue::Dict(display_dict(snapshot))],
        },
        ControlSignal::StartMenuToggleRequested => WireSignal {
            interface: IFACE_ROOT.to_owned(),
            member: "StartMenuToggleRequested".to_owned(),
            args: Vec::new(),
        },
        ControlSignal::LauncherToggleRequested => WireSignal {
            interface: IFACE_ROOT.to_owned(),
            member: "LauncherToggleRequested".to_owned(),
            args: Vec::new(),
        },
        ControlSignal::WindowSwitcherChanged {
            open,
            immediate,
            selected,
            entries,
        } => WireSignal {
            interface: IFACE_ROOT.to_owned(),
            member: "WindowSwitcherChanged".to_owned(),
            args: vec![
                WireValue::Bool(*open),
                WireValue::Bool(*immediate),
                WireValue::U32(*selected),
                WireValue::Array(entries.iter().map(switcher_entry_wire_value).collect()),
            ],
        },
        ControlSignal::ShortcutsChanged { revision } => {
            revision_signal(IFACE_SHORTCUTS, "ShortcutsChanged", *revision)
        }
        ControlSignal::PanelsChanged { revision } => {
            revision_signal(IFACE_PANELS, "PanelsChanged", *revision)
        }
        ControlSignal::PanelsSnapshotChanged { snapshot } => WireSignal {
            interface: IFACE_PANELS.to_owned(),
            member: "PanelsSnapshotChanged".to_owned(),
            args: vec![WireValue::Dict(panels_dict(snapshot))],
        },
        ControlSignal::DisplayTransactionChanged {
            transaction,
            state,
            deadline_ms,
        } => WireSignal {
            interface: IFACE_DISPLAYS.to_owned(),
            member: "DisplayTransactionChanged".to_owned(),
            args: vec![
                WireValue::U64(*transaction),
                WireValue::String(state.clone()),
                WireValue::U64(*deadline_ms),
            ],
        },
    }
}

pub fn decode_signal(signal: &WireSignal) -> Result<ControlSignal, ControlError> {
    let call = WireCall::new(&signal.interface, &signal.member, signal.args.clone());
    match (signal.interface.as_str(), signal.member.as_str()) {
        (IFACE_WINDOWS, "WindowsChanged") => Ok(ControlSignal::WindowsChanged {
            revision: signal_revision(&call)?,
        }),
        (IFACE_WINDOWS, "WindowsSnapshotChanged") => {
            expect_arity(&call, 2)?;
            Ok(ControlSignal::WindowsSnapshotChanged {
                revision: required_u64(&call.args[0], "revision")?,
                windows: required_array(&call.args[1], "windows")?
                    .iter()
                    .map(window_snapshot)
                    .collect::<Result<Vec<_>, _>>()?,
            })
        }
        (IFACE_WEATHER, "FeedChanged") => {
            expect_arity(&call, 1)?;
            Ok(ControlSignal::WeatherFeedChanged {
                snapshot: weather_feed_snapshot(required_dict(&call.args[0], "snapshot")?)?,
            })
        }
        (IFACE_SYSTEM, "SystemChanged") => Ok(ControlSignal::SystemChanged {
            revision: signal_revision(&call)?,
        }),
        (IFACE_SYSTEM, "SystemSnapshotChanged") => {
            expect_arity(&call, 1)?;
            Ok(ControlSignal::SystemSnapshotChanged {
                snapshot: system_snapshot(required_dict(&call.args[0], "snapshot")?)?,
            })
        }
        (IFACE_SETTINGS, "SettingsChanged") => {
            expect_arity(&call, 2)?;
            Ok(ControlSignal::SettingsChanged {
                revision: required_u64(&call.args[0], "revision")?,
                keys: required_string_array(&call.args[1], "keys")?.to_vec(),
            })
        }
        (IFACE_SETTINGS, "SettingsSnapshotChanged") => {
            expect_arity(&call, 1)?;
            Ok(ControlSignal::SettingsSnapshotChanged {
                snapshot: settings_snapshot(required_dict(&call.args[0], "snapshot")?)?,
            })
        }
        (IFACE_WORKSPACES, "WorkspacesChanged") => Ok(ControlSignal::WorkspacesChanged {
            revision: signal_revision(&call)?,
        }),
        (IFACE_WORKSPACES, "WorkspacesSnapshotChanged") => {
            expect_arity(&call, 1)?;
            Ok(ControlSignal::WorkspacesSnapshotChanged {
                snapshot: workspace_snapshot(required_dict(&call.args[0], "snapshot")?)?,
            })
        }
        (IFACE_DISPLAYS, "TopologyChanged") => Ok(ControlSignal::TopologyChanged {
            generation: signal_revision(&call)?,
        }),
        (IFACE_DISPLAYS, "DisplaysSnapshotChanged") => {
            expect_arity(&call, 1)?;
            Ok(ControlSignal::DisplaysSnapshotChanged {
                snapshot: display_snapshot(required_dict(&call.args[0], "snapshot")?)?,
            })
        }
        (IFACE_ROOT, "StartMenuToggleRequested") => {
            expect_arity(&call, 0)?;
            Ok(ControlSignal::StartMenuToggleRequested)
        }
        (IFACE_ROOT, "LauncherToggleRequested") => {
            expect_arity(&call, 0)?;
            Ok(ControlSignal::LauncherToggleRequested)
        }
        (IFACE_ROOT, "WindowSwitcherChanged") => {
            expect_arity(&call, 4)?;
            Ok(ControlSignal::WindowSwitcherChanged {
                open: required_bool(&call.args[0], "open")?,
                immediate: required_bool(&call.args[1], "immediate")?,
                selected: required_u32(&call.args[2], "selected")?,
                entries: required_array(&call.args[3], "entries")?
                    .iter()
                    .map(switcher_entry)
                    .collect::<Result<Vec<_>, _>>()?,
            })
        }
        (IFACE_SHORTCUTS, "ShortcutsChanged") => Ok(ControlSignal::ShortcutsChanged {
            revision: signal_revision(&call)?,
        }),
        (IFACE_PANELS, "PanelsChanged") => Ok(ControlSignal::PanelsChanged {
            revision: signal_revision(&call)?,
        }),
        (IFACE_PANELS, "PanelsSnapshotChanged") => {
            expect_arity(&call, 1)?;
            Ok(ControlSignal::PanelsSnapshotChanged {
                snapshot: panels_snapshot(required_dict(&call.args[0], "snapshot")?)?,
            })
        }
        _ => Err(invalid("unknown Flame Control signal")),
    }
}

fn switcher_entry_wire_value(entry: &SwitcherEntry) -> WireValue {
    WireValue::Dict(dict_of([
        ("title", WireValue::String(entry.title.clone())),
        ("appId", WireValue::String(entry.app_id.clone())),
    ]))
}

fn switcher_entry(value: &WireValue) -> Result<SwitcherEntry, ControlError> {
    let dict = required_dict(value, "switcher entry")?;
    Ok(SwitcherEntry {
        title: dict_string(dict, "title")?.to_owned(),
        app_id: dict_string(dict, "appId")?.to_owned(),
    })
}

fn weather_feed_dict(snapshot: &flamewm_api::weather::WeatherFeedSnapshot) -> WireDict {
    dict_of([
        ("revision", WireValue::U64(snapshot.revision)),
        ("payload", WireValue::String(snapshot.payload.clone())),
    ])
}

fn weather_feed_snapshot(
    dict: &WireDict,
) -> Result<flamewm_api::weather::WeatherFeedSnapshot, ControlError> {
    Ok(flamewm_api::weather::WeatherFeedSnapshot {
        revision: dict_u64(dict, "revision")?,
        payload: dict_string(dict, "payload")?.to_owned(),
    })
}

fn revision_signal(interface: &str, member: &str, revision: u64) -> WireSignal {
    WireSignal {
        interface: interface.to_owned(),
        member: member.to_owned(),
        args: vec![WireValue::U64(revision)],
    }
}

fn signal_revision(call: &WireCall) -> Result<u64, ControlError> {
    expect_arity(call, 1)?;
    required_u64(&call.args[0], "revision")
}

#[must_use]
pub fn settings_dict(snapshot: &SettingsSnapshot) -> WireDict {
    let mut out = WireDict::new();
    out.insert("revision".to_owned(), WireValue::U64(snapshot.revision));
    for (key, value) in &snapshot.values {
        out.insert(key.clone(), setting_wire_value(value));
    }
    out
}

#[must_use]
pub fn workspace_dict(snapshot: &WorkspaceSnapshot) -> WireDict {
    dict_of([
        ("revision", WireValue::U64(snapshot.revision)),
        ("count", WireValue::U32(u32_saturating(snapshot.count))),
        (
            "activeIndex",
            WireValue::I32(i32_saturating(snapshot.active_index)),
        ),
        (
            "lastIndex",
            WireValue::I32(snapshot.last_index.map_or(-1, i32_saturating)),
        ),
        ("names", WireValue::StringArray(snapshot.names.clone())),
    ])
}

#[must_use]
pub fn shell_bootstrap_dict(snapshot: &ShellBootstrapSnapshot) -> WireDict {
    dict_of([
        ("windowsRevision", WireValue::U64(snapshot.windows_revision)),
        (
            "windows",
            WireValue::Array(snapshot.windows.iter().map(window_wire_value).collect()),
        ),
        (
            "workspaces",
            WireValue::Dict(workspace_dict(&snapshot.workspaces)),
        ),
        ("panels", WireValue::Dict(panels_dict(&snapshot.panels))),
        (
            "applications",
            WireValue::Array(
                snapshot
                    .applications
                    .iter()
                    .map(application_wire_value)
                    .collect(),
            ),
        ),
        (
            "sessionCapabilities",
            WireValue::StringArray(session_capability_names(snapshot.session)),
        ),
        (
            "displays",
            WireValue::Dict(display_dict(&snapshot.displays)),
        ),
        ("system", WireValue::Dict(system_dict(&snapshot.system))),
    ])
}

#[must_use]
pub fn display_dict(snapshot: &DisplaySnapshot) -> WireDict {
    let mut out = WireDict::new();
    out.insert("generation".to_owned(), WireValue::U64(snapshot.generation));
    out.insert(
        "outputCount".to_owned(),
        WireValue::U32(u32_saturating(snapshot.outputs.len())),
    );
    out.insert(
        "hasPending".to_owned(),
        WireValue::Bool(snapshot.pending.is_some()),
    );
    if let Some(pending) = &snapshot.pending {
        out.insert(
            "pendingTx".to_owned(),
            WireValue::U64(pending.transaction.0),
        );
        out.insert(
            "pendingDeadlineMs".to_owned(),
            WireValue::U64(pending.deadline_ms),
        );
        out.insert(
            "pendingOutput".to_owned(),
            WireValue::String(pending.output.as_str().to_owned()),
        );
        out.insert("pendingMode".to_owned(), WireValue::U64(pending.mode.0));
    }
    out.insert(
        "outputs".to_owned(),
        WireValue::Array(snapshot.outputs.iter().map(output_wire_value).collect()),
    );
    out
}

#[must_use]
pub fn shortcut_dict(snapshot: &ShortcutSnapshot) -> WireDict {
    let mut out = WireDict::new();
    out.insert("revision".to_owned(), WireValue::U64(snapshot.revision));
    for (action, binding) in &snapshot.bindings {
        out.insert(
            action.clone(),
            WireValue::String(binding.0.clone().unwrap_or_default()),
        );
    }
    out
}

#[must_use]
pub fn system_dict(snapshot: &SystemSnapshot) -> WireDict {
    let mut out = WireDict::new();
    out.insert("revision".to_owned(), WireValue::U64(snapshot.revision));
    out.insert(
        "networkAvailability".to_owned(),
        WireValue::String(availability_name(snapshot.network.availability).to_owned()),
    );
    out.insert(
        "networkGeneration".to_owned(),
        WireValue::U64(snapshot.network.generation),
    );
    out.insert(
        "networkKind".to_owned(),
        WireValue::String(network_kind_name(snapshot.network.kind).to_owned()),
    );
    out.insert(
        "networkLabel".to_owned(),
        WireValue::String(snapshot.network.label.clone()),
    );
    out.insert(
        "networkStrength".to_owned(),
        WireValue::U32(u32::from(snapshot.network.strength_percent)),
    );
    out.insert(
        "wifiEnabled".to_owned(),
        WireValue::Bool(snapshot.network.wifi_enabled),
    );
    out.insert(
        "networkingEnabled".to_owned(),
        WireValue::Bool(snapshot.network.networking_enabled),
    );
    out.insert(
        "networkActivePath".to_owned(),
        WireValue::String(snapshot.network.active_path.clone()),
    );
    out.insert(
        "networkActiveAccessPointPath".to_owned(),
        WireValue::String(snapshot.network.active_access_point_path.clone()),
    );
    out.insert(
        "accessPoints".to_owned(),
        WireValue::Array(
            snapshot
                .network
                .access_points
                .iter()
                .map(access_point_wire_value)
                .collect(),
        ),
    );
    out.insert(
        "hasPendingNetworkSecret".to_owned(),
        WireValue::Bool(snapshot.network.pending_secret.is_some()),
    );
    if let Some(pending) = &snapshot.network.pending_secret {
        out.insert(
            "pendingNetworkSecretRequestId".to_owned(),
            WireValue::U64(pending.request_id),
        );
        out.insert(
            "pendingNetworkSecretAccessPointPath".to_owned(),
            WireValue::String(pending.access_point_path.clone()),
        );
    }
    out.insert(
        "mediaAvailability".to_owned(),
        WireValue::String(availability_name(snapshot.media.availability).to_owned()),
    );
    out.insert(
        "mediaGeneration".to_owned(),
        WireValue::U64(snapshot.media.generation),
    );
    out.insert(
        "mediaBusName".to_owned(),
        WireValue::String(snapshot.media.active_bus_name.clone()),
    );
    out.insert(
        "mediaIdentity".to_owned(),
        WireValue::String(snapshot.media.identity.clone()),
    );
    out.insert(
        "mediaTitle".to_owned(),
        WireValue::String(snapshot.media.title.clone()),
    );
    out.insert(
        "mediaArtist".to_owned(),
        WireValue::String(snapshot.media.artist.clone()),
    );
    out.insert(
        "mediaPlayback".to_owned(),
        WireValue::String(playback_name(snapshot.media.playback).to_owned()),
    );
    out.insert(
        "mediaCanPlay".to_owned(),
        WireValue::Bool(snapshot.media.can_play),
    );
    out.insert(
        "mediaCanPause".to_owned(),
        WireValue::Bool(snapshot.media.can_pause),
    );
    out.insert(
        "mediaCanNext".to_owned(),
        WireValue::Bool(snapshot.media.can_next),
    );
    out.insert(
        "mediaCanPrevious".to_owned(),
        WireValue::Bool(snapshot.media.can_previous),
    );
    out.insert(
        "powerAvailability".to_owned(),
        WireValue::String(availability_name(snapshot.power.available).to_owned()),
    );
    out.insert(
        "powerPresent".to_owned(),
        WireValue::Bool(snapshot.power.present),
    );
    out.insert(
        "powerPercent".to_owned(),
        WireValue::U32(u32::from(snapshot.power.percent)),
    );
    out.insert(
        "powerState".to_owned(),
        WireValue::String(power_state_name(snapshot.power.state).to_owned()),
    );
    // Optional: absent while the battery menu is closed; older peers ignore the whole group.
    if let Some(cpu) = snapshot.power.cpu_percent {
        out.insert("powerCpuPercent".to_owned(), WireValue::U32(u32::from(cpu)));
    }
    if let Some(milliwatts) = snapshot.power.power_rate_mw {
        out.insert("powerRateMilliwatts".to_owned(), WireValue::U32(milliwatts));
    }
    out.insert(
        "audioAvailability".to_owned(),
        WireValue::String(availability_name(snapshot.audio.availability).to_owned()),
    );
    out.insert(
        "audioGeneration".to_owned(),
        WireValue::U64(snapshot.audio.generation),
    );
    out.insert(
        "audioServerGeneration".to_owned(),
        WireValue::U64(snapshot.audio.server_generation),
    );
    out.insert(
        "audioSinkName".to_owned(),
        WireValue::String(snapshot.audio.sink_name.clone()),
    );
    out.insert(
        "audioVolume".to_owned(),
        WireValue::U32(u32::from(snapshot.audio.volume_percent)),
    );
    out.insert(
        "audioMuted".to_owned(),
        WireValue::Bool(snapshot.audio.muted),
    );
    out.insert(
        "audioEndpoints".to_owned(),
        WireValue::Array(
            snapshot
                .audio
                .endpoints()
                .iter()
                .map(audio_endpoint_wire_value)
                .collect(),
        ),
    );
    out.insert(
        "audioStreams".to_owned(),
        WireValue::Array(
            snapshot
                .audio
                .streams()
                .iter()
                .map(audio_stream_wire_value)
                .collect(),
        ),
    );
    out
}

fn access_point_wire_value(point: &NetworkAccessPointSnapshot) -> WireValue {
    let mut item = WireDict::new();
    item.insert("path".to_owned(), WireValue::String(point.path.clone()));
    item.insert("ssid".to_owned(), WireValue::String(point.ssid.clone()));
    item.insert(
        "strength".to_owned(),
        WireValue::U32(u32::from(point.strength_percent)),
    );
    item.insert("secured".to_owned(), WireValue::Bool(point.secured));
    item.insert("known".to_owned(), WireValue::Bool(point.known));
    WireValue::Dict(item)
}

/// Generates a wire-name encoder (exhaustive) and a strict wire-name decoder.
macro_rules! wire_enum_codec {
    ($ty:ident, $name_fn:ident, $parse_fn:ident, $err:literal, { $($variant:ident => $wire:literal),+ $(,)? }) => {
        const fn $name_fn(value: $ty) -> &'static str {
            match value {
                $($ty::$variant => $wire,)+
            }
        }

        fn $parse_fn(value: &str) -> Result<$ty, ControlError> {
            match value {
                $($wire => Ok($ty::$variant),)+
                _ => Err(invalid($err)),
            }
        }
    };
}

wire_enum_codec!(ServiceAvailability, availability_name, parse_availability, "availability is invalid", {
    Unknown => "unknown",
    Available => "available",
    Unavailable => "unavailable",
});

wire_enum_codec!(NetworkKind, network_kind_name, parse_network_kind, "network kind is invalid", {
    Unavailable => "unavailable",
    Disconnected => "disconnected",
    Connecting => "connecting",
    Wired => "wired",
    Wireless => "wireless",
});

wire_enum_codec!(PowerState, power_state_name, parse_power_state, "power state is invalid", {
    Charging => "charging",
    Discharging => "discharging",
    Full => "full",
    Empty => "empty",
    Unknown => "unknown",
});

wire_enum_codec!(PlaybackState, playback_name, parse_playback, "playback state is invalid", {
    Playing => "playing",
    Paused => "paused",
    Stopped => "stopped",
    Unavailable => "unavailable",
});

wire_enum_codec!(AudioEndpointKind, audio_endpoint_kind_name, parse_audio_endpoint_kind, "audio endpoint kind must be sink or source", {
    Sink => "sink",
    Source => "source",
});

fn audio_target(kind: &str, endpoint_kind: &str, id: u32) -> Result<AudioTarget, ControlError> {
    match kind {
        "endpoint" => Ok(AudioTarget::Endpoint {
            id,
            kind: parse_audio_endpoint_kind(endpoint_kind)?,
        }),
        "stream" if endpoint_kind.is_empty() => Ok(AudioTarget::Stream { id }),
        "stream" => Err(invalid("audio stream endpointKind must be empty")),
        _ => Err(invalid("audio target kind must be endpoint or stream")),
    }
}

fn access_point_snapshot(value: &WireValue) -> Result<NetworkAccessPointSnapshot, ControlError> {
    let dict = required_dict(value, "accessPoint")?;
    let strength = dict_u32(dict, "strength")?;
    Ok(NetworkAccessPointSnapshot {
        path: dict_string(dict, "path")?.to_owned(),
        ssid: dict_string(dict, "ssid")?.to_owned(),
        strength_percent: u8::try_from(strength)
            .map_err(|_| invalid("access point strength exceeds u8"))?,
        secured: dict_bool(dict, "secured")?,
        known: dict_bool(dict, "known")?,
    })
}

/// Absent power keys (older peers) decode to the default snapshot.
fn power_snapshot(dict: &WireDict) -> Result<PowerSnapshot, ControlError> {
    if dict.get("powerAvailability").is_none() {
        return Ok(PowerSnapshot::default());
    }
    let percent = dict_u32(dict, "powerPercent")?;
    let cpu_percent = dict
        .get("powerCpuPercent")
        .map(|_| {
            u8::try_from(dict_u32(dict, "powerCpuPercent")?)
                .map_err(|_| invalid("power cpu percent exceeds u8"))
        })
        .transpose()?;
    let power_rate_mw = dict
        .get("powerRateMilliwatts")
        .map(|_| dict_u32(dict, "powerRateMilliwatts"))
        .transpose()?;
    Ok(PowerSnapshot {
        available: parse_availability(dict_string(dict, "powerAvailability")?)?,
        present: dict_bool(dict, "powerPresent")?,
        percent: u8::try_from(percent).map_err(|_| invalid("power percent exceeds u8"))?,
        state: parse_power_state(dict_string(dict, "powerState")?)?,
        cpu_percent,
        power_rate_mw,
    })
}

fn system_snapshot(dict: &WireDict) -> Result<SystemSnapshot, ControlError> {
    let strength = dict_u32(dict, "networkStrength")?;
    let volume = dict_u32(dict, "audioVolume")?;
    Ok(SystemSnapshot {
        revision: dict_u64(dict, "revision")?,
        network: NetworkSnapshot {
            availability: parse_availability(dict_string(dict, "networkAvailability")?)?,
            generation: dict_u64(dict, "networkGeneration")?,
            kind: parse_network_kind(dict_string(dict, "networkKind")?)?,
            label: dict_string(dict, "networkLabel")?.to_owned(),
            strength_percent: u8::try_from(strength)
                .map_err(|_| invalid("network strength exceeds u8"))?,
            wifi_enabled: dict_bool(dict, "wifiEnabled")?,
            networking_enabled: dict_bool(dict, "networkingEnabled")?,
            active_path: dict_string(dict, "networkActivePath")?.to_owned(),
            // Optional for older peers that predate this key: absent means no connected AP.
            active_access_point_path: match dict.get("networkActiveAccessPointPath") {
                Some(value) => required_string(value, "networkActiveAccessPointPath")?.to_owned(),
                None => String::new(),
            },
            access_points: dict_value(dict, "accessPoints").and_then(|value| {
                required_array(value, "accessPoints")?
                    .iter()
                    .map(access_point_snapshot)
                    .collect::<Result<Vec<_>, _>>()
            })?,
            pending_secret: if dict_bool(dict, "hasPendingNetworkSecret")? {
                Some(NetworkSecretRequestSnapshot {
                    request_id: dict_u64(dict, "pendingNetworkSecretRequestId")?,
                    access_point_path: dict_string(dict, "pendingNetworkSecretAccessPointPath")?
                        .to_owned(),
                })
            } else {
                None
            },
        },
        media: MediaSnapshot {
            availability: parse_availability(dict_string(dict, "mediaAvailability")?)?,
            generation: dict_u64(dict, "mediaGeneration")?,
            active_bus_name: dict_string(dict, "mediaBusName")?.to_owned(),
            identity: dict_string(dict, "mediaIdentity")?.to_owned(),
            title: dict_string(dict, "mediaTitle")?.to_owned(),
            artist: dict_string(dict, "mediaArtist")?.to_owned(),
            playback: parse_playback(dict_string(dict, "mediaPlayback")?)?,
            can_play: dict_bool(dict, "mediaCanPlay")?,
            can_pause: dict_bool(dict, "mediaCanPause")?,
            can_next: dict_bool(dict, "mediaCanNext")?,
            can_previous: dict_bool(dict, "mediaCanPrevious")?,
        },
        power: power_snapshot(dict)?,
        audio: AudioSnapshot {
            availability: parse_availability(dict_string(dict, "audioAvailability")?)?,
            generation: dict_u64(dict, "audioGeneration")?,
            server_generation: dict_u64(dict, "audioServerGeneration")?,
            sink_name: dict_string(dict, "audioSinkName")?.to_owned(),
            volume_percent: u8::try_from(volume).map_err(|_| invalid("audio volume exceeds u8"))?,
            muted: dict_bool(dict, "audioMuted")?,
            ..AudioSnapshot::default()
        }
        .with_items(
            dict_value(dict, "audioEndpoints")
                .and_then(|value| required_array(value, "audioEndpoints"))?
                .iter()
                .map(audio_endpoint_snapshot)
                .collect::<Result<Vec<_>, _>>()?,
            dict_value(dict, "audioStreams")
                .and_then(|value| required_array(value, "audioStreams"))?
                .iter()
                .map(audio_stream_snapshot)
                .collect::<Result<Vec<_>, _>>()?,
        ),
    })
}

fn audio_endpoint_wire_value(endpoint: &AudioEndpointSnapshot) -> WireValue {
    WireValue::Dict(dict_of([
        ("id", WireValue::U32(endpoint.id)),
        (
            "kind",
            WireValue::String(audio_endpoint_kind_name(endpoint.kind).to_owned()),
        ),
        ("name", WireValue::String(endpoint.name.clone())),
        (
            "description",
            WireValue::String(endpoint.description.clone()),
        ),
        ("volume", WireValue::U32(u32::from(endpoint.volume_percent))),
        ("muted", WireValue::Bool(endpoint.muted)),
        ("default", WireValue::Bool(endpoint.is_default)),
    ]))
}

fn audio_stream_wire_value(stream: &AudioStreamSnapshot) -> WireValue {
    let mut out = WireDict::new();
    out.insert("id".to_owned(), WireValue::U32(stream.id));
    out.insert("endpointId".to_owned(), WireValue::U32(stream.endpoint_id));
    out.insert("name".to_owned(), WireValue::String(stream.name.clone()));
    out.insert(
        "volume".to_owned(),
        WireValue::U32(u32::from(stream.volume_percent)),
    );
    out.insert("muted".to_owned(), WireValue::Bool(stream.muted));
    // Optional identity keys are omitted when unknown so older peers see the original shape.
    for (key, value) in [
        ("appName", &stream.app_name),
        ("appId", &stream.app_id),
        ("iconName", &stream.icon_name),
        ("processBinary", &stream.process_binary),
        ("mediaName", &stream.media_name),
    ] {
        if let Some(value) = value {
            out.insert(key.to_owned(), WireValue::String(value.clone()));
        }
    }
    WireValue::Dict(out)
}

fn audio_endpoint_snapshot(value: &WireValue) -> Result<AudioEndpointSnapshot, ControlError> {
    let dict = required_dict(value, "audio endpoint")?;
    Ok(AudioEndpointSnapshot {
        id: dict_u32(dict, "id")?,
        kind: parse_audio_endpoint_kind(dict_string(dict, "kind")?)?,
        name: dict_string(dict, "name")?.to_owned(),
        description: dict_string(dict, "description")?.to_owned(),
        volume_percent: u8::try_from(dict_u32(dict, "volume")?)
            .map_err(|_| invalid("audio endpoint volume exceeds u8"))?,
        muted: dict_bool(dict, "muted")?,
        is_default: dict_bool(dict, "default")?,
    })
}

fn audio_stream_snapshot(value: &WireValue) -> Result<AudioStreamSnapshot, ControlError> {
    let dict = required_dict(value, "audio stream")?;
    Ok(AudioStreamSnapshot {
        id: dict_u32(dict, "id")?,
        endpoint_id: dict_u32(dict, "endpointId")?,
        name: dict_string(dict, "name")?.to_owned(),
        volume_percent: u8::try_from(dict_u32(dict, "volume")?)
            .map_err(|_| invalid("audio stream volume exceeds u8"))?,
        muted: dict_bool(dict, "muted")?,
        app_name: optional_string(dict, "appName")?,
        app_id: optional_string(dict, "appId")?,
        icon_name: optional_string(dict, "iconName")?,
        process_binary: optional_string(dict, "processBinary")?,
        media_name: optional_string(dict, "mediaName")?,
    })
}

/// Absent key means `None`; a present key must be a string.
fn optional_string(dict: &WireDict, key: &str) -> Result<Option<String>, ControlError> {
    dict.get(key)
        .map(|value| required_string(value, key).map(str::to_owned))
        .transpose()
}

#[must_use]
pub fn panels_dict(snapshot: &PanelsSnapshot) -> WireDict {
    let mut out = WireDict::new();
    out.insert("revision".to_owned(), WireValue::U64(snapshot.revision));
    out.insert(
        "panelCount".to_owned(),
        WireValue::U32(u32_saturating(snapshot.panels.len())),
    );
    out.insert(
        "taskCount".to_owned(),
        WireValue::U32(u32_saturating(snapshot.tasks.len())),
    );
    out.insert(
        "pinnedCount".to_owned(),
        WireValue::U32(u32_saturating(snapshot.pinned_apps.len())),
    );
    out.insert(
        "panels".to_owned(),
        WireValue::Array(snapshot.panels.iter().map(panel_wire_value).collect()),
    );
    out.insert(
        "tasks".to_owned(),
        WireValue::Array(
            snapshot
                .tasks
                .iter()
                .map(|entry| {
                    let mut item = WireDict::new();
                    item.insert(
                        "id".to_owned(),
                        WireValue::String(entry.id.as_str().to_owned()),
                    );
                    item.insert(
                        "appId".to_owned(),
                        WireValue::String(entry.app_id.as_str().to_owned()),
                    );
                    item.insert(
                        "order".to_owned(),
                        WireValue::U32(u32_saturating(entry.order_index)),
                    );
                    match &entry.kind {
                        TaskEntryKind::PinnedSlot { window } => {
                            item.insert("kind".to_owned(), WireValue::String("pin".to_owned()));
                            if let Some(window) = window {
                                item.insert("windowId".to_owned(), WireValue::U64(window.id));
                                item.insert(
                                    "windowGeneration".to_owned(),
                                    WireValue::U64(window.generation),
                                );
                            }
                        }
                        TaskEntryKind::Window { window } => {
                            item.insert("kind".to_owned(), WireValue::String("window".to_owned()));
                            item.insert("windowId".to_owned(), WireValue::U64(window.id));
                            item.insert(
                                "windowGeneration".to_owned(),
                                WireValue::U64(window.generation),
                            );
                        }
                    }
                    WireValue::Dict(item)
                })
                .collect(),
        ),
    );
    out.insert(
        "pinnedApps".to_owned(),
        WireValue::StringArray(
            snapshot
                .pinned_apps
                .iter()
                .map(|app| app.as_str().to_owned())
                .collect(),
        ),
    );
    out
}

#[must_use]
pub fn session_capability_names(capabilities: SessionCapabilities) -> Vec<String> {
    [
        (capabilities.lock, "lock"),
        (capabilities.logout, "logout"),
        (capabilities.suspend, "suspend"),
        (capabilities.reboot, "reboot"),
        (capabilities.shutdown, "shutdown"),
    ]
    .into_iter()
    .filter(|(enabled, _)| *enabled)
    .map(|(_, name)| name.to_owned())
    .collect()
}

fn output_wire_value(output: &OutputSnapshot) -> WireValue {
    let mut item = WireDict::new();
    item.insert(
        "id".to_owned(),
        WireValue::String(output.id.as_str().to_owned()),
    );
    item.insert(
        "connector".to_owned(),
        WireValue::String(output.connector.clone()),
    );
    item.insert(
        "edid".to_owned(),
        WireValue::String(output.edid_identity.clone()),
    );
    item.insert("connected".to_owned(), WireValue::Bool(output.connected));
    item.insert("primary".to_owned(), WireValue::Bool(output.primary));
    item.insert("x".to_owned(), WireValue::I32(output.geometry.x));
    item.insert("y".to_owned(), WireValue::I32(output.geometry.y));
    item.insert("width".to_owned(), WireValue::I32(output.geometry.width));
    item.insert("height".to_owned(), WireValue::I32(output.geometry.height));
    item.insert("mode".to_owned(), WireValue::U64(output.current_mode.0));
    item.insert(
        "scale".to_owned(),
        WireValue::U32(u32::from(output.shell_scale_percent)),
    );
    item.insert(
        "modes".to_owned(),
        WireValue::Array(
            output
                .modes
                .iter()
                .map(|mode| {
                    let mut value = WireDict::new();
                    value.insert("id".to_owned(), WireValue::U64(mode.id.0));
                    value.insert("width".to_owned(), WireValue::I32(mode.resolution.width));
                    value.insert("height".to_owned(), WireValue::I32(mode.resolution.height));
                    value.insert(
                        "refreshMillihz".to_owned(),
                        WireValue::I32(mode.refresh_millihz),
                    );
                    value.insert("preferred".to_owned(), WireValue::Bool(mode.preferred));
                    WireValue::Dict(value)
                })
                .collect(),
        ),
    );
    WireValue::Dict(item)
}

fn window_wire_value(window: &WindowSnapshot) -> WireValue {
    WireValue::Dict(dict_of([
        ("windowId", WireValue::U64(window.reference.id)),
        ("generation", WireValue::U64(window.reference.generation)),
        ("title", WireValue::String(window.title.clone())),
        (
            "appId",
            WireValue::String(window.app_id.as_str().to_owned()),
        ),
        ("outerX", WireValue::I32(window.outer_geometry.x)),
        ("outerY", WireValue::I32(window.outer_geometry.y)),
        ("outerWidth", WireValue::I32(window.outer_geometry.width)),
        ("outerHeight", WireValue::I32(window.outer_geometry.height)),
        ("restoreX", WireValue::I32(window.restore_geometry.x)),
        ("restoreY", WireValue::I32(window.restore_geometry.y)),
        (
            "restoreWidth",
            WireValue::I32(window.restore_geometry.width),
        ),
        (
            "restoreHeight",
            WireValue::I32(window.restore_geometry.height),
        ),
        (
            "state",
            WireValue::String(window_state_name(window.state).to_owned()),
        ),
        ("sticky", WireValue::Bool(window.sticky)),
        ("focused", WireValue::Bool(window.focused)),
        ("workspaceIndex", WireValue::I32(window.workspace.index)),
        (
            "workspaceRevision",
            WireValue::U64(window.workspace.revision),
        ),
        (
            "output",
            WireValue::String(window.output.as_str().to_owned()),
        ),
        ("stateGeneration", WireValue::U64(window.state_generation)),
    ]))
}

fn application_wire_value(application: &DesktopApplication) -> WireValue {
    WireValue::Dict(dict_of([
        ("id", WireValue::String(application.id.as_str().to_owned())),
        ("name", WireValue::String(application.name.clone())),
        (
            "genericName",
            WireValue::String(application.generic_name.clone()),
        ),
        ("comment", WireValue::String(application.comment.clone())),
        (
            "startupWmClass",
            WireValue::String(application.startup_wm_class.clone()),
        ),
        ("argv", WireValue::StringArray(application.argv.clone())),
        (
            "keywords",
            WireValue::StringArray(application.keywords.clone()),
        ),
        ("iconName", WireValue::String(application.icon_name.clone())),
        (
            "categories",
            WireValue::StringArray(application.categories.clone()),
        ),
    ]))
}

const fn window_state_name(state: WindowState) -> &'static str {
    match state {
        WindowState::Normal => "normal",
        WindowState::Minimized => "minimized",
        WindowState::Maximized => "maximized",
        WindowState::Fullscreen => "fullscreen",
        WindowState::Hidden => "hidden",
    }
}

fn panel_wire_value(panel: &PanelSnapshot) -> WireValue {
    let mut item = WireDict::new();
    item.insert(
        "output".to_owned(),
        WireValue::String(panel.output.as_str().to_owned()),
    );
    item.insert(
        "edge".to_owned(),
        WireValue::String(edge_name(panel.edge).to_owned()),
    );
    item.insert(
        "size".to_owned(),
        WireValue::U32(u32::from(panel.logical_size)),
    );
    item.insert("visible".to_owned(), WireValue::Bool(panel.visible));
    item.insert("x".to_owned(), WireValue::I32(panel.geometry.x));
    item.insert("y".to_owned(), WireValue::I32(panel.geometry.y));
    item.insert("width".to_owned(), WireValue::I32(panel.geometry.width));
    item.insert("height".to_owned(), WireValue::I32(panel.geometry.height));
    WireValue::Dict(item)
}

fn setting_wire_value(value: &SettingValue) -> WireValue {
    match value {
        SettingValue::Boolean(value) => WireValue::Bool(*value),
        SettingValue::Integer(value) => WireValue::I64(*value),
        SettingValue::Percent(value) => WireValue::U32(u32::from(*value)),
        SettingValue::Rgb(value) => WireValue::U32(*value),
        SettingValue::Text(value) => WireValue::String(value.clone()),
    }
}

fn setting_value(value: &WireValue) -> Result<SettingValue, ControlError> {
    let invalid = || ControlError {
        name: flamewm_control_core::error_name(ErrorCode::InvalidArgument),
        code: ErrorCode::InvalidArgument,
        message: "settings change contains unsupported variant type".to_owned(),
    };
    match value {
        WireValue::Bool(value) => Ok(SettingValue::Boolean(*value)),
        WireValue::I64(value) => Ok(SettingValue::Integer(*value)),
        WireValue::I32(value) => Ok(SettingValue::Integer(i64::from(*value))),
        WireValue::U32(value) => Ok(SettingValue::Integer(i64::from(*value))),
        WireValue::String(value) => Ok(SettingValue::Text(value.clone())),
        _ => Err(invalid()),
    }
}

fn setting_value_for_key(key: &str, value: &WireValue) -> Result<SettingValue, ControlError> {
    match key {
        "DesktopSelectionFillOpacity"
        | "WindowSnapPreviewFillOpacity"
        | "startMenuOpacity"
        | "surfaceOpacity"
        | "surfaceBlurPercent"
        | "surfaceSaturationPercent"
        | "surfaceRimFadePercent"
        | "surfaceShadowPercent"
        | "surfaceHighlightPercent"
        | "glassOpacityPercent"
        | "glassBlurPx"
        | "taskbarOpacityPercent"
        | "startMenuBlurPercent"
        | "accentOpacity" => {
            let percent = required_u32(value, key)?;
            Ok(SettingValue::Percent(
                u8::try_from(percent).map_err(|_| invalid("percentage exceeds u8"))?,
            ))
        }
        _ => setting_value(value),
    }
}

fn workspace_index_request(
    call: &WireCall,
    build: impl FnOnce(usize, u64) -> ControlRequest,
) -> Result<ControlRequest, ControlError> {
    expect_arity(call, 2)?;
    let index = usize::try_from(required_u32(&call.args[0], "index")?)
        .map_err(|_| invalid("index does not fit usize"))?;
    Ok(build(
        index,
        required_u64(&call.args[1], "expectedRevision")?,
    ))
}

fn shortcut_action_request(
    call: &WireCall,
    build: impl FnOnce(String, u64) -> ControlRequest,
) -> Result<ControlRequest, ControlError> {
    expect_arity(call, 2)?;
    Ok(build(
        required_string(&call.args[0], "action")?.to_owned(),
        required_u64(&call.args[1], "expectedRevision")?,
    ))
}

fn panel_app_request(
    call: &WireCall,
    build: impl FnOnce(DesktopAppId, u64) -> ControlRequest,
) -> Result<ControlRequest, ControlError> {
    expect_arity(call, 2)?;
    Ok(build(
        DesktopAppId::new(required_string(&call.args[0], "appId")?),
        required_u64(&call.args[1], "expectedRevision")?,
    ))
}

fn session_action(call: &WireCall, action: SessionAction) -> Result<ControlRequest, ControlError> {
    expect_arity(call, 0)?;
    Ok(ControlRequest::SessionAction(action))
}

fn system_action_request(call: &WireCall, member: &str) -> Result<ControlRequest, ControlError> {
    let (action, expected_revision) = match member {
        "SetWifiEnabled" => {
            expect_arity(call, 2)?;
            (
                SystemAction::SetWifiEnabled(
                    call.args[0]
                        .as_bool()
                        .ok_or_else(|| invalid("enabled must be bool"))?,
                ),
                required_u64(&call.args[1], "expectedRevision")?,
            )
        }
        "ConnectKnown" => {
            expect_arity(call, 2)?;
            (
                SystemAction::ConnectKnown {
                    access_point_path: required_string(&call.args[0], "accessPointPath")?
                        .to_owned(),
                },
                required_u64(&call.args[1], "expectedRevision")?,
            )
        }
        "ConnectWifi" => {
            expect_arity(call, 3)?;
            (
                SystemAction::ConnectWifi {
                    access_point_path: required_string(&call.args[0], "accessPointPath")?
                        .to_owned(),
                    generation: required_u64(&call.args[1], "generation")?,
                },
                required_u64(&call.args[2], "expectedRevision")?,
            )
        }
        "SubmitNetworkSecret" => {
            expect_arity(call, 4)?;
            (
                SystemAction::SubmitNetworkSecret {
                    request_id: required_u64(&call.args[0], "requestId")?,
                    generation: required_u64(&call.args[1], "generation")?,
                    secret: required_string(&call.args[2], "secret")?.to_owned(),
                },
                required_u64(&call.args[3], "expectedRevision")?,
            )
        }
        "CancelNetworkSecret" => {
            expect_arity(call, 3)?;
            (
                SystemAction::CancelNetworkSecret {
                    request_id: required_u64(&call.args[0], "requestId")?,
                    generation: required_u64(&call.args[1], "generation")?,
                },
                required_u64(&call.args[2], "expectedRevision")?,
            )
        }
        "Disconnect" => system_simple_action(call, SystemAction::Disconnect)?,
        "Scan" => system_simple_action(call, SystemAction::Scan)?,
        "BatteryMenuOpened" => system_simple_action(call, SystemAction::BatteryMenuOpened)?,
        "BatteryMenuClosed" => system_simple_action(call, SystemAction::BatteryMenuClosed)?,
        "Play" => system_media_action(call, |bus_name| SystemAction::Play { bus_name })?,
        "Pause" => system_media_action(call, |bus_name| SystemAction::Pause { bus_name })?,
        "PlayPause" => system_media_action(call, |bus_name| SystemAction::PlayPause { bus_name })?,
        "Next" => system_media_action(call, |bus_name| SystemAction::Next { bus_name })?,
        "Previous" => system_media_action(call, |bus_name| SystemAction::Previous { bus_name })?,
        "SetVolume" => {
            expect_arity(call, 7)?;
            let percent = u8::try_from(required_u32(&call.args[3], "percent")?)
                .map_err(|_| invalid("percent exceeds u8"))?;
            if percent > 150 {
                return Err(invalid("percent must be in 0..=150"));
            }
            (
                SystemAction::SetVolume(AudioVolumeAction {
                    target: audio_target(
                        required_string(&call.args[0], "targetKind")?,
                        required_string(&call.args[1], "endpointKind")?,
                        required_u32(&call.args[2], "id")?,
                    )?,
                    percent,
                    generation: required_u64(&call.args[4], "generation")?,
                    server_generation: required_u64(&call.args[5], "serverGeneration")?,
                }),
                required_u64(&call.args[6], "expectedRevision")?,
            )
        }
        "SetMute" => {
            expect_arity(call, 7)?;
            (
                SystemAction::SetMute(AudioMuteAction {
                    target: audio_target(
                        required_string(&call.args[0], "targetKind")?,
                        required_string(&call.args[1], "endpointKind")?,
                        required_u32(&call.args[2], "id")?,
                    )?,
                    muted: call.args[3]
                        .as_bool()
                        .ok_or_else(|| invalid("muted must be bool"))?,
                    generation: required_u64(&call.args[4], "generation")?,
                    server_generation: required_u64(&call.args[5], "serverGeneration")?,
                }),
                required_u64(&call.args[6], "expectedRevision")?,
            )
        }
        _ => {
            return Err(ControlError {
                name: flamewm_control_core::error_name(ErrorCode::NotFound),
                code: ErrorCode::NotFound,
                message: format!("unknown Flame Control system method {member}"),
            });
        }
    };
    Ok(ControlRequest::SystemAction {
        action,
        expected_revision,
    })
}

fn system_simple_action(
    call: &WireCall,
    action: SystemAction,
) -> Result<(SystemAction, u64), ControlError> {
    expect_arity(call, 1)?;
    Ok((action, required_u64(&call.args[0], "expectedRevision")?))
}

fn system_media_action(
    call: &WireCall,
    action: impl FnOnce(String) -> SystemAction,
) -> Result<(SystemAction, u64), ControlError> {
    expect_arity(call, 2)?;
    Ok((
        action(required_string(&call.args[0], "busName")?.to_owned()),
        required_u64(&call.args[1], "expectedRevision")?,
    ))
}

fn system_action_call(action: &SystemAction, expected_revision: u64) -> WireCall {
    let (member, mut args) = match action {
        SystemAction::SetWifiEnabled(enabled) => {
            ("SetWifiEnabled", vec![WireValue::Bool(*enabled)])
        }
        SystemAction::ConnectKnown { access_point_path } => (
            "ConnectKnown",
            vec![WireValue::String(access_point_path.clone())],
        ),
        SystemAction::ConnectWifi {
            access_point_path,
            generation,
        } => (
            "ConnectWifi",
            vec![
                WireValue::String(access_point_path.clone()),
                WireValue::U64(*generation),
            ],
        ),
        SystemAction::SubmitNetworkSecret {
            request_id,
            generation,
            secret,
        } => (
            "SubmitNetworkSecret",
            vec![
                WireValue::U64(*request_id),
                WireValue::U64(*generation),
                WireValue::String(secret.clone()),
            ],
        ),
        SystemAction::CancelNetworkSecret {
            request_id,
            generation,
        } => (
            "CancelNetworkSecret",
            vec![WireValue::U64(*request_id), WireValue::U64(*generation)],
        ),
        SystemAction::Disconnect => ("Disconnect", Vec::new()),
        SystemAction::Scan => ("Scan", Vec::new()),
        SystemAction::BatteryMenuOpened => ("BatteryMenuOpened", Vec::new()),
        SystemAction::BatteryMenuClosed => ("BatteryMenuClosed", Vec::new()),
        SystemAction::Play { bus_name } => ("Play", vec![WireValue::String(bus_name.clone())]),
        SystemAction::Pause { bus_name } => ("Pause", vec![WireValue::String(bus_name.clone())]),
        SystemAction::PlayPause { bus_name } => {
            ("PlayPause", vec![WireValue::String(bus_name.clone())])
        }
        SystemAction::Next { bus_name } => ("Next", vec![WireValue::String(bus_name.clone())]),
        SystemAction::Previous { bus_name } => {
            ("Previous", vec![WireValue::String(bus_name.clone())])
        }
        SystemAction::SetVolume(action) => (
            "SetVolume",
            audio_action_args(
                action.target,
                Some(action.percent),
                None,
                action.generation,
                action.server_generation,
            ),
        ),
        SystemAction::SetMute(action) => (
            "SetMute",
            audio_action_args(
                action.target,
                None,
                Some(action.muted),
                action.generation,
                action.server_generation,
            ),
        ),
    };
    args.push(WireValue::U64(expected_revision));
    WireCall::new(IFACE_SYSTEM, member, args)
}

fn audio_action_args(
    target: AudioTarget,
    percent: Option<u8>,
    muted: Option<bool>,
    generation: u64,
    server_generation: u64,
) -> Vec<WireValue> {
    let (target_kind, endpoint_kind, id) = match target {
        AudioTarget::Endpoint { id, kind } => ("endpoint", audio_endpoint_kind_name(kind), id),
        AudioTarget::Stream { id } => ("stream", "", id),
    };
    let mut args = vec![
        WireValue::String(target_kind.to_owned()),
        WireValue::String(endpoint_kind.to_owned()),
        WireValue::U32(id),
    ];
    if let Some(percent) = percent {
        args.push(WireValue::U32(u32::from(percent)));
    }
    if let Some(muted) = muted {
        args.push(WireValue::Bool(muted));
    }
    args.push(WireValue::U64(generation));
    args.push(WireValue::U64(server_generation));
    args
}

fn expect_arity(call: &WireCall, expected: usize) -> Result<(), ControlError> {
    if call.args.len() == expected {
        Ok(())
    } else {
        Err(invalid(&format!(
            "{} expects {expected} arguments",
            call.member
        )))
    }
}

fn required_u64(value: &WireValue, name: &str) -> Result<u64, ControlError> {
    value
        .as_u64()
        .ok_or_else(|| invalid(&format!("{name} must be uint64")))
}

fn required_u32(value: &WireValue, name: &str) -> Result<u32, ControlError> {
    value
        .as_u32()
        .ok_or_else(|| invalid(&format!("{name} must be uint32")))
}

fn required_bool(value: &WireValue, name: &str) -> Result<bool, ControlError> {
    value
        .as_bool()
        .ok_or_else(|| invalid(&format!("{name} must be bool")))
}

fn required_string<'a>(value: &'a WireValue, name: &str) -> Result<&'a str, ControlError> {
    value
        .as_str()
        .ok_or_else(|| invalid(&format!("{name} must be string")))
}

fn required_dict<'a>(value: &'a WireValue, name: &str) -> Result<&'a WireDict, ControlError> {
    value
        .as_dict()
        .ok_or_else(|| invalid(&format!("{name} must be a{{sv}}")))
}

fn parse_edge(value: &str) -> Result<PanelEdge, ControlError> {
    match value.to_ascii_lowercase().as_str() {
        "bottom" => Ok(PanelEdge::Bottom),
        "top" => Ok(PanelEdge::Top),
        "left" => Ok(PanelEdge::Left),
        "right" => Ok(PanelEdge::Right),
        _ => Err(invalid("edge must be bottom/top/left/right")),
    }
}

#[must_use]
pub const fn edge_name(edge: PanelEdge) -> &'static str {
    match edge {
        PanelEdge::Bottom => "bottom",
        PanelEdge::Top => "top",
        PanelEdge::Left => "left",
        PanelEdge::Right => "right",
    }
}

fn nullary(call: &WireCall, request: ControlRequest) -> Result<ControlRequest, ControlError> {
    expect_arity(call, 0).map(|()| request)
}

fn nullary_call(interface: &str, member: &str) -> WireCall {
    WireCall::new(interface, member, Vec::new())
}

fn invalid(message: &str) -> ControlError {
    ControlError {
        name: flamewm_control_core::error_name(ErrorCode::InvalidArgument),
        code: ErrorCode::InvalidArgument,
        message: message.to_owned(),
    }
}

fn u32_saturating(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn i32_saturating(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_api::settings::SettingValue;
    use flamewm_api::system::{
        AudioSnapshot, MediaSnapshot, NetworkAccessPointSnapshot, NetworkKind, NetworkSnapshot,
        PlaybackState, ServiceAvailability,
    };

    fn assert_request_round_trip(request: &ControlRequest) -> WireCall {
        let call = encode_call(request).expect("encode");
        assert_eq!(decode_call(&call).expect("decode"), *request);
        call
    }

    fn assert_reply_round_trip(call: &WireCall, response: &ControlResponse) {
        let reply = encode_reply(call, response).expect("encode reply");
        assert_eq!(decode_reply(call, &reply).expect("decode reply"), *response);
    }

    #[test]
    fn settings_apply_decodes_typed_variants_without_stringly_typed_core() {
        let mut changes = WireDict::new();
        changes.insert("PanelSize".to_owned(), WireValue::I64(44));
        changes.insert("Dark".to_owned(), WireValue::Bool(true));
        changes.insert("surfaceOpacity".to_owned(), WireValue::U32(83));
        let call = WireCall::new(
            IFACE_SETTINGS,
            "Apply",
            vec![WireValue::U64(7), WireValue::Dict(changes)],
        );
        let ControlRequest::ApplySettings(transaction) = decode_call(&call).expect("valid call")
        else {
            panic!("wrong request");
        };
        assert_eq!(transaction.expected_revision, 7);
        assert_eq!(transaction.changes.len(), 3);
        assert!(
            transaction
                .changes
                .iter()
                .any(|change| change.key == "surfaceOpacity"
                    && change.value == Some(SettingValue::Percent(83)))
        );
    }

    #[test]
    fn settings_snapshot_preserves_real_variant_types() {
        let mut snapshot = SettingsSnapshot::new(9);
        snapshot
            .values
            .insert("Dark".to_owned(), SettingValue::Boolean(true));
        snapshot
            .values
            .insert("AccentColor".to_owned(), SettingValue::Rgb(0x00EF_4048));
        let dict = settings_dict(&snapshot);
        assert_eq!(dict.get("revision"), Some(&WireValue::U64(9)));
        assert_eq!(dict.get("Dark"), Some(&WireValue::Bool(true)));
        assert_eq!(dict.get("AccentColor"), Some(&WireValue::U32(0x00EF_4048)));
    }

    #[test]
    fn begin_display_mode_uses_reactor_clock_not_untrusted_bus_argument() {
        let call = WireCall::new(
            IFACE_DISPLAYS,
            "BeginModeChange",
            vec![
                WireValue::String("HDMI-1".to_owned()),
                WireValue::U64(42),
                WireValue::U64(5),
            ],
        )
        .with_now_ms(12_345);
        assert_eq!(
            decode_call(&call).expect("valid"),
            ControlRequest::BeginDisplayMode {
                output: OutputId::new("HDMI-1"),
                mode: ModeId(42),
                expected_generation: 5,
                now_ms: 12_345,
            }
        );
    }

    #[test]
    fn keep_display_mode_uses_reactor_clock_for_deadline_enforcement() {
        let call =
            WireCall::new(IFACE_DISPLAYS, "Keep", vec![WireValue::U64(7)]).with_now_ms(15_001);
        assert_eq!(
            decode_call(&call).expect("valid"),
            ControlRequest::KeepDisplayMode {
                transaction: TransactionId(7),
                now_ms: 15_001,
            }
        );
    }

    #[test]
    fn v8_reorder_decodes_stable_entry_identity() {
        let call = WireCall::new(
            IFACE_PANELS,
            "Reorder",
            vec![
                WireValue::String("win:99:2".to_owned()),
                WireValue::U32(1),
                WireValue::U64(8),
            ],
        );
        assert_eq!(
            decode_call(&call).expect("valid"),
            ControlRequest::ReorderTask {
                entry_id: TaskEntryId::new("win:99:2"),
                index: 1,
                expected_revision: 8,
            }
        );
    }

    #[test]
    fn reset_section_reuses_atomic_settings_transaction() {
        let call = WireCall::new(
            IFACE_SETTINGS,
            "ResetSection",
            vec![
                WireValue::U64(3),
                WireValue::String("appearance".to_owned()),
            ],
        );
        let ControlRequest::ApplySettings(transaction) = decode_call(&call).expect("valid") else {
            panic!("wrong request");
        };
        assert_eq!(transaction.reset_section.as_deref(), Some("appearance"));
        assert!(transaction.changes.is_empty());
    }

    #[test]
    fn settings_mutation_reply_round_trips_full_authoritative_snapshot() {
        let call = WireCall::new(IFACE_SETTINGS, "Apply", Vec::new());
        let mut source = SettingsSnapshot::new(4);
        source
            .values
            .insert("fontBold".to_owned(), SettingValue::Boolean(true));
        assert_reply_round_trip(&call, &ControlResponse::Settings(source.clone()));
    }

    // CONTRACT-REGRESSION: T11 SettingsSnapshotChanged must preserve the complete settings snapshot.
    // TRIGGER: decode and re-encode a non-default snapshot-bearing settings signal.
    // OBSERVABLE: the exact wire signal, including typed values and revision, round-trips unchanged.
    // GAP: the current vocabulary admits only the legacy SettingsChanged signal.
    // MUTATION: remove the SettingsSnapshotChanged wire arm; this test must fail.
    #[test]
    fn t11_settings_snapshot_changed_round_trips_exact_wire_payload() {
        let mut snapshot = SettingsSnapshot::new(11);
        snapshot
            .values
            .insert("fontBold".to_owned(), SettingValue::Boolean(true));
        snapshot
            .values
            .insert("surfaceOpacity".to_owned(), SettingValue::Percent(83));
        let signal = WireSignal {
            interface: IFACE_SETTINGS.to_owned(),
            member: "SettingsSnapshotChanged".to_owned(),
            args: vec![WireValue::Dict(settings_dict(&snapshot))],
        };

        let decoded = decode_signal(&signal).expect("SettingsSnapshotChanged must decode");
        assert_eq!(encode_signal(&decoded), signal);
    }

    // CONTRACT-REGRESSION: T12 successful ApplySettings publishes reply, snapshot signal, legacy signal.
    // TRIGGER: encode the revision N+1 reply and decode both ordered signal payloads.
    // OBSERVABLE: the snapshot signal precedes the legacy signal and both carry N+1.
    // GAP: no snapshot-bearing settings signal exists in the current wire vocabulary.
    // MUTATION: emit only SettingsChanged or reorder the two signals; this test must fail.
    #[test]
    fn t12_apply_settings_success_orders_snapshot_then_legacy_signal() {
        let mut snapshot = SettingsSnapshot::new(12);
        snapshot
            .values
            .insert("fontBold".to_owned(), SettingValue::Boolean(true));
        let apply = WireCall::new(IFACE_SETTINGS, "Apply", Vec::new());
        assert_reply_round_trip(&apply, &ControlResponse::Settings(snapshot.clone()));

        let ordered = [
            WireSignal {
                interface: IFACE_SETTINGS.to_owned(),
                member: "SettingsSnapshotChanged".to_owned(),
                args: vec![WireValue::Dict(settings_dict(&snapshot))],
            },
            encode_signal(&ControlSignal::SettingsChanged {
                revision: snapshot.revision,
                keys: vec!["fontBold".to_owned()],
            }),
        ];
        let decoded = ordered
            .iter()
            .map(|signal| decode_signal(signal).expect("ordered signal decode"))
            .collect::<Vec<_>>();
        assert_eq!(encode_signal(&decoded[0]), ordered[0]);
        assert_eq!(
            decoded[1],
            ControlSignal::SettingsChanged {
                revision: snapshot.revision,
                keys: vec!["fontBold".to_owned()],
            }
        );
    }

    // CONTRACT-REGRESSION: T12 stale/failed ApplySettings must not publish success signals.
    // TRIGGER: represent a rejected mutation result and inspect its emitted signal list.
    // OBSERVABLE: an error produces no snapshot or legacy success signal.
    // MUTATION: publish the success pair on an error path; this test must fail.
    #[test]
    fn t12_apply_settings_failure_emits_no_success_signal() {
        let failure: Result<ControlResponse, ControlError> = Err(invalid("stale settings"));
        let signals = match failure {
            Ok(ControlResponse::Settings(snapshot)) => vec![
                WireSignal {
                    interface: IFACE_SETTINGS.to_owned(),
                    member: "SettingsSnapshotChanged".to_owned(),
                    args: vec![WireValue::Dict(settings_dict(&snapshot))],
                },
                encode_signal(&ControlSignal::SettingsChanged {
                    revision: snapshot.revision,
                    keys: vec!["fontBold".to_owned()],
                }),
            ],
            Ok(other) => panic!("unexpected successful response: {other:?}"),
            Err(_) => Vec::<WireSignal>::new(),
        };
        assert!(signals.is_empty());
    }

    #[test]
    fn workspace_snapshot_round_trips_through_client_codec() {
        let call = WireCall::new(IFACE_WORKSPACES, "GetSnapshot", Vec::new());
        let source = WorkspaceSnapshot {
            revision: 12,
            count: 3,
            active_index: 1,
            last_index: Some(0),
            names: vec!["1".to_owned(), "2".to_owned(), "3".to_owned()],
        };
        assert_reply_round_trip(&call, &ControlResponse::Workspaces(source.clone()));
    }

    #[test]
    fn settings_reply_decoder_restores_percent_semantics_for_known_keys() {
        let call = WireCall::new(IFACE_SETTINGS, "GetSnapshot", Vec::new());
        let mut source = SettingsSnapshot::new(2);
        source
            .values
            .insert("surfaceOpacity".to_owned(), SettingValue::Percent(83));
        assert_reply_round_trip(&call, &ControlResponse::Settings(source.clone()));
    }

    fn window_fixture() -> WindowSnapshot {
        WindowSnapshot {
            reference: WindowRef::new(42, 3),
            title: "Editor".to_owned(),
            app_id: DesktopAppId::new("org.example.Editor"),
            outer_geometry: Rect::new(10, 20, 800, 600),
            restore_geometry: Rect::new(30, 40, 640, 480),
            state: WindowState::Maximized,
            sticky: false,
            focused: true,
            workspace: flamewm_api::WorkspaceRef::new(1, 8),
            output: OutputId::new("HDMI-1"),
            state_generation: 9,
        }
    }

    fn application_fixture() -> DesktopApplication {
        DesktopApplication {
            id: DesktopAppId::new("org.example.Editor"),
            name: "Editor".to_owned(),
            generic_name: "Text Editor".to_owned(),
            comment: "Edit text files".to_owned(),
            startup_wm_class: "Editor".to_owned(),
            argv: vec!["editor".to_owned(), "%U".to_owned()],
            keywords: vec!["write".to_owned()],
            icon_name: "editor".to_owned(),
            categories: vec!["Utility".to_owned()],
        }
    }

    #[test]
    fn shell_bootstrap_round_trips_aggregate_snapshots() {
        use flamewm_api::display::DisplaySnapshot;
        use flamewm_api::panels::PanelsSnapshot;
        use flamewm_api::session::SessionCapabilities;
        use flamewm_api::shell_bootstrap::ShellBootstrapSnapshot;
        use flamewm_api::system::SystemSnapshot;
        use flamewm_api::workspace::WorkspaceSnapshot;
        let call = WireCall::new(IFACE_ROOT, "GetShellBootstrap", Vec::new());
        let source = ShellBootstrapSnapshot {
            windows_revision: 9,
            windows: vec![window_fixture()],
            workspaces: WorkspaceSnapshot {
                revision: 3,
                count: 2,
                active_index: 0,
                last_index: None,
                names: vec!["1".to_owned(), "2".to_owned()],
            },
            panels: PanelsSnapshot {
                revision: 4,
                panels: Vec::new(),
                tasks: Vec::new(),
                pinned_apps: Vec::new(),
            },
            applications: vec![application_fixture()],
            session: SessionCapabilities {
                lock: true,
                logout: true,
                suspend: false,
                reboot: false,
                shutdown: true,
            },
            displays: DisplaySnapshot {
                generation: 5,
                outputs: Vec::new(),
                pending: None,
            },
            system: SystemSnapshot::default(),
        };
        assert_reply_round_trip(&call, &ControlResponse::ShellBootstrap(source.clone()));
    }

    #[test]
    fn lock_preview_session_action_round_trips_through_wire_dispatch() {
        let request = ControlRequest::SessionAction(SessionAction::LockPreview);
        let call = assert_request_round_trip(&request);
        assert_eq!(call.member, "LockPreview");
        assert_ne!(
            decode_call(&WireCall::new(IFACE_SESSION, "Lock", Vec::new())).expect("decode lock"),
            request
        );
    }

    #[test]
    fn screensaver_preview_session_action_round_trips_additively() {
        let request = ControlRequest::SessionAction(SessionAction::ScreensaverPreview);
        let call = assert_request_round_trip(&request);
        assert_eq!(call.interface, IFACE_SESSION);
        assert_eq!(call.member, "ScreensaverPreview");
        assert!(call.args.is_empty());
        assert_ne!(
            decode_call(&WireCall::new(IFACE_SESSION, "LockPreview", Vec::new())).expect("decode"),
            request
        );
        assert_reply_round_trip(&call, &ControlResponse::Unit);
    }

    #[test]
    fn shell_snapshot_requests_round_trip_through_wire_mapping() {
        for request in [ControlRequest::GetWindows, ControlRequest::GetApplications] {
            assert_request_round_trip(&request);
        }
    }

    #[test]
    fn application_launch_request_round_trips_options_through_wire_mapping() {
        let request = ControlRequest::LaunchApplication {
            app: DesktopAppId::new("org.example.Browser"),
            options: ApplicationLaunchOptions {
                extra_args: vec!["--private".to_owned()],
                uris: vec!["https://example.test".to_owned()],
            },
        };
        let call = assert_request_round_trip(&request);
        assert_reply_round_trip(&call, &ControlResponse::Unit);
    }

    #[test]
    fn window_control_requests_round_trip_through_wire_mapping() {
        for request in [
            ControlRequest::ActivateWindow(WindowRef::new(7, 3)),
            ControlRequest::MinimizeWindow(WindowRef::new(7, 3)),
            ControlRequest::RestoreWindow(WindowRef::new(7, 3)),
            ControlRequest::CloseWindow(WindowRef::new(7, 3)),
        ] {
            let call = assert_request_round_trip(&request);
            assert_reply_round_trip(&call, &ControlResponse::Unit);
        }
    }

    #[test]
    fn window_snapshot_reply_round_trips_through_wire_mapping() {
        let call = WireCall::new(IFACE_WINDOWS, "GetWindows", Vec::new());
        let source = vec![window_fixture()];
        assert_reply_round_trip(&call, &ControlResponse::Windows(source.clone()));
    }

    #[test]
    fn application_snapshot_reply_round_trips_through_wire_mapping() {
        let call = WireCall::new(IFACE_APPLICATIONS, "GetApplications", Vec::new());
        let source = vec![application_fixture()];
        assert_reply_round_trip(&call, &ControlResponse::Applications(source.clone()));
    }

    fn system_fixture() -> SystemSnapshot {
        SystemSnapshot {
            revision: 11,
            network: NetworkSnapshot {
                availability: ServiceAvailability::Available,
                generation: 3,
                kind: NetworkKind::Wireless,
                label: "ArkNet 5G".to_owned(),
                strength_percent: 82,
                wifi_enabled: true,
                networking_enabled: true,
                active_path: "/nm/ap/1".to_owned(),
                active_access_point_path: "/nm/ap/1".to_owned(),
                access_points: vec![NetworkAccessPointSnapshot {
                    path: "/nm/ap/1".to_owned(),
                    ssid: "ArkNet 5G".to_owned(),
                    strength_percent: 82,
                    secured: true,
                    known: true,
                }],
                pending_secret: None,
            },
            media: MediaSnapshot {
                availability: ServiceAvailability::Available,
                generation: 5,
                active_bus_name: "org.mpris.elisa".to_owned(),
                identity: "Elisa".to_owned(),
                title: "Neon Skyline".to_owned(),
                artist: "Prototype Player".to_owned(),
                playback: PlaybackState::Playing,
                can_play: true,
                can_pause: true,
                can_next: true,
                can_previous: false,
            },
            audio: AudioSnapshot {
                availability: ServiceAvailability::Available,
                generation: 7,
                server_generation: 2,
                sink_name: "alsa_output".to_owned(),
                volume_percent: 72,
                muted: false,
                ..AudioSnapshot::default()
            }
            .with_items(
                vec![AudioEndpointSnapshot {
                    id: 7,
                    kind: AudioEndpointKind::Sink,
                    name: "alsa_output".to_owned(),
                    description: "Speakers".to_owned(),
                    volume_percent: 72,
                    muted: false,
                    is_default: true,
                }],
                vec![AudioStreamSnapshot {
                    id: 8,
                    endpoint_id: 7,
                    name: "Player".to_owned(),
                    volume_percent: 72,
                    muted: false,
                    app_name: Some("Spotify".to_owned()),
                    app_id: Some("com.spotify.Client".to_owned()),
                    icon_name: Some("spotify-client".to_owned()),
                    process_binary: Some("spotify".to_owned()),
                    media_name: Some("Track".to_owned()),
                }],
            ),
            power: PowerSnapshot {
                available: ServiceAvailability::Available,
                present: true,
                percent: 87,
                state: PowerState::Discharging,
                cpu_percent: Some(13),
                power_rate_mw: Some(12340),
            },
        }
    }

    #[test]
    fn power_snapshot_round_trips_with_and_without_cpu_sample() {
        for cpu_percent in [Some(13), None] {
            let source = SystemSnapshot {
                power: PowerSnapshot {
                    cpu_percent,
                    ..system_fixture().power
                },
                ..system_fixture()
            };
            let dict = system_dict(&source);
            assert_eq!(system_snapshot(&dict).expect("decode"), source);
            assert_eq!(dict.contains_key("powerCpuPercent"), cpu_percent.is_some());
        }
        for power_rate_mw in [Some(12340), None] {
            let source = SystemSnapshot {
                power: PowerSnapshot {
                    power_rate_mw,
                    ..system_fixture().power
                },
                ..system_fixture()
            };
            let dict = system_dict(&source);
            assert_eq!(system_snapshot(&dict).expect("decode"), source);
            assert_eq!(
                dict.contains_key("powerRateMilliwatts"),
                power_rate_mw.is_some()
            );
        }
    }

    #[test]
    fn older_peer_without_power_keys_decodes_to_default_power() {
        let mut dict = system_dict(&system_fixture());
        dict.retain(|key, _| !key.starts_with("power"));
        let decoded = system_snapshot(&dict).expect("older peer must decode");
        assert_eq!(decoded.power, PowerSnapshot::default());
        assert_eq!(decoded.network, system_fixture().network);
    }

    #[test]
    fn battery_menu_actions_round_trip_through_wire_mapping() {
        for action in [
            SystemAction::BatteryMenuOpened,
            SystemAction::BatteryMenuClosed,
        ] {
            let request = ControlRequest::SystemAction {
                action,
                expected_revision: 4,
            };
            assert_request_round_trip(&request);
        }
    }

    #[test]
    fn weather_feed_requests_and_reply_round_trip_through_wire() {
        let call = assert_request_round_trip(&ControlRequest::GetWeatherFeed);
        assert_eq!((call.interface.as_str(), call.member.as_str()), (IFACE_WEATHER, "GetSnapshot"));
        let publish = assert_request_round_trip(&ControlRequest::PublishWeatherFeed {
            payload: "a\u{1f}b".to_owned(),
        });
        assert_eq!((publish.interface.as_str(), publish.member.as_str()), (IFACE_WEATHER, "Publish"));
        let feed = flamewm_api::weather::WeatherFeedSnapshot { revision: 4, payload: "p".to_owned() };
        assert_reply_round_trip(
            &nullary_call(IFACE_WEATHER, "GetSnapshot"),
            &ControlResponse::WeatherFeed(feed.clone()),
        );
        let signal = ControlSignal::WeatherFeedChanged { snapshot: feed };
        let wire = encode_signal(&signal);
        assert_eq!((wire.interface.as_str(), wire.member.as_str()), (IFACE_WEATHER, "FeedChanged"));
        assert_eq!(decode_signal(&wire).expect("feed signal decodes"), signal);
    }

    #[test]
    fn system_call_round_trips_through_wire_mapping() {
        let call = assert_request_round_trip(&ControlRequest::GetSystem);
        assert_eq!(call.interface, IFACE_SYSTEM);
        assert_eq!(call.member, "GetSnapshot");
    }

    #[test]
    fn system_reply_round_trips_full_authoritative_snapshot() {
        let call = WireCall::new(IFACE_SYSTEM, "GetSnapshot", Vec::new());
        let source = system_fixture();
        assert_reply_round_trip(&call, &ControlResponse::System(source.clone()));
    }

    #[test]
    fn audio_stream_identity_round_trips_with_and_without_optional_keys() {
        let bare = AudioStreamSnapshot {
            id: 3,
            endpoint_id: 7,
            name: "Stream".to_owned(),
            volume_percent: 40,
            muted: true,
            ..AudioStreamSnapshot::default()
        };
        let bare_wire = audio_stream_wire_value(&bare);
        let WireValue::Dict(bare_dict) = &bare_wire else {
            panic!("stream must encode as a dict");
        };
        for key in ["appName", "appId", "iconName", "processBinary", "mediaName"] {
            assert!(
                !bare_dict.contains_key(key),
                "absent identity must not be encoded: {key}"
            );
        }
        assert_eq!(
            audio_stream_snapshot(&bare_wire).expect("decode bare"),
            bare
        );

        let identified = AudioStreamSnapshot {
            app_name: Some("Spotify".to_owned()),
            app_id: Some("com.spotify.Client".to_owned()),
            icon_name: Some("spotify-client".to_owned()),
            process_binary: Some("spotify".to_owned()),
            media_name: Some("Track".to_owned()),
            ..bare
        };
        let identified_wire = audio_stream_wire_value(&identified);
        assert_eq!(
            audio_stream_snapshot(&identified_wire).expect("decode identified"),
            identified
        );
    }

    #[test]
    fn system_actions_and_change_signal_round_trip_through_wire_mapping() {
        let request = ControlRequest::SystemAction {
            action: SystemAction::ConnectWifi {
                access_point_path: "/network/ap/1".to_owned(),
                generation: 3,
            },
            expected_revision: 11,
        };
        let call = assert_request_round_trip(&request);
        assert_eq!(call.member, "ConnectWifi");
        let signal = ControlSignal::SystemChanged { revision: 12 };
        assert_eq!(
            decode_signal(&encode_signal(&signal)).expect("decode signal"),
            signal
        );
    }

    #[test]
    fn audio_target_actions_round_trip_with_identity_and_generations() {
        for action in [
            SystemAction::SetVolume(AudioVolumeAction {
                target: AudioTarget::Endpoint {
                    id: 7,
                    kind: AudioEndpointKind::Sink,
                },
                percent: 150,
                generation: 5,
                server_generation: 3,
            }),
            SystemAction::SetMute(AudioMuteAction {
                target: AudioTarget::Stream { id: 11 },
                muted: true,
                generation: 5,
                server_generation: 3,
            }),
        ] {
            let request = ControlRequest::SystemAction {
                action,
                expected_revision: 9,
            };
            assert_request_round_trip(&request);
        }
    }

    #[test]
    fn audio_volume_above_provider_range_is_rejected() {
        let call = WireCall::new(
            IFACE_SYSTEM,
            "SetVolume",
            vec![
                WireValue::String("endpoint".to_owned()),
                WireValue::String("sink".to_owned()),
                WireValue::U32(7),
                WireValue::U32(151),
                WireValue::U64(5),
                WireValue::U64(3),
                WireValue::U64(9),
            ],
        );
        assert_eq!(
            decode_call(&call).expect_err("out of range").code,
            ErrorCode::InvalidArgument
        );
    }

    #[test]
    fn snapshot_signals_round_trip_revision_and_snapshot() {
        let windows = ControlSignal::WindowsSnapshotChanged {
            revision: 9,
            windows: vec![window_fixture()],
        };
        assert_eq!(
            decode_signal(&encode_signal(&windows)).expect("decode"),
            windows
        );
        let legacy_windows = ControlSignal::WindowsChanged { revision: 9 };
        assert_eq!(
            decode_signal(&encode_signal(&legacy_windows)).expect("decode"),
            legacy_windows
        );
        let workspaces = ControlSignal::WorkspacesSnapshotChanged {
            snapshot: WorkspaceSnapshot {
                revision: 12,
                count: 2,
                active_index: 1,
                last_index: Some(0),
                names: vec!["1".to_owned(), "2".to_owned()],
            },
        };
        assert_eq!(
            decode_signal(&encode_signal(&workspaces)).expect("decode"),
            workspaces
        );
        let legacy_workspaces = ControlSignal::WorkspacesChanged { revision: 12 };
        assert_eq!(
            decode_signal(&encode_signal(&legacy_workspaces)).expect("decode"),
            legacy_workspaces
        );
        let panels = ControlSignal::PanelsSnapshotChanged {
            snapshot: PanelsSnapshot {
                revision: 4,
                panels: Vec::new(),
                tasks: Vec::new(),
                pinned_apps: Vec::new(),
            },
        };
        assert_eq!(
            decode_signal(&encode_signal(&panels)).expect("decode"),
            panels
        );
        let legacy_panels = ControlSignal::PanelsChanged { revision: 4 };
        assert_eq!(
            decode_signal(&encode_signal(&legacy_panels)).expect("decode"),
            legacy_panels
        );
        let displays = ControlSignal::DisplaysSnapshotChanged {
            snapshot: DisplaySnapshot {
                generation: 9,
                outputs: Vec::new(),
                pending: None,
            },
        };
        assert_eq!(
            decode_signal(&encode_signal(&displays)).expect("decode"),
            displays
        );
    }

    // Start-menu toggle is a zero-argument root-interface signal; it must survive the
    // wire round trip as the unit variant and must reject a stray payload argument.
    #[test]
    fn start_menu_toggle_requested_round_trips_through_wire() {
        let signal = ControlSignal::StartMenuToggleRequested;
        let wire = encode_signal(&signal);
        assert_eq!(wire.interface, IFACE_ROOT);
        assert_eq!(wire.member, "StartMenuToggleRequested");
        assert!(wire.args.is_empty());
        assert_eq!(decode_signal(&wire), Ok(signal));

        let with_payload = WireSignal {
            interface: IFACE_ROOT.to_owned(),
            member: "StartMenuToggleRequested".to_owned(),
            args: vec![WireValue::U64(1)],
        };
        assert!(decode_signal(&with_payload).is_err());
    }

    // Launcher toggle mirrors the Start toggle: zero-argument root signal, unit variant.
    #[test]
    fn launcher_toggle_requested_round_trips_through_wire() {
        let signal = ControlSignal::LauncherToggleRequested;
        let wire = encode_signal(&signal);
        assert_eq!(wire.interface, IFACE_ROOT);
        assert_eq!(wire.member, "LauncherToggleRequested");
        assert!(wire.args.is_empty());
        assert_eq!(decode_signal(&wire), Ok(signal));

        let with_payload = WireSignal {
            interface: IFACE_ROOT.to_owned(),
            member: "LauncherToggleRequested".to_owned(),
            args: vec![WireValue::U64(1)],
        };
        assert!(decode_signal(&with_payload).is_err());
    }

    // The switcher signal carries its full entry list in MRU order; the decoder keeps order.
    #[test]
    fn window_switcher_changed_round_trips_through_wire() {
        let signal = ControlSignal::WindowSwitcherChanged {
            open: true,
            immediate: false,
            selected: 1,
            entries: vec![
                SwitcherEntry {
                    title: "Terminal".to_owned(),
                    app_id: "org.flame.terminal".to_owned(),
                },
                SwitcherEntry {
                    title: "Files".to_owned(),
                    app_id: String::new(),
                },
            ],
        };
        let wire = encode_signal(&signal);
        assert_eq!(wire.interface, IFACE_ROOT);
        assert_eq!(wire.member, "WindowSwitcherChanged");
        assert_eq!(wire.args.len(), 4);
        assert_eq!(decode_signal(&wire), Ok(signal));

        let closed = ControlSignal::WindowSwitcherChanged {
            open: false,
            immediate: false,
            selected: 0,
            entries: Vec::new(),
        };
        assert_eq!(decode_signal(&encode_signal(&closed)), Ok(closed));

        let short = WireSignal {
            interface: IFACE_ROOT.to_owned(),
            member: "WindowSwitcherChanged".to_owned(),
            args: vec![WireValue::Bool(true)],
        };
        assert!(decode_signal(&short).is_err());
    }

    // CONTRACT-REGRESSION: T05 SystemSnapshotChanged carries the complete authoritative snapshot.
    // TRIGGER: receive the canonical SystemSnapshotChanged interface/member and dictionary payload.
    // OBSERVABLE: the decoder admits the signal and preserves every snapshot field exactly.
    // GAP: the existing SystemChanged test covers only the compatibility revision signal.
    // MUTATION: omit the SystemSnapshotChanged decoder arm or decode its payload as SystemChanged; test must fail.
    // CASE: full network, media, and audio fixture exercises nested and non-default fields.
    #[test]
    fn t05_system_snapshot_changed_round_trips_exact_snapshot_through_wire() {
        let snapshot = system_fixture();
        let expected = ControlSignal::SystemSnapshotChanged {
            snapshot: snapshot.clone(),
        };
        assert_eq!(decode_signal(&encode_signal(&expected)), Ok(expected));
        let signal = WireSignal {
            interface: IFACE_SYSTEM.to_owned(),
            member: "SystemSnapshotChanged".to_owned(),
            args: vec![WireValue::Dict(system_dict(&snapshot))],
        };

        let decoded = decode_signal(&signal).expect("SystemSnapshotChanged must decode");
        assert_eq!(
            format!("{decoded:?}"),
            format!("SystemSnapshotChanged {{ snapshot: {:?} }}", snapshot)
        );

        let legacy = ControlSignal::SystemChanged { revision: 12 };
        assert_eq!(decode_signal(&encode_signal(&legacy)), Ok(legacy));
    }
}

#[cfg(test)]
mod hotkey_capture_lease_wire_tests {
    use super::*;

    #[test]
    fn begin_capture_request_and_lease_reply_round_trip_through_wire() {
        let request = ControlRequest::BeginHotkeyCapture {
            action: flamewm_api::shortcuts::ACTION_TOGGLE_START_MENU.to_owned(),
            expected_revision: 7,
        };
        let call = encode_call(&request).expect("encode request");
        assert_eq!(call.interface, IFACE_SHORTCUTS);
        assert_eq!(call.member, "BeginCapture");
        assert_eq!(decode_call(&call).expect("decode request"), request);

        let response = ControlResponse::CaptureLease(3);
        let reply = encode_reply(&call, &response).expect("encode reply");
        assert_eq!(reply.values, vec![WireValue::U64(3)]);
        assert_eq!(decode_reply(&call, &reply).expect("decode reply"), response);
    }

    #[test]
    fn end_capture_request_and_unit_reply_round_trip_through_wire() {
        let request = ControlRequest::EndHotkeyCapture { lease: 3 };
        let call = encode_call(&request).expect("encode request");
        assert_eq!(call.interface, IFACE_SHORTCUTS);
        assert_eq!(call.member, "EndCapture");
        assert_eq!(call.args, vec![WireValue::U64(3)]);
        assert_eq!(decode_call(&call).expect("decode request"), request);

        let reply = encode_reply(&call, &ControlResponse::Unit).expect("encode reply");
        assert!(reply.values.is_empty());
        assert_eq!(
            decode_reply(&call, &reply).expect("decode reply"),
            ControlResponse::Unit
        );
    }

    #[test]
    fn capture_lease_calls_reject_wrong_arity_and_types() {
        let begin_wrong_type = WireCall::new(
            IFACE_SHORTCUTS,
            "BeginCapture",
            vec![WireValue::U64(1), WireValue::U64(1)],
        );
        assert!(decode_call(&begin_wrong_type).is_err());

        let begin_short = WireCall::new(
            IFACE_SHORTCUTS,
            "BeginCapture",
            vec![WireValue::String("x".to_owned())],
        );
        assert!(decode_call(&begin_short).is_err());

        let end_wrong_type = WireCall::new(
            IFACE_SHORTCUTS,
            "EndCapture",
            vec![WireValue::String("3".to_owned())],
        );
        assert!(decode_call(&end_wrong_type).is_err());
    }
    #[test]
    fn glass_keys_decode_to_their_persisted_wire_types() {
        for (key, value) in [
            ("glassOpacityPercent", WireValue::U32(42)),
            ("glassBlurPx", WireValue::U32(54)),
            ("taskbarOpacityPercent", WireValue::U32(75)),
        ] {
            assert!(
                matches!(
                    super::setting_value_for_key(key, &value).expect("percent"),
                    SettingValue::Percent(_)
                ),
                "{key}"
            );
        }
        assert_eq!(
            super::setting_value_for_key("glassifyContrastZones", &WireValue::Bool(true))
                .expect("switch"),
            SettingValue::Boolean(true)
        );
        assert_eq!(
            super::setting_value_for_key("reduceTransparency", &WireValue::Bool(false))
                .expect("switch"),
            SettingValue::Boolean(false)
        );
        assert_eq!(
            super::setting_value_for_key(
                "taskbarMaterialMode",
                &WireValue::String("custom-tint".to_owned())
            )
            .expect("mode"),
            SettingValue::Text("custom-tint".to_owned())
        );
        assert_eq!(
            super::setting_value_for_key("taskbarCustomTint", &WireValue::String("#3DAEE9".into()))
                .expect("tint"),
            SettingValue::Text("#3DAEE9".to_owned())
        );
    }

    #[test]
    fn blur_percent_keys_decode_as_percent_like_the_opacities() {
        for key in [
            "startMenuBlurPercent",
            "surfaceOpacity",
            "surfaceBlurPercent",
        ] {
            assert_eq!(
                super::setting_value_for_key(key, &WireValue::U32(80)).expect("percent"),
                SettingValue::Percent(80),
                "{key}"
            );
        }
    }
}

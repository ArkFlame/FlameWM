use std::collections::BTreeMap;
use std::path::PathBuf;

use flamewm_api::applications::ApplicationLaunchOptions;
use flamewm_api::background::BackgroundState;
use flamewm_api::display::{DisplayMode, DisplaySnapshot, OutputSnapshot};
use flamewm_api::input::PointerPosition;
use flamewm_api::ports::*;
use flamewm_api::session::{SessionAction, SessionCapabilities};
use flamewm_api::settings::{SettingValue, SettingsChange, SettingsTransaction};
use flamewm_api::shortcuts::KeyBinding;
use flamewm_api::window::WindowSnapshot;
use flamewm_api::workspace::{WorkspacePlan, WorkspaceSnapshot};
use flamewm_api::{
    DesktopAppId, FlameResult, ModeId, OutputId, Point, Rect, Size, TransactionId, WindowRef,
};
use flamewm_control_core::{ControlRequest, ControlResponse, Dispatcher};
use flamewm_platform::host::PlatformHost;

struct FakeDesktop {
    workspaces: WorkspaceSnapshot,
    display: DisplaySnapshot,
    reservations: Vec<(OutputId, Rect)>,
    tray: Option<OutputId>,
    staged_shortcuts: Option<BTreeMap<String, KeyBinding>>,
    next_handle: u64,
    previous_mode: Option<ModeId>,
    window: WindowSnapshot,
    closed_windows: Vec<WindowRef>,
}

impl FakeDesktop {
    fn new() -> Self {
        let output = OutputId::new("eDP-1");
        Self {
            workspaces: WorkspaceSnapshot {
                revision: 1,
                count: 2,
                active_index: 0,
                last_index: None,
                names: vec!["Workspace 1".to_owned(), "Workspace 2".to_owned()],
            },
            display: DisplaySnapshot {
                generation: 1,
                outputs: vec![OutputSnapshot {
                    id: output.clone(),
                    connector: "eDP-1".to_owned(),
                    edid_identity: "panel".to_owned(),
                    connected: true,
                    primary: true,
                    geometry: Rect::new(0, 0, 1920, 1080),
                    current_mode: ModeId(1),
                    modes: vec![
                        DisplayMode {
                            id: ModeId(1),
                            resolution: Size::new(1920, 1080),
                            refresh_millihz: 60_000,
                            preferred: true,
                        },
                        DisplayMode {
                            id: ModeId(2),
                            resolution: Size::new(1600, 900),
                            refresh_millihz: 60_000,
                            preferred: false,
                        },
                    ],
                    shell_scale_percent: 100,
                }],
                pending: None,
            },
            reservations: Vec::new(),
            tray: None,
            staged_shortcuts: None,
            next_handle: 1,
            previous_mode: None,
            window: WindowSnapshot {
                reference: WindowRef::new(42, 3),
                title: "Fake window".to_owned(),
                app_id: DesktopAppId::new("org.example.Fake"),
                outer_geometry: Rect::new(0, 0, 800, 600),
                restore_geometry: Rect::new(0, 0, 800, 600),
                state: flamewm_api::window::WindowState::Normal,
                sticky: false,
                focused: false,
                workspace: flamewm_api::WorkspaceRef::new(0, 1),
                output: output.clone(),
                state_generation: 1,
            },
            closed_windows: Vec::new(),
        }
    }

    fn handle(&mut self) -> u64 {
        let value = self.next_handle;
        self.next_handle += 1;
        value
    }
}

impl WindowPort for FakeDesktop {
    fn get(&self, window: WindowRef) -> FlameResult<WindowSnapshot> {
        if window.id == self.window.reference.id {
            Ok(self.window.clone())
        } else {
            Err(flamewm_api::FlameError::new(
                flamewm_api::ErrorCode::NotFound,
                "window not found in fake",
            ))
        }
    }
    fn snapshot(&self) -> FlameResult<Vec<WindowSnapshot>> {
        Ok(vec![self.window.clone()])
    }
    fn activate(&mut self, _window: WindowRef) -> FlameResult<()> {
        Ok(())
    }
    fn minimize(&mut self, _window: WindowRef) -> FlameResult<()> {
        Ok(())
    }
    fn maximize(&mut self, _window: WindowRef) -> FlameResult<()> {
        Ok(())
    }
    fn restore(&mut self, _window: WindowRef) -> FlameResult<()> {
        Ok(())
    }
    fn close(&mut self, window: WindowRef) -> FlameResult<()> {
        self.closed_windows.push(window);
        Ok(())
    }
    fn set_outer_geometry(&mut self, _window: WindowRef, _geometry: Rect) -> FlameResult<()> {
        Ok(())
    }
    fn work_area(&self, _window: WindowRef) -> FlameResult<Rect> {
        Ok(Rect::new(0, 0, 1920, 1036))
    }
    fn output(&self, _window: WindowRef) -> FlameResult<OutputId> {
        Ok(OutputId::new("eDP-1"))
    }
}

impl WorkspacePort for FakeDesktop {
    fn workspace_snapshot(&self) -> FlameResult<WorkspaceSnapshot> {
        Ok(self.workspaces.clone())
    }
    fn activate_workspace(&mut self, index: usize, expected_revision: u64) -> FlameResult<()> {
        if self.workspaces.revision != expected_revision {
            return Err(flamewm_api::FlameError::stale("workspace revision"));
        }
        self.workspaces.last_index = Some(self.workspaces.active_index);
        self.workspaces.active_index = index;
        self.workspaces.revision += 1;
        Ok(())
    }
    fn move_window_to_workspace(&mut self, _window: WindowRef, _target: usize) -> FlameResult<()> {
        Ok(())
    }
    fn switch_workspace_with_window(
        &mut self,
        _window: WindowRef,
        target: usize,
        expected_revision: u64,
    ) -> FlameResult<()> {
        self.activate_workspace(target, expected_revision)
    }
    fn apply_workspace_plan(&mut self, plan: &WorkspacePlan) -> FlameResult<()> {
        if self.workspaces.revision != plan.expected_revision {
            return Err(flamewm_api::FlameError::stale("workspace plan revision"));
        }
        self.workspaces = WorkspaceSnapshot {
            revision: plan.new_revision,
            count: plan.new_count,
            active_index: plan.new_active,
            last_index: plan.new_last,
            names: plan.new_names.clone(),
        };
        Ok(())
    }
}

impl DisplayPort for FakeDesktop {
    fn display_snapshot(&self) -> FlameResult<DisplaySnapshot> {
        Ok(self.display.clone())
    }
    fn apply_mode(&mut self, output: &OutputId, mode: ModeId) -> FlameResult<TransactionId> {
        let selected = self
            .display
            .outputs
            .iter_mut()
            .find(|candidate| candidate.id == *output)
            .expect("fake output exists");
        self.previous_mode = Some(selected.current_mode);
        selected.current_mode = mode;
        Ok(TransactionId(9))
    }
    fn keep_mode(&mut self, _transaction: TransactionId) -> FlameResult<()> {
        self.previous_mode = None;
        Ok(())
    }
    fn revert_mode(&mut self, _transaction: TransactionId) -> FlameResult<()> {
        if let Some(previous) = self.previous_mode.take() {
            self.display.outputs[0].current_mode = previous;
        }
        Ok(())
    }
}

impl ShortcutPort for FakeDesktop {
    fn prepare_shortcuts(&mut self, desired: &BTreeMap<String, KeyBinding>) -> FlameResult<()> {
        self.staged_shortcuts = Some(desired.clone());
        Ok(())
    }
    fn commit_shortcuts(&mut self) -> FlameResult<()> {
        Ok(())
    }
    fn rollback_shortcuts(&mut self) {
        self.staged_shortcuts = None;
    }
}

impl WorkAreaPort for FakeDesktop {
    fn base_work_areas(&self) -> FlameResult<Vec<(OutputId, Rect)>> {
        Ok(vec![(OutputId::new("eDP-1"), Rect::new(0, 0, 1920, 1080))])
    }
    fn apply_flame_reservations(&mut self, reservations: &[(OutputId, Rect)]) -> FlameResult<()> {
        self.reservations = reservations.to_vec();
        Ok(())
    }
    fn request_recompute(&mut self) {}
}

impl MainLoopPort for FakeDesktop {
    fn add_poll(
        &mut self,
        _fd: i32,
        _events: FdEvents,
        _callback: FdCallback,
    ) -> FlameResult<FdHandle> {
        Ok(FdHandle(self.handle()))
    }
    fn remove_poll(&mut self, _handle: FdHandle) {}
    fn add_timer(
        &mut self,
        _delay_ms: u64,
        _callback: TimerCallback,
        _repeat: bool,
    ) -> FlameResult<TimerHandle> {
        Ok(TimerHandle(self.handle()))
    }
    fn remove_timer(&mut self, _handle: TimerHandle) {}
    fn defer(&mut self, _callback: TimerCallback) -> FlameResult<TimerHandle> {
        Ok(TimerHandle(self.handle()))
    }
}

impl InputPort for FakeDesktop {
    fn root_pointer(&self) -> FlameResult<PointerPosition> {
        Ok(PointerPosition {
            root: Point::new(10, 10),
            output: Some(OutputId::new("eDP-1")),
        })
    }
}

impl ApplicationPort for FakeDesktop {
    fn launch(
        &mut self,
        _app: &DesktopAppId,
        _options: &ApplicationLaunchOptions,
    ) -> FlameResult<()> {
        Ok(())
    }
    fn launch_uri(&mut self, _uri: &str) -> FlameResult<()> {
        Ok(())
    }
}

impl SessionPort for FakeDesktop {
    fn session_capabilities(&self) -> SessionCapabilities {
        SessionCapabilities {
            lock: true,
            logout: true,
            suspend: true,
            reboot: true,
            shutdown: true,
        }
    }
    fn perform_session_action(&mut self, _action: SessionAction) -> FlameResult<()> {
        Ok(())
    }
}

impl BackgroundPort for FakeDesktop {
    fn project_background(&mut self, _state: &BackgroundState) -> FlameResult<()> {
        Ok(())
    }
    fn reload_background(&mut self) -> FlameResult<()> {
        Ok(())
    }
}

impl TrayPort for FakeDesktop {
    fn set_tray_owner(&mut self, output: &OutputId) -> FlameResult<()> {
        self.tray = Some(output.clone());
        Ok(())
    }
    fn clear_tray_owner(&mut self) -> FlameResult<()> {
        self.tray = None;
        Ok(())
    }
    fn tray_owner(&self) -> FlameResult<Option<OutputId>> {
        Ok(self.tray.clone())
    }
}

fn settings_path(name: &str) -> PathBuf {
    std::env::temp_dir()
        .join(format!("flamewm-rust-{name}-{}", std::process::id()))
        .join("settings")
}

#[test]
fn platform_start_projects_panel_without_mutating_native_reservations() {
    let path = settings_path("start");
    let mut host = PlatformHost::new(FakeDesktop::new(), path);
    host.start().expect("host starts");
    let snapshot = host.panels_snapshot();
    assert_eq!(snapshot.panels.len(), 1);
    assert_eq!(snapshot.panels[0].output, OutputId::new("eDP-1"));
    assert_eq!(snapshot.panels[0].geometry, Rect::new(8, 1028, 1904, 44));
    assert!(host.engine().reservations.is_empty());
}

#[test]
fn control_workspace_insert_uses_revisioned_platform_transaction() {
    let path = settings_path("workspace");
    let mut host = PlatformHost::new(FakeDesktop::new(), path);
    host.start().expect("host starts");
    let dispatcher = Dispatcher::new();
    let response = dispatcher
        .dispatch(
            &mut host,
            ControlRequest::InsertWorkspaceAfter {
                index: 0,
                expected_revision: 1,
            },
        )
        .expect("transaction succeeds");
    assert_eq!(response, ControlResponse::Unit);
    let snapshot = match dispatcher
        .dispatch(&mut host, ControlRequest::GetWorkspaces)
        .expect("snapshot")
    {
        ControlResponse::Workspaces(snapshot) => snapshot,
        other => panic!("unexpected response: {other:?}"),
    };
    assert_eq!(snapshot.count, 3);
}

#[test]
fn control_close_window_dispatches_canonical_service_and_rejects_stale_reference() {
    let path = settings_path("close-window");
    let mut host = PlatformHost::new(FakeDesktop::new(), path);
    let dispatcher = Dispatcher::new();
    let window = WindowRef::new(42, 3);

    assert_eq!(
        dispatcher
            .dispatch(&mut host, ControlRequest::CloseWindow(window))
            .expect("close succeeds"),
        ControlResponse::Unit
    );
    assert_eq!(host.engine().closed_windows, vec![window]);

    let error = dispatcher
        .dispatch(
            &mut host,
            ControlRequest::CloseWindow(WindowRef::new(42, 2)),
        )
        .expect_err("stale window generation");
    assert_eq!(error.code, flamewm_api::ErrorCode::StaleRevision);
    assert_eq!(host.engine().closed_windows, vec![window]);
}

#[test]
fn settings_control_transaction_persists_only_after_native_shortcut_commit() {
    let path = settings_path("settings");
    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    let mut host = PlatformHost::new(FakeDesktop::new(), path.clone());
    host.start().expect("host starts");
    let dispatcher = Dispatcher::new();
    let transaction = SettingsTransaction {
        expected_revision: 1,
        changes: vec![SettingsChange {
            key: "fontBold".to_owned(),
            value: Some(SettingValue::Boolean(true)),
        }],
        reset_section: None,
    };
    let response = dispatcher
        .dispatch(&mut host, ControlRequest::ApplySettings(transaction))
        .expect("settings apply");
    let snapshot = match response {
        ControlResponse::Settings(snapshot) => snapshot,
        other => panic!("unexpected response: {other:?}"),
    };
    assert_eq!(snapshot.revision, 2);
    assert_eq!(
        snapshot.values.get("fontBold"),
        Some(&SettingValue::Boolean(true))
    );
    assert!(path.exists());
    let text = std::fs::read_to_string(&path).expect("settings file");
    assert!(text.contains("fontBold=1"));
}

#[test]
fn control_system_returns_host_snapshot_without_engine_round_trip() {
    let path = settings_path("system");
    let mut host = PlatformHost::new(FakeDesktop::new(), path);
    host.start().expect("host starts");
    host.system_mut().update_audio(
        flamewm_api::system::AudioSnapshot {
            availability: flamewm_api::system::ServiceAvailability::Available,
            generation: 1,
            server_generation: 1,
            sink_name: "alsa_output".to_owned(),
            volume_percent: 72,
            muted: false,
            ..flamewm_api::system::AudioSnapshot::default()
        }
        .with_items(
            vec![flamewm_api::system::AudioEndpointSnapshot {
                id: 7,
                kind: flamewm_api::system::AudioEndpointKind::Sink,
                name: "alsa_output".to_owned(),
                description: "Speakers".to_owned(),
                volume_percent: 72,
                muted: false,
                is_default: true,
            }],
            vec![flamewm_api::system::AudioStreamSnapshot {
                id: 9,
                endpoint_id: 7,
                name: "Player".to_owned(),
                volume_percent: 72,
                muted: false,
            }],
        ),
    );
    let expected = host.system_snapshot();
    let dispatcher = Dispatcher::new();
    let response = dispatcher
        .dispatch(&mut host, ControlRequest::GetSystem)
        .expect("system snapshot");
    assert_eq!(response, ControlResponse::System(expected));
}

#[test]
fn system_action_reports_explicit_stale_and_unsupported_errors() {
    let path = settings_path("system-action");
    let mut host = PlatformHost::new(FakeDesktop::new(), path);
    let dispatcher = Dispatcher::new();
    let stale = dispatcher
        .dispatch(
            &mut host,
            ControlRequest::SystemAction {
                action: flamewm_api::system::SystemAction::Scan,
                expected_revision: 1,
            },
        )
        .expect_err("stale revision");
    assert_eq!(stale.code, flamewm_api::ErrorCode::StaleRevision);
    let unsupported = dispatcher
        .dispatch(
            &mut host,
            ControlRequest::SystemAction {
                action: flamewm_api::system::SystemAction::Scan,
                expected_revision: 0,
            },
        )
        .expect_err("no provider installed");
    assert_eq!(unsupported.code, flamewm_api::ErrorCode::Unsupported);
}

#[test]
fn canonical_dbus_protocol_keeps_snapshot_signals_with_legacy_compat() {
    const XML: &str = include_str!("../protocol/com.arkflame.FlameWM1.xml");
    for member in [
        "WindowsChanged",
        "WindowsSnapshotChanged",
        "SettingsChanged",
        "SettingsSnapshotChanged",
        "WorkspacesChanged",
        "WorkspacesSnapshotChanged",
        "PanelsChanged",
        "PanelsSnapshotChanged",
    ] {
        assert!(XML.contains(&format!("name=\"{member}\"")));
    }
}

#[test]
fn canonical_dbus_displays_snapshot_signal_matches_wire_contract() {
    const XML: &str = include_str!("../protocol/com.arkflame.FlameWM1.xml");
    let interface = XML
        .split("<interface name=\"com.arkflame.FlameWM1.Displays\">")
        .nth(1)
        .and_then(|body| body.split("</interface>").next())
        .expect("Displays interface is declared");
    let signal = interface
        .split("<signal name=\"DisplaysSnapshotChanged\">")
        .nth(1)
        .and_then(|body| body.split("</signal>").next())
        .expect("DisplaysSnapshotChanged signal is declared");
    assert!(signal.contains("<arg name=\"snapshot\" type=\"a{sv}\"/>"));

    // The active wire contract places this signal on Displays with one dict snapshot.
    assert!(interface.contains("DisplaysSnapshotChanged"));
    assert_eq!(signal.matches("<arg ").count(), 1);
}

#[test]
fn canonical_dbus_start_menu_toggle_signal_matches_wire_contract() {
    const XML: &str = include_str!("../protocol/com.arkflame.FlameWM1.xml");
    let root = XML
        .split("<interface name=\"com.arkflame.FlameWM1\">")
        .nth(1)
        .and_then(|body| body.split("</interface>").next())
        .expect("root interface is declared");
    assert!(root.contains("<signal name=\"StartMenuToggleRequested\"/>"));
    // Self-closing tag: the start-menu request is one zero-argument signal on the root object.
    assert_eq!(root.matches("<signal ").count(), 1);
}

#[test]
fn canonical_dbus_protocol_keeps_v8_identity_based_reorder() {
    const XML: &str = include_str!("../protocol/com.arkflame.FlameWM1.xml");
    assert!(XML.contains("<method name=\"Reorder\">"));
    assert!(XML.contains("<arg name=\"entryId\" type=\"s\" direction=\"in\"/>"));
    assert!(XML.contains("<arg name=\"index\" type=\"u\" direction=\"in\"/>"));
    assert!(XML.contains("<arg name=\"expectedRevision\" type=\"t\" direction=\"in\"/>"));
}

#[test]
fn canonical_dbus_protocol_matches_system_action_wire_members() {
    const XML: &str = include_str!("../protocol/com.arkflame.FlameWM1.xml");
    for member in [
        "SetWifiEnabled",
        "ConnectKnown",
        "ConnectWifi",
        "SubmitNetworkSecret",
        "CancelNetworkSecret",
        "Disconnect",
        "Scan",
        "Play",
        "Pause",
        "PlayPause",
        "Next",
        "Previous",
        "SetVolume",
        "SetMute",
        "SystemChanged",
    ] {
        assert!(XML.contains(&format!("name=\"{member}\"")));
    }
}

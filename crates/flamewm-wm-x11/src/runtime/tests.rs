use std::collections::BTreeMap;

use flamewm_api::applications::ApplicationLaunchOptions;
use flamewm_api::display::{DisplaySnapshot, OutputSnapshot};
use flamewm_api::input::PointerPosition;
use flamewm_api::ports::{
    ApplicationPort, DisplayPort, InputPort, SessionPort, ShortcutPort, WindowPort, WorkspacePort,
};
use flamewm_api::session::{SessionAction, SessionCapabilities};
use flamewm_api::shortcuts::KeyBinding;
use flamewm_api::window::WindowSnapshot;
use flamewm_api::workspace::{WorkspacePlan, WorkspaceSnapshot};
use flamewm_api::{DesktopAppId, FlameResult, ModeId, OutputId, Rect, TransactionId, WindowRef};

use super::collect_wm_signals;
use crate::wm::WmChangeSet;

struct FakeEngine {
    windows: Vec<WindowSnapshot>,
    workspaces: WorkspaceSnapshot,
}

fn fake_window(id: u64, generation: u64) -> WindowSnapshot {
    WindowSnapshot {
        reference: WindowRef::new(id, 0),
        title: format!("w{id}"),
        app_id: DesktopAppId::new("org.example.Fake"),
        outer_geometry: Rect::new(0, 0, 100, 100),
        restore_geometry: Rect::new(0, 0, 100, 100),
        state: flamewm_api::window::WindowState::Normal,
        sticky: false,
        focused: false,
        workspace: flamewm_api::WorkspaceRef::new(0, 0),
        output: OutputId::new("eDP-1"),
        state_generation: generation,
        unresponsive: false,
        attention: false,
        skip_taskbar: false,
    }
}

impl FakeEngine {
    fn new() -> Self {
        Self {
            windows: vec![fake_window(1, 1)],
            workspaces: WorkspaceSnapshot {
                revision: 2,
                count: 2,
                active_index: 0,
                last_index: None,
                occupancy: Vec::new(),
                names: Vec::new(),
            },
        }
    }

    /// Simulates a desktop change that bypasses `activate_workspace`, as a
    /// root-property change observed by the WM would. Bumps the revision.
    fn external_desktop_change(&mut self, index: usize) {
        self.workspaces.active_index = index;
        self.workspaces.revision += 1;
    }

    fn host(self) -> PlatformHost<Self> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("flamewm-j13-{}-{id}", std::process::id()));
        let mut host = PlatformHost::new(self, path);
        host.start().expect("host starts");
        host
    }
}

use flamewm_platform::host::PlatformHost;

impl WindowPort for FakeEngine {
    fn get(&self, window: WindowRef) -> FlameResult<WindowSnapshot> {
        self.windows
            .iter()
            .find(|item| item.reference.id == window.id)
            .cloned()
            .ok_or_else(|| flamewm_api::FlameError::not_found("missing"))
    }
    fn snapshot(&self) -> FlameResult<Vec<WindowSnapshot>> {
        Ok(self.windows.clone())
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
        self.windows.retain(|item| item.reference.id != window.id);
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

impl WorkspacePort for FakeEngine {
    fn workspace_snapshot(&self) -> FlameResult<WorkspaceSnapshot> {
        Ok(self.workspaces.clone())
    }
    fn activate_workspace(&mut self, index: usize, revision: u64) -> FlameResult<()> {
        if revision != self.workspaces.revision {
            return Err(flamewm_api::FlameError::stale("workspace revision changed"));
        }
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
        revision: u64,
    ) -> FlameResult<()> {
        self.activate_workspace(target, revision)
    }
    fn apply_workspace_plan(&mut self, _plan: &WorkspacePlan) -> FlameResult<()> {
        Ok(())
    }
}

impl DisplayPort for FakeEngine {
    fn display_snapshot(&self) -> FlameResult<DisplaySnapshot> {
        Ok(DisplaySnapshot {
            generation: 1,
            outputs: vec![OutputSnapshot {
                id: OutputId::new("eDP-1"),
                connector: "eDP-1".to_owned(),
                edid_identity: "panel".to_owned(),
                connected: true,
                primary: true,
                geometry: Rect::new(0, 0, 1920, 1080),
                current_mode: ModeId(1),
                modes: Vec::new(),
                shell_scale_percent: 100,

                rotation: flamewm_api::display::Rotation::Normal,
            }],
            pending: None,
        })
    }
    fn apply_mode(&mut self, _output: &OutputId, _mode: ModeId) -> FlameResult<TransactionId> {
        Ok(TransactionId(1))
    }
    fn keep_mode(&mut self, _transaction: TransactionId) -> FlameResult<()> {
        Ok(())
    }
    fn revert_mode(&mut self, _transaction: TransactionId) -> FlameResult<()> {
        Ok(())
    }
}

impl ShortcutPort for FakeEngine {
    fn prepare_shortcuts(&mut self, _desired: &BTreeMap<String, KeyBinding>) -> FlameResult<()> {
        Ok(())
    }
    fn commit_shortcuts(&mut self) -> FlameResult<()> {
        Ok(())
    }
    fn rollback_shortcuts(&mut self) {}
}

impl InputPort for FakeEngine {
    fn root_pointer(&self) -> FlameResult<PointerPosition> {
        Ok(PointerPosition {
            root: flamewm_api::Point::new(0, 0),
            output: None,
        })
    }
}

impl ApplicationPort for FakeEngine {
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

impl SessionPort for FakeEngine {
    fn session_capabilities(&self) -> SessionCapabilities {
        SessionCapabilities {
            lock: false,
            logout: false,
            suspend: false,
            reboot: false,
            shutdown: false,
        }
    }
    fn perform_session_action(&mut self, _action: SessionAction) -> FlameResult<()> {
        Ok(())
    }
}

fn change(windows: bool, workspaces: bool, panels: bool) -> WmChangeSet {
    WmChangeSet {
        windows,
        workspaces,
        panels,
        work_area: false,
        start_menu_toggle: false,
        launcher_toggle: false,
        dock_auto_hide_toggle: false,
        media_audio_key: None,
        media_brightness_key: None,
    }
}

fn members(signals: &[flamewm_control_wire::ControlSignal]) -> Vec<&'static str> {
    signals
        .iter()
        .map(|signal| match signal {
            flamewm_control_wire::ControlSignal::WindowsSnapshotChanged { .. } => {
                "WindowsSnapshotChanged"
            }
            flamewm_control_wire::ControlSignal::WindowsChanged { .. } => "WindowsChanged",
            flamewm_control_wire::ControlSignal::WorkspacesSnapshotChanged { .. } => {
                "WorkspacesSnapshotChanged"
            }
            flamewm_control_wire::ControlSignal::WorkspacesChanged { .. } => "WorkspacesChanged",
            flamewm_control_wire::ControlSignal::PanelsSnapshotChanged { .. } => {
                "PanelsSnapshotChanged"
            }
            _ => "other",
        })
        .collect()
}

#[test]
fn empty_changeset_emits_no_signals() {
    let mut host = FakeEngine::new().host();
    let signals = collect_wm_signals(&mut host, change(false, false, false)).expect("collect");
    assert_eq!(signals.len(), 0);
}

// REGRESSION: a managed window changes its workspace occupancy, so the workspace snapshot is
// announced once in the same turn as the window list.
#[test]
fn manage_window_emits_windows_panels_and_occupancy_once() {
    let mut host = FakeEngine::new().host();
    let _ = collect_wm_signals(&mut host, change(true, false, false)).expect("seed");
    host.engine_mut().windows.push(fake_window(2, 2));
    let signals = collect_wm_signals(&mut host, change(true, false, false)).expect("collect");
    assert_eq!(
        members(&signals),
        vec![
            "WindowsSnapshotChanged",
            "WindowsChanged",
            "WorkspacesSnapshotChanged",
            "PanelsSnapshotChanged"
        ]
    );
}

#[test]
fn unchanged_windows_emit_no_window_signals() {
    let mut host = FakeEngine::new().host();
    let _ = collect_wm_signals(&mut host, change(true, false, false)).expect("seed");
    let signals = collect_wm_signals(&mut host, change(true, false, false)).expect("collect");
    assert_eq!(signals.len(), 0);
}

#[test]
fn workspace_switch_emits_workspaces_plus_panels() {
    let mut host = FakeEngine::new().host();
    let signals = collect_wm_signals(&mut host, change(false, true, false)).expect("collect");
    assert_eq!(
        members(&signals),
        vec![
            "WorkspacesSnapshotChanged",
            "WorkspacesChanged",
            "PanelsSnapshotChanged"
        ]
    );
}

#[test]
fn workspace_activation_rejects_revision_stale_after_external_desktop_change() {
    let mut engine = FakeEngine::new();
    let revision = engine.workspace_snapshot().expect("snapshot").revision;
    engine.external_desktop_change(1);
    let after = engine.workspace_snapshot().expect("snapshot");
    assert_ne!(after.revision, revision);
    let err = engine
        .activate_workspace(0, revision)
        .expect_err("stale revision must be rejected");
    assert_eq!(err.code, flamewm_api::ErrorCode::StaleRevision);
}

#[test]
fn work_area_only_emits_nothing() {
    let mut host = FakeEngine::new().host();
    let only_area = WmChangeSet {
        work_area: true,
        ..change(false, false, false)
    };
    let signals = collect_wm_signals(&mut host, only_area).expect("collect");
    assert_eq!(signals.len(), 0);
}

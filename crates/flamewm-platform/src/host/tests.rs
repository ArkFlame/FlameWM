use std::collections::BTreeMap;

use flamewm_api::display::{DisplayMode, DisplaySnapshot, OutputSnapshot};
use flamewm_api::geometry::{Rect, Size};
use flamewm_api::input::PointerPosition;
use flamewm_api::ports::{
    ApplicationPort, DisplayPort, InputPort, SessionPort, ShortcutPort, WindowPort,
    WorkspacePort,
};
use flamewm_api::settings::{SettingValue, SettingsChange, SettingsTransaction};
use flamewm_api::shortcuts::{ACTION_TOGGLE_START_MENU, ACTION_WINDOW_CLOSE};
use flamewm_api::window::WindowSnapshot;
use flamewm_api::workspace::{WorkspacePlan, WorkspaceSnapshot};

use super::*;

struct FakeEngine {
    display: DisplaySnapshot,
    reverted: Vec<TransactionId>,
}

impl WindowPort for FakeEngine {
    fn get(&self, _window: WindowRef) -> FlameResult<WindowSnapshot> {
        Err(FlameError::new(ErrorCode::NotFound, "no windows"))
    }
    fn snapshot(&self) -> FlameResult<Vec<WindowSnapshot>> {
        Ok(Vec::new())
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
    fn close(&mut self, _window: WindowRef) -> FlameResult<()> {
        Ok(())
    }
    fn set_outer_geometry(&mut self, _window: WindowRef, _geometry: Rect) -> FlameResult<()> {
        Ok(())
    }
    fn work_area(&self, _window: WindowRef) -> FlameResult<Rect> {
        Err(FlameError::new(ErrorCode::NotFound, "no windows"))
    }
    fn output(&self, _window: WindowRef) -> FlameResult<OutputId> {
        Err(FlameError::new(ErrorCode::NotFound, "no windows"))
    }
}

impl WorkspacePort for FakeEngine {
    fn workspace_snapshot(&self) -> FlameResult<WorkspaceSnapshot> {
        Err(FlameError::new(ErrorCode::Unsupported, "fake workspace"))
    }
    fn activate_workspace(
        &mut self,
        _index: usize,
        _expected_revision: u64,
    ) -> FlameResult<()> {
        Err(FlameError::new(ErrorCode::Unsupported, "fake workspace"))
    }
    fn move_window_to_workspace(
        &mut self,
        _window: WindowRef,
        _target: usize,
    ) -> FlameResult<()> {
        Err(FlameError::new(ErrorCode::Unsupported, "fake workspace"))
    }
    fn switch_workspace_with_window(
        &mut self,
        _window: WindowRef,
        _target: usize,
        _expected_revision: u64,
    ) -> FlameResult<()> {
        Err(FlameError::new(ErrorCode::Unsupported, "fake workspace"))
    }
    fn apply_workspace_plan(&mut self, _plan: &WorkspacePlan) -> FlameResult<()> {
        Err(FlameError::new(ErrorCode::Unsupported, "fake workspace"))
    }
}

impl DisplayPort for FakeEngine {
    fn display_snapshot(&self) -> FlameResult<DisplaySnapshot> {
        Ok(self.display.clone())
    }
    fn apply_mode(&mut self, _output: &OutputId, _mode: ModeId) -> FlameResult<TransactionId> {
        Ok(TransactionId(7))
    }
    fn keep_mode(&mut self, _transaction: TransactionId) -> FlameResult<()> {
        Ok(())
    }
    fn revert_mode(&mut self, transaction: TransactionId) -> FlameResult<()> {
        self.reverted.push(transaction);
        Ok(())
    }
}

impl ShortcutPort for FakeEngine {
    fn prepare_shortcuts(
        &mut self,
        _desired: &BTreeMap<String, KeyBinding>,
    ) -> FlameResult<()> {
        Ok(())
    }
    fn commit_shortcuts(&mut self) -> FlameResult<()> {
        Ok(())
    }
    fn rollback_shortcuts(&mut self) {}
}

impl InputPort for FakeEngine {
    fn root_pointer(&self) -> FlameResult<PointerPosition> {
        Err(FlameError::new(ErrorCode::Unsupported, "fake input"))
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
        SessionCapabilities::default()
    }
    fn perform_session_action(&mut self, _action: SessionAction) -> FlameResult<()> {
        Ok(())
    }
}

fn fake_display(shell_scale_percent: u16) -> DisplaySnapshot {
    DisplaySnapshot {
        generation: 1,
        outputs: vec![OutputSnapshot {
            id: OutputId::new("default"),
            connector: "default".to_owned(),
            edid_identity: String::new(),
            connected: true,
            primary: true,
            geometry: Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            current_mode: ModeId(1),
            modes: Vec::new(),
            shell_scale_percent,
        }],
        pending: None,
    }
}

#[test]
fn unconfirmed_display_mode_reverts_only_after_deadline_tick() {
    let path = fixture_path("display-tick");
    std::fs::write(&path, "").expect("write settings fixture");
    let mut display = fake_display(100);
    display.outputs[0].modes = vec![
        DisplayMode {
            id: ModeId(1),
            resolution: Size {
                width: 1920,
                height: 1080,
            },
            refresh_millihz: 60_000,
            preferred: true,
        },
        DisplayMode {
            id: ModeId(5),
            resolution: Size {
                width: 1280,
                height: 720,
            },
            refresh_millihz: 60_000,
            preferred: false,
        },
    ];
    let mut host = host_with_display(&path, display);
    host.start().expect("host starts");
    let generation = display_generation(&mut host);
    let begun = host.begin_display_mode(&OutputId::new("default"), ModeId(5), generation, 0);
    let _ = std::fs::remove_file(&path);
    assert_eq!(begun.expect("mode change begins"), TransactionId(7));

    // Before the 15s confirmation deadline the tick must not revert.
    assert!(!host.tick_display_transactions(14_999).expect("tick"));
    assert!(host.engine().reverted.is_empty());

    // At the deadline the pending transaction is reverted and cleared.
    assert!(host.tick_display_transactions(15_000).expect("tick"));
    assert_eq!(host.engine().reverted, vec![TransactionId(7)]);
    let snapshot = host.display_snapshot().expect("display snapshot");
    assert!(snapshot.pending.is_none());
}

#[test]
fn start_applies_persisted_per_output_scale_to_display_snapshot() {
    let path = fixture_path("scale");
    std::fs::write(&path, "scale.default=125\n").expect("write settings fixture");
    let mut host = host_at(&path, 100);
    let started = host.start();
    let _ = std::fs::remove_file(&path);
    started.expect("host starts");
    assert_eq!(cached_scale(&host), Some(125));
}

#[test]
fn startup_seeded_scale_sizes_panel_like_live_scale_change() {
    use crate::panel::logical_to_physical;

    let path = fixture_path("startup-panel-scale");
    std::fs::write(
        &path,
        "scale.default=125\ntaskbarHeight=72\ntaskbarStyle=classic\n",
    )
    .expect("write settings fixture");
    let mut display = fake_display(100);
    display.outputs[0].geometry = Rect {
        x: 0,
        y: 0,
        width: 1350,
        height: 641,
    };
    let mut host = host_with_display(&path, display);
    let started = host.start();
    let _ = std::fs::remove_file(&path);
    started.expect("host starts");

    let panel = host.panels.snapshot().panels[0].clone();
    assert_eq!(panel.logical_size, 72);
    assert_eq!(logical_to_physical(72, 125), 90);
    assert_eq!(panel.geometry.height, 90);
    assert_eq!(panel.geometry.width, 1350);
}

#[test]
fn runtime_scale_setting_resyncs_panel_thickness() {
    let path = fixture_path("runtime-panel-scale");
    std::fs::write(&path, "").expect("write settings fixture");
    let mut host = host_at(&path, 100);
    host.start().expect("host starts");
    assert_eq!(host.panels.snapshot().panels[0].geometry.height, 44);

    let revision = host.settings_snapshot().revision;
    let applied = host.apply_settings(&SettingsTransaction {
        expected_revision: revision,
        changes: vec![SettingsChange {
            key: "scale.default".to_owned(),
            value: Some(SettingValue::Integer(125)),
        }],
        reset_section: None,
    });
    let _ = std::fs::remove_file(&path);
    applied.expect("valid runtime scale applies");

    let panels = host.panels.snapshot();
    assert_eq!(panels.panels[0].logical_size, 44);
    assert_eq!(panels.panels[0].geometry.height, 55);
}

#[test]
fn runtime_scale_setting_updates_display_snapshot_and_rejects_invalid_values() {
    let path = fixture_path("runtime-scale");
    std::fs::write(&path, "").expect("write settings fixture");
    let mut host = host_at(&path, 100);
    host.start().expect("host starts");
    let transaction = |revision: u64, scale: i64| SettingsTransaction {
        expected_revision: revision,
        changes: vec![SettingsChange {
            key: "scale.default".to_owned(),
            value: Some(SettingValue::Integer(scale)),
        }],
        reset_section: None,
    };

    let revision = host.settings_snapshot().revision;
    let applied = host.apply_settings(&transaction(revision, 125));
    assert!(applied.is_ok(), "valid scale applies: {applied:?}");
    assert_eq!(cached_scale(&host), Some(125));

    let revision = host.settings_snapshot().revision;
    let rejected = host.apply_settings(&transaction(revision, 133));
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        rejected.expect_err("invalid scale is rejected").code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(cached_scale(&host), Some(125));
}

#[test]
fn set_shell_scale_resyncs_panel_thickness() {
    let path = fixture_path("set-shell-scale-panel");
    std::fs::write(&path, "").expect("write settings fixture");
    let mut host = host_at(&path, 100);
    host.start().expect("host starts");
    assert_eq!(host.panels.snapshot().panels[0].geometry.height, 44);

    let generation = display_generation(&mut host);
    let changed = host
        .set_shell_scale(&OutputId::new("default"), 125, generation)
        .expect("shell scale applies");
    let _ = std::fs::remove_file(&path);
    assert!(changed);

    // 44 logical px at 125% rounds to 55 physical px via logical_to_physical.
    let panels = host.panels.snapshot();
    assert_eq!(panels.panels[0].logical_size, 44);
    assert_eq!(panels.panels[0].geometry.height, 55);
}

#[test]
fn set_shell_scale_persists_scale_across_restart() {
    let path = fixture_path("set-shell-scale-persist");
    std::fs::write(&path, "").expect("write settings fixture");
    let mut host = host_at(&path, 100);
    host.start().expect("host starts");
    let revision_before = host.settings_snapshot().revision;
    let generation = display_generation(&mut host);
    let changed = host
        .set_shell_scale(&OutputId::new("default"), 125, generation)
        .expect("shell scale applies");
    assert!(changed);
    assert_eq!(
        host.settings_snapshot().revision,
        revision_before,
        "scale persistence must not make Settings clients stale"
    );
    drop(host);

    // A WM restart re-seeds the display snapshot from the settings file, as at first start.
    let mut restarted = host_at(&path, 100);
    let started = restarted.start();
    let persisted = std::fs::read_to_string(&path);
    let _ = std::fs::remove_file(&path);
    started.expect("host restarts");
    assert_eq!(cached_scale(&restarted), Some(125));
    let persisted = persisted.expect("settings file is written");
    assert!(persisted.contains("scale.default=125\n"), "{persisted}");
}

#[test]
fn arbitrary_five_percent_scales_survive_restart() {
    for scale in [105_u16, 135, 195, 200] {
        let path = fixture_path(&format!("scale-restart-{scale}"));
        let mut host = host_at(&path, 100);
        host.start().expect("start");
        let generation = display_generation(&mut host);
        assert!(
            host.set_shell_scale(&OutputId::new("default"), scale, generation)
                .expect("scale")
        );
        drop(host);
        let mut next = host_at(&path, 100);
        next.start().expect("restart");
        assert_eq!(cached_scale(&next), Some(scale));
        let _ = std::fs::remove_file(&path);
    }
}

#[test]
fn failed_shell_scale_persistence_restores_display_scale() {
    // The settings parent becomes a regular file after start, so persistence cannot create it.
    let dir = fixture_path("scale-rollback-dir");
    std::fs::create_dir_all(&dir).expect("create settings dir");
    let path = dir.join("settings.conf");
    let mut host = host_at(&path, 100);
    host.start().expect("host starts");
    std::fs::remove_dir_all(&dir).expect("remove settings dir");
    std::fs::write(&dir, "").expect("replace settings dir with a file");

    let generation = display_generation(&mut host);
    let error = host
        .set_shell_scale(&OutputId::new("default"), 125, generation)
        .expect_err("persistence failure is surfaced");
    assert_ne!(error.code, ErrorCode::StaleRevision);
    assert_eq!(cached_scale(&host), Some(100));
    let _ = std::fs::remove_file(&dir);
}

fn fixture_path(name: &str) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("flamewm-host-{name}-{}.conf", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path
}

fn host_with_display(
    path: &std::path::Path,
    display: DisplaySnapshot,
) -> PlatformHost<FakeEngine> {
    PlatformHost::new(
        FakeEngine {
            display,
            reverted: Vec::new(),
        },
        path.to_path_buf(),
    )
}

fn host_at(path: &std::path::Path, scale: u16) -> PlatformHost<FakeEngine> {
    host_with_display(path, fake_display(scale))
}

fn display_generation(host: &mut PlatformHost<FakeEngine>) -> u64 {
    host.display_snapshot()
        .expect("display snapshot")
        .generation
}

fn cached_scale(host: &PlatformHost<FakeEngine>) -> Option<u16> {
    host.cached_display_snapshot().and_then(|snapshot| {
        snapshot
            .outputs
            .first()
            .map(|output| output.shell_scale_percent)
    })
}

/// Starts a host on `path` and grabs its effective shortcut set, as a WM restart would.
fn started_hotkey_host(path: &std::path::Path) -> PlatformHost<FakeEngine> {
    let mut host = host_at(path, 100);
    host.start().expect("host starts");
    host.commit_startup_shortcuts().expect("startup grabs");
    host
}

// CONTRACT-REGRESSION: a Panels.SetEdge value persists across a WM restart (docs/TODO.md taskbar
// edge persistence) without moving the settings revision, so open Settings clients stay current.
// TRIGGER: set the top edge, drop the host, start a fresh host from the same settings file.
// OBSERVABLE: the reloaded panel reports Top and the settings file holds panel.edge.default=top.
// MUTATION: skip persist_panel_edge or the startup seed and the reloaded panel reports Bottom.
#[test]
fn set_panel_edge_persists_edge_across_restart_without_settings_revision_bump() {
    let path = fixture_path("panel-edge-persist");
    std::fs::write(&path, "").expect("write settings fixture");
    let mut host = host_at(&path, 100);
    host.start().expect("host starts");
    let settings_revision = host.settings_snapshot().revision;
    let panels_revision = host.panels_snapshot().revision;
    host.set_panel_edge(&OutputId::new("default"), PanelEdge::Top, panels_revision)
        .expect("edge applies");
    assert_eq!(host.settings_snapshot().revision, settings_revision);
    drop(host);

    let persisted = std::fs::read_to_string(&path).expect("edge is persisted");
    assert!(
        persisted.contains("panel.edge.default=top\n"),
        "{persisted}"
    );
    let mut restarted = host_at(&path, 100);
    restarted.start().expect("restart starts");
    assert_eq!(restarted.panels_snapshot().panels[0].edge, PanelEdge::Top);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn panel_edge_persists_across_output_disconnect_reconnect() {
    let path = fixture_path("panel-edge-hotplug");
    std::fs::write(&path, "").expect("create settings fixture");
    let mut host = host_at(&path, 100);
    host.start().expect("host starts");
    host.set_panel_edge(
        &OutputId::new("default"),
        PanelEdge::Top,
        host.panels_snapshot().revision,
    )
    .expect("persist panel edge");
    let original = host.cached_display_snapshot().expect("displays cached");
    let mut disconnected = original.clone();
    disconnected.outputs[0].connected = false;
    disconnected.outputs[0].geometry = Rect::default();
    assert!(
        host.resync_panels_for_displays(&disconnected)
            .expect("detach")
    );
    assert!(host.panels_snapshot().panels.is_empty());
    assert!(
        host.resync_panels_for_displays(&original)
            .expect("reattach")
    );
    let panels = host.panels_snapshot();
    let panel = &panels.panels[0];
    assert_eq!(panel.edge, PanelEdge::Top);
    // Classic (the product default) sits flush on the top edge, so no floating margin.
    assert_eq!(panel.geometry.y, 0);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn cleared_hotkey_persists_and_stays_unassigned_after_restart() {
    let path = fixture_path("hotkey-clear");
    let mut host = started_hotkey_host(&path);
    let revision = host.shortcut_snapshot().revision;
    host.clear_shortcut(ACTION_TOGGLE_START_MENU, revision)
        .expect("clear applies");
    // Idempotent: clearing an already-unassigned action succeeds without a new revision.
    host.clear_shortcut(ACTION_TOGGLE_START_MENU, revision + 1)
        .expect("repeated clear applies");
    let persisted = std::fs::read_to_string(&path).expect("clear is persisted");
    assert!(
        persisted.contains("hotkey.ToggleStartMenu=\n"),
        "{persisted}"
    );

    let reloaded = started_hotkey_host(&path);
    assert_eq!(
        reloaded
            .shortcut_snapshot()
            .bindings
            .get(ACTION_TOGGLE_START_MENU),
        Some(&KeyBinding::unassigned())
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn set_shortcut_persists_binding_across_restart() {
    let path = fixture_path("hotkey-set");
    let mut host = started_hotkey_host(&path);
    let revision = host.shortcut_snapshot().revision;
    host.set_shortcut(ACTION_WINDOW_CLOSE, "Super+F8", revision)
        .expect("set applies");
    let persisted = std::fs::read_to_string(&path).expect("set is persisted");
    assert!(
        persisted.contains("hotkey.WindowClose=Super+F8\n"),
        "{persisted}"
    );

    let reloaded = started_hotkey_host(&path);
    assert_eq!(
        reloaded
            .shortcut_snapshot()
            .bindings
            .get(ACTION_WINDOW_CLOSE),
        Some(&KeyBinding::assigned("Super+F8"))
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn reset_shortcut_removes_persisted_override_and_restores_default() {
    let path = fixture_path("hotkey-reset");
    std::fs::write(&path, "hotkey.WindowClose=Super+F8\n").expect("write override fixture");
    let mut host = started_hotkey_host(&path);
    assert_eq!(
        host.shortcut_snapshot().bindings.get(ACTION_WINDOW_CLOSE),
        Some(&KeyBinding::assigned("Super+F8"))
    );
    let revision = host.shortcut_snapshot().revision;
    host.reset_shortcut(ACTION_WINDOW_CLOSE, revision)
        .expect("reset applies");
    let persisted = std::fs::read_to_string(&path).expect("reset is persisted");
    assert!(!persisted.contains("hotkey.WindowClose="), "{persisted}");

    let reloaded = started_hotkey_host(&path);
    let defaults = ShortcutRegistry::default();
    assert_eq!(
        reloaded
            .shortcut_snapshot()
            .bindings
            .get(ACTION_WINDOW_CLOSE),
        defaults.binding(ACTION_WINDOW_CLOSE)
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn stale_hotkey_revision_is_refused_without_persisting() {
    let path = fixture_path("hotkey-stale");
    let mut host = started_hotkey_host(&path);
    let stale = host.shortcut_snapshot().revision + 1;
    let error = host
        .clear_shortcut(ACTION_TOGGLE_START_MENU, stale)
        .expect_err("stale revision is refused");
    assert_eq!(error.code, ErrorCode::StaleRevision);
    assert!(!path.exists());
    assert!(host.shortcut_snapshot().bindings[ACTION_TOGGLE_START_MENU].is_assigned());
}

#[test]
fn failed_hotkey_persistence_rolls_back_the_shortcut_mutation() {
    // The settings parent becomes a regular file after start, so persistence cannot create it.
    let dir = fixture_path("hotkey-rollback-dir");
    std::fs::create_dir_all(&dir).expect("create settings dir");
    let path = dir.join("settings.conf");
    let mut host = started_hotkey_host(&path);
    let before = host.shortcut_snapshot();
    std::fs::remove_dir_all(&dir).expect("remove settings dir");
    std::fs::write(&dir, "").expect("replace settings dir with a file");

    let error = host
        .clear_shortcut(ACTION_TOGGLE_START_MENU, before.revision)
        .expect_err("persistence failure is surfaced");
    assert_ne!(error.code, ErrorCode::StaleRevision);
    assert_eq!(host.shortcut_snapshot(), before);
    let _ = std::fs::remove_file(&dir);
}

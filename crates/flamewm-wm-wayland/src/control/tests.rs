//! Unit tests for the control module.

use super::media::*;
use super::publish::*;
use super::session::*;
use super::settings::*;
use super::*;
use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_api::shortcuts::KeyBinding;
use flamewm_api::system::{BrightnessSnapshot, ServiceAvailability, SystemAction, SystemSnapshot};
use flamewm_api::window::WindowSnapshot;
use flamewm_api::{DesktopAppId, ErrorCode, FlameError, OutputId, WindowRef};
use flamewm_integrations_linux::NetworkManagerSlot;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

/// Without `NetworkManager` on the system bus every network action fails `Unavailable`, and the
/// published snapshot stays unavailable: the shell never receives a fabricated network.
#[test]
fn network_actions_fail_unavailable_without_networkmanager() {
    let network = RefCell::new(
        NetworkManagerSlot::connect_with(|| Err(FlameError::unavailable("system bus absent")))
            .expect("an absent bus is an unavailable slot, not a constructor error"),
    );
    let snapshot = SystemSnapshot::default();
    for action in [
        SystemAction::Scan,
        SystemAction::Disconnect,
        SystemAction::SetWifiEnabled(true),
    ] {
        let error = network_action(&network, action, &snapshot).expect_err("no provider to act on");
        assert_eq!(error.code, ErrorCode::Unavailable);
    }
    assert_eq!(
        network.borrow().snapshot().availability,
        ServiceAvailability::Unavailable
    );
}

/// The shell's OSD treats its first observation as a baseline, so the media-key path must only
/// announce a system snapshot that actually differs from the one already published.
#[test]
fn brightness_updates_are_announced_only_when_the_level_moves() {
    let level = |percent: u8| SystemSnapshot {
        brightness: BrightnessSnapshot {
            availability: ServiceAvailability::Available,
            generation: 1,
            percent,
        },
        ..SystemSnapshot::default()
    };
    let mut published = None;
    assert_eq!(
        system_change(&mut published, level(40)),
        Some(level(40)),
        "the first observed level is announced once"
    );
    assert_eq!(
        system_change(&mut published, level(40)),
        None,
        "an unchanged level is not announced again"
    );
    assert_eq!(
        system_change(&mut published, level(45)),
        Some(level(45)),
        "a brightness key step is announced"
    );
}

/// The media-key volume cap follows the same `audioRaiseMaximum` key the mixer reads.
/// A Panels.SetEdge changes only the panels snapshot; it must be announced even when no window or
/// workspace moved, or the shell keeps the old edge until restart.
#[test]
fn panel_edge_change_is_announced_without_window_changes() {
    use flamewm_api::panels::PanelSnapshot;
    use flamewm_api::{PanelEdge, Rect};
    let panels = |edge| PanelsSnapshot {
        revision: 1,
        panels: vec![PanelSnapshot {
            output: OutputId::new("DP-1"),
            edge,
            logical_size: 44,
            visible: true,
            geometry: Rect::new(0, 0, 1350, 44),
        }],
        tasks: Vec::new(),
        pinned_apps: Vec::new(),
    };
    let mut published = None;
    assert!(
        panels_change(&mut published, panels(PanelEdge::Bottom)).is_some(),
        "the first observed panels snapshot is announced once"
    );
    assert!(
        panels_change(&mut published, panels(PanelEdge::Bottom)).is_none(),
        "an unchanged panels snapshot is not announced again"
    );
    let moved = panels_change(&mut published, panels(PanelEdge::Left))
        .expect("an edge change is announced");
    assert_eq!(moved.panels[0].edge, PanelEdge::Left);
}

#[test]
fn raise_maximum_sets_the_media_volume_cap() {
    let mut snapshot = SettingsSnapshot::new(1);
    assert_eq!(volume_cap(&snapshot), 100, "missing key keeps the base cap");
    snapshot.values.insert(
        flamewm_platform::system::AUDIO_RAISE_MAXIMUM_KEY.to_owned(),
        SettingValue::Boolean(true),
    );
    assert_eq!(volume_cap(&snapshot), 150);
    snapshot.values.insert(
        flamewm_platform::system::AUDIO_RAISE_MAXIMUM_KEY.to_owned(),
        SettingValue::Boolean(false),
    );
    assert_eq!(volume_cap(&snapshot), 100);
}

/// A saved scale is reported for the output it names.
#[test]
fn saved_scales_are_reported_per_output() {
    let scales = shell_scales_from("scale.wayland-0=125\nscale.hdmi-a-1=150\n");
    assert_eq!(scales.get("wayland-0"), Some(&125));
    assert_eq!(scales.get("hdmi-a-1"), Some(&150));
}

/// An out-of-range scale invalidates the file, so the compositor falls back to the default
/// scale instead of reporting a value the rest of the product would reject.
#[test]
fn an_invalid_scale_is_not_reported() {
    assert!(shell_scales_from("scale.wayland-0=111\n").is_empty());
    assert!(shell_scales_from("scale.wayland-0=50\n").is_empty());
    assert!(shell_scales_from("scale.wayland-0=500\n").is_empty());
}

/// Unreadable settings must degrade to the default scale rather than failing the snapshot.
#[test]
fn unparsable_settings_yield_no_saved_scales() {
    assert!(shell_scales_from("").is_empty());
    assert!(shell_scales_from("not = [valid").is_empty());
}
/// Logout is offered only when the wrapper owns this process and is still its parent.
#[test]
fn logout_requires_the_wrapper_as_exact_parent() {
    use std::ffi::OsStr;
    let parent = rustix::process::Pid::from_raw(1234);
    assert!(managed_logout(
        Some(OsStr::new("1")),
        Some(OsStr::new("1234")),
        parent
    ));
    assert!(
        !managed_logout(Some(OsStr::new("1")), Some(OsStr::new("1235")), parent),
        "a different supervisor pid is not our parent"
    );
    assert!(
        !managed_logout(None, Some(OsStr::new("1234")), parent),
        "the wrapper must claim ownership"
    );
    assert!(
        !managed_logout(Some(OsStr::new("0")), Some(OsStr::new("1234")), parent),
        "any owner value other than 1 is not ownership"
    );
    assert!(
        !managed_logout(Some(OsStr::new("1")), Some(OsStr::new("bad")), parent),
        "an unparsable pid never matches"
    );
    assert!(
        !managed_logout(Some(OsStr::new("1")), Some(OsStr::new("1234")), None),
        "no parent means no owned session"
    );
}
/// The scale reader must reach a real file too, and a missing one must not fail the snapshot.
#[test]
fn saved_scales_come_from_the_settings_file_on_disk() {
    let dir = std::env::temp_dir().join(format!("flamewm-wayland-scale-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.toml");
    std::fs::write(&path, "scale.wayland-0=150\n").unwrap();
    assert_eq!(shell_scales_at(&path).get("wayland-0"), Some(&150));
    assert!(shell_scales_at(&dir.join("absent.toml")).is_empty());
    std::fs::remove_dir_all(&dir).ok();
}
/// A shell geometry request must reject the same rects X11 rejects, before it reaches the
/// model, so a bad request cannot leave a window collapsed to zero size.
#[test]
fn geometry_requests_reject_invalid_rects() {
    for width in [0, -1] {
        let rect = flamewm_api::Rect::new(0, 0, width, 100);
        assert!(!rect.is_valid(), "width {width} must be invalid");
    }
    for height in [0, -5] {
        let rect = flamewm_api::Rect::new(0, 0, 100, height);
        assert!(!rect.is_valid(), "height {height} must be invalid");
    }
    // A negative origin is legal: windows may sit off the top-left corner, as in X11.
    assert!(flamewm_api::Rect::new(-4, 9, 640, 480).is_valid());
}
/// The staged/commit split must be safe on the real port: an uncommitted prepare changes
/// nothing, a rollback drops the staged set, and committing with nothing prepared is refused
/// instead of silently doing nothing.
#[test]
fn shortcut_staging_requires_a_commit() {
    use std::rc::Rc;

    use flamewm_api::ports::ShortcutPort;

    fn port() -> WaylandDesktop {
        WaylandDesktop {
            mirror: Rc::new(RefCell::new(Mirror::default())),
            commands: Rc::new(RefCell::new(Vec::new())),
            staged_shortcuts: RefCell::new(None),
            shell_scales: BTreeMap::new(),
        }
    }

    let mut desktop = port();
    let mut desired = BTreeMap::new();
    desired.insert("window.close".to_owned(), KeyBinding::assigned("super+q"));

    assert!(
        desktop.commit_shortcuts().is_err(),
        "committing with nothing prepared must be refused"
    );

    desktop.prepare_shortcuts(&desired).unwrap();
    assert!(
        desktop.commands.borrow().is_empty(),
        "prepare alone must not touch the live table"
    );

    desktop.rollback_shortcuts();
    assert!(
        desktop.commit_shortcuts().is_err(),
        "a rolled-back set must not be committable"
    );

    desktop.prepare_shortcuts(&desired).unwrap();
    desktop.commit_shortcuts().unwrap();
    let commands = desktop.commands.borrow();
    assert_eq!(
        commands.as_slice(),
        [Command::SetShortcuts(desired)],
        "commit must queue exactly the prepared bindings"
    );
    assert!(
        desktop.staged_shortcuts.borrow().is_none(),
        "commit must drain the staged set"
    );
}
/// A hotkey saved in the settings file must reach the compositor at startup. This drives the
/// same `effective_registry` + `commit_startup` path the compositor runs, against a real file,
/// so a rebind survives a restart instead of falling back to compiled-in defaults.
#[test]
fn saved_hotkeys_reach_the_compositor_at_startup() {
    use flamewm_platform::services::shortcuts::{ShortcutService, effective_registry};
    use flamewm_platform::settings::ConfigStore;

    let dir = std::env::temp_dir().join(format!("flamewm-wayland-hotkeys-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.toml");
    std::fs::write(&path, "hotkey.WorkspaceLeft=Ctrl+Super+h\n").unwrap();

    let settings = ConfigStore::default()
        .parse_snapshot(&std::fs::read_to_string(&path).unwrap())
        .unwrap();
    let registry = effective_registry(&settings.hotkeys, settings.assistant_enabled).unwrap();
    assert_eq!(
        registry.bindings().get("WorkspaceLeft").cloned(),
        Some(KeyBinding::assigned("Ctrl+Super+h")),
        "the saved rebound chord must win over the default"
    );

    let mut desktop = WaylandDesktop {
        mirror: Rc::new(RefCell::new(Mirror::default())),
        commands: Rc::new(RefCell::new(Vec::new())),
        staged_shortcuts: RefCell::new(None),
        shell_scales: BTreeMap::new(),
    };
    ShortcutService::default()
        .commit_startup(&mut desktop, registry.clone())
        .unwrap();

    let commands = desktop.commands.borrow();
    match commands.as_slice() {
        [Command::SetShortcuts(bindings)] => {
            assert_eq!(
                bindings.get("WorkspaceLeft"),
                Some(&KeyBinding::assigned("Ctrl+Super+h")),
                "the compositor must receive the rebind, not the default"
            );
            assert_eq!(
                bindings.len(),
                registry.bindings().len(),
                "the whole effective set is delivered, not just the rebound action"
            );
        }
        other => panic!("expected one SetShortcuts command, got {other:?}"),
    }
    drop(commands);
    std::fs::remove_dir_all(&dir).ok();
}
/// `titlebars` and `windowMargin` are read out of the committed snapshot the way X11 reads its
/// live copies: a missing value keeps the product default, and an out-of-range margin falls back
/// instead of being applied.
#[test]
fn edge_snap_settings_default_and_clamp_like_x11() {
    use flamewm_api::settings::{SettingValue, SettingsSnapshot};

    let mut snapshot = SettingsSnapshot {
        revision: 1,
        values: BTreeMap::new(),
    };
    assert_eq!(edge_snap_settings(&snapshot), (true, 16));
    snapshot
        .values
        .insert("edgeSnap.enabled".into(), SettingValue::Boolean(false));
    snapshot
        .values
        .insert("edgeSnap.thresholdPx".into(), SettingValue::Integer(40));
    assert_eq!(edge_snap_settings(&snapshot), (false, 40));
    snapshot
        .values
        .insert("edgeSnap.thresholdPx".into(), SettingValue::Integer(500));
    assert_eq!(
        edge_snap_settings(&snapshot),
        (false, 16),
        "out of range keeps the default"
    );
    snapshot.values.insert(
        "edgeSnap.thresholdPx".into(),
        SettingValue::Text("wide".into()),
    );
    assert_eq!(edge_snap_settings(&snapshot), (false, 16));
}

#[test]
fn chrome_settings_follow_the_committed_snapshot() {
    use flamewm_api::settings::{SettingValue, SettingsSnapshot};
    use flamewm_platform::settings::ProductSettings;

    let defaults = ProductSettings::default();
    let mut snapshot = SettingsSnapshot {
        revision: 1,
        values: BTreeMap::new(),
    };
    assert_eq!(
        chrome_settings(&snapshot),
        (defaults.titlebars, i32::from(defaults.window_margin)),
        "an empty snapshot yields the product defaults"
    );

    snapshot
        .values
        .insert("titlebars".to_owned(), SettingValue::Boolean(false));
    snapshot
        .values
        .insert("windowMargin".to_owned(), SettingValue::Integer(8));
    assert_eq!(chrome_settings(&snapshot), (false, 8));

    // `validate` rejects a margin above 32, so a value that slipped through falls back.
    snapshot
        .values
        .insert("windowMargin".to_owned(), SettingValue::Integer(33));
    assert_eq!(
        chrome_settings(&snapshot).1,
        i32::from(defaults.window_margin),
        "an out-of-range margin falls back to the default"
    );
    snapshot
        .values
        .insert("windowMargin".to_owned(), SettingValue::Integer(-1));
    assert_eq!(
        chrome_settings(&snapshot).1,
        i32::from(defaults.window_margin)
    );
    // 0 is a real value (no margin), not a fallback to the default.
    snapshot
        .values
        .insert("windowMargin".to_owned(), SettingValue::Integer(0));
    assert_eq!(chrome_settings(&snapshot).1, 0);

    // An ill-typed value must not be trusted either.
    snapshot
        .values
        .insert("titlebars".to_owned(), SettingValue::Text("yes".to_owned()));
    assert_eq!(chrome_settings(&snapshot).0, defaults.titlebars);
}
/// Carrying a window to another workspace must move it *and* reveal that workspace, with no
/// intermediate state: the point of the combined call is that the two halves cannot disagree.
#[test]
fn carrying_a_window_switches_to_the_target_workspace() {
    fn port() -> WaylandDesktop {
        WaylandDesktop {
            mirror: Rc::new(RefCell::new(Mirror::default())),
            commands: Rc::new(RefCell::new(Vec::new())),
            staged_shortcuts: RefCell::new(None),
            shell_scales: BTreeMap::new(),
        }
    }

    use flamewm_api::ports::WorkspacePort;

    let mut desktop = port();
    // The window must be managed, so put a snapshot in the mirror the port reads.
    desktop.mirror.borrow_mut().windows.push(WindowSnapshot {
        reference: WindowRef {
            id: 1,
            generation: 1,
        },
        title: String::new(),
        app_id: DesktopAppId::new("test"),
        outer_geometry: flamewm_api::Rect::default(),
        restore_geometry: flamewm_api::Rect::default(),
        state: flamewm_api::window::WindowState::Normal,
        sticky: false,
        focused: false,
        workspace: flamewm_api::WorkspaceRef::default(),
        output: OutputId::new("wayland-0"),
        state_generation: 1,
        unresponsive: false,
        skip_taskbar: false,
        attention: false,
    });
    desktop
        .switch_workspace_with_window(
            WindowRef {
                id: 1,
                generation: 1,
            },
            2,
            0,
        )
        .unwrap();
    assert_eq!(
        desktop.commands.borrow().as_slice(),
        [Command::SwitchWithWindow { id: 1, target: 2 }],
        "one command carries both halves"
    );
}

// REGRESSION: Wayland occupancy counts the mirror's windows per workspace, excluding sticky windows,
// so the pager and GetWorkspaces agree with the compositor window model.
#[test]
fn mirror_occupancy_counts_windows_per_workspace() {
    use flamewm_api::WorkspaceRef;
    use flamewm_api::window::WindowState;

    fn window_on(index: i32, id: u64) -> WindowSnapshot {
        WindowSnapshot {
            reference: WindowRef::new(id, 1),
            title: String::new(),
            app_id: DesktopAppId::default(),
            outer_geometry: flamewm_api::Rect::new(0, 0, 10, 10),
            restore_geometry: flamewm_api::Rect::new(0, 0, 10, 10),
            state: WindowState::Normal,
            sticky: index < 0,
            focused: false,
            workspace: WorkspaceRef::new(index, 1),
            output: OutputId::new("wayland-0"),
            state_generation: 1,
            unresponsive: false,
            skip_taskbar: false,
            attention: false,
        }
    }

    let mirror = Mirror {
        windows: vec![window_on(1, 1), window_on(1, 2), window_on(-1, 3)],
        workspace_count: 3,
        ..Mirror::default()
    };
    assert_eq!(mirror.workspace_occupancy(), vec![0, 2, 0]);
}

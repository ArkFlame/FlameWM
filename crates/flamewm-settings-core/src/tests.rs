use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use flamewm_api::display::{DisplayMode, OutputSnapshot};
use flamewm_api::panels::{PanelSnapshot, PanelsSnapshot};
use flamewm_api::{Rect, Size};

use super::*;

#[derive(Default)]
struct FakeTransport {
    responses: VecDeque<Result<ControlResponse, ControlError>>,
    requests: Vec<ControlRequest>,
}

impl ControlTransport for FakeTransport {
    fn call(&mut self, request: ControlRequest) -> Result<ControlResponse, ControlError> {
        self.requests.push(request);
        self.responses.pop_front().expect("queued response")
    }
}

fn fake_client(
    responses: impl IntoIterator<Item = Result<ControlResponse, ControlError>>,
) -> SettingsClient<FakeTransport> {
    SettingsClient::new(FakeTransport {
        responses: responses.into_iter().collect(),
        ..Default::default()
    })
}

struct RecordingTransport {
    responses: VecDeque<Result<ControlResponse, ControlError>>,
    requests: Rc<RefCell<Vec<ControlRequest>>>,
}

impl ControlTransport for RecordingTransport {
    fn call(&mut self, request: ControlRequest) -> Result<ControlResponse, ControlError> {
        self.requests.borrow_mut().push(request);
        self.responses.pop_front().expect("queued response")
    }
}

fn display_snapshot() -> DisplaySnapshot {
    let mode = ModeId(7);
    DisplaySnapshot {
        generation: 3,
        outputs: vec![OutputSnapshot {
            id: OutputId::new("HDMI-1"),
            connector: "HDMI-1".to_owned(),
            edid_identity: "edid".to_owned(),
            connected: true,
            primary: true,
            geometry: Rect::new(0, 0, 1920, 1080),
            current_mode: mode,
            modes: vec![DisplayMode {
                id: mode,
                resolution: Size::new(1920, 1080),
                refresh_millihz: 60_000,
                preferred: true,
            }],
            shell_scale_percent: 100,
        }],
        pending: None,
    }
}

fn panels_snapshot(revision: u64, edge: PanelEdge, logical_size: u16) -> PanelsSnapshot {
    PanelsSnapshot {
        revision,
        panels: vec![PanelSnapshot {
            output: OutputId::new("HDMI-1"),
            edge,
            logical_size,
            visible: true,
            geometry: Rect::new(0, 1036, 1920, i32::from(logical_size)),
        }],
        tasks: Vec::new(),
        pinned_apps: Vec::new(),
    }
}

#[test]
fn display_refresh_selects_primary_output() {
    let mut client = fake_client([Ok(ControlResponse::Displays(display_snapshot()))]);
    client.refresh_displays().expect("refresh");
    assert_eq!(
        client.ui().selected_output.as_ref().map(|id| id.0.as_str()),
        Some("HDMI-1")
    );
}

#[test]
fn set_shell_scale_accepts_unit_after_authoritative_success() {
    let mut updated = display_snapshot();
    updated.generation = 4;
    updated.outputs[0].shell_scale_percent = 125;

    let mut client = fake_client([
        Ok(ControlResponse::Displays(display_snapshot())),
        Ok(ControlResponse::Unit),
        Ok(ControlResponse::Displays(updated)),
    ]);
    client.refresh_displays().expect("initial display snapshot");
    assert!(
        client.set_shell_scale(125).is_ok(),
        "void SetShellScale success must not become InternalFailure"
    );

    assert_eq!(
        client.displays().map(|snapshot| snapshot.generation),
        Some(4)
    );
    assert_eq!(
        client
            .displays()
            .and_then(|snapshot| snapshot.outputs.first())
            .map(|output| output.shell_scale_percent),
        Some(125)
    );
    assert!(matches!(
        client.transport.requests.as_slice(),
        [
            ControlRequest::GetDisplays,
            ControlRequest::SetShellScale {
                output,
                percent: 125,
                expected_revision: 3,
            },
            ControlRequest::GetDisplays,
        ] if output == &OutputId::new("HDMI-1")
    ));
}

#[test]
fn escape_capture_commits_clear_binding() {
    let mut client = fake_client([
        Ok(ControlResponse::Shortcuts(ShortcutSnapshot {
            revision: 1,
            bindings: [(
                "ToggleStartMenu".to_owned(),
                flamewm_api::shortcuts::KeyBinding(Some("Super".to_owned())),
            )]
            .into_iter()
            .collect(),
        })),
        Ok(ControlResponse::CaptureLease(9)),
        Ok(ControlResponse::Unit),
        Ok(ControlResponse::Unit),
        Ok(ControlResponse::Shortcuts(ShortcutSnapshot {
            revision: 2,
            bindings: Default::default(),
        })),
        Ok(ControlResponse::Settings(SettingsSnapshot::new(3))),
    ]);
    client
        .refresh_shortcuts()
        .expect("initial shortcut snapshot");
    client
        .begin_hotkey_capture("ToggleStartMenu")
        .expect("begin lease");
    assert!(client.capture_hotkey("Escape"));
    client.commit_hotkey_capture().expect("commit");
    assert_eq!(
        client.shortcuts().map(|snapshot| snapshot.revision),
        Some(2)
    );
    assert_eq!(client.ui().hotkey_capture_lease, None);
    assert!(matches!(
        client.transport.requests.as_slice(),
        [
            ControlRequest::GetShortcuts,
            ControlRequest::BeginHotkeyCapture { action, expected_revision: 1 },
            ControlRequest::EndHotkeyCapture { lease: 9 },
            ControlRequest::ClearShortcut { action: clear_action, expected_revision: 1 },
            ControlRequest::GetShortcuts,
            ControlRequest::GetSettings,
        ] if action == "ToggleStartMenu" && clear_action == action
    ));
}

/// Stateful stand-in for the WM: every hotkey write bumps the settings revision as well as the
/// shortcut revision, and `ApplySettings` refuses any expected revision that is not current.
#[derive(Default)]
struct RevisionServer {
    settings_revision: u64,
    shortcut_revision: u64,
    bindings: std::collections::BTreeMap<String, flamewm_api::shortcuts::KeyBinding>,
    applied_expected_revisions: Vec<u64>,
}

impl RevisionServer {
    fn stale(message: &str) -> ControlError {
        ControlError {
            name: "com.arkflame.FlameWM1.Error.StaleRevision",
            code: flamewm_api::ErrorCode::StaleRevision,
            message: message.to_owned(),
        }
    }

    fn check_shortcut_revision(&self, expected_revision: u64) -> Result<(), ControlError> {
        if expected_revision == self.shortcut_revision {
            Ok(())
        } else {
            Err(Self::stale("shortcut revision is stale"))
        }
    }

    fn bump_hotkey_revisions(&mut self) {
        self.shortcut_revision += 1;
        self.settings_revision += 1;
    }
}

impl ControlTransport for RevisionServer {
    fn call(&mut self, request: ControlRequest) -> Result<ControlResponse, ControlError> {
        match request {
            ControlRequest::GetSettings => Ok(ControlResponse::Settings(
                SettingsSnapshot::new(self.settings_revision),
            )),
            ControlRequest::GetShortcuts => Ok(ControlResponse::Shortcuts(ShortcutSnapshot {
                revision: self.shortcut_revision,
                bindings: self.bindings.clone(),
            })),
            ControlRequest::BeginHotkeyCapture { .. } => Ok(ControlResponse::CaptureLease(1)),
            ControlRequest::EndHotkeyCapture { .. } => Ok(ControlResponse::Unit),
            ControlRequest::SetShortcut {
                action,
                binding,
                expected_revision,
            } => {
                self.check_shortcut_revision(expected_revision)?;
                self.bindings
                    .insert(action, flamewm_api::shortcuts::KeyBinding(Some(binding)));
                self.bump_hotkey_revisions();
                Ok(ControlResponse::Unit)
            }
            ControlRequest::ClearShortcut {
                action,
                expected_revision,
            } => {
                self.check_shortcut_revision(expected_revision)?;
                self.bindings
                    .insert(action, flamewm_api::shortcuts::KeyBinding(None));
                self.bump_hotkey_revisions();
                Ok(ControlResponse::Unit)
            }
            ControlRequest::ResetShortcut {
                action,
                expected_revision,
            } => {
                self.check_shortcut_revision(expected_revision)?;
                self.bindings.remove(&action);
                self.bump_hotkey_revisions();
                Ok(ControlResponse::Unit)
            }
            ControlRequest::ApplySettings(transaction) => {
                if transaction.expected_revision != self.settings_revision {
                    return Err(Self::stale("settings revision is stale"));
                }
                self.applied_expected_revisions
                    .push(transaction.expected_revision);
                self.settings_revision += 1;
                Ok(ControlResponse::Settings(SettingsSnapshot::new(
                    self.settings_revision,
                )))
            }
            _ => panic!("unexpected request in hotkey revision test"),
        }
    }
}

#[test]
fn hotkey_mutations_refresh_settings_revision_for_next_apply() {
    let mut client = SettingsClient::new(RevisionServer {
        settings_revision: 4,
        shortcut_revision: 1,
        ..RevisionServer::default()
    });
    client
        .refresh_settings()
        .expect("initial settings snapshot");
    client
        .refresh_shortcuts()
        .expect("initial shortcut snapshot");

    client
        .begin_hotkey_capture("ToggleStartMenu")
        .expect("begin set lease");
    assert!(client.capture_hotkey("Super"));
    client.commit_hotkey_capture().expect("set binding");
    client
        .apply_setting("wallpaper", SettingValue::Text("a.png".to_owned()))
        .expect("apply after set");

    client
        .begin_hotkey_capture("ToggleStartMenu")
        .expect("begin clear lease");
    assert!(client.capture_hotkey("Escape"));
    client.commit_hotkey_capture().expect("clear binding");
    client
        .apply_setting("wallpaper", SettingValue::Text("b.png".to_owned()))
        .expect("apply after clear");

    client
        .reset_hotkey("ToggleStartMenu")
        .expect("reset binding");
    client
        .apply_setting("wallpaper", SettingValue::Text("c.png".to_owned()))
        .expect("apply after reset");

    assert_eq!(client.transport.applied_expected_revisions, vec![5, 7, 9]);
}

#[test]
fn hotkey_cancel_restores_grabs_and_end_failure_blocks_mutation() {
    let mut client = fake_client([
        Ok(ControlResponse::Shortcuts(ShortcutSnapshot {
            revision: 3,
            bindings: [(
                "ToggleWorkspaceLeft".to_owned(),
                flamewm_api::shortcuts::KeyBinding(Some("Ctrl+Left".to_owned())),
            )]
            .into_iter()
            .collect(),
        })),
        Ok(ControlResponse::CaptureLease(44)),
        Err(ControlError {
            name: "com.arkflame.FlameWM1.Error.Unavailable",
            code: flamewm_api::ErrorCode::Unavailable,
            message: "bus lost".to_owned(),
        }),
        Ok(ControlResponse::Unit),
    ]);
    client
        .refresh_shortcuts()
        .expect("initial shortcut snapshot");
    client
        .begin_hotkey_capture("ToggleWorkspaceLeft")
        .expect("begin lease");
    assert!(client.capture_hotkey("Super+Left"));
    assert!(client.commit_hotkey_capture().is_err());
    assert_eq!(client.ui().hotkey_capture_lease, Some(44));
    assert_eq!(
        client.transport.requests.len(),
        3,
        "snapshot, lease acquisition, and failed End; no shortcut mutation"
    );
    assert!(matches!(
        client.transport.requests.as_slice(),
        [
            ControlRequest::GetShortcuts,
            ControlRequest::BeginHotkeyCapture { action, expected_revision: 3 },
            ControlRequest::EndHotkeyCapture { lease: 44 },
        ] if action == "ToggleWorkspaceLeft"
    ));
    assert_eq!(
        client
            .shortcuts()
            .and_then(|snapshot| snapshot.bindings.get("ToggleWorkspaceLeft")),
        Some(&flamewm_api::shortcuts::KeyBinding(Some(
            "Ctrl+Left".to_owned()
        ))),
        "failed lease end preserves the authoritative binding"
    );
    client.cancel_hotkey_capture().expect("retry lease end");
    assert_eq!(client.ui().hotkey_capture_lease, None);
    assert_eq!(client.ui().hotkey_capture, None);
    assert!(matches!(
        client.transport.requests.as_slice(),
        [
            ControlRequest::GetShortcuts,
            ControlRequest::BeginHotkeyCapture {
                expected_revision: 3,
                ..
            },
            ControlRequest::EndHotkeyCapture { lease: 44 },
            ControlRequest::EndHotkeyCapture { lease: 44 },
        ]
    ));
}

#[test]
fn unchanged_capture_ends_lease_without_shortcut_mutation() {
    let mut client = fake_client([
        Ok(ControlResponse::Shortcuts(ShortcutSnapshot {
            revision: 8,
            bindings: [(
                "ToggleStartMenu".to_owned(),
                flamewm_api::shortcuts::KeyBinding(Some("Super".to_owned())),
            )]
            .into_iter()
            .collect(),
        })),
        Ok(ControlResponse::CaptureLease(23)),
        Ok(ControlResponse::Unit),
    ]);
    client.refresh_shortcuts().expect("initial snapshot");
    client
        .begin_hotkey_capture("ToggleStartMenu")
        .expect("begin lease");
    assert!(client.capture_hotkey("Super"));
    client.commit_hotkey_capture().expect("unchanged capture");
    assert!(matches!(
        client.transport.requests.as_slice(),
        [
            ControlRequest::GetShortcuts,
            ControlRequest::BeginHotkeyCapture {
                expected_revision: 8,
                ..
            },
            ControlRequest::EndHotkeyCapture { lease: 23 },
        ]
    ));
}

#[test]
fn settings_helpers_map_reset_and_font_values_and_refresh_cache() {
    let requests = Rc::new(RefCell::new(Vec::new()));
    let initial = SettingsSnapshot::new(4);
    let updated = SettingsSnapshot::new(5);
    let transport = RecordingTransport {
        responses: VecDeque::from([
            Ok(ControlResponse::Settings(initial)),
            Ok(ControlResponse::Settings(updated.clone())),
            Ok(ControlResponse::Settings(SettingsSnapshot::new(6))),
        ]),
        requests: requests.clone(),
    };
    let mut client = SettingsClient::new(transport);
    client
        .refresh_settings()
        .expect("initial settings snapshot");
    client
        .set_fonts("Inter", true, 2)
        .expect("font settings apply");
    client.reset_section("fonts").expect("section reset");

    let requests = requests.borrow();
    assert!(matches!(
        requests.get(1),
        Some(ControlRequest::ApplySettings(transaction))
            if transaction.expected_revision == 4
                && transaction.reset_section.is_none()
                && transaction.changes == vec![
                    SettingsChange {
                        key: "fontFamily".to_owned(),
                        value: Some(SettingValue::Text("Inter".to_owned())),
                    },
                    SettingsChange {
                        key: "fontBold".to_owned(),
                        value: Some(SettingValue::Boolean(true)),
                    },
                    SettingsChange {
                        key: "fontSizeOffset".to_owned(),
                        value: Some(SettingValue::Integer(2)),
                    },
                ]
    ));
    assert!(matches!(
        requests.get(2),
        Some(ControlRequest::ApplySettings(transaction))
            if transaction.expected_revision == 5
                && transaction.changes.is_empty()
                && transaction.reset_section.as_deref() == Some("fonts")
    ));
    drop(requests);
    assert_eq!(client.settings().map(|snapshot| snapshot.revision), Some(6));
}

#[test]
fn glass_section_reset_and_preset_send_one_transaction_each() {
    let requests = Rc::new(RefCell::new(Vec::new()));
    let transport = RecordingTransport {
        responses: VecDeque::from([
            Ok(ControlResponse::Settings(SettingsSnapshot::new(3))),
            Ok(ControlResponse::Settings(SettingsSnapshot::new(4))),
            Ok(ControlResponse::Settings(SettingsSnapshot::new(5))),
        ]),
        requests: requests.clone(),
    };
    let mut client = SettingsClient::new(transport);
    client
        .refresh_settings()
        .expect("initial settings snapshot");
    client
        .apply_settings(vec![
            SettingsChange {
                key: "glassOpacityPercent".to_owned(),
                value: Some(SettingValue::Percent(23)),
            },
            SettingsChange {
                key: "glassBlurPx".to_owned(),
                value: Some(SettingValue::Percent(13)),
            },
        ])
        .expect("glass preset");
    client.reset_section("glass").expect("glass reset");

    let requests = requests.borrow();
    assert!(matches!(
        requests.get(1),
        Some(ControlRequest::ApplySettings(transaction))
            if transaction.expected_revision == 3
                && transaction.reset_section.is_none()
                && transaction.changes.len() == 2
    ));
    assert!(matches!(
        requests.get(2),
        Some(ControlRequest::ApplySettings(transaction))
            if transaction.expected_revision == 4
                && transaction.changes.is_empty()
                && transaction.reset_section.as_deref() == Some("glass")
    ));
}

#[test]
fn panel_helpers_map_requests_and_refresh_cached_snapshot() {
    let requests = Rc::new(RefCell::new(Vec::new()));
    let transport = RecordingTransport {
        responses: VecDeque::from([
            Ok(ControlResponse::Panels(panels_snapshot(
                4,
                PanelEdge::Bottom,
                44,
            ))),
            Ok(ControlResponse::Unit),
            Ok(ControlResponse::Panels(panels_snapshot(
                5,
                PanelEdge::Top,
                44,
            ))),
            Ok(ControlResponse::Unit),
            Ok(ControlResponse::Panels(panels_snapshot(
                6,
                PanelEdge::Top,
                52,
            ))),
        ]),
        requests: requests.clone(),
    };
    let mut client = SettingsClient::new(transport);
    client.refresh_panels().expect("initial panels snapshot");
    client
        .set_panel_edge(OutputId::new("HDMI-1"), PanelEdge::Top)
        .expect("panel edge apply");
    client
        .set_panel_size(OutputId::new("HDMI-1"), 52)
        .expect("panel size apply");

    let requests = requests.borrow();
    assert!(matches!(
        requests.get(1),
        Some(ControlRequest::SetPanelEdge {
            output,
            edge: PanelEdge::Top,
            expected_revision: 4,
        }) if output == &OutputId::new("HDMI-1")
    ));
    assert!(matches!(
        requests.get(3),
        Some(ControlRequest::SetPanelSize {
            output,
            logical_size: 52,
            expected_revision: 5,
        }) if output == &OutputId::new("HDMI-1")
    ));
    drop(requests);
    assert_eq!(client.panels().map(|snapshot| snapshot.revision), Some(6));
    assert_eq!(
        client
            .panels()
            .and_then(|snapshot| snapshot.panels.first())
            .map(|panel| panel.logical_size),
        Some(52)
    );
}

#[test]
fn panel_edge_change_recovers_once_after_stale_revision() {
    let requests = Rc::new(RefCell::new(Vec::new()));
    let transport = RecordingTransport {
        responses: VecDeque::from([
            Ok(ControlResponse::Panels(panels_snapshot(
                3,
                PanelEdge::Bottom,
                44,
            ))),
            Err(ControlError {
                name: "com.arkflame.FlameWM1.Error.StaleRevision",
                code: flamewm_api::ErrorCode::StaleRevision,
                message: "panel revision is stale".to_owned(),
            }),
            Ok(ControlResponse::Panels(panels_snapshot(
                4,
                PanelEdge::Bottom,
                44,
            ))),
            Ok(ControlResponse::Unit),
            Ok(ControlResponse::Panels(panels_snapshot(
                5,
                PanelEdge::Top,
                44,
            ))),
        ]),
        requests: requests.clone(),
    };
    let mut client = SettingsClient::new(transport);

    client.refresh_panels().expect("initial panels snapshot");
    client
        .set_panel_edge(OutputId::new("HDMI-1"), PanelEdge::Top)
        .expect("stale panel edge apply recovers");

    let requests = requests.borrow();
    assert!(matches!(
        requests.as_slice(),
        [
            ControlRequest::GetPanels,
            ControlRequest::SetPanelEdge {
                output,
                edge: PanelEdge::Top,
                expected_revision: 3,
            },
            ControlRequest::GetPanels,
            ControlRequest::SetPanelEdge {
                output: retry_output,
                edge: PanelEdge::Top,
                expected_revision: 4,
            },
            ControlRequest::GetPanels,
        ] if output == &OutputId::new("HDMI-1")
            && retry_output == &OutputId::new("HDMI-1")
    ));
    drop(requests);
    assert_eq!(client.panels().map(|snapshot| snapshot.revision), Some(5));
    assert_eq!(
        client
            .panels()
            .and_then(|snapshot| snapshot.panels.first())
            .map(|panel| panel.edge),
        Some(PanelEdge::Top)
    );
}

#[test]
fn panel_edge_change_returns_second_stale_without_looping() {
    let requests = Rc::new(RefCell::new(Vec::new()));
    let transport = RecordingTransport {
        responses: VecDeque::from([
            Ok(ControlResponse::Panels(panels_snapshot(
                3,
                PanelEdge::Bottom,
                44,
            ))),
            Err(ControlError {
                name: "com.arkflame.FlameWM1.Error.StaleRevision",
                code: flamewm_api::ErrorCode::StaleRevision,
                message: "panel revision is stale".to_owned(),
            }),
            Ok(ControlResponse::Panels(panels_snapshot(
                4,
                PanelEdge::Bottom,
                44,
            ))),
            Err(ControlError {
                name: "com.arkflame.FlameWM1.Error.StaleRevision",
                code: flamewm_api::ErrorCode::StaleRevision,
                message: "panel revision is stale again".to_owned(),
            }),
        ]),
        requests: requests.clone(),
    };
    let mut client = SettingsClient::new(transport);

    client.refresh_panels().expect("initial panels snapshot");
    let error = client
        .set_panel_edge(OutputId::new("HDMI-1"), PanelEdge::Top)
        .expect_err("second stale revision propagates");

    assert_eq!(error.code, flamewm_api::ErrorCode::StaleRevision);
    assert_eq!(error.message, "panel revision is stale again");
    let requests = requests.borrow();
    assert!(matches!(
        requests.as_slice(),
        [
            ControlRequest::GetPanels,
            ControlRequest::SetPanelEdge {
                expected_revision: 3,
                ..
            },
            ControlRequest::GetPanels,
            ControlRequest::SetPanelEdge {
                expected_revision: 4,
                ..
            },
        ]
    ));
    drop(requests);
    assert_eq!(client.panels().map(|snapshot| snapshot.revision), Some(4));
    assert_eq!(
        client
            .panels()
            .and_then(|snapshot| snapshot.panels.first())
            .map(|panel| panel.edge),
        Some(PanelEdge::Bottom)
    );
}

#[test]
fn settings_reset_error_preserves_cached_authoritative_snapshot() {
    let initial = SettingsSnapshot::new(4);
    let mut client = fake_client([
        Ok(ControlResponse::Settings(initial.clone())),
        Err(ControlError {
            name: "com.arkflame.FlameWM1.Error.StaleRevision",
            code: flamewm_api::ErrorCode::StaleRevision,
            message: "settings revision is stale".to_owned(),
        }),
    ]);
    client
        .refresh_settings()
        .expect("initial settings snapshot");
    assert_eq!(
        client.reset_section("desktop").unwrap_err().code,
        flamewm_api::ErrorCode::StaleRevision
    );
    assert_eq!(client.settings(), Some(&initial));
}

#[test]
fn native_settings_navigation_order_matches_current_root() {
    let names = page_specs()
        .iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec![
            "Appearance",
            "Desktop",
            "Taskbar",
            "Displays",
            "Fonts",
            "Hotkeys",
            "About"
        ]
    );
}

/// Fake two-output topology: DP-B (primary, selected by default) has 12 modes, DP-A has 2.
fn two_output_display_snapshot() -> DisplaySnapshot {
    let mode = |id: u64| DisplayMode {
        id: ModeId(id),
        resolution: Size::new(640 + id as i32, 360),
        refresh_millihz: 60_000,
        preferred: false,
    };
    let output =
        |id: &str, primary: bool, current: u64, modes: Vec<DisplayMode>| OutputSnapshot {
            id: OutputId::new(id),
            connector: id.to_owned(),
            edid_identity: format!("edid-{id}"),
            connected: true,
            primary,
            geometry: Rect::new(0, 0, 1920, 1080),
            current_mode: ModeId(current),
            modes,
            shell_scale_percent: 100,
        };
    DisplaySnapshot {
        generation: 5,
        outputs: vec![
            output("DP-A", false, 1, vec![mode(1), mode(2)]),
            output("DP-B", true, 101, (101..=112).map(mode).collect()),
        ],
        pending: None,
    }
}

#[test]
fn two_output_fixture_reaches_every_mode_and_begins_exact_output_and_mode() {
    let mut transport = FakeTransport::default();
    transport
        .responses
        .push_back(Ok(ControlResponse::Displays(two_output_display_snapshot())));
    for transaction in 1..=13 {
        transport
            .responses
            .push_back(Ok(ControlResponse::Transaction(TransactionId(transaction))));
    }
    let mut client = SettingsClient::new(transport);
    client.refresh_displays().expect("refresh");
    assert_eq!(
        client
            .ui()
            .selected_output
            .as_ref()
            .map(flamewm_api::OutputId::as_str),
        Some("DP-B")
    );

    // Slots cover the first DISPLAY_MODE_SLOTS modes; the rest are overflow but still reachable.
    let slots: Vec<ModeId> = client.display_mode_slots().iter().map(|m| m.id).collect();
    assert_eq!(slots, (101..=104).map(ModeId).collect::<Vec<_>>());
    assert_eq!(client.display_mode_overflow(), 12 - DISPLAY_MODE_SLOTS);
    assert_eq!(client.display_mode_for_slot(8), None);
    let reachable: Vec<ModeId> = (0..12)
        .filter_map(|index| client.display_mode_for_index(index))
        .collect();
    assert_eq!(reachable, (101..=112).map(ModeId).collect::<Vec<_>>());
    assert_eq!(client.display_mode_for_index(12), None);

    // Selected/current state of the selected output.
    let current = client
        .displays()
        .and_then(|snapshot| snapshot.outputs.iter().find(|o| o.id.as_str() == "DP-B"))
        .map(|output| output.current_mode);
    assert_eq!(current, Some(ModeId(101)));

    // Every B mode, slot and overflow alike, produces a BeginDisplayMode for DP-B and that ModeId.
    for index in 0..12u64 {
        let mode = client.display_mode_for_index(index as usize).expect("mode");
        client
            .begin_display_mode(mode, 1000 + index)
            .expect("begin");
        assert_eq!(
            client.transport.requests[index as usize + 1],
            ControlRequest::BeginDisplayMode {
                output: OutputId::new("DP-B"),
                mode: ModeId(101 + index),
                expected_generation: 5,
                now_ms: 1000 + index,
            }
        );
    }
    assert_eq!(
        client.ui().pending_display_transaction,
        Some(TransactionId(12))
    );

    // Switching to the two-mode output begins on DP-A with its own ModeId.
    assert!(client.select_output(&OutputId::new("DP-A")));
    assert_eq!(client.display_mode_for_index(1), Some(ModeId(2)));
    client
        .begin_display_mode(ModeId(2), 2000)
        .expect("begin on DP-A");
    assert_eq!(
        client.transport.requests.last(),
        Some(&ControlRequest::BeginDisplayMode {
            output: OutputId::new("DP-A"),
            mode: ModeId(2),
            expected_generation: 5,
            now_ms: 2000,
        })
    );
}

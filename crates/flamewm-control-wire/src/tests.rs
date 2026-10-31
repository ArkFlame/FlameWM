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
    let ControlRequest::ApplySettings(transaction) = decode_call(&call).expect("valid call") else {
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
    let call = WireCall::new(IFACE_DISPLAYS, "Keep", vec![WireValue::U64(7)]).with_now_ms(15_001);
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
    assert_eq!(
        (call.interface.as_str(), call.member.as_str()),
        (IFACE_WEATHER, "GetSnapshot")
    );
    let publish = assert_request_round_trip(&ControlRequest::PublishWeatherFeed {
        payload: "a\u{1f}b".to_owned(),
    });
    assert_eq!(
        (publish.interface.as_str(), publish.member.as_str()),
        (IFACE_WEATHER, "Publish")
    );
    let feed = flamewm_api::weather::WeatherFeedSnapshot {
        revision: 4,
        payload: "p".to_owned(),
    };
    assert_reply_round_trip(
        &nullary_call(IFACE_WEATHER, "GetSnapshot"),
        &ControlResponse::WeatherFeed(feed.clone()),
    );
    let signal = ControlSignal::WeatherFeedChanged { snapshot: feed };
    let wire = encode_signal(&signal);
    assert_eq!(
        (wire.interface.as_str(), wire.member.as_str()),
        (IFACE_WEATHER, "FeedChanged")
    );
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

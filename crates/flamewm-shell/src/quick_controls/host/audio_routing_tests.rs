use super::*;
use flamewm_api::system::{
    AudioEndpointKind, AudioEndpointSnapshot, AudioStreamSnapshot, AudioTarget,
    ServiceAvailability, SystemAction,
};

// Fake provider: one default output at 50%, one app stream at 30%.
struct AcceptingShortcuts;

impl flamewm_api::ports::ShortcutPort for AcceptingShortcuts {
    fn prepare_shortcuts(
        &mut self,
        _desired: &std::collections::BTreeMap<String, flamewm_api::shortcuts::KeyBinding>,
    ) -> flamewm_api::FlameResult<()> {
        Ok(())
    }

    fn commit_shortcuts(&mut self) -> flamewm_api::FlameResult<()> {
        Ok(())
    }

    fn rollback_shortcuts(&mut self) {}
}

/// Pick -> transaction fenced on the store revision -> accepted by the real settings service
/// (not a fake) -> the popup feed reads the enabled location. The store has advanced past
/// its initial revision, so a transaction fenced on any other revision would be stale.
#[test]
fn weather_pick_is_accepted_by_real_settings_store_and_enables_feed() {
    use flamewm_api::settings::{SettingValue, SettingsChange, SettingsTransaction};
    use flamewm_platform::services::settings::SettingsService;

    let dir = std::env::temp_dir().join(format!("flamewm-weather-pick-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut service = SettingsService::new(dir.join("settings.conf"));
    let initial = service.snapshot().revision;
    service
        .apply(
            &mut AcceptingShortcuts,
            &SettingsTransaction {
                expected_revision: initial,
                changes: vec![SettingsChange {
                    key: "taskbarShowClock".to_owned(),
                    value: Some(SettingValue::Boolean(true)),
                }],
                reset_section: None,
            },
        )
        .expect("unrelated change");
    let store_revision = service.snapshot().revision;
    assert_eq!(store_revision, initial + 1);

    let mut snapshot = quick(AudioTab::Devices, false);
    snapshot.settings = service.snapshot();
    snapshot.weather.settings.enabled = true;
    snapshot.weather_search.results = vec![flamewm_shell_core::weather::WeatherLocation::new(
        "Berlin",
        52.52,
        13.405,
        Some("Germany"),
    )
    .unwrap()];
    let pick = weather_pick_for(&snapshot, 0).expect("pick");
    let ControlRequest::ApplySettings(transaction) = &pick.request else {
        panic!("pick must be a settings transaction");
    };
    assert_eq!(transaction.expected_revision, store_revision);
    let applied = service
        .apply(&mut AcceptingShortcuts, transaction)
        .expect("real store accepts the pick");
    let _ = std::fs::remove_dir_all(&dir);

    let feed = flamewm_shell_core::weather::WeatherSettings::from_snapshot(&applied);
    assert!(feed.enabled);
    assert_eq!(
        feed.location.as_ref().map(|l| l.name.as_str()),
        Some("Berlin")
    );
    assert_eq!(
        applied.values.get(crate::weather_auto::AUTO_LOCATION_KEY),
        Some(&SettingValue::Boolean(false)),
        "a manual pick turns automatic location off"
    );
}

/// Volume percent a pointer at `x` on the Devices slider requests, if any.
fn audio_slider_percent(raise: bool, x: f32) -> Option<u8> {
    match audio_pointer_request(&quick(AudioTab::Devices, raise), 0, x) {
        Some(ControlRequest::SystemAction {
            action: SystemAction::SetVolume(volume),
            ..
        }) => Some(volume.percent),
        _ => None,
    }
}

fn quick(tab: AudioTab, raise_maximum: bool) -> QuickControlSnapshot {
    let mut system = flamewm_api::system::SystemSnapshot {
        revision: 4,
        ..flamewm_api::system::SystemSnapshot::default()
    };
    system.audio.availability = ServiceAvailability::Available;
    system.audio.endpoints = vec![AudioEndpointSnapshot {
        id: 1,
        kind: AudioEndpointKind::Sink,
        name: "speakers".to_owned(),
        description: "Built-in Speakers".to_owned(),
        volume_percent: 50,
        muted: false,
        is_default: true,
    }];
    system.audio.streams = vec![AudioStreamSnapshot {
        id: 10,
        endpoint_id: 1,
        name: "Firefox".to_owned(),
        volume_percent: 30,
        muted: false,
        ..Default::default()
    }];
    QuickControlSnapshot {
        system,
        audio_tab: tab,
        audio_settings: AudioSettings { raise_maximum },
        ..super::tests::blank_snapshot()
    }
}

#[test]
fn wheel_scroll_moves_audio_rows_by_whole_rows_and_clamps_to_the_model() {
    let mut quick = quick(AudioTab::Devices, false);
    for id in 2..=6 {
        quick.system.audio.endpoints.push(AudioEndpointSnapshot {
            id,
            kind: AudioEndpointKind::Sink,
            name: format!("sink{id}"),
            description: format!("Sink {id}"),
            volume_percent: 10,
            muted: false,
            is_default: false,
        });
    }
    // Six device rows in a four-row viewport: two rows of scroll room.
    scroll_audio_rows(&mut quick, 48.0);
    assert_eq!(quick.audio_scroll_offset, 84);
    scroll_audio_rows(&mut quick, 48.0);
    assert_eq!(quick.audio_scroll_offset, 168);
    scroll_audio_rows(&mut quick, 48.0);
    assert_eq!(quick.audio_scroll_offset, 168);
    scroll_audio_rows(&mut quick, -48.0);
    assert_eq!(quick.audio_scroll_offset, 84);
    // The projected window and the routed slot both start at the scrolled row.
    let rows = crate::taskbar::status::audio::visible_rows(
        &quick.system.audio,
        quick.audio_tab,
        quick.audio_scroll_offset,
    );
    assert_eq!(rows[0].stable_id(), "endpoint:sink:2");
    let request = helper_control_request("audio.slot.1.mute", &quick);
    assert_eq!(
        target_of(request),
        Some(AudioTarget::Endpoint {
            id: 2,
            kind: AudioEndpointKind::Sink
        })
    );
}

// RED before the fix: a released tab action must both select the tab and
// request a reprojection, or the popup keeps showing the old tab.
#[test]
fn audio_tab_release_selects_tab_and_requests_reprojection() {
    let mut snapshot = quick(AudioTab::Devices, false);
    snapshot.audio_scroll_offset = 40;
    assert!(select_audio_tab(&mut snapshot, "audio.tab.apps"));
    assert_eq!(snapshot.audio_tab, AudioTab::Applications);
    assert_eq!(snapshot.audio_scroll_offset, 0);
    // Re-selecting the shown tab with its window at the top changes nothing.
    assert!(!select_audio_tab(&mut snapshot, "audio.tab.apps"));
    assert!(select_audio_tab(&mut snapshot, "audio.tab.devices"));
    assert_eq!(snapshot.audio_tab, AudioTab::Devices);
    assert!(!select_audio_tab(&mut snapshot, "audio.raise.toggle"));
    assert_eq!(snapshot.audio_tab, AudioTab::Devices);
}

fn target_of(request: Option<ControlRequest>) -> Option<AudioTarget> {
    match request? {
        ControlRequest::SystemAction {
            action: SystemAction::SetMute(mute),
            ..
        } => Some(mute.target),
        ControlRequest::SystemAction {
            action: SystemAction::SetVolume(volume),
            ..
        } => Some(volume.target),
        _ => None,
    }
}

// Slot numbers follow the projected tab, not the raw provider order.
#[test]
fn row_mute_routes_to_the_projected_tab_row() {
    let devices = quick(AudioTab::Devices, false);
    assert_eq!(
        target_of(helper_control_request("audio.slot.1.mute", &devices)),
        Some(AudioTarget::Endpoint {
            id: 1,
            kind: AudioEndpointKind::Sink
        })
    );
    let apps = quick(AudioTab::Applications, false);
    assert_eq!(
        target_of(helper_control_request("audio.slot.1.mute", &apps)),
        Some(AudioTarget::Stream { id: 10 })
    );
}

#[test]
fn pointer_on_thumb_centre_reads_back_its_value_and_drag_clamps_at_both_ends() {
    use crate::taskbar::status::audio::{AUDIO_TRACK_WIDTH, AUDIO_TRACK_X};
    let percent = audio_slider_percent;
    // Thumb centre for 120% under cap 150 is X + (120 / 150) * W; pointer there reads 120 back.
    assert_eq!(
        percent(true, AUDIO_TRACK_X + AUDIO_TRACK_WIDTH * 120.0 / 150.0),
        Some(120)
    );
    assert_eq!(
        percent(false, AUDIO_TRACK_X + AUDIO_TRACK_WIDTH * 0.5),
        Some(50)
    );
    // Drag past either end clamps to 0 and to the active cap.
    assert_eq!(percent(false, AUDIO_TRACK_X - 500.0), Some(0));
    assert_eq!(
        percent(false, AUDIO_TRACK_X + AUDIO_TRACK_WIDTH + 500.0),
        Some(100)
    );
    assert_eq!(
        percent(true, AUDIO_TRACK_X + AUDIO_TRACK_WIDTH + 500.0),
        Some(150)
    );
}

#[test]
fn pointer_drag_to_track_end_is_capped_at_100_or_150() {
    use crate::taskbar::status::audio::{AUDIO_TRACK_WIDTH, AUDIO_TRACK_X};
    let end = AUDIO_TRACK_X + AUDIO_TRACK_WIDTH;
    let percent = audio_slider_percent;
    assert_eq!(percent(false, end), Some(100));
    assert_eq!(percent(true, end), Some(150));
    assert_eq!(percent(false, AUDIO_TRACK_X), Some(0));
    assert_eq!(
        percent(true, AUDIO_TRACK_X + AUDIO_TRACK_WIDTH / 2.0),
        Some(75)
    );
}

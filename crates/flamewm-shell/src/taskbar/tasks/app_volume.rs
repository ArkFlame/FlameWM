//! Per-app volume on a task button: one wheel notch moves the application's playback stream
//! (PulseAudio/PipeWire sink-input) by one media step. Pure matching, step and request builders.
//! The request is the typed `SystemAction::SetVolume` on `AudioTarget::Stream`, sent through the
//! same control path as the tray volume wheel.

use flamewm_api::system::{AudioStreamSnapshot, SystemSnapshot};
use flamewm_control_core::ControlRequest;
use flamewm_platform::system::stepped_volume;

use crate::taskbar::status::audio::system_action_for_row;

/// Trimmed, lowercased id with an optional `.desktop` suffix removed; `None` when blank.
fn normalize(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let trimmed = trimmed.strip_suffix(".desktop").unwrap_or(trimmed).trim();
    (!trimmed.is_empty()).then(|| trimmed.to_ascii_lowercase())
}

/// Stream id of the playback stream that belongs to `app_id`. Tiers, in order:
/// `application.id`, then `application.process.binary`, then `application.name`. Within a
/// tier the first stream in list order wins, so a stronger tier always beats a weaker one.
#[must_use]
pub fn stream_for_app(streams: &[AudioStreamSnapshot], app_id: &str) -> Option<u32> {
    let wanted = normalize(app_id)?;
    let tiers: [fn(&AudioStreamSnapshot) -> Option<&str>; 3] = [
        |stream| stream.app_id.as_deref(),
        |stream| stream.process_binary.as_deref(),
        |stream| stream.app_name.as_deref(),
    ];
    tiers.iter().find_map(|field| {
        streams
            .iter()
            .find(|stream| {
                field(stream)
                    .and_then(normalize)
                    .is_some_and(|candidate| candidate == wanted)
            })
            .map(|stream| stream.id)
    })
}

/// One wheel notch on a task button: the matching stream's volume, stepped by one media step
/// and clamped to `0..=cap`. `None` when no stream matches, audio is unavailable, or the step
/// leaves the volume unchanged (a clamped notch sends nothing).
#[must_use]
pub fn app_volume_request(
    snapshot: &SystemSnapshot,
    app_id: &str,
    cap: u8,
    up: bool,
) -> Option<ControlRequest> {
    let streams = snapshot.audio.streams();
    let id = stream_for_app(streams, app_id)?;
    let stream = streams.iter().find(|stream| stream.id == id)?;
    let percent = stepped_volume(stream.volume_percent, up, cap);
    if percent == stream.volume_percent {
        return None;
    }
    system_action_for_row(snapshot, &format!("stream:{id}"), Some(percent), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_api::system::{
        AudioSnapshot, AudioTarget, AudioVolumeAction, ServiceAvailability, SystemAction,
    };

    fn stream(id: u32, volume: u8) -> AudioStreamSnapshot {
        AudioStreamSnapshot {
            id,
            endpoint_id: 1,
            name: format!("stream-{id}"),
            volume_percent: volume,
            ..Default::default()
        }
    }

    fn snapshot(streams: Vec<AudioStreamSnapshot>) -> SystemSnapshot {
        let mut snapshot = SystemSnapshot {
            revision: 9,
            ..SystemSnapshot::default()
        };
        snapshot.audio = AudioSnapshot::default().with_items(Vec::new(), streams);
        snapshot.audio.availability = ServiceAvailability::Available;
        snapshot.audio.generation = 3;
        snapshot.audio.server_generation = 4;
        snapshot
    }

    #[test]
    fn matches_application_id_ignoring_case_whitespace_and_desktop_suffix() {
        let mut firefox = stream(2, 40);
        firefox.app_id = Some("org.mozilla.firefox".to_owned());
        let streams = vec![stream(1, 10), firefox];
        assert_eq!(
            stream_for_app(&streams, "  ORG.Mozilla.Firefox.desktop "),
            Some(2)
        );
        assert_eq!(stream_for_app(&streams, "unrelated"), None);
        assert_eq!(stream_for_app(&streams, "   "), None);
    }

    #[test]
    fn falls_back_to_binary_then_name_and_prefers_stronger_tier() {
        let mut by_binary = stream(1, 10);
        by_binary.process_binary = Some("spotify".to_owned());
        let mut by_name = stream(2, 10);
        by_name.app_name = Some("Spotify".to_owned());
        let mut by_id = stream(3, 10);
        by_id.app_id = Some("spotify".to_owned());
        assert_eq!(
            stream_for_app(&[by_binary.clone(), by_name.clone()], "spotify"),
            Some(1)
        );
        assert_eq!(
            stream_for_app(&[by_name, by_binary, by_id], "spotify"),
            Some(3)
        );
    }

    #[test]
    fn step_is_one_media_step_and_clamps_to_cap() {
        let streams = vec![stream(4, 98)];
        let up = app_volume_request(&snapshot(streams.clone()), "x", 100, true);
        assert!(
            up.is_none(),
            "no match for an unrelated id must send nothing"
        );
        let mut app = stream(4, 98);
        app.app_id = Some("app".to_owned());
        let snap = snapshot(vec![app]);
        assert_eq!(
            percent_of(app_volume_request(&snap, "app", 100, true)),
            Some(100)
        );
        assert_eq!(app_volume_request(&snap, "app", 98, true), None);
        assert_eq!(
            percent_of(app_volume_request(&snap, "app", 100, false)),
            Some(93)
        );
    }

    #[test]
    fn down_clamps_at_zero_and_sends_nothing_at_zero() {
        let mut app = stream(5, 3);
        app.app_id = Some("app".to_owned());
        let snap = snapshot(vec![app.clone()]);
        assert_eq!(
            percent_of(app_volume_request(&snap, "app", 100, false)),
            Some(0)
        );
        app.volume_percent = 0;
        assert_eq!(
            app_volume_request(&snapshot(vec![app]), "app", 100, false),
            None
        );
    }

    #[test]
    fn request_is_typed_set_volume_on_the_matched_stream() {
        let mut app = stream(7, 50);
        app.app_id = Some("app".to_owned());
        let snap = snapshot(vec![app]);
        match app_volume_request(&snap, "app", 100, true) {
            Some(ControlRequest::SystemAction {
                action:
                    SystemAction::SetVolume(AudioVolumeAction {
                        target,
                        percent,
                        generation,
                        server_generation,
                    }),
                expected_revision,
            }) => {
                assert!(matches!(target, AudioTarget::Stream { id: 7 }));
                assert_eq!(percent, 55);
                assert_eq!(
                    (generation, server_generation, expected_revision),
                    (3, 4, 9)
                );
            }
            other => panic!("app wheel must send SetVolume on a stream, got {other:?}"),
        }
    }

    #[test]
    fn unavailable_audio_sends_nothing() {
        let mut app = stream(7, 50);
        app.app_id = Some("app".to_owned());
        let mut snap = snapshot(vec![app]);
        snap.audio.availability = ServiceAvailability::Unavailable;
        assert_eq!(app_volume_request(&snap, "app", 100, true), None);
    }

    fn percent_of(request: Option<ControlRequest>) -> Option<u8> {
        match request? {
            ControlRequest::SystemAction {
                action: SystemAction::SetVolume(AudioVolumeAction { percent, .. }),
                ..
            } => Some(percent),
            other => panic!("expected SetVolume, got {other:?}"),
        }
    }
}

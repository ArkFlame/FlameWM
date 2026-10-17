use flamewm_api::system::{
    AudioMuteAction, AudioSnapshot, AudioTarget, AudioVolumeAction, ServiceAvailability,
    SystemAction, SystemSnapshot,
};
use flamewm_control_core::ControlRequest;
use flamewm_shell_core::{AudioPopoverModel, AudioView};

use super::super::intent::StatusIntent;

/// Maximum audio rows a popover page can show. The compiled audio template
/// currently exposes only summary nodes, so paging selects which model rows
/// are visible to callers; the model itself is never truncated.
pub const AUDIO_SLOT_COUNT: usize = 3;

/// Rows materialized by the Audio Volume popup template (`audio-slot-1..4`).
pub const AUDIO_POPUP_ROWS: usize = 4;
/// Pixel height of one popup row group and of the scrolling list viewport.
pub const AUDIO_ROW_HEIGHT: u32 = 84;
pub const AUDIO_LIST_VIEWPORT: u32 = AUDIO_ROW_HEIGHT * AUDIO_POPUP_ROWS as u32;
/// Slider track left edge and width in popup coordinates (see audio.html).
pub const AUDIO_TRACK_X: f32 = 58.0;
pub const AUDIO_TRACK_WIDTH: f32 = 350.0;
pub const AUDIO_TRACK_HEIGHT: f32 = 6.0;
/// Thumb is 12px (`.audio-thumb`); at 150% it ends at popup x=414, where the percent label starts.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioRowKind {
    Endpoint,
    Stream,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioRowView {
    pub slot: usize,
    pub id: String,
    pub kind: AudioRowKind,
    pub label: String,
    pub volume_percent: u8,
    pub muted: bool,
    pub selected: bool,
}

/// Paged audio rows with stable model IDs.
///
/// Rows come only from `snapshot.audio.endpoints`/`snapshot.audio.streams`.
/// No synthetic rows are fabricated; paging selects a stable window and the
/// full model is preserved. Stable IDs are `endpoint:sink:<id>` /
/// `endpoint:source:<id>` for endpoints (kind included so a sink and a
/// source sharing a numeric id can never collide) and `stream:<id>`.
///
/// Legacy `endpoint:<id>` rows (no kind segment) resolve sink-first for
/// backward compatibility.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioPopoverView {
    pub popover: AudioPopoverModel,
    pub active_page: usize,
    pub page_count: usize,
    pub visible_rows: Vec<AudioRowView>,
}

/// Single availability predicate for audio visibility, popover and actions.
/// `Unknown` means pending (the provider has not reported yet): it stays
/// visible and its actions are allowed, because the write is revision- and
/// generation-fenced and a rejected write is reconciled by the provider.
/// Only `Unavailable` disables the surface.
#[must_use]
pub fn audio_available(snapshot: &SystemSnapshot) -> bool {
    snapshot.audio.availability != ServiceAvailability::Unavailable
}

#[must_use]
pub fn project(snapshot: &SystemSnapshot) -> AudioView {
    let mut view = flamewm_shell_core::StatusViews::from_snapshot(snapshot).audio;
    view.visible = audio_available(snapshot);
    view
}

#[must_use]
pub fn page_count(snapshot: &AudioSnapshot) -> usize {
    ceil_div(
        snapshot.endpoints().len() + snapshot.streams().len(),
        AUDIO_SLOT_COUNT,
    )
}

/// Page containing the default endpoint when present, else the first page.
/// Keeps the selected endpoint visible while paging the full model.
#[must_use]
pub fn active_page(snapshot: &AudioSnapshot) -> usize {
    let position = snapshot
        .endpoints()
        .iter()
        .position(|endpoint| endpoint.is_default);
    match position {
        Some(index) => index / AUDIO_SLOT_COUNT,
        None => 0,
    }
}

/// `(slot, row)` pairs for the active page, endpoints first then streams.
#[must_use]
pub fn paged_rows(snapshot: &AudioSnapshot) -> Vec<(usize, RowSource)> {
    paged_rows_for_page(snapshot, active_page(snapshot))
}

#[must_use]
pub fn paged_rows_for_page(snapshot: &AudioSnapshot, page: usize) -> Vec<(usize, RowSource)> {
    let total = snapshot.endpoints().len() + snapshot.streams().len();
    if total == 0 {
        return Vec::new();
    }
    let pages = ceil_div(total, AUDIO_SLOT_COUNT);
    let clamped = page.min(pages.saturating_sub(1));
    snapshot
        .endpoints()
        .iter()
        .map(RowSource::from_endpoint)
        .chain(snapshot.streams().iter().map(RowSource::from_stream))
        .skip(clamped * AUDIO_SLOT_COUNT)
        .take(AUDIO_SLOT_COUNT)
        .enumerate()
        .collect()
}

/// Stable row identity for audio slot `slot` on the active page.
#[must_use]
pub fn slot_id(snapshot: &AudioSnapshot, slot: usize) -> Option<String> {
    paged_rows(snapshot)
        .into_iter()
        .find(|(page_slot, _)| *page_slot == slot)
        .map(|(_, row)| row.stable_id().to_owned())
}

#[must_use]
pub fn popover(snapshot: &SystemSnapshot) -> Option<AudioPopoverModel> {
    if !audio_available(snapshot) {
        return None;
    }
    flamewm_shell_core::StatusPopovers::from_snapshot(snapshot).audio
}

#[must_use]
pub fn popover_view(snapshot: &SystemSnapshot) -> Option<AudioPopoverView> {
    let popover = popover(snapshot)?;
    let count = page_count(&snapshot.audio);
    let page = active_page(&snapshot.audio).min(count.saturating_sub(1));
    let visible_rows = paged_rows_for_page(&snapshot.audio, page)
        .into_iter()
        .map(|(slot, row)| row.into_view(slot))
        .collect();
    Some(AudioPopoverView {
        popover,
        active_page: page,
        page_count: count,
        visible_rows,
    })
}

/// Tab rows in popup order. Devices: default output, other outputs, then
/// inputs (endpoints only). Applications: playback/recording streams.
#[must_use]
pub fn tab_rows(
    snapshot: &AudioSnapshot,
    tab: flamewm_shell_core::status::AudioTab,
) -> Vec<RowSource> {
    use flamewm_api::system::AudioEndpointKind;
    use flamewm_shell_core::status::AudioTab;
    match tab {
        AudioTab::Applications => snapshot
            .streams()
            .iter()
            .map(RowSource::from_stream)
            .collect(),
        AudioTab::Devices => {
            let mut endpoints: Vec<_> = snapshot.endpoints().iter().collect();
            endpoints.sort_by_key(|endpoint| match (endpoint.kind, endpoint.is_default) {
                (AudioEndpointKind::Sink, true) => 0,
                (AudioEndpointKind::Sink, false) => 1,
                (AudioEndpointKind::Source, _) => 2,
            });
            endpoints
                .into_iter()
                .map(RowSource::from_endpoint)
                .collect()
        }
    }
}

/// Visible popup rows for `tab` at `scroll_offset` (px). Projection and host
/// routing both call this so `audio.slot.N.*` always names the projected row.
#[must_use]
pub fn visible_rows(
    snapshot: &AudioSnapshot,
    tab: flamewm_shell_core::status::AudioTab,
    scroll_offset: u32,
) -> Vec<RowSource> {
    let all = tab_rows(snapshot, tab);
    let (first, last) = flamewm_shell_core::status::visible_window(
        all.len(),
        AUDIO_ROW_HEIGHT,
        AUDIO_LIST_VIEWPORT,
        scroll_offset,
    );
    all.into_iter()
        .skip(first)
        .take(last.saturating_sub(first).min(AUDIO_POPUP_ROWS))
        .collect()
}

#[must_use]
pub fn default_endpoint_row(snapshot: &AudioSnapshot) -> Option<RowSource> {
    snapshot
        .endpoints()
        .iter()
        .find(|endpoint| endpoint.is_default)
        .map(RowSource::from_endpoint)
}

/// Resolve a stable endpoint/stream row id into one revision-fenced action.
#[must_use]
pub fn system_action_for_row(
    snapshot: &SystemSnapshot,
    row_id: &str,
    percent: Option<u8>,
    muted: Option<bool>,
) -> Option<ControlRequest> {
    if !audio_available(snapshot) {
        return None;
    }
    let target = if let Some(rest) = row_id.strip_prefix("endpoint:") {
        let (kind, id): (flamewm_api::system::AudioEndpointKind, u32) =
            if let Some((kind_name, id_text)) = rest.split_once(':') {
                let id = id_text.parse::<u32>().ok()?;
                match kind_name {
                    "sink" => (flamewm_api::system::AudioEndpointKind::Sink, id),
                    "source" => (flamewm_api::system::AudioEndpointKind::Source, id),
                    _ => return None,
                }
            } else {
                // Legacy `endpoint:<id>`: resolve through the endpoint that
                // owns the numeric id, so the result always matches row
                // identity instead of first-match order.
                let id = rest.parse::<u32>().ok()?;
                let endpoint = snapshot
                    .audio
                    .endpoints()
                    .iter()
                    .find(|endpoint| endpoint.id == id)?;
                (endpoint.kind, id)
            };
        snapshot
            .audio
            .endpoints()
            .iter()
            .find(|endpoint| endpoint.id == id && endpoint.kind == kind)?;
        AudioTarget::Endpoint { id, kind }
    } else if let Some(id) = row_id.strip_prefix("stream:") {
        let id = id.parse::<u32>().ok()?;
        snapshot
            .audio
            .streams()
            .iter()
            .find(|stream| stream.id == id)?;
        AudioTarget::Stream { id }
    } else {
        return None;
    };
    let action = match (percent, muted) {
        (Some(percent), None) if percent <= 150 => SystemAction::SetVolume(AudioVolumeAction {
            target,
            percent,
            generation: snapshot.audio.generation,
            server_generation: snapshot.audio.server_generation,
        }),
        (None, Some(muted)) => SystemAction::SetMute(AudioMuteAction {
            target,
            muted,
            generation: snapshot.audio.generation,
            server_generation: snapshot.audio.server_generation,
        }),
        _ => return None,
    };
    Some(ControlRequest::SystemAction {
        action,
        expected_revision: snapshot.revision,
    })
}

/// Trimmed candidate text, or `None` when absent or blank.
fn non_blank(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowSource {
    pub kind: AudioRowKind,
    pub id: String,
    pub label: String,
    pub volume_percent: u8,
    pub muted: bool,
    pub selected: bool,
}

impl RowSource {
    fn endpoint_kind_name(kind: flamewm_api::system::AudioEndpointKind) -> &'static str {
        match kind {
            flamewm_api::system::AudioEndpointKind::Sink => "sink",
            flamewm_api::system::AudioEndpointKind::Source => "source",
        }
    }

    #[must_use]
    pub fn from_endpoint(endpoint: &flamewm_api::system::AudioEndpointSnapshot) -> Self {
        let fallback = match endpoint.kind {
            flamewm_api::system::AudioEndpointKind::Sink => "Output device",
            flamewm_api::system::AudioEndpointKind::Source => "Input device",
        };
        let label = non_blank(Some(&endpoint.description))
            .unwrap_or(fallback)
            .to_owned();
        Self {
            kind: AudioRowKind::Endpoint,
            id: format!(
                "endpoint:{}:{}",
                Self::endpoint_kind_name(endpoint.kind),
                endpoint.id
            ),
            label,
            volume_percent: endpoint.volume_percent,
            muted: endpoint.muted,
            selected: endpoint.is_default,
        }
    }

    #[must_use]
    pub fn from_stream(stream: &flamewm_api::system::AudioStreamSnapshot) -> Self {
        Self {
            kind: AudioRowKind::Stream,
            id: format!("stream:{}", stream.id),
            label: [
                stream.app_name.as_deref(),
                stream.app_id.as_deref(),
                stream.process_binary.as_deref(),
                stream.media_name.as_deref(),
                Some(stream.name.as_str()),
            ]
            .into_iter()
            .find_map(non_blank)
            .unwrap_or("Unknown application")
            .to_owned(),
            volume_percent: stream.volume_percent,
            muted: stream.muted,
            selected: false,
        }
    }

    #[must_use]
    pub fn stable_id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn into_view(self, slot: usize) -> AudioRowView {
        AudioRowView {
            slot,
            id: self.id,
            kind: self.kind,
            label: self.label,
            volume_percent: self.volume_percent,
            muted: self.muted,
            selected: self.selected,
        }
    }
}

#[must_use]
pub fn intent(snapshot: &SystemSnapshot) -> Option<StatusIntent> {
    audio_available(snapshot).then_some(StatusIntent::Audio)
}

fn ceil_div(total: usize, per_page: usize) -> usize {
    if total == 0 {
        1
    } else {
        (total + per_page - 1) / per_page
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_api::system::{AudioEndpointKind, AudioEndpointSnapshot, AudioStreamSnapshot};

    #[test]
    fn endpoint_row_maps_to_revision_fenced_volume_action() {
        let mut snapshot = SystemSnapshot::default();
        snapshot.revision = 9;
        snapshot.audio.availability = ServiceAvailability::Available;
        snapshot.audio.generation = 3;
        snapshot.audio.server_generation = 4;
        snapshot.audio.endpoints.push(AudioEndpointSnapshot {
            id: 7,
            kind: AudioEndpointKind::Sink,
            name: "sink".to_owned(),
            description: String::new(),
            volume_percent: 50,
            muted: false,
            is_default: true,
        });
        assert_eq!(
            system_action_for_row(&snapshot, "endpoint:sink:7", Some(60), None),
            Some(ControlRequest::SystemAction {
                action: SystemAction::SetVolume(AudioVolumeAction {
                    target: AudioTarget::Endpoint {
                        id: 7,
                        kind: AudioEndpointKind::Sink
                    },
                    percent: 60,
                    generation: 3,
                    server_generation: 4
                }),
                expected_revision: 9
            })
        );
    }

    #[test]
    fn active_page_keeps_default_endpoint_and_stream_rows_reachable() {
        let mut snapshot = AudioSnapshot::default();
        for id in 1..=4 {
            snapshot.endpoints.push(AudioEndpointSnapshot {
                id,
                kind: AudioEndpointKind::Sink,
                name: format!("sink-{id}"),
                description: String::new(),
                volume_percent: 50,
                muted: false,
                is_default: id == 4,
            });
        }
        snapshot.streams.push(AudioStreamSnapshot {
            id: 9,
            endpoint_id: 4,
            name: "app".to_owned(),
            volume_percent: 50,
            muted: false,
            ..Default::default()
        });
        assert_eq!(active_page(&snapshot), 1);
        assert_eq!(slot_id(&snapshot, 0), Some("endpoint:sink:4".to_owned()));
        assert_eq!(slot_id(&snapshot, 1), Some("stream:9".to_owned()));
    }

    fn sink_fixture(availability: ServiceAvailability) -> SystemSnapshot {
        let mut snapshot = SystemSnapshot::default();
        snapshot.revision = 5;
        snapshot.audio.availability = availability;
        snapshot.audio.generation = 1;
        snapshot.audio.server_generation = 2;
        snapshot.audio.endpoints.push(AudioEndpointSnapshot {
            id: 7,
            kind: AudioEndpointKind::Sink,
            name: "sink".to_owned(),
            description: String::new(),
            volume_percent: 50,
            muted: false,
            is_default: true,
        });
        snapshot
    }

    #[test]
    fn availability_predicate_is_shared_by_visibility_popover_actions_and_intent() {
        for availability in [
            ServiceAvailability::Unknown,
            ServiceAvailability::Available,
            ServiceAvailability::Unavailable,
        ] {
            let snapshot = sink_fixture(availability);
            let allowed = audio_available(&snapshot);
            assert_eq!(
                project(&snapshot).visible,
                allowed,
                "{availability:?} visibility"
            );
            assert_eq!(
                popover(&snapshot).is_some(),
                allowed,
                "{availability:?} popover"
            );
            assert_eq!(
                system_action_for_row(&snapshot, "endpoint:sink:7", Some(60), None).is_some(),
                allowed,
                "{availability:?} volume action"
            );
            assert_eq!(
                intent(&snapshot).is_some(),
                allowed,
                "{availability:?} intent"
            );
        }
    }

    #[test]
    fn devices_order_is_default_output_other_outputs_then_inputs() {
        use flamewm_shell_core::status::AudioTab;
        let mut snapshot = AudioSnapshot::default();
        for (id, kind, is_default) in [
            (3, AudioEndpointKind::Source, false),
            (2, AudioEndpointKind::Sink, false),
            (1, AudioEndpointKind::Sink, true),
        ] {
            snapshot.endpoints.push(AudioEndpointSnapshot {
                id,
                kind,
                name: format!("dev-{id}"),
                description: String::new(),
                volume_percent: 10,
                muted: false,
                is_default,
            });
        }
        let ids: Vec<String> = visible_rows(&snapshot, AudioTab::Devices, 0)
            .into_iter()
            .map(|row| row.id)
            .collect();
        assert_eq!(
            ids,
            ["endpoint:sink:1", "endpoint:sink:2", "endpoint:source:3"]
        );
        assert!(visible_rows(&snapshot, AudioTab::Applications, 0).is_empty());
    }

    #[test]
    fn popup_window_caps_visible_rows_at_the_popup_slot_count() {
        use flamewm_shell_core::status::AudioTab;
        let mut snapshot = AudioSnapshot::default();
        for id in 1..=6 {
            snapshot.streams.push(AudioStreamSnapshot {
                id,
                endpoint_id: 1,
                name: format!("app-{id}"),
                volume_percent: 10,
                muted: false,
                ..Default::default()
            });
        }
        assert_eq!(
            visible_rows(&snapshot, AudioTab::Applications, 0).len(),
            AUDIO_POPUP_ROWS
        );
        let scrolled = visible_rows(&snapshot, AudioTab::Applications, AUDIO_ROW_HEIGHT);
        assert_eq!(scrolled[0].id, "stream:2");
    }

    #[test]
    fn pending_unknown_audio_allows_volume_and_mute_actions() {
        let snapshot = sink_fixture(ServiceAvailability::Unknown);
        assert!(system_action_for_row(&snapshot, "endpoint:sink:7", None, Some(true)).is_some());
    }

    #[test]
    fn endpoint_without_description_is_labelled_output_device() {
        let endpoint = AudioEndpointSnapshot {
            id: 7,
            kind: AudioEndpointKind::Sink,
            name: "alsa_output.pci-x".to_owned(),
            description: "   ".to_owned(),
            volume_percent: 50,
            muted: false,
            is_default: true,
        };
        assert_eq!(RowSource::from_endpoint(&endpoint).label, "Output device");
        let described = AudioEndpointSnapshot {
            description: "Built-in Audio Analog Stereo".to_owned(),
            ..endpoint
        };
        assert_eq!(
            RowSource::from_endpoint(&described).label,
            "Built-in Audio Analog Stereo"
        );
    }

    #[test]
    fn stream_label_prefers_app_name_then_app_id_then_binary_then_media_then_name() {
        let base = AudioStreamSnapshot {
            id: 9,
            endpoint_id: 1,
            name: "  stream name  ".to_owned(),
            volume_percent: 50,
            muted: false,
            ..Default::default()
        };
        let label = |stream: &AudioStreamSnapshot| RowSource::from_stream(stream).label;
        assert_eq!(label(&base), "stream name");
        let media = AudioStreamSnapshot {
            media_name: Some("Track".to_owned()),
            ..base.clone()
        };
        assert_eq!(label(&media), "Track");
        let binary = AudioStreamSnapshot {
            process_binary: Some("firefox".to_owned()),
            media_name: Some("Track".to_owned()),
            ..base.clone()
        };
        assert_eq!(label(&binary), "firefox");
        let app_id = AudioStreamSnapshot {
            app_id: Some("org.mozilla.firefox".to_owned()),
            process_binary: Some("firefox".to_owned()),
            ..base.clone()
        };
        assert_eq!(label(&app_id), "org.mozilla.firefox");
        let app_name = AudioStreamSnapshot {
            app_name: Some("Firefox".to_owned()),
            app_id: Some("org.mozilla.firefox".to_owned()),
            ..base.clone()
        };
        assert_eq!(label(&app_name), "Firefox");
    }

    #[test]
    fn stream_label_skips_blank_candidates_and_falls_back_to_unknown_application() {
        let blank = AudioStreamSnapshot {
            id: 3,
            endpoint_id: 1,
            name: " ".to_owned(),
            app_name: Some("   ".to_owned()),
            app_id: Some(String::new()),
            process_binary: None,
            media_name: Some("\t".to_owned()),
            ..Default::default()
        };
        let row = RowSource::from_stream(&blank);
        assert_eq!(row.label, "Unknown application");
        assert_eq!(row.id, "stream:3");
    }
}

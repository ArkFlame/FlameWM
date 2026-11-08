use super::calendar::ACTIVE_TAB_FILL;
use super::*;
use crate::taskbar::status::audio::{AUDIO_THUMB_SIZE, AUDIO_TRACK_HEIGHT, AUDIO_TRACK_WIDTH};

pub fn project_audio(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
) -> Result<(), String> {
    project_audio_full(
        document,
        snapshot,
        flamewm_shell_core::status::AudioTab::Devices,
        flamewm_shell_core::status::AudioSettings::default(),
        0,
    )
}

/// Full audio projection: Devices/Applications tabs, `RangeSpec` slider state,
/// mute + percent, raise-maximum cap. Scroll/virtualized lists are declared
/// in the template (`overflow-y:auto` on `#audio-list`); the retained
/// `scroll_offset` selects the visible window without truncating the model.
pub fn project_audio_full(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
    tab: flamewm_shell_core::status::AudioTab,
    settings: flamewm_shell_core::status::AudioSettings,
    scroll_offset: u32,
) -> Result<(), String> {
    let devices = tab == flamewm_shell_core::status::AudioTab::Devices;
    for (id, active) in [("audio-tab-devices", devices), ("audio-tab-apps", !devices)] {
        if active {
            document.background(id, ACTIVE_TAB_FILL)?;
        } else {
            document.background_clear(id)?;
        }
    }
    // Rows come from the tab-filtered, scroll-windowed list shared with host routing.
    let rows =
        crate::taskbar::status::audio::visible_rows(&snapshot.system.audio, tab, scroll_offset);
    document.visible("audio-empty", rows.is_empty())?;
    document.text_ref(
        "audio-empty",
        if rows.is_empty() {
            if devices {
                "No audio devices"
            } else {
                "No active applications"
            }
        } else {
            ""
        },
    )?;
    for slot in 0..crate::taskbar::status::audio::AUDIO_POPUP_ROWS {
        let n = slot + 1;
        let row = rows.get(slot);
        document.visible(&format!("audio-slot-{n}"), row.is_some())?;
        let Some(row) = row else { continue };
        // Per-row RangeSpec under the current 100/150 cap; fill snapped into it.
        let spec = settings.slider_spec(row.volume_percent);
        let shown = spec.snap(f32::from(row.volume_percent));
        let is_stream = row.kind == crate::taskbar::status::audio::AudioRowKind::Stream;
        let is_input = row.id.starts_with("endpoint:source:");
        document.text_ref(&format!("audio-slot-{n}-label"), &row.label)?;
        document.text(
            &format!("audio-slot-{n}-value"),
            format!("{}%", row.volume_percent),
        )?;
        document.visible(&format!("audio-slot-{n}-mute-icon-on"), row.muted)?;
        document.visible(&format!("audio-slot-{n}-mute-icon-off"), !row.muted)?;
        document.size(
            &format!("audio-slot-{n}-fill"),
            audio_fill_width(shown, spec.max),
            AUDIO_TRACK_HEIGHT,
        )?;
        document.position(
            &format!("audio-slot-{n}-thumb"),
            audio_thumb_left(shown, spec.max),
            AUDIO_THUMB_TOP,
        )?;
        document.visible(&format!("audio-slot-{n}-icon-out"), !is_stream && !is_input)?;
        document.visible(&format!("audio-slot-{n}-icon-in"), !is_stream && is_input)?;
        document.visible(&format!("audio-slot-{n}-icon-app"), is_stream)?;
    }
    document.toggle(
        "audio-raise-track",
        "audio-raise-knob",
        settings.raise_maximum,
    )?;
    if settings.raise_maximum {
        // The shared toggle recipe paints a fixed skin red; the on-track follows the live accent.
        document.background("audio-raise-track", crate::projection::task_accent())?;
    }
    Ok(())
}

/// Lane x of the track's left edge (the `.audio-lane` starts 6px left of the track in audio.html).
const AUDIO_LANE_TRACK_OFFSET: f32 = 6.0;

/// Thumb top in lane coordinates: centres the knob on the track height.
const AUDIO_THUMB_TOP: f32 = (AUDIO_TRACK_HEIGHT - AUDIO_THUMB_SIZE) / 2.0;

/// Fill width: the knob's left half, the value's share of the knob travel, and the right half.
/// At 0% the fill is one knob wide, so the knob sits flush at the track's left end; at max the
/// fill spans the track and the knob sits flush at the right end.
fn audio_fill_width(shown: f32, max: f32) -> f32 {
    AUDIO_THUMB_SIZE / 2.0
        + shown / max * (AUDIO_TRACK_WIDTH - AUDIO_THUMB_SIZE)
        + AUDIO_THUMB_SIZE / 2.0
}

/// Thumb left edge in lane coordinates: the knob's left edge is the fill end minus one knob,
/// so it stays inside the track. `value_from_pointer` inverts this mapping for drag.
fn audio_thumb_left(shown: f32, max: f32) -> f32 {
    AUDIO_LANE_TRACK_OFFSET + shown / max * (AUDIO_TRACK_WIDTH - AUDIO_THUMB_SIZE)
}

#[cfg(test)]
mod audio_template_tests {
    use super::*;
    use crate::taskbar::status::audio::AUDIO_TRACK_X;
    use flamewm_api::system::{
        AudioEndpointKind, AudioEndpointSnapshot, AudioStreamSnapshot, ServiceAvailability,
    };
    use flamewm_shell_core::status::{AudioSettings, AudioTab};

    const AUDIO_ARTIFACT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-audio.rwr"));

    // Forwards every write to the real compiled audio template while recording
    // what the projection assigned, so the test reads back per-row state.
    #[derive(Default)]
    struct Recorded {
        texts: std::collections::HashMap<String, String>,
        visible: std::collections::HashMap<String, bool>,
        widths: std::collections::HashMap<String, f32>,
        positions: std::collections::HashMap<String, (f32, f32)>,
        toggles: std::collections::HashMap<String, bool>,
        backgrounds: std::collections::HashMap<String, UiColor>,
    }

    struct AudioRecorder<'a> {
        document: &'a mut flamewm_ui_x11::UiDocument,
        recorded: Recorded,
    }

    impl<'a> AudioRecorder<'a> {
        fn new(document: &'a mut flamewm_ui_x11::UiDocument) -> Self {
            Self {
                document,
                recorded: Recorded::default(),
            }
        }
    }

    impl UiDocumentAccess for AudioRecorder<'_> {
        fn text(&mut self, id: &str, text: String) -> Result<(), String> {
            self.recorded.texts.insert(id.to_owned(), text.clone());
            UiDocumentAccess::text(self.document, id, text)
        }
        fn visible(&mut self, id: &str, visible: bool) -> Result<(), String> {
            self.recorded.visible.insert(id.to_owned(), visible);
            UiDocumentAccess::visible(self.document, id, visible)
        }
        fn size(&mut self, id: &str, width: f32, height: f32) -> Result<(), String> {
            self.recorded.widths.insert(id.to_owned(), width);
            UiDocumentAccess::size(self.document, id, width, height)
        }
        fn position(&mut self, id: &str, left: f32, top: f32) -> Result<(), String> {
            self.recorded.positions.insert(id.to_owned(), (left, top));
            UiDocumentAccess::position(self.document, id, left, top)
        }
        fn toggle(&mut self, track_id: &str, knob_id: &str, checked: bool) -> Result<(), String> {
            self.recorded.toggles.insert(track_id.to_owned(), checked);
            UiDocumentAccess::toggle(self.document, track_id, knob_id, checked)
        }
        fn background(&mut self, id: &str, color: UiColor) -> Result<(), String> {
            self.recorded.backgrounds.insert(id.to_owned(), color);
            UiDocumentAccess::background(self.document, id, color)
        }
        fn background_clear(&mut self, id: &str) -> Result<(), String> {
            self.recorded.backgrounds.remove(id);
            UiDocumentAccess::background_clear(self.document, id)
        }
        forward!(
            image_rgb8(id: &str, image: RuntimeImage),
            image_rgba8(id: &str, image: RuntimeImage),
            border(id: &str, color: UiColor),
            foreground(id: &str, color: UiColor),
            border_clear(id: &str),
            foreground_clear(id: &str),
            font_size(id: &str, size_px: f32),
            font_weight(id: &str, weight: u16),
            overflow(id: &str, x: flamewm_ui_x11::Overflow, y: flamewm_ui_x11::Overflow),
            layer(id: &str, layer: flamewm_ui_core::style::UiLayer),
        );
    }

    // Two outputs (default speakers at 120%, muted headphones at 40%), one
    // input at 70%, and two app streams at 80% and 25%.
    fn fixture(tab: AudioTab, raise_maximum: bool) -> Recorded {
        let mut snapshot = crate::ShellSnapshot::default();
        snapshot.system.audio.availability = ServiceAvailability::Available;
        snapshot.system.audio.endpoints = vec![
            AudioEndpointSnapshot {
                id: 1,
                kind: AudioEndpointKind::Sink,
                name: "speakers".to_owned(),
                description: "Built-in Speakers".to_owned(),
                volume_percent: 120,
                muted: false,
                is_default: true,
            },
            AudioEndpointSnapshot {
                id: 2,
                kind: AudioEndpointKind::Sink,
                name: "headphones".to_owned(),
                description: "Headphones".to_owned(),
                volume_percent: 40,
                muted: true,
                is_default: false,
            },
            AudioEndpointSnapshot {
                id: 3,
                kind: AudioEndpointKind::Source,
                name: "mic".to_owned(),
                description: "Microphone".to_owned(),
                volume_percent: 70,
                muted: false,
                is_default: false,
            },
        ];
        snapshot.system.audio.streams = vec![
            AudioStreamSnapshot {
                id: 10,
                endpoint_id: 1,
                name: "Firefox".to_owned(),
                volume_percent: 80,
                muted: false,
                ..Default::default()
            },
            AudioStreamSnapshot {
                id: 11,
                endpoint_id: 1,
                name: "Spotify".to_owned(),
                volume_percent: 25,
                muted: true,
                ..Default::default()
            },
        ];
        project(&snapshot, tab, AudioSettings { raise_maximum })
    }

    fn project(
        snapshot: &crate::ShellSnapshot,
        tab: AudioTab,
        settings: AudioSettings,
    ) -> Recorded {
        let mut document =
            flamewm_ui_x11::decode_document(AUDIO_ARTIFACT).expect("audio artifact must decode");
        let mut recorder = AudioRecorder::new(&mut document);
        project_audio_full(&mut recorder, snapshot, tab, settings, 0)
            .expect("every projected audio id must be a valid target");
        recorder.recorded
    }

    fn text<'a>(recorded: &'a Recorded, id: &str) -> &'a str {
        recorded.texts.get(id).map_or("<unset>", String::as_str)
    }

    #[test]
    fn devices_tab_projects_default_output_then_outputs_then_inputs_with_own_slider() {
        let recorded = fixture(AudioTab::Devices, false);
        assert_eq!(text(&recorded, "audio-slot-1-label"), "Built-in Speakers");
        assert_eq!(text(&recorded, "audio-slot-2-label"), "Headphones");
        assert_eq!(text(&recorded, "audio-slot-3-label"), "Microphone");
        assert_eq!(recorded.visible.get("audio-slot-4"), Some(&false));
        // Cap 100: the 120% default output shows its real value but its fill snaps to the cap.
        assert_eq!(text(&recorded, "audio-slot-1-value"), "120%");
        assert_eq!(recorded.widths.get("audio-slot-1-fill"), Some(&350.0));
        assert_eq!(text(&recorded, "audio-slot-2-value"), "40%");
        assert_approx(
            recorded.widths.get("audio-slot-2-fill").copied(),
            16.0 + 334.0 * 0.4,
        );
        assert_eq!(
            recorded.visible.get("audio-slot-2-mute-icon-on"),
            Some(&true)
        );
        assert_eq!(
            recorded.visible.get("audio-slot-2-mute-icon-off"),
            Some(&false)
        );
        assert_eq!(
            recorded.visible.get("audio-slot-1-mute-icon-on"),
            Some(&false)
        );
        assert_eq!(
            recorded.visible.get("audio-slot-1-mute-icon-off"),
            Some(&true)
        );
        assert_eq!(recorded.visible.get("audio-slot-1-icon-app"), Some(&false));
        // Output rows keep the volume icon; the source (microphone) row swaps to the input icon.
        assert_eq!(recorded.visible.get("audio-slot-1-icon-out"), Some(&true));
        assert_eq!(recorded.visible.get("audio-slot-1-icon-in"), Some(&false));
        assert_eq!(recorded.visible.get("audio-slot-3-icon-out"), Some(&false));
        assert_eq!(recorded.visible.get("audio-slot-3-icon-in"), Some(&true));
        assert_eq!(recorded.visible.get("audio-empty"), Some(&false));
        assert_eq!(recorded.toggles.get("audio-raise-track"), Some(&false));
    }

    #[test]
    fn raise_maximum_extends_the_slider_range_to_150_without_rescaling_values() {
        let recorded = fixture(AudioTab::Devices, true);
        assert_eq!(text(&recorded, "audio-slot-1-value"), "120%");
        assert_approx(
            recorded.widths.get("audio-slot-1-fill").copied(),
            16.0 + 334.0 * 0.8,
        );
        assert_approx(
            recorded.widths.get("audio-slot-2-fill").copied(),
            16.0 + 334.0 * 40.0 / 150.0,
        );
        assert_eq!(recorded.toggles.get("audio-raise-track"), Some(&true));
    }

    fn assert_approx(actual: Option<f32>, expected: f32) {
        let actual = actual.expect("fill width recorded");
        assert!(
            (actual - expected).abs() < 0.01,
            "got {actual}, want {expected}"
        );
    }

    fn assert_thumb_at(recorded: &Recorded, id: &str, left: f32) {
        let (x, y) = *recorded.positions.get(id).expect("thumb position recorded");
        assert!(
            (x - left).abs() < 0.01 && y == 0.0,
            "{id}: got ({x}, {y}), want ({left}, 0)"
        );
    }

    #[test]
    fn thumb_centre_maps_fraction_of_track_at_0_100_and_150_caps() {
        // The knob stays inside the track: left edge flush at 0% (lane x 6), right edge flush
        // at max (lane x 6 + 350 - 16 = 340 left edge).
        assert_eq!(audio_thumb_left(0.0, 100.0), 6.0);
        assert_eq!(audio_thumb_left(100.0, 100.0), 340.0);
        assert_eq!(audio_thumb_left(150.0, 150.0), 340.0);
        assert_eq!(audio_thumb_left(75.0, 150.0), 173.0);
        assert_eq!(
            AUDIO_THUMB_TOP, 0.0,
            "16px knob spans the 16px track height"
        );
        // Fill is one knob wide at 0% and spans the track at max.
        assert_eq!(audio_fill_width(0.0, 100.0), 16.0);
        assert_eq!(audio_fill_width(100.0, 100.0), 350.0);
    }

    #[test]
    fn thumb_position_follows_snapped_fill_under_cap() {
        // Cap 100: 120% snaps to the track end, so the knob's right edge is flush at lane 6 + 350.
        let capped = fixture(AudioTab::Devices, false);
        assert_thumb_at(&capped, "audio-slot-1-thumb", 340.0);
        assert_thumb_at(&capped, "audio-slot-2-thumb", 6.0 + 334.0 * 0.4);
        // Raise maximum: 120% of 150 is 0.8 of the travel.
        let raised = fixture(AudioTab::Devices, true);
        assert_thumb_at(&raised, "audio-slot-1-thumb", 6.0 + 334.0 * 0.8);
    }

    #[test]
    fn applications_tab_projects_playback_streams_with_app_icons() {
        let recorded = fixture(AudioTab::Applications, false);
        assert_eq!(text(&recorded, "audio-slot-1-label"), "Firefox");
        assert_eq!(text(&recorded, "audio-slot-2-label"), "Spotify");
        assert_eq!(
            recorded.visible.get("audio-slot-2-mute-icon-on"),
            Some(&true)
        );
        assert_eq!(
            recorded.visible.get("audio-slot-2-mute-icon-off"),
            Some(&false)
        );
        assert_eq!(recorded.visible.get("audio-slot-1-icon-app"), Some(&true));
        assert_eq!(recorded.visible.get("audio-slot-1-icon-out"), Some(&false));
        assert_eq!(recorded.visible.get("audio-slot-3"), Some(&false));
        assert_eq!(
            recorded.backgrounds.get("audio-tab-apps"),
            Some(&ACTIVE_TAB_FILL)
        );
        assert_eq!(recorded.backgrounds.get("audio-tab-devices"), None);
    }

    #[test]
    fn devices_tab_active_fill_matches_calendar_tab_mechanism() {
        let recorded = fixture(AudioTab::Devices, false);
        assert_eq!(
            recorded.backgrounds.get("audio-tab-devices"),
            Some(&ACTIVE_TAB_FILL)
        );
        assert_eq!(recorded.backgrounds.get("audio-tab-apps"), None);
        assert!(!recorded.visible.contains_key("audio-tab-devices-bg"));
    }

    #[test]
    fn applications_tab_without_streams_shows_empty_state() {
        let mut snapshot = crate::ShellSnapshot::default();
        snapshot.system.audio.availability = ServiceAvailability::Available;
        let recorded = project(&snapshot, AudioTab::Applications, AudioSettings::default());
        assert_eq!(recorded.visible.get("audio-empty"), Some(&true));
        assert_eq!(text(&recorded, "audio-empty"), "No active applications");
    }

    const POPUP_VIEWPORT: (f32, f32) = (459.0, 460.0);

    // Synthetic layout fixture: sink 0%, sink 100% and source 50% on Devices;
    // an app stream at 150% on Applications. Raise-maximum is on, so the cap is 150.
    // Raw PulseAudio sink ids never reach the popup; the Devices row shows the
    // endpoint description instead.
    #[test]
    fn popup_hides_raw_sink_name_and_devices_row_uses_description() {
        let mut snapshot = crate::ShellSnapshot::default();
        snapshot.system.audio.availability = ServiceAvailability::Available;
        snapshot.system.audio.sink_name = "alsa_output.pci-x".to_owned();
        snapshot.system.audio.endpoints = vec![AudioEndpointSnapshot {
            id: 1,
            kind: AudioEndpointKind::Sink,
            name: "alsa_output.pci-x".to_owned(),
            description: "Built-in Audio Analog Stereo".to_owned(),
            volume_percent: 50,
            muted: false,
            is_default: true,
        }];
        let recorded = project(&snapshot, AudioTab::Devices, AudioSettings::default());
        assert!(
            recorded
                .texts
                .values()
                .all(|value| !value.contains("alsa_output")),
            "raw sink id leaked: {:?}",
            recorded.texts
        );
        assert!(!recorded.texts.contains_key("audio-details"));
        assert_eq!(
            text(&recorded, "audio-slot-1-label"),
            "Built-in Audio Analog Stereo"
        );
    }

    fn layout_document(
        snapshot: &crate::ShellSnapshot,
        tab: AudioTab,
    ) -> flamewm_ui_x11::UiDocument {
        let mut document =
            flamewm_ui_x11::decode_document(AUDIO_ARTIFACT).expect("audio artifact must decode");
        {
            let mut recorder = AudioRecorder::new(&mut document);
            project_audio_full(
                &mut recorder,
                snapshot,
                tab,
                AudioSettings {
                    raise_maximum: true,
                },
                0,
            )
            .expect("every projected audio id must be a valid target");
        }
        document
    }

    fn layout_rect(document: &flamewm_ui_x11::UiDocument, id: &str) -> (f32, f32, f32, f32) {
        document
            .layout_box_by_id(id, POPUP_VIEWPORT)
            .unwrap_or_else(|| panic!("no layout box for {id}"))
    }

    fn overlaps(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
        a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
    }

    fn inside(inner: (f32, f32, f32, f32), outer: (f32, f32, f32, f32)) -> bool {
        inner.0 >= outer.0
            && inner.1 >= outer.1
            && inner.0 + inner.2 <= outer.0 + outer.2
            && inner.1 + inner.3 <= outer.1 + outer.3
    }

    fn layout_snapshot() -> crate::ShellSnapshot {
        let mut snapshot = crate::ShellSnapshot::default();
        snapshot.system.audio.availability = ServiceAvailability::Available;
        snapshot.system.audio.endpoints = vec![
            AudioEndpointSnapshot {
                id: 1,
                kind: AudioEndpointKind::Sink,
                name: "sink-zero".to_owned(),
                description: "Sink Zero".to_owned(),
                volume_percent: 0,
                muted: false,
                is_default: true,
            },
            AudioEndpointSnapshot {
                id: 2,
                kind: AudioEndpointKind::Sink,
                name: "sink-full".to_owned(),
                description: "Sink Full".to_owned(),
                volume_percent: 100,
                muted: false,
                is_default: false,
            },
            AudioEndpointSnapshot {
                id: 3,
                kind: AudioEndpointKind::Source,
                name: "source-half".to_owned(),
                description: "Source Half".to_owned(),
                volume_percent: 50,
                muted: false,
                is_default: false,
            },
        ];
        snapshot.system.audio.streams = vec![AudioStreamSnapshot {
            id: 10,
            endpoint_id: 1,
            name: "Loud App".to_owned(),
            volume_percent: 150,
            muted: false,
            ..Default::default()
        }];
        snapshot
    }

    #[test]
    fn layout_thumbs_centre_on_track_fraction_with_labels_clear() {
        let snapshot = layout_snapshot();
        let devices = layout_document(&snapshot, AudioTab::Devices);
        let apps = layout_document(&snapshot, AudioTab::Applications);
        // (document, slot, percent): fraction = percent / 150 under raise-maximum.
        let rows = [
            (&devices, 1, 0.0_f32),
            (&devices, 2, 100.0),
            (&devices, 3, 50.0),
            (&apps, 1, 150.0),
        ];
        for (document, n, percent) in rows {
            let track = layout_rect(document, &format!("audio-slot-{n}-track"));
            let thumb = layout_rect(document, &format!("audio-slot-{n}-thumb"));
            let value = layout_rect(document, &format!("audio-slot-{n}-value"));
            let label = layout_rect(document, &format!("audio-slot-{n}-label"));
            let mute = layout_rect(document, &format!("audio-slot-{n}-mute"));
            let group = layout_rect(document, &format!("audio-slot-{n}"));

            // Knob centre is inset by half a knob: X + d/2 + fraction * (W - d).
            let travel = track.2 - AUDIO_THUMB_SIZE;
            let expected_x = track.0 + AUDIO_THUMB_SIZE / 2.0 + (percent / 150.0) * travel;
            let fill = layout_rect(document, &format!("audio-slot-{n}-fill"));
            assert!(
                (fill.3 - track.3).abs() <= 1.0 && (fill.1 - track.1).abs() <= 1.0,
                "slot {n} ({percent}%): fill {fill:?} must share track {track:?} height and top"
            );
            assert!(
                (fill.2 - (AUDIO_THUMB_SIZE + (percent / 150.0) * travel)).abs() <= 1.0,
                "slot {n} ({percent}%): fill width {} != knob + value x travel",
                fill.2
            );
            assert!(
                thumb.0 >= track.0 - 1.0 && thumb.0 + thumb.2 <= track.0 + track.2 + 1.0,
                "slot {n} ({percent}%): knob {thumb:?} must stay inside track {track:?}"
            );
            let thumb_cx = thumb.0 + thumb.2 / 2.0;
            assert!(
                (thumb_cx - expected_x).abs() <= 1.0,
                "slot {n} ({percent}%): thumb centre x {thumb_cx} != expected {expected_x} (track {track:?} thumb {thumb:?} group {group:?})"
            );
            assert!(
                ((track.0 - group.0) - AUDIO_TRACK_X).abs() <= 1.0
                    && (track.2 - AUDIO_TRACK_WIDTH).abs() <= 1.0,
                "slot {n}: track {track:?} is not at row x {AUDIO_TRACK_X} width {AUDIO_TRACK_WIDTH}"
            );
            let thumb_cy = thumb.1 + thumb.3 / 2.0;
            let track_cy = track.1 + track.3 / 2.0;
            assert!(
                (thumb_cy - track_cy).abs() <= 1.0,
                "slot {n}: thumb centre y {thumb_cy} != track centre y {track_cy}"
            );
            assert!(
                thumb.0 >= 0.0 && thumb.0 + thumb.2 <= POPUP_VIEWPORT.0,
                "slot {n}: thumb {thumb:?} leaves the popup width"
            );
            assert!(
                !overlaps(value, thumb),
                "slot {n} ({percent}%): percent label {value:?} intersects thumb {thumb:?}"
            );
            assert!(
                !overlaps(value, label),
                "slot {n}: value label {value:?} intersects name label {label:?}"
            );
            assert!(
                inside(mute, group),
                "slot {n}: mute button {mute:?} is outside row {group:?}"
            );
        }
    }
}

#[cfg(test)]
mod audio_list_css_tests {
    /// The device list must keep the template's 336px height: a stale shared `max-height`
    /// would clip the third row and leave a dead band above the footer.
    #[test]
    fn audio_list_is_not_capped_below_its_template_height() {
        let css = include_str!("../../../../ui/shell/flamewm.css");
        let start = css.find(".audio-list {").expect("audio-list rule");
        let body = &css[start..start + css[start..].find('}').expect("rule end")];
        assert!(!body.contains("max-height"), "{body}");
        assert!(include_str!("../../../../ui/shell/audio.html")
            .contains(".audio-list { position:relative; height:336px;"));
    }
}

#[cfg(test)]
mod audio_render_tests {
    //! Headless pixels of the real audio popup. The compiled template is projected, laid out and
    //! painted through the software chain (`PaintCommand` -> glyph masks -> `PixelOp` -> RGBA8),
    //! so the slider is checked on painted pixels, not only on layout boxes. Set
    //! `FLAMEWM_AUDIO_SHOT_DIR` to also dump each step as a raw RGBA8 frame.
    use super::*;
    use flamewm_api::system::{AudioEndpointKind, AudioEndpointSnapshot, ServiceAvailability};
    use flamewm_shell_core::status::{AudioSettings, AudioTab};

    const AUDIO_ARTIFACT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-audio.rwr"));
    const VIEWPORT: (f32, f32) = (459.0, 460.0);
    /// Opaque panel-like backdrop the popup is composited over.
    const BACKDROP: [u8; 4] = [30, 33, 35, 255];

    fn output_snapshot(percent: u8) -> crate::ShellSnapshot {
        let mut snapshot = crate::ShellSnapshot::default();
        snapshot.system.audio.availability = ServiceAvailability::Available;
        snapshot.system.audio.endpoints = vec![AudioEndpointSnapshot {
            id: 1,
            kind: AudioEndpointKind::Sink,
            name: "speakers".to_owned(),
            description: "Built-in Speakers".to_owned(),
            volume_percent: percent,
            muted: false,
            is_default: true,
        }];
        snapshot
    }

    /// The Devices tab projected at `percent` with the 100% cap (no raise-maximum).
    fn popup(percent: u8) -> flamewm_ui_x11::UiDocument {
        let mut document =
            flamewm_ui_x11::decode_document(AUDIO_ARTIFACT).expect("audio artifact must decode");
        project_audio_full(
            &mut document,
            &output_snapshot(percent),
            AudioTab::Devices,
            AudioSettings {
                raise_maximum: false,
            },
            0,
        )
        .expect("every projected audio id must be a valid target");
        document
    }

    fn pixel(frame: &[u8], x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * VIEWPORT.0 as usize + x as usize) * 4;
        [frame[i], frame[i + 1], frame[i + 2], frame[i + 3]]
    }

    /// Bright enough to be the white fill or knob rather than the dim track.
    fn is_fill(pixel: [u8; 4]) -> bool {
        pixel[0] > 200 && pixel[1] > 200 && pixel[2] > 200
    }

    fn rect_of(document: &flamewm_ui_x11::UiDocument, id: &str) -> (f32, f32, f32, f32) {
        document
            .layout_box_by_id(id, VIEWPORT)
            .unwrap_or_else(|| panic!("no layout box for {id}"))
    }

    #[test]
    fn slider_paints_pill_track_fill_and_knob_at_every_step() {
        let out_dir = std::env::var_os("FLAMEWM_AUDIO_SHOT_DIR").map(std::path::PathBuf::from);
        for percent in [0u8, 25, 50, 75, 100] {
            let document = popup(percent);
            let track = rect_of(&document, "audio-slot-1-track");
            let fill = rect_of(&document, "audio-slot-1-fill");
            let knob = rect_of(&document, "audio-slot-1-thumb");
            let frame =
                flamewm_ui_x11::test_support::paint_popup_software(document, VIEWPORT, BACKDROP);
            if let Some(dir) = &out_dir {
                std::fs::write(dir.join(format!("audio-{percent:03}.rgba")), &frame)
                    .expect("write shot");
            }
            let mid_y = (track.1 + track.3 / 2.0) as u32;

            // Fill and track share a height: the fill column is white for exactly the track rows.
            if percent > 0 {
                let column = (fill.0 + fill.2 / 2.0) as u32;
                let top = track.1 as u32;
                let rows = (top..top + track.3 as u32)
                    .filter(|&y| is_fill(pixel(&frame, column, y)))
                    .count();
                assert_eq!(
                    rows, track.3 as usize,
                    "{percent}%: fill rows must match track height"
                );
                assert!(
                    !is_fill(pixel(&frame, column, top - 2)),
                    "{percent}%: fill leaks above track"
                );
                assert!(
                    !is_fill(pixel(&frame, column, top + track.3 as u32 + 1)),
                    "{percent}%: fill leaks below track"
                );
            } else {
                assert!(
                    !is_fill(pixel(&frame, (track.0 + track.2 / 2.0) as u32, mid_y)),
                    "0%: track must be unfilled"
                );
            }

            // The knob is white at its centre and stays inside the track at every step.
            let knob_centre = (knob.0 + knob.2 / 2.0, knob.1 + knob.3 / 2.0);
            assert!(
                is_fill(pixel(&frame, knob_centre.0 as u32, knob_centre.1 as u32)),
                "{percent}%: knob must paint white at its centre"
            );
            assert!(
                knob.0 >= track.0 - 1.0 && knob.0 + knob.2 <= track.0 + track.2 + 1.0,
                "{percent}%: knob {knob:?} leaves track {track:?}"
            );
            let expected_x = track.0
                + AUDIO_THUMB_SIZE / 2.0
                + (track.2 - AUDIO_THUMB_SIZE) * f32::from(percent) / 100.0;
            assert!(
                (knob_centre.0 - expected_x).abs() <= 1.0,
                "{percent}%: knob centre {} != inset expectation {expected_x}",
                knob_centre.0
            );
        }
    }
}

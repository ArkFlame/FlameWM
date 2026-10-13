use super::*;

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

/// Full audio projection: Devices/Applications tabs, RangeSpec slider state,
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
    let view = crate::taskbar::status::audio::project(&snapshot.system);
    let popover = crate::taskbar::status::audio::popover_view(&snapshot.system);
    let details = if snapshot.system.audio.availability == ServiceAvailability::Unavailable {
        "Audio unavailable".to_owned()
    } else {
        popover
            .as_ref()
            .map(|popover| popover.popover.details.clone())
            .unwrap_or_else(|| view.label.clone())
    };
    document.text("audio-details", details)?;
    let devices = tab == flamewm_shell_core::status::AudioTab::Devices;
    document.visible("audio-tab-devices-bg", devices)?;
    document.visible("audio-tab-apps-bg", !devices)?;
    // Rows come from the tab-filtered, scroll-windowed list shared with host routing.
    let rows =
        crate::taskbar::status::audio::visible_rows(&snapshot.system.audio, tab, scroll_offset);
    document.visible("audio-empty", rows.is_empty())?;
    document.text(
        "audio-empty",
        if rows.is_empty() {
            if devices {
                "No audio devices"
            } else {
                "No active applications"
            }
            .to_owned()
        } else {
            String::new()
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
        document.text(&format!("audio-slot-{n}-label"), row.label.clone())?;
        document.text(
            &format!("audio-slot-{n}-value"),
            format!("{}%", row.volume_percent),
        )?;
        document.text(
            &format!("audio-slot-{n}-mute"),
            if row.muted { "Unmute" } else { "Mute" }.to_owned(),
        )?;
        document.size(
            &format!("audio-slot-{n}-fill"),
            crate::taskbar::status::audio::AUDIO_TRACK_WIDTH * shown / spec.max,
            6.0,
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
    Ok(())
}


#[cfg(test)]
mod audio_template_tests {
    use super::*;
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
        toggles: std::collections::HashMap<String, bool>,
    }

    struct AudioRecorder<'a> {
        document: &'a mut flamewm_ui_x11::UiDocument,
        recorded: Recorded,
    }

    macro_rules! forward {
        ($($method:ident($($arg:ident: $ty:ty),*)),* $(,)?) => {
            $(fn $method(&mut self, $($arg: $ty),*) -> Result<(), String> {
                UiDocumentAccess::$method(self.document, $($arg),*)
            })*
        };
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
        fn toggle(&mut self, track_id: &str, knob_id: &str, checked: bool) -> Result<(), String> {
            self.recorded.toggles.insert(track_id.to_owned(), checked);
            UiDocumentAccess::toggle(self.document, track_id, knob_id, checked)
        }
        forward!(
            image_rgb8(id: &str, image: RuntimeImage),
            image_rgba8(id: &str, image: RuntimeImage),
            position(id: &str, left: f32, top: f32),
            background(id: &str, color: UiColor),
            border(id: &str, color: UiColor),
            foreground(id: &str, color: UiColor),
            background_clear(id: &str),
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
            },
            AudioStreamSnapshot {
                id: 11,
                endpoint_id: 1,
                name: "Spotify".to_owned(),
                volume_percent: 25,
                muted: true,
            },
        ];
        let settings = AudioSettings { raise_maximum };
        let mut document =
            flamewm_ui_x11::decode_document(AUDIO_ARTIFACT).expect("audio artifact must decode");
        let mut recorder = AudioRecorder {
            document: &mut document,
            recorded: Recorded::default(),
        };
        project_audio_full(&mut recorder, &snapshot, tab, settings, 0)
            .expect("every projected audio id must be a valid target");
        recorder.recorded
    }

    fn text<'a>(recorded: &'a Recorded, id: &str) -> &'a str {
        recorded
            .texts
            .get(id)
            .map(String::as_str)
            .unwrap_or("<unset>")
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
        assert_eq!(recorded.widths.get("audio-slot-2-fill"), Some(&140.0));
        assert_eq!(text(&recorded, "audio-slot-2-mute"), "Unmute");
        assert_eq!(text(&recorded, "audio-slot-1-mute"), "Mute");
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
        assert_eq!(recorded.widths.get("audio-slot-1-fill"), Some(&280.0));
        let headphones = recorded
            .widths
            .get("audio-slot-2-fill")
            .copied()
            .unwrap_or(-1.0);
        assert!(
            (headphones - 350.0 * 40.0 / 150.0).abs() < 0.01,
            "got {headphones}"
        );
        assert_eq!(recorded.toggles.get("audio-raise-track"), Some(&true));
    }

    #[test]
    fn applications_tab_projects_playback_streams_with_app_icons() {
        let recorded = fixture(AudioTab::Applications, false);
        assert_eq!(text(&recorded, "audio-slot-1-label"), "Firefox");
        assert_eq!(text(&recorded, "audio-slot-2-label"), "Spotify");
        assert_eq!(text(&recorded, "audio-slot-2-mute"), "Unmute");
        assert_eq!(recorded.visible.get("audio-slot-1-icon-app"), Some(&true));
        assert_eq!(recorded.visible.get("audio-slot-1-icon-out"), Some(&false));
        assert_eq!(recorded.visible.get("audio-slot-3"), Some(&false));
        assert_eq!(recorded.visible.get("audio-tab-apps-bg"), Some(&true));
        assert_eq!(recorded.visible.get("audio-tab-devices-bg"), Some(&false));
    }

    #[test]
    fn applications_tab_without_streams_shows_empty_state() {
        let mut snapshot = crate::ShellSnapshot::default();
        snapshot.system.audio.availability = ServiceAvailability::Available;
        let mut document =
            flamewm_ui_x11::decode_document(AUDIO_ARTIFACT).expect("audio artifact must decode");
        let mut recorder = AudioRecorder {
            document: &mut document,
            recorded: Recorded::default(),
        };
        project_audio_full(
            &mut recorder,
            &snapshot,
            AudioTab::Applications,
            AudioSettings::default(),
            0,
        )
        .expect("empty applications projection must succeed");
        assert_eq!(recorder.recorded.visible.get("audio-empty"), Some(&true));
        assert_eq!(
            text(&recorder.recorded, "audio-empty"),
            "No active applications"
        );
    }
}

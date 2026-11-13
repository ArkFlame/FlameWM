use super::*;

/// `audio.slot.<n>.<command>` -> (zero-based slot, command).
/// Wheel `dy` (positive = down) moves the audio row list by one whole row,
/// clamped to the rows beyond the popup viewport.
pub(crate) fn scroll_audio_rows(snapshot: &mut QuickControlSnapshot, dy: f32) {
    use crate::taskbar::status::audio::{AUDIO_POPUP_ROWS, AUDIO_ROW_HEIGHT};
    if dy == 0.0 {
        return;
    }
    let total =
        crate::taskbar::status::audio::tab_rows(&snapshot.system.audio, snapshot.audio_tab).len();
    let max_rows = total.saturating_sub(AUDIO_POPUP_ROWS);
    let row = (snapshot.audio_scroll_offset / AUDIO_ROW_HEIGHT) as usize;
    let next = if dy > 0.0 {
        (row + 1).min(max_rows)
    } else {
        row.saturating_sub(1)
    };
    snapshot.audio_scroll_offset = (next as u32) * AUDIO_ROW_HEIGHT;
}

pub(crate) fn audio_slot_command(action: &str) -> Option<(usize, &str)> {
    let (slot_text, command) = action.strip_prefix("audio.slot.")?.rsplit_once('.')?;
    let slot = slot_text.parse::<usize>().ok()?.checked_sub(1)?;
    Some((slot, command))
}

/// Keyboard value for one slider (WAI-ARIA APG slider): Up/Right +1 step, Down/Left -1 step,
/// `Page_Up`/`Page_Down` +/-10 steps, Home/End min/max. `None` for any other key.
pub(crate) fn slider_key_value(key: &str, spec: flamewm_ui_core::range::RangeSpec) -> Option<f32> {
    let value = match key {
        "Up" | "Right" => spec.value + spec.step,
        "Down" | "Left" => spec.value - spec.step,
        "Page_Up" => spec.value + 10.0 * spec.step,
        "Page_Down" => spec.value - 10.0 * spec.step,
        "Home" => spec.min,
        "End" => spec.max,
        _ => return None,
    };
    Some(spec.clamp_value(value))
}

/// Visible audio rows on the card, one focus stop each.
pub(crate) fn audio_row_count(snapshot: &QuickControlSnapshot) -> usize {
    crate::taskbar::status::audio::visible_rows(
        &snapshot.system.audio,
        snapshot.audio_tab,
        snapshot.audio_scroll_offset,
    )
    .len()
}

/// Focus stops in Tab order: one per visible row, then the raise-maximum toggle.
pub(crate) fn audio_stop_count(rows: usize) -> usize {
    rows + 1
}

/// Space or Enter on the focused raise-maximum toggle (the stop after `rows`) flips it once.
/// Precedent: WAI-ARIA APG switch (Space and Enter toggle); the Tab walk is `focus_keys`.
pub(crate) fn audio_raise_activation(
    text: Option<&str>,
    focus: Option<usize>,
    rows: usize,
) -> bool {
    focus == Some(rows) && matches!(text, Some("\n" | "\r" | " "))
}

/// Tab moves focus forward, Shift+Tab backward, wrapping over `count` visible rows.
pub(crate) fn next_audio_focus(focus: Option<usize>, count: usize, backward: bool) -> usize {
    match focus {
        None => 0,
        Some(slot) if backward => (slot + count - 1) % count,
        Some(slot) => (slot + 1) % count,
    }
}

/// Typed key while the audio popup is open. Tab moves focus; the first slider key on an
/// unfocused list only focuses row 0; otherwise the focused row's slider moves one key step.
pub(crate) fn audio_key_request(
    snapshot: &mut QuickControlSnapshot,
    payload: &str,
) -> Option<ControlRequest> {
    let (modifiers, key) = flamewm_shell::launcher::parse_typed_key(payload)?;
    let rows = crate::taskbar::status::audio::visible_rows(
        &snapshot.system.audio,
        snapshot.audio_tab,
        snapshot.audio_scroll_offset,
    );
    let count = rows.len();
    let stops = audio_stop_count(count);
    let focus = snapshot.audio_focus_slot.filter(|&slot| slot < stops);
    if key == "Tab" {
        let backward = modifiers.split('+').any(|modifier| modifier == "Shift");
        snapshot.audio_focus_slot = Some(next_audio_focus(focus, stops, backward));
        return None;
    }
    let Some(slot) = focus else {
        snapshot.audio_focus_slot = Some(0);
        return None;
    };
    // The raise-maximum toggle stop takes Space or Enter (see `audio_raise_activation`), not a step.
    if slot == count {
        return None;
    }
    let row = rows.into_iter().nth(slot)?;
    let base = key_base(
        snapshot.audio_key_base.as_ref(),
        &row.id,
        row.volume_percent,
    );
    let spec = snapshot.audio_settings.slider_spec(base);
    let percent = slider_key_value(key, spec)?.round() as u8;
    let request = crate::taskbar::status::audio::system_action_for_row(
        &snapshot.system,
        &row.id,
        Some(percent),
        None,
    )?;
    // The projected row lags the request by one helper round trip; the next key steps from here.
    snapshot.audio_key_base = Some((row.id, percent));
    Some(request)
}

/// Value a key press steps from: the last keyed target for the same row, else the projected
/// volume. Repeats before the helper publishes the new level must not re-apply the same step.
pub(crate) fn key_base(last: Option<&(String, u8)>, row_id: &str, projected: u8) -> u8 {
    match last {
        Some((id, percent)) if id == row_id => *percent,
        _ => projected,
    }
}

/// Pointer x on the slider track -> snapped percent under the current cap.
pub(crate) fn audio_pointer_request(
    snapshot: &QuickControlSnapshot,
    slot: usize,
    x: f32,
) -> Option<ControlRequest> {
    use crate::taskbar::status::audio::{AUDIO_THUMB_SIZE, AUDIO_TRACK_WIDTH, AUDIO_TRACK_X};
    let row = crate::taskbar::status::audio::visible_rows(
        &snapshot.system.audio,
        snapshot.audio_tab,
        snapshot.audio_scroll_offset,
    )
    .into_iter()
    .nth(slot)?;
    let spec = snapshot.audio_settings.slider_spec(row.volume_percent);
    // The knob centre travels inside the track: pointer at knob centre reads back its value.
    let value = flamewm_ui_core::range::RangeState::value_from_pointer(
        spec,
        AUDIO_TRACK_X + AUDIO_THUMB_SIZE / 2.0,
        AUDIO_TRACK_WIDTH - AUDIO_THUMB_SIZE,
        x,
    );
    crate::taskbar::status::audio::system_action_for_row(
        &snapshot.system,
        &row.id,
        Some(value.round() as u8),
        None,
    )
}

pub(crate) fn helper_control_request(
    action: &str,
    snapshot: &QuickControlSnapshot,
) -> Option<ControlRequest> {
    let system = &snapshot.system;
    if let Some(slot) = action
        .strip_prefix("vpn.toggle.")
        .and_then(|text| text.parse::<usize>().ok())
    {
        return crate::projection::vpn_toggle_action(&system.network, slot).map(|action| {
            ControlRequest::SystemAction {
                action,
                expected_revision: system.revision,
            }
        });
    }
    if action == "bluetooth.trust.toggle" {
        return crate::projection::bluetooth_trust_action(&system.bluetooth).map(|action| {
            ControlRequest::SystemAction {
                action,
                expected_revision: system.revision,
            }
        });
    }
    if action == "power.profile.cycle" {
        return crate::projection::power_profile_cycle_action(&system.power_profiles).map(
            |action| ControlRequest::SystemAction {
                action,
                expected_revision: system.revision,
            },
        );
    }
    if matches!(action, "brightness.down" | "brightness.up") {
        return crate::projection::brightness_step_action(
            &system.brightness,
            action == "brightness.up",
        )
        .map(|action| ControlRequest::SystemAction {
            action,
            expected_revision: system.revision,
        });
    }
    if action == "network.wifi.toggle" {
        return Some(ControlRequest::SystemAction {
            action: flamewm_api::system::SystemAction::SetWifiEnabled(!system.network.wifi_enabled),
            expected_revision: system.revision,
        });
    }
    if action == "network.scan" {
        return crate::taskbar::status::network::system_action_for_path(system, "", true, false);
    }
    if action == "network.disconnect" {
        return crate::taskbar::status::network::system_action_for_path(system, "", false, true);
    }
    if let Some(slot) = action
        .strip_prefix("network.connect.")
        .and_then(|value| value.parse::<usize>().ok())
        .and_then(|slot| slot.checked_sub(1))
    {
        let path = crate::taskbar::status::network::project_visible_rows(
            system,
            &snapshot.network_query,
            snapshot.network_scroll_offset,
        )
        .get(slot)
        .map(|row| row.path.clone())?;
        return crate::taskbar::status::network::system_action_for_path(
            system, &path, false, false,
        );
    }
    let row = if matches!(action, "volume.mute" | "volume.down" | "volume.up") {
        crate::taskbar::status::audio::default_endpoint_row(&system.audio)
    } else if matches!(action, "input.mute" | "input.down" | "input.up") {
        crate::taskbar::status::audio::default_source_row(&system.audio)
    } else if let Some((kind_slot, _)) = action
        .strip_prefix("audio.")
        .and_then(|value| value.rsplit_once('.'))
    {
        let (_, slot) = kind_slot.rsplit_once('.')?;
        let slot = slot.parse::<usize>().ok()?.checked_sub(1)?;
        crate::taskbar::status::audio::visible_rows(
            &system.audio,
            snapshot.audio_tab,
            snapshot.audio_scroll_offset,
        )
        .into_iter()
        .nth(slot)
    } else {
        None
    }?;
    let command = action.rsplit('.').next()?;
    let cap = snapshot.audio_settings.max_percent();
    let (percent, muted) = match command {
        "up" => (
            Some(
                snapshot
                    .audio_settings
                    .clamp_percent(row.volume_percent.saturating_add(5)),
            ),
            None,
        ),
        "down" => (Some(row.volume_percent.saturating_sub(5).min(cap)), None),
        "mute" => (None, Some(!row.muted)),
        _ => return None,
    };
    crate::taskbar::status::audio::system_action_for_row(system, &row.id, percent, muted)
}

#[cfg(test)]
mod slider_key_tests {
    use super::*;
    use flamewm_ui_core::range::RangeSpec;

    fn spec(value: f32, max: f32) -> RangeSpec {
        RangeSpec::new(0.0, max, 1.0, value)
    }

    #[test]
    fn arrows_step_one_and_clamp_at_the_ends() {
        assert_eq!(slider_key_value("Up", spec(50.0, 100.0)), Some(51.0));
        assert_eq!(slider_key_value("Right", spec(50.0, 100.0)), Some(51.0));
        assert_eq!(slider_key_value("Down", spec(50.0, 100.0)), Some(49.0));
        assert_eq!(slider_key_value("Left", spec(50.0, 100.0)), Some(49.0));
        assert_eq!(slider_key_value("Up", spec(100.0, 100.0)), Some(100.0));
        assert_eq!(slider_key_value("Down", spec(0.0, 100.0)), Some(0.0));
    }

    #[test]
    fn paging_moves_ten_steps_and_home_end_reach_the_range() {
        assert_eq!(slider_key_value("Page_Up", spec(50.0, 100.0)), Some(60.0));
        assert_eq!(slider_key_value("Page_Down", spec(50.0, 100.0)), Some(40.0));
        assert_eq!(slider_key_value("Page_Up", spec(95.0, 100.0)), Some(100.0));
        assert_eq!(slider_key_value("Home", spec(50.0, 100.0)), Some(0.0));
        assert_eq!(slider_key_value("End", spec(50.0, 150.0)), Some(150.0));
    }

    #[test]
    fn other_keys_do_not_move_a_slider() {
        assert_eq!(slider_key_value("Return", spec(50.0, 100.0)), None);
        assert_eq!(slider_key_value("a", spec(50.0, 100.0)), None);
    }

    #[test]
    fn repeats_step_from_the_last_keyed_target_until_the_row_publishes() {
        // Stale projection (50) with a pending keyed target (52) on the same row: step from 52.
        let last = ("endpoint:sink:1".to_owned(), 52);
        assert_eq!(key_base(Some(&last), "endpoint:sink:1", 50), 52);
        // Another row, or no pending target, falls back to the projected volume.
        assert_eq!(key_base(Some(&last), "endpoint:sink:2", 30), 30);
        assert_eq!(key_base(None, "endpoint:sink:1", 50), 50);
    }

    #[test]
    fn tab_wraps_forward_and_backward_and_starts_at_row_zero() {
        assert_eq!(next_audio_focus(None, 3, false), 0);
        assert_eq!(next_audio_focus(Some(0), 3, false), 1);
        assert_eq!(next_audio_focus(Some(2), 3, false), 0);
        assert_eq!(next_audio_focus(Some(0), 3, true), 2);
    }

    // OBSERVABLE: the raise-maximum switch is the stop after the visible rows; Tab reaches it and
    // wraps. MUTATION: dropping the extra stop, or counting it as a row, fails here.
    #[test]
    fn raise_switch_is_the_stop_after_the_rows_and_takes_space_or_enter() {
        assert_eq!(audio_stop_count(3), 4);
        assert_eq!(next_audio_focus(Some(2), audio_stop_count(3), false), 3);
        assert_eq!(next_audio_focus(Some(3), audio_stop_count(3), false), 0);
        for text in ["\n", "\r", " "] {
            assert!(audio_raise_activation(Some(text), Some(3), 3));
            assert!(!audio_raise_activation(Some(text), Some(2), 3));
            assert!(!audio_raise_activation(Some(text), None, 3));
        }
        assert!(!audio_raise_activation(Some("x"), Some(3), 3));
    }

    // REGRESSION: the switch id exists in audio.html and its rule keeps a 1px border box, or the
    // focus ring never paints.
    #[test]
    fn audio_raise_focus_id_exists_in_html_with_a_ring_border_box() {
        let html = include_str!("../../../../../ui/shell/audio.html");
        assert!(html.contains("id=\"audio-raise-track\""));
        assert!(html.contains(".audio-raise-track { position:relative; width:39px; height:19px; box-sizing:border-box; border:1px solid"));
    }
}

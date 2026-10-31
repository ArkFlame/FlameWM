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

/// Pointer x on the slider track -> snapped percent under the current cap.
pub(crate) fn audio_pointer_request(
    snapshot: &QuickControlSnapshot,
    slot: usize,
    x: f32,
) -> Option<ControlRequest> {
    use crate::taskbar::status::audio::{AUDIO_TRACK_WIDTH, AUDIO_TRACK_X};
    let row = crate::taskbar::status::audio::visible_rows(
        &snapshot.system.audio,
        snapshot.audio_tab,
        snapshot.audio_scroll_offset,
    )
    .into_iter()
    .nth(slot)?;
    let spec = snapshot.audio_settings.slider_spec(row.volume_percent);
    let value = flamewm_ui_core::range::RangeState::value_from_pointer(
        spec,
        AUDIO_TRACK_X,
        AUDIO_TRACK_WIDTH,
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

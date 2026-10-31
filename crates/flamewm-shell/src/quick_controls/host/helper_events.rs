use super::*;

/// Route one helper surface event: Escape/outside closes the helper popup,
/// typed audio/network actions forward via the existing dispatcher
/// contract. Events inside the active popup or its submodules are kept;
/// a release outside the active popup closes it on the same turn.
/// Returns true when the loop should keep running.
pub fn route_helper_event(
    event: &flamewm_ui_x11::SurfaceEvent,
    runtime: &mut SurfaceRuntime,
    surfaces: &mut HelperSurfaces,
    snapshot: &mut QuickControlSnapshot,
    dispatcher: Option<&std::sync::Arc<flamewm_control_dbus::MutationDispatcher>>,
    reprojection: &mut bool,
) -> bool {
    sync_network_secret(snapshot);
    let action = event.action.action.as_str();
    if action == "network.secret.focus" {
        snapshot.network_secret_focused = true;
        snapshot.network_filter_focused = false;
        snapshot.weather_search.focused = false;
    }
    if action == "network.filter.focus" {
        snapshot.network_filter_focused = true;
        snapshot.network_secret_focused = false;
        snapshot.weather_search.focused = false;
    }
    if action == "weather.location.focus" {
        snapshot.weather_search.focused = true;
        snapshot.network_filter_focused = false;
        snapshot.network_secret_focused = false;
    }
    if action == "keyboard.input" && event.action.phase == ActionPhase::Release {
        if let Some(text) = event.action.text.as_deref() {
            if text == "\u{1b}" {
                surfaces.close(runtime);
                return true;
            }
            if snapshot.weather_search.focused {
                match text {
                    "\u{8}" | "\u{7f}" => {
                        snapshot.weather_search.query.pop();
                    }
                    "\r" | "\n" => {
                        snapshot.weather_search.submit(spawn_location_search);
                    }
                    other => snapshot.weather_search.query.push_str(other),
                }
                *reprojection = true;
            }
            if text == "\u{8}" || text == "\u{7f}" {
                if snapshot.network_secret_focused {
                    snapshot.network_secret.pop();
                    snapshot.network_secret_masked_len = snapshot.network_secret.chars().count();
                    *reprojection = true;
                } else if snapshot.network_filter_focused {
                    snapshot.network_query.filter.pop();
                    snapshot.network_scroll_offset = 0;
                    *reprojection = true;
                }
            } else if text == "\r" || text == "\n" {
                if snapshot.network_secret_focused {
                    *reprojection = true;
                    return submit_network_secret(snapshot, dispatcher);
                }
            } else if snapshot.network_secret_focused {
                snapshot.network_secret.push_str(text);
                snapshot.network_secret_masked_len = snapshot.network_secret.chars().count();
                snapshot.network_secret_error = false;
                *reprojection = true;
            } else if snapshot.network_filter_focused {
                snapshot.network_query.filter.push_str(text);
                snapshot.network_scroll_offset = 0;
                *reprojection = true;
            }
        }
    }
    let open_surface = surfaces.open.and_then(|kind| surfaces.handle(kind));
    let inside_active = match open_surface {
        Some(handle) => event.surface == handle || event.action.inside,
        None => event.action.inside,
    };
    let outside = event.action.phase == ActionPhase::Release
        && (action == OUTSIDE_RELEASE_ACTION || (surfaces.open.is_some() && !inside_active));
    if outside {
        snapshot.audio_drag_slot = None;
        surfaces.close(runtime);
        return true;
    }
    if surfaces.open.is_none() {
        return true;
    }
    if action == "scroll" && surfaces.open == Some(QuickControlKind::Audio) {
        if let Some(dy) = event
            .action
            .text
            .as_deref()
            .and_then(|text| text.split_once(','))
            .and_then(|(_, dy)| dy.parse::<f32>().ok())
        {
            scroll_audio_rows(snapshot, dy);
        }
        return true;
    }
    // A drag owns the pointer until release: motion keeps routing to the grabbed
    // slot even when the pointer leaves its track; release or close ends it.
    match event.action.phase {
        ActionPhase::Press => {
            if let Some((slot, "set")) = audio_slot_command(action) {
                snapshot.audio_drag_slot = Some(slot);
                if let Some(request) = audio_pointer_request(snapshot, slot, event.action.x) {
                    crate::control_actions::enqueue_action(dispatcher, request);
                }
            }
        }
        ActionPhase::Motion => {
            if let Some(slot) = snapshot.audio_drag_slot {
                if let Some(request) = audio_pointer_request(snapshot, slot, event.action.x) {
                    crate::control_actions::enqueue_action(dispatcher, request);
                }
            }
        }
        ActionPhase::Release => snapshot.audio_drag_slot = None,
        ActionPhase::Hover => {}
    }
    if event.action.phase == ActionPhase::Release && event.action.inside {
        match action {
            "audio.back" => surfaces.close(runtime),
            "audio.tab.devices" | "audio.tab.apps" => {
                if select_audio_tab(snapshot, action) {
                    *reprojection = true;
                }
            }
            "audio.raise.toggle" => {
                snapshot.audio_settings.raise_maximum = !snapshot.audio_settings.raise_maximum;
                if let Some(dispatcher) = dispatcher {
                    let request =
                        ControlRequest::ApplySettings(flamewm_api::settings::SettingsTransaction {
                            expected_revision: snapshot.settings.revision,
                            changes: vec![flamewm_api::settings::SettingsChange {
                                key: AudioSettings::SETTINGS_KEY.to_owned(),
                                value: Some(SettingValue::Boolean(
                                    snapshot.audio_settings.raise_maximum,
                                )),
                            }],
                            reset_section: None,
                        });
                    crate::control_actions::enqueue_action(Some(dispatcher), request);
                }
            }
            "network.filter.clear" => snapshot.network_query.filter.clear(),
            "network.filter.focus" | "network.secret.focus" | "weather.location.focus" => {}
            value if value.starts_with("weather.location.pick.") => {
                if let Ok(index) = value["weather.location.pick.".len()..].parse::<usize>() {
                    pick_weather_location(snapshot, index, dispatcher);
                }
                *reprojection = true;
            }
            "network.secret.cancel" => {
                if let Some((request_id, generation)) = snapshot.network_secret_request {
                    let request = ControlRequest::SystemAction {
                        action: flamewm_api::system::SystemAction::CancelNetworkSecret {
                            request_id,
                            generation,
                        },
                        expected_revision: snapshot.system.revision,
                    };
                    crate::control_actions::enqueue_action(dispatcher, request);
                }
                clear_network_secret(snapshot);
                return true;
            }
            "network.disconnect" => {
                clear_network_secret(snapshot);
                if let Some(request) = helper_control_request(action, snapshot) {
                    crate::control_actions::enqueue_action(dispatcher, request);
                }
                return true;
            }
            "network.secret.submit" => {
                *reprojection = true;
                return submit_network_secret(snapshot, dispatcher);
            }
            value if value.starts_with("network.filter.") => {
                value["network.filter.".len()..].clone_into(&mut snapshot.network_query.filter);
                snapshot.network_scroll_offset = 0;
            }
            value if value.starts_with("calendar.") => {
                snapshot
                    .calendar
                    .apply(value, crate::projection::local_date());
            }
            _ => {}
        }
        if let Some(request) = helper_control_request(action, snapshot) {
            crate::control_actions::enqueue_action(dispatcher, request);
        }
    }
    true
}

use super::*;

pub(crate) fn clear_network_secret(snapshot: &mut QuickControlSnapshot) {
    snapshot.network_secret.clear();
    snapshot.network_secret_masked_len = 0;
    snapshot.network_secret_request = None;
    snapshot.network_secret_submit_id = None;
    snapshot.network_secret_error = false;
}

/// Persists the chosen location, enables weather, and turns automatic location off so
/// the IP lookup cannot overwrite the manual pick. The popup shows Loading only after
/// the settings transaction is accepted (see [`finish_weather_pick`]).
pub(crate) fn pick_weather_location(
    snapshot: &mut QuickControlSnapshot,
    index: usize,
    dispatcher: Option<&std::sync::Arc<flamewm_control_dbus::MutationDispatcher>>,
) {
    let Some(pick) = weather_pick_for(snapshot, index) else {
        return;
    };
    snapshot.weather_search.reset();
    if crate::control_actions::enqueue_weather_pick(dispatcher, pick).is_none() {
        snapshot.weather_search.status = SearchStatus::SaveFailed;
    }
}

/// The settings transaction for picking result `index`, fenced on the settings revision
/// (the store's revision), not the system snapshot revision.
pub(crate) fn weather_pick_for(
    snapshot: &QuickControlSnapshot,
    index: usize,
) -> Option<crate::control_actions::WeatherPick> {
    let location = snapshot.weather_search.results.get(index).cloned()?;
    let mut settings = snapshot.weather.settings.clone();
    settings.location = Some(location);
    settings.enabled = true;
    Some(weather_pick(settings, snapshot.settings.revision, 0))
}

pub(crate) fn weather_pick(
    settings: flamewm_shell_core::weather::WeatherSettings,
    revision: u64,
    attempt: u8,
) -> crate::control_actions::WeatherPick {
    let mut changes = settings.to_changes();
    changes.push(flamewm_api::settings::SettingsChange {
        key: crate::weather_auto::AUTO_LOCATION_KEY.to_owned(),
        value: Some(SettingValue::Boolean(false)),
    });
    crate::control_actions::WeatherPick {
        request: ControlRequest::ApplySettings(flamewm_api::settings::SettingsTransaction {
            expected_revision: revision,
            changes,
            reset_section: None,
        }),
        settings,
        attempt,
    }
}

/// Applies one settled weather pick. A stale revision is resubmitted once against the
/// freshly read settings revision; any other failure is shown in the picker.
pub(crate) fn finish_weather_pick(
    state: &mut HostState,
    outcome: crate::control_actions::WeatherPickOutcome,
) {
    match outcome.result {
        Ok(applied) => {
            if let Some(settings) = applied {
                state.snapshot.store_settings(settings);
            }
            state.snapshot.weather.settings = outcome.pick.settings.clone();
            state.snapshot.weather.state = if outcome.pick.settings.enabled {
                flamewm_shell_core::weather::WeatherState::Loading
            } else {
                flamewm_shell_core::weather::WeatherState::Disabled
            };
            state.system_reprojection_pending = true;
        }
        Err(flamewm_api::error::ErrorCode::StaleRevision) if outcome.pick.attempt == 0 => {
            if let Ok(client) = ControlClient::connect(BusKind::Session) {
                state.snapshot.refresh_settings(&client);
            }
            let revision = state.snapshot.settings.revision;
            let retry = weather_pick(outcome.pick.settings, revision, 1);
            let retried =
                crate::control_actions::enqueue_weather_pick(state.dispatcher.as_ref(), retry);
            if retried.is_none() {
                state.snapshot.weather_search.status = SearchStatus::SaveFailed;
                state.system_reprojection_pending = true;
            }
        }
        Err(_) => {
            state.snapshot.weather_search.status = SearchStatus::SaveFailed;
            state.system_reprojection_pending = true;
        }
    }
}

pub(crate) fn submit_network_secret(
    snapshot: &mut QuickControlSnapshot,
    dispatcher: Option<&std::sync::Arc<flamewm_control_dbus::MutationDispatcher>>,
) -> bool {
    let Some(identity) = snapshot.network_secret_request else {
        return true;
    };
    let Some(request) =
        network_secret_submit_request(identity, &snapshot.network_secret, snapshot.system.revision)
    else {
        // Empty secret: keep the prompt and report it; never submit.
        snapshot.network_secret_error = true;
        return true;
    };
    snapshot.network_secret_submit_id =
        crate::control_actions::enqueue_network_secret(dispatcher, identity, request);
    if snapshot.network_secret_submit_id.is_none() {
        snapshot.network_secret_error = true;
    }
    true
}

/// Builds the `SecretAgent` submit for the pending identity. An empty password is
/// never a valid secured-AP credential, so it yields no request.
pub(crate) fn network_secret_submit_request(
    identity: (u64, u64),
    secret: &str,
    expected_revision: u64,
) -> Option<ControlRequest> {
    if secret.is_empty() {
        return None;
    }
    let (request_id, generation) = identity;
    Some(ControlRequest::SystemAction {
        action: flamewm_api::system::SystemAction::SubmitNetworkSecret {
            request_id,
            generation,
            secret: secret.to_owned(),
        },
        expected_revision,
    })
}

pub(crate) fn sync_network_secret(snapshot: &mut QuickControlSnapshot) {
    let key = snapshot
        .system
        .network
        .pending_secret
        .as_ref()
        .map(|pending| (pending.request_id, snapshot.system.network.generation));
    if snapshot.network_secret_request != key {
        clear_network_secret(snapshot);
        snapshot.network_secret_focused = false;
        snapshot.network_filter_focused = false;
        snapshot.network_secret_request = key;
    }
}

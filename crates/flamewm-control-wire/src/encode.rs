use super::*;

#[must_use]
pub fn settings_dict(snapshot: &SettingsSnapshot) -> WireDict {
    let mut out = WireDict::new();
    out.insert("revision".to_owned(), WireValue::U64(snapshot.revision));
    for (key, value) in &snapshot.values {
        out.insert(key.clone(), setting_wire_value(value));
    }
    out
}

#[must_use]
pub fn workspace_dict(snapshot: &WorkspaceSnapshot) -> WireDict {
    dict_of([
        ("revision", WireValue::U64(snapshot.revision)),
        ("count", WireValue::U32(u32_saturating(snapshot.count))),
        (
            "activeIndex",
            WireValue::I32(i32_saturating(snapshot.active_index)),
        ),
        (
            "lastIndex",
            WireValue::I32(snapshot.last_index.map_or(-1, i32_saturating)),
        ),
        ("names", WireValue::StringArray(snapshot.names.clone())),
    ])
}

#[must_use]
pub fn shell_bootstrap_dict(snapshot: &ShellBootstrapSnapshot) -> WireDict {
    dict_of([
        ("windowsRevision", WireValue::U64(snapshot.windows_revision)),
        (
            "windows",
            WireValue::Array(snapshot.windows.iter().map(window_wire_value).collect()),
        ),
        (
            "workspaces",
            WireValue::Dict(workspace_dict(&snapshot.workspaces)),
        ),
        ("panels", WireValue::Dict(panels_dict(&snapshot.panels))),
        (
            "applications",
            WireValue::Array(
                snapshot
                    .applications
                    .iter()
                    .map(application_wire_value)
                    .collect(),
            ),
        ),
        (
            "sessionCapabilities",
            WireValue::StringArray(session_capability_names(snapshot.session)),
        ),
        (
            "displays",
            WireValue::Dict(display_dict(&snapshot.displays)),
        ),
        ("system", WireValue::Dict(system_dict(&snapshot.system))),
    ])
}

#[must_use]
pub fn display_dict(snapshot: &DisplaySnapshot) -> WireDict {
    let mut out = WireDict::new();
    out.insert("generation".to_owned(), WireValue::U64(snapshot.generation));
    out.insert(
        "outputCount".to_owned(),
        WireValue::U32(u32_saturating(snapshot.outputs.len())),
    );
    out.insert(
        "hasPending".to_owned(),
        WireValue::Bool(snapshot.pending.is_some()),
    );
    if let Some(pending) = &snapshot.pending {
        out.insert(
            "pendingTx".to_owned(),
            WireValue::U64(pending.transaction.0),
        );
        out.insert(
            "pendingDeadlineMs".to_owned(),
            WireValue::U64(pending.deadline_ms),
        );
        out.insert(
            "pendingOutput".to_owned(),
            WireValue::String(pending.output.as_str().to_owned()),
        );
        out.insert("pendingMode".to_owned(), WireValue::U64(pending.mode.0));
    }
    out.insert(
        "outputs".to_owned(),
        WireValue::Array(snapshot.outputs.iter().map(output_wire_value).collect()),
    );
    out
}

#[must_use]
pub fn shortcut_dict(snapshot: &ShortcutSnapshot) -> WireDict {
    let mut out = WireDict::new();
    out.insert("revision".to_owned(), WireValue::U64(snapshot.revision));
    for (action, binding) in &snapshot.bindings {
        out.insert(
            action.clone(),
            WireValue::String(binding.0.clone().unwrap_or_default()),
        );
    }
    out
}

#[must_use]
pub fn system_dict(snapshot: &SystemSnapshot) -> WireDict {
    let mut out = WireDict::new();
    out.insert("revision".to_owned(), WireValue::U64(snapshot.revision));
    out.insert(
        "networkAvailability".to_owned(),
        WireValue::String(availability_name(snapshot.network.availability).to_owned()),
    );
    out.insert(
        "networkGeneration".to_owned(),
        WireValue::U64(snapshot.network.generation),
    );
    out.insert(
        "networkKind".to_owned(),
        WireValue::String(network_kind_name(snapshot.network.kind).to_owned()),
    );
    out.insert(
        "networkLabel".to_owned(),
        WireValue::String(snapshot.network.label.clone()),
    );
    out.insert(
        "networkStrength".to_owned(),
        WireValue::U32(u32::from(snapshot.network.strength_percent)),
    );
    out.insert(
        "wifiEnabled".to_owned(),
        WireValue::Bool(snapshot.network.wifi_enabled),
    );
    out.insert(
        "networkingEnabled".to_owned(),
        WireValue::Bool(snapshot.network.networking_enabled),
    );
    out.insert(
        "networkActivePath".to_owned(),
        WireValue::String(snapshot.network.active_path.clone()),
    );
    out.insert(
        "networkActiveAccessPointPath".to_owned(),
        WireValue::String(snapshot.network.active_access_point_path.clone()),
    );
    out.insert(
        "accessPoints".to_owned(),
        WireValue::Array(
            snapshot
                .network
                .access_points
                .iter()
                .map(access_point_wire_value)
                .collect(),
        ),
    );
    out.insert(
        "hasPendingNetworkSecret".to_owned(),
        WireValue::Bool(snapshot.network.pending_secret.is_some()),
    );
    if let Some(pending) = &snapshot.network.pending_secret {
        out.insert(
            "pendingNetworkSecretRequestId".to_owned(),
            WireValue::U64(pending.request_id),
        );
        out.insert(
            "pendingNetworkSecretAccessPointPath".to_owned(),
            WireValue::String(pending.access_point_path.clone()),
        );
    }
    out.insert(
        "mediaAvailability".to_owned(),
        WireValue::String(availability_name(snapshot.media.availability).to_owned()),
    );
    out.insert(
        "mediaGeneration".to_owned(),
        WireValue::U64(snapshot.media.generation),
    );
    out.insert(
        "mediaBusName".to_owned(),
        WireValue::String(snapshot.media.active_bus_name.clone()),
    );
    out.insert(
        "mediaIdentity".to_owned(),
        WireValue::String(snapshot.media.identity.clone()),
    );
    out.insert(
        "mediaTitle".to_owned(),
        WireValue::String(snapshot.media.title.clone()),
    );
    out.insert(
        "mediaArtist".to_owned(),
        WireValue::String(snapshot.media.artist.clone()),
    );
    out.insert(
        "mediaPlayback".to_owned(),
        WireValue::String(playback_name(snapshot.media.playback).to_owned()),
    );
    out.insert(
        "mediaCanPlay".to_owned(),
        WireValue::Bool(snapshot.media.can_play),
    );
    out.insert(
        "mediaCanPause".to_owned(),
        WireValue::Bool(snapshot.media.can_pause),
    );
    out.insert(
        "mediaCanNext".to_owned(),
        WireValue::Bool(snapshot.media.can_next),
    );
    out.insert(
        "mediaCanPrevious".to_owned(),
        WireValue::Bool(snapshot.media.can_previous),
    );
    out.insert(
        "powerAvailability".to_owned(),
        WireValue::String(availability_name(snapshot.power.available).to_owned()),
    );
    out.insert(
        "powerPresent".to_owned(),
        WireValue::Bool(snapshot.power.present),
    );
    out.insert(
        "powerPercent".to_owned(),
        WireValue::U32(u32::from(snapshot.power.percent)),
    );
    out.insert(
        "powerState".to_owned(),
        WireValue::String(power_state_name(snapshot.power.state).to_owned()),
    );
    // Optional: absent while the battery menu is closed; older peers ignore the whole group.
    if let Some(cpu) = snapshot.power.cpu_percent {
        out.insert("powerCpuPercent".to_owned(), WireValue::U32(u32::from(cpu)));
    }
    if let Some(milliwatts) = snapshot.power.power_rate_mw {
        out.insert("powerRateMilliwatts".to_owned(), WireValue::U32(milliwatts));
    }
    out.insert(
        "audioAvailability".to_owned(),
        WireValue::String(availability_name(snapshot.audio.availability).to_owned()),
    );
    out.insert(
        "audioGeneration".to_owned(),
        WireValue::U64(snapshot.audio.generation),
    );
    out.insert(
        "audioServerGeneration".to_owned(),
        WireValue::U64(snapshot.audio.server_generation),
    );
    out.insert(
        "audioSinkName".to_owned(),
        WireValue::String(snapshot.audio.sink_name.clone()),
    );
    out.insert(
        "audioVolume".to_owned(),
        WireValue::U32(u32::from(snapshot.audio.volume_percent)),
    );
    out.insert(
        "audioMuted".to_owned(),
        WireValue::Bool(snapshot.audio.muted),
    );
    out.insert(
        "audioEndpoints".to_owned(),
        WireValue::Array(
            snapshot
                .audio
                .endpoints()
                .iter()
                .map(audio_endpoint_wire_value)
                .collect(),
        ),
    );
    out.insert(
        "audioStreams".to_owned(),
        WireValue::Array(
            snapshot
                .audio
                .streams()
                .iter()
                .map(audio_stream_wire_value)
                .collect(),
        ),
    );
    out
}

#[must_use]
pub fn panels_dict(snapshot: &PanelsSnapshot) -> WireDict {
    let mut out = WireDict::new();
    out.insert("revision".to_owned(), WireValue::U64(snapshot.revision));
    out.insert(
        "panelCount".to_owned(),
        WireValue::U32(u32_saturating(snapshot.panels.len())),
    );
    out.insert(
        "taskCount".to_owned(),
        WireValue::U32(u32_saturating(snapshot.tasks.len())),
    );
    out.insert(
        "pinnedCount".to_owned(),
        WireValue::U32(u32_saturating(snapshot.pinned_apps.len())),
    );
    out.insert(
        "panels".to_owned(),
        WireValue::Array(snapshot.panels.iter().map(panel_wire_value).collect()),
    );
    out.insert(
        "tasks".to_owned(),
        WireValue::Array(
            snapshot
                .tasks
                .iter()
                .map(|entry| {
                    let mut item = WireDict::new();
                    item.insert(
                        "id".to_owned(),
                        WireValue::String(entry.id.as_str().to_owned()),
                    );
                    item.insert(
                        "appId".to_owned(),
                        WireValue::String(entry.app_id.as_str().to_owned()),
                    );
                    item.insert(
                        "order".to_owned(),
                        WireValue::U32(u32_saturating(entry.order_index)),
                    );
                    match &entry.kind {
                        TaskEntryKind::PinnedSlot { window } => {
                            item.insert("kind".to_owned(), WireValue::String("pin".to_owned()));
                            if let Some(window) = window {
                                item.insert("windowId".to_owned(), WireValue::U64(window.id));
                                item.insert(
                                    "windowGeneration".to_owned(),
                                    WireValue::U64(window.generation),
                                );
                            }
                        }
                        TaskEntryKind::Window { window } => {
                            item.insert("kind".to_owned(), WireValue::String("window".to_owned()));
                            item.insert("windowId".to_owned(), WireValue::U64(window.id));
                            item.insert(
                                "windowGeneration".to_owned(),
                                WireValue::U64(window.generation),
                            );
                        }
                    }
                    WireValue::Dict(item)
                })
                .collect(),
        ),
    );
    out.insert(
        "pinnedApps".to_owned(),
        WireValue::StringArray(
            snapshot
                .pinned_apps
                .iter()
                .map(|app| app.as_str().to_owned())
                .collect(),
        ),
    );
    out
}

#[must_use]
pub fn session_capability_names(capabilities: SessionCapabilities) -> Vec<String> {
    [
        (capabilities.lock, "lock"),
        (capabilities.logout, "logout"),
        (capabilities.suspend, "suspend"),
        (capabilities.reboot, "reboot"),
        (capabilities.shutdown, "shutdown"),
    ]
    .into_iter()
    .filter(|(enabled, _)| *enabled)
    .map(|(_, name)| name.to_owned())
    .collect()
}

pub(crate) fn output_wire_value(output: &OutputSnapshot) -> WireValue {
    let mut item = WireDict::new();
    item.insert(
        "id".to_owned(),
        WireValue::String(output.id.as_str().to_owned()),
    );
    item.insert(
        "connector".to_owned(),
        WireValue::String(output.connector.clone()),
    );
    item.insert(
        "edid".to_owned(),
        WireValue::String(output.edid_identity.clone()),
    );
    item.insert("connected".to_owned(), WireValue::Bool(output.connected));
    item.insert("primary".to_owned(), WireValue::Bool(output.primary));
    item.insert("x".to_owned(), WireValue::I32(output.geometry.x));
    item.insert("y".to_owned(), WireValue::I32(output.geometry.y));
    item.insert("width".to_owned(), WireValue::I32(output.geometry.width));
    item.insert("height".to_owned(), WireValue::I32(output.geometry.height));
    item.insert("mode".to_owned(), WireValue::U64(output.current_mode.0));
    item.insert(
        "scale".to_owned(),
        WireValue::U32(u32::from(output.shell_scale_percent)),
    );
    item.insert(
        "modes".to_owned(),
        WireValue::Array(
            output
                .modes
                .iter()
                .map(|mode| {
                    let mut value = WireDict::new();
                    value.insert("id".to_owned(), WireValue::U64(mode.id.0));
                    value.insert("width".to_owned(), WireValue::I32(mode.resolution.width));
                    value.insert("height".to_owned(), WireValue::I32(mode.resolution.height));
                    value.insert(
                        "refreshMillihz".to_owned(),
                        WireValue::I32(mode.refresh_millihz),
                    );
                    value.insert("preferred".to_owned(), WireValue::Bool(mode.preferred));
                    WireValue::Dict(value)
                })
                .collect(),
        ),
    );
    WireValue::Dict(item)
}

pub(crate) fn window_wire_value(window: &WindowSnapshot) -> WireValue {
    WireValue::Dict(dict_of([
        ("windowId", WireValue::U64(window.reference.id)),
        ("generation", WireValue::U64(window.reference.generation)),
        ("title", WireValue::String(window.title.clone())),
        (
            "appId",
            WireValue::String(window.app_id.as_str().to_owned()),
        ),
        ("outerX", WireValue::I32(window.outer_geometry.x)),
        ("outerY", WireValue::I32(window.outer_geometry.y)),
        ("outerWidth", WireValue::I32(window.outer_geometry.width)),
        ("outerHeight", WireValue::I32(window.outer_geometry.height)),
        ("restoreX", WireValue::I32(window.restore_geometry.x)),
        ("restoreY", WireValue::I32(window.restore_geometry.y)),
        (
            "restoreWidth",
            WireValue::I32(window.restore_geometry.width),
        ),
        (
            "restoreHeight",
            WireValue::I32(window.restore_geometry.height),
        ),
        (
            "state",
            WireValue::String(window_state_name(window.state).to_owned()),
        ),
        ("sticky", WireValue::Bool(window.sticky)),
        ("focused", WireValue::Bool(window.focused)),
        ("workspaceIndex", WireValue::I32(window.workspace.index)),
        (
            "workspaceRevision",
            WireValue::U64(window.workspace.revision),
        ),
        (
            "output",
            WireValue::String(window.output.as_str().to_owned()),
        ),
        ("stateGeneration", WireValue::U64(window.state_generation)),
    ]))
}

pub(crate) fn application_wire_value(application: &DesktopApplication) -> WireValue {
    WireValue::Dict(dict_of([
        ("id", WireValue::String(application.id.as_str().to_owned())),
        ("name", WireValue::String(application.name.clone())),
        (
            "genericName",
            WireValue::String(application.generic_name.clone()),
        ),
        ("comment", WireValue::String(application.comment.clone())),
        (
            "startupWmClass",
            WireValue::String(application.startup_wm_class.clone()),
        ),
        ("argv", WireValue::StringArray(application.argv.clone())),
        (
            "keywords",
            WireValue::StringArray(application.keywords.clone()),
        ),
        ("iconName", WireValue::String(application.icon_name.clone())),
        (
            "categories",
            WireValue::StringArray(application.categories.clone()),
        ),
    ]))
}

pub(crate) const fn window_state_name(state: WindowState) -> &'static str {
    match state {
        WindowState::Normal => "normal",
        WindowState::Minimized => "minimized",
        WindowState::Maximized => "maximized",
        WindowState::Fullscreen => "fullscreen",
        WindowState::Hidden => "hidden",
    }
}

pub(crate) fn panel_wire_value(panel: &PanelSnapshot) -> WireValue {
    let mut item = WireDict::new();
    item.insert(
        "output".to_owned(),
        WireValue::String(panel.output.as_str().to_owned()),
    );
    item.insert(
        "edge".to_owned(),
        WireValue::String(edge_name(panel.edge).to_owned()),
    );
    item.insert(
        "size".to_owned(),
        WireValue::U32(u32::from(panel.logical_size)),
    );
    item.insert("visible".to_owned(), WireValue::Bool(panel.visible));
    item.insert("x".to_owned(), WireValue::I32(panel.geometry.x));
    item.insert("y".to_owned(), WireValue::I32(panel.geometry.y));
    item.insert("width".to_owned(), WireValue::I32(panel.geometry.width));
    item.insert("height".to_owned(), WireValue::I32(panel.geometry.height));
    WireValue::Dict(item)
}

pub(crate) fn setting_wire_value(value: &SettingValue) -> WireValue {
    match value {
        SettingValue::Boolean(value) => WireValue::Bool(*value),
        SettingValue::Integer(value) => WireValue::I64(*value),
        SettingValue::Percent(value) => WireValue::U32(u32::from(*value)),
        SettingValue::Rgb(value) => WireValue::U32(*value),
        SettingValue::Text(value) => WireValue::String(value.clone()),
    }
}

pub(crate) fn setting_value(value: &WireValue) -> Result<SettingValue, ControlError> {
    let invalid = || ControlError {
        name: flamewm_control_core::error_name(ErrorCode::InvalidArgument),
        code: ErrorCode::InvalidArgument,
        message: "settings change contains unsupported variant type".to_owned(),
    };
    match value {
        WireValue::Bool(value) => Ok(SettingValue::Boolean(*value)),
        WireValue::I64(value) => Ok(SettingValue::Integer(*value)),
        WireValue::I32(value) => Ok(SettingValue::Integer(i64::from(*value))),
        WireValue::U32(value) => Ok(SettingValue::Integer(i64::from(*value))),
        WireValue::String(value) => Ok(SettingValue::Text(value.clone())),
        _ => Err(invalid()),
    }
}

pub(crate) fn setting_value_for_key(
    key: &str,
    value: &WireValue,
) -> Result<SettingValue, ControlError> {
    match key {
        "DesktopSelectionFillOpacity"
        | "WindowSnapPreviewFillOpacity"
        | "surfaceOpacity"
        | "surfaceBlurPercent"
        | "surfaceSaturationPercent"
        | "glassOpacityPercent"
        | "glassBlurPx"
        | "taskbarOpacityPercent"
        | "accentOpacity" => {
            let percent = required_u32(value, key)?;
            Ok(SettingValue::Percent(
                u8::try_from(percent).map_err(|_| invalid("percentage exceeds u8"))?,
            ))
        }
        _ => setting_value(value),
    }
}

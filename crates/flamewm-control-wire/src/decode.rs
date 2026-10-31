use super::*;

pub(crate) fn settings_snapshot(dict: &WireDict) -> Result<SettingsSnapshot, ControlError> {
    let revision = dict_u64(dict, "revision")?;
    let mut snapshot = SettingsSnapshot::new(revision);
    for (key, value) in dict {
        if key == "revision" {
            continue;
        }
        let typed = setting_value_for_key(key, value)?;
        snapshot.values.insert(key.clone(), typed);
    }
    Ok(snapshot)
}

pub(crate) fn window_snapshot(value: &WireValue) -> Result<WindowSnapshot, ControlError> {
    let dict = required_dict(value, "window")?;
    let state = match dict_string(dict, "state")? {
        "normal" => WindowState::Normal,
        "minimized" => WindowState::Minimized,
        "maximized" => WindowState::Maximized,
        "fullscreen" => WindowState::Fullscreen,
        "hidden" => WindowState::Hidden,
        _ => return Err(invalid("window state is invalid")),
    };
    Ok(WindowSnapshot {
        reference: WindowRef::new(dict_u64(dict, "windowId")?, dict_u64(dict, "generation")?),
        title: dict_string(dict, "title")?.to_owned(),
        app_id: DesktopAppId::new(dict_string(dict, "appId")?),
        outer_geometry: rect_dict(dict, "outer")?,
        restore_geometry: rect_dict(dict, "restore")?,
        state,
        sticky: dict_bool(dict, "sticky")?,
        focused: dict_bool(dict, "focused")?,
        workspace: flamewm_api::WorkspaceRef::new(
            dict_i32(dict, "workspaceIndex")?,
            dict_u64(dict, "workspaceRevision")?,
        ),
        output: OutputId::new(dict_string(dict, "output")?),
        state_generation: dict_u64(dict, "stateGeneration")?,
    })
}

pub(crate) fn application_snapshot(value: &WireValue) -> Result<DesktopApplication, ControlError> {
    let dict = required_dict(value, "application")?;
    Ok(DesktopApplication {
        id: DesktopAppId::new(dict_string(dict, "id")?),
        name: dict_string(dict, "name")?.to_owned(),
        generic_name: dict_string(dict, "genericName")?.to_owned(),
        comment: dict_string(dict, "comment")?.to_owned(),
        startup_wm_class: dict_string(dict, "startupWmClass")?.to_owned(),
        argv: dict_string_array(dict, "argv")?.to_vec(),
        keywords: dict_string_array(dict, "keywords")?.to_vec(),
        icon_name: dict_string(dict, "iconName")?.to_owned(),
        categories: dict_string_array(dict, "categories")?.to_vec(),
    })
}

pub(crate) fn rect_dict(dict: &WireDict, prefix: &str) -> Result<Rect, ControlError> {
    Ok(Rect::new(
        dict_i32(dict, &format!("{prefix}X"))?,
        dict_i32(dict, &format!("{prefix}Y"))?,
        dict_i32(dict, &format!("{prefix}Width"))?,
        dict_i32(dict, &format!("{prefix}Height"))?,
    ))
}

pub(crate) fn workspace_snapshot(dict: &WireDict) -> Result<WorkspaceSnapshot, ControlError> {
    let count = usize::try_from(dict_u32(dict, "count")?)
        .map_err(|_| invalid("workspace count does not fit usize"))?;
    let active = usize::try_from(dict_i32(dict, "activeIndex")?)
        .map_err(|_| invalid("active workspace must be nonnegative"))?;
    let last_raw = dict_i32(dict, "lastIndex")?;
    let last_index = if last_raw < 0 {
        None
    } else {
        Some(usize::try_from(last_raw).map_err(|_| invalid("last workspace does not fit usize"))?)
    };
    let names = dict_string_array(dict, "names")?.to_vec();
    let snapshot = WorkspaceSnapshot {
        revision: dict_u64(dict, "revision")?,
        count,
        active_index: active,
        last_index,
        names,
    };
    snapshot.validate().map_err(ControlError::from)?;
    Ok(snapshot)
}

pub(crate) fn shell_bootstrap_snapshot(
    dict: &WireDict,
) -> Result<ShellBootstrapSnapshot, ControlError> {
    Ok(ShellBootstrapSnapshot {
        windows_revision: dict_u64(dict, "windowsRevision")?,
        windows: required_array(dict_value(dict, "windows")?, "windows")?
            .iter()
            .map(window_snapshot)
            .collect::<Result<Vec<_>, _>>()?,
        workspaces: workspace_snapshot(required_dict(
            dict_value(dict, "workspaces")?,
            "workspaces",
        )?)?,
        panels: panels_snapshot(required_dict(dict_value(dict, "panels")?, "panels")?)?,
        applications: required_array(dict_value(dict, "applications")?, "applications")?
            .iter()
            .map(application_snapshot)
            .collect::<Result<Vec<_>, _>>()?,
        session: session_capabilities(dict_string_array(dict, "sessionCapabilities")?),
        displays: display_snapshot(required_dict(dict_value(dict, "displays")?, "displays")?)?,
        system: system_snapshot(required_dict(dict_value(dict, "system")?, "system")?)?,
    })
}

pub(crate) fn display_snapshot(dict: &WireDict) -> Result<DisplaySnapshot, ControlError> {
    let generation = dict_u64(dict, "generation")?;
    let outputs = dict.get("outputs").map_or(Ok(Vec::new()), |value| {
        required_array(value, "outputs")?
            .iter()
            .map(output_snapshot)
            .collect::<Result<Vec<_>, _>>()
    })?;
    let pending = if dict
        .get("hasPending")
        .and_then(WireValue::as_bool)
        .unwrap_or(false)
    {
        Some(PendingModeChange {
            transaction: TransactionId(dict_u64(dict, "pendingTx")?),
            output: OutputId::new(dict_string(dict, "pendingOutput")?),
            mode: ModeId(dict_u64(dict, "pendingMode")?),
            deadline_ms: dict_u64(dict, "pendingDeadlineMs")?,
        })
    } else {
        None
    };
    Ok(DisplaySnapshot {
        generation,
        outputs,
        pending,
    })
}

pub(crate) fn output_snapshot(value: &WireValue) -> Result<OutputSnapshot, ControlError> {
    let dict = required_dict(value, "output")?;
    let modes = dict.get("modes").map_or(Ok(Vec::new()), |modes| {
        required_array(modes, "modes")?
            .iter()
            .map(display_mode)
            .collect::<Result<Vec<_>, _>>()
    })?;
    let scale = dict_u32(dict, "scale")?;
    Ok(OutputSnapshot {
        id: OutputId::new(dict_string(dict, "id")?),
        connector: dict_string(dict, "connector")?.to_owned(),
        edid_identity: dict_string(dict, "edid")?.to_owned(),
        connected: dict_bool(dict, "connected")?,
        primary: dict_bool(dict, "primary")?,
        geometry: Rect::new(
            dict_i32(dict, "x")?,
            dict_i32(dict, "y")?,
            dict_i32(dict, "width")?,
            dict_i32(dict, "height")?,
        ),
        current_mode: ModeId(dict_u64(dict, "mode")?),
        modes,
        shell_scale_percent: u16::try_from(scale).map_err(|_| invalid("scale exceeds u16"))?,
    })
}

pub(crate) fn display_mode(value: &WireValue) -> Result<DisplayMode, ControlError> {
    let dict = required_dict(value, "mode")?;
    Ok(DisplayMode {
        id: ModeId(dict_u64(dict, "id")?),
        resolution: Size::new(dict_i32(dict, "width")?, dict_i32(dict, "height")?),
        refresh_millihz: dict_i32(dict, "refreshMillihz")?,
        preferred: dict_bool(dict, "preferred")?,
    })
}

pub(crate) fn shortcut_snapshot(dict: &WireDict) -> Result<ShortcutSnapshot, ControlError> {
    let revision = dict_u64(dict, "revision")?;
    let bindings = dict
        .iter()
        .filter(|(key, _)| key.as_str() != "revision")
        .map(|(key, value)| {
            Ok((
                key.clone(),
                KeyBinding::assigned(required_string(value, key)?),
            ))
        })
        .collect::<Result<BTreeMap<_, _>, ControlError>>()?;
    Ok(ShortcutSnapshot { revision, bindings })
}

pub(crate) fn panels_snapshot(dict: &WireDict) -> Result<PanelsSnapshot, ControlError> {
    let panels = dict.get("panels").map_or(Ok(Vec::new()), |value| {
        required_array(value, "panels")?
            .iter()
            .map(panel_snapshot)
            .collect::<Result<Vec<_>, _>>()
    })?;
    let tasks = dict.get("tasks").map_or(Ok(Vec::new()), |value| {
        required_array(value, "tasks")?
            .iter()
            .map(task_entry)
            .collect::<Result<Vec<_>, _>>()
    })?;
    let pinned_apps = match dict.get("pinnedApps") {
        Some(value) => required_string_array(value, "pinnedApps")?
            .iter()
            .map(DesktopAppId::new)
            .collect(),
        None => Vec::new(),
    };
    Ok(PanelsSnapshot {
        revision: dict_u64(dict, "revision")?,
        panels,
        tasks,
        pinned_apps,
    })
}

pub(crate) fn panel_snapshot(value: &WireValue) -> Result<PanelSnapshot, ControlError> {
    let dict = required_dict(value, "panel")?;
    let size = dict_u32(dict, "size")?;
    Ok(PanelSnapshot {
        output: OutputId::new(dict_string(dict, "output")?),
        edge: parse_edge(dict_string(dict, "edge")?)?,
        logical_size: u16::try_from(size).map_err(|_| invalid("panel size exceeds u16"))?,
        visible: dict_bool(dict, "visible")?,
        geometry: Rect::new(
            dict_i32(dict, "x")?,
            dict_i32(dict, "y")?,
            dict_i32(dict, "width")?,
            dict_i32(dict, "height")?,
        ),
    })
}

pub(crate) fn task_entry(value: &WireValue) -> Result<TaskEntry, ControlError> {
    let dict = required_dict(value, "task")?;
    let kind = match dict_string(dict, "kind")? {
        "pin" => {
            let window = if dict.contains_key("windowId") {
                Some(WindowRef::new(
                    dict_u64(dict, "windowId")?,
                    dict_u64(dict, "windowGeneration")?,
                ))
            } else {
                None
            };
            TaskEntryKind::PinnedSlot { window }
        }
        "window" => TaskEntryKind::Window {
            window: WindowRef::new(
                dict_u64(dict, "windowId")?,
                dict_u64(dict, "windowGeneration")?,
            ),
        },
        _ => return Err(invalid("task kind must be pin or window")),
    };
    Ok(TaskEntry {
        id: TaskEntryId::new(dict_string(dict, "id")?),
        app_id: DesktopAppId::new(dict_string(dict, "appId")?),
        kind,
        order_index: usize::try_from(dict_u32(dict, "order")?)
            .map_err(|_| invalid("task order does not fit usize"))?,
    })
}

pub(crate) fn session_capabilities(names: &[String]) -> SessionCapabilities {
    SessionCapabilities {
        lock: names.iter().any(|name| name == "lock"),
        logout: names.iter().any(|name| name == "logout"),
        suspend: names.iter().any(|name| name == "suspend"),
        reboot: names.iter().any(|name| name == "reboot"),
        shutdown: names.iter().any(|name| name == "shutdown"),
    }
}

/// Build a wire dict from literal entries, so each key is inserted exactly once.
pub(crate) fn dict_of<const N: usize>(entries: [(&str, WireValue); N]) -> WireDict {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

/// Single-value reply: enforces arity 1 and returns the only value.
pub(crate) fn single_reply(reply: &WireReply) -> Result<&WireValue, ControlError> {
    expect_reply_arity(reply, 1)?;
    Ok(&reply.values[0])
}

pub(crate) fn expect_reply_arity(reply: &WireReply, expected: usize) -> Result<(), ControlError> {
    if reply.values.len() == expected {
        Ok(())
    } else {
        Err(invalid(&format!("reply expects {expected} values")))
    }
}

pub(crate) fn required_array<'a>(
    value: &'a WireValue,
    name: &str,
) -> Result<&'a [WireValue], ControlError> {
    value
        .as_array()
        .ok_or_else(|| invalid(&format!("{name} must be array")))
}

pub(crate) fn required_string_array<'a>(
    value: &'a WireValue,
    name: &str,
) -> Result<&'a [String], ControlError> {
    value
        .as_string_array()
        .ok_or_else(|| invalid(&format!("{name} must be string array")))
}

pub(crate) fn dict_value<'a>(dict: &'a WireDict, key: &str) -> Result<&'a WireValue, ControlError> {
    dict.get(key)
        .ok_or_else(|| invalid(&format!("snapshot missing {key}")))
}

pub(crate) fn dict_u64(dict: &WireDict, key: &str) -> Result<u64, ControlError> {
    required_u64(dict_value(dict, key)?, key)
}

pub(crate) fn dict_u32(dict: &WireDict, key: &str) -> Result<u32, ControlError> {
    required_u32(dict_value(dict, key)?, key)
}

pub(crate) fn dict_i32(dict: &WireDict, key: &str) -> Result<i32, ControlError> {
    dict_value(dict, key)?
        .as_i32()
        .ok_or_else(|| invalid(&format!("{key} must be int32")))
}

pub(crate) fn dict_bool(dict: &WireDict, key: &str) -> Result<bool, ControlError> {
    dict_value(dict, key)?
        .as_bool()
        .ok_or_else(|| invalid(&format!("{key} must be bool")))
}

pub(crate) fn dict_string<'a>(dict: &'a WireDict, key: &str) -> Result<&'a str, ControlError> {
    required_string(dict_value(dict, key)?, key)
}

pub(crate) fn dict_string_array<'a>(
    dict: &'a WireDict,
    key: &str,
) -> Result<&'a [String], ControlError> {
    required_string_array(dict_value(dict, key)?, key)
}

use super::*;

pub(crate) fn workspace_index_request(
    call: &WireCall,
    build: impl FnOnce(usize, u64) -> ControlRequest,
) -> Result<ControlRequest, ControlError> {
    expect_arity(call, 2)?;
    let index = usize::try_from(required_u32(&call.args[0], "index")?)
        .map_err(|_| invalid("index does not fit usize"))?;
    Ok(build(
        index,
        required_u64(&call.args[1], "expectedRevision")?,
    ))
}

pub(crate) fn shortcut_action_request(
    call: &WireCall,
    build: impl FnOnce(String, u64) -> ControlRequest,
) -> Result<ControlRequest, ControlError> {
    expect_arity(call, 2)?;
    Ok(build(
        required_string(&call.args[0], "action")?.to_owned(),
        required_u64(&call.args[1], "expectedRevision")?,
    ))
}

pub(crate) fn panel_app_request(
    call: &WireCall,
    build: impl FnOnce(DesktopAppId, u64) -> ControlRequest,
) -> Result<ControlRequest, ControlError> {
    expect_arity(call, 2)?;
    Ok(build(
        DesktopAppId::new(required_string(&call.args[0], "appId")?),
        required_u64(&call.args[1], "expectedRevision")?,
    ))
}

pub(crate) fn session_action(
    call: &WireCall,
    action: SessionAction,
) -> Result<ControlRequest, ControlError> {
    expect_arity(call, 0)?;
    Ok(ControlRequest::SessionAction(action))
}

pub(crate) fn system_action_request(
    call: &WireCall,
    member: &str,
) -> Result<ControlRequest, ControlError> {
    let (action, expected_revision) = match member {
        "SetWifiEnabled" => {
            expect_arity(call, 2)?;
            (
                SystemAction::SetWifiEnabled(
                    call.args[0]
                        .as_bool()
                        .ok_or_else(|| invalid("enabled must be bool"))?,
                ),
                required_u64(&call.args[1], "expectedRevision")?,
            )
        }
        "ConnectKnown" => {
            expect_arity(call, 2)?;
            (
                SystemAction::ConnectKnown {
                    access_point_path: required_string(&call.args[0], "accessPointPath")?
                        .to_owned(),
                },
                required_u64(&call.args[1], "expectedRevision")?,
            )
        }
        "ConnectWifi" => {
            expect_arity(call, 3)?;
            (
                SystemAction::ConnectWifi {
                    access_point_path: required_string(&call.args[0], "accessPointPath")?
                        .to_owned(),
                    generation: required_u64(&call.args[1], "generation")?,
                },
                required_u64(&call.args[2], "expectedRevision")?,
            )
        }
        "SubmitNetworkSecret" => {
            expect_arity(call, 4)?;
            (
                SystemAction::SubmitNetworkSecret {
                    request_id: required_u64(&call.args[0], "requestId")?,
                    generation: required_u64(&call.args[1], "generation")?,
                    secret: required_string(&call.args[2], "secret")?.to_owned(),
                },
                required_u64(&call.args[3], "expectedRevision")?,
            )
        }
        "CancelNetworkSecret" => {
            expect_arity(call, 3)?;
            (
                SystemAction::CancelNetworkSecret {
                    request_id: required_u64(&call.args[0], "requestId")?,
                    generation: required_u64(&call.args[1], "generation")?,
                },
                required_u64(&call.args[2], "expectedRevision")?,
            )
        }
        "Disconnect" => system_simple_action(call, SystemAction::Disconnect)?,
        "Scan" => system_simple_action(call, SystemAction::Scan)?,
        "BatteryMenuOpened" => system_simple_action(call, SystemAction::BatteryMenuOpened)?,
        "BatteryMenuClosed" => system_simple_action(call, SystemAction::BatteryMenuClosed)?,
        "Play" => system_media_action(call, |bus_name| SystemAction::Play { bus_name })?,
        "Pause" => system_media_action(call, |bus_name| SystemAction::Pause { bus_name })?,
        "PlayPause" => system_media_action(call, |bus_name| SystemAction::PlayPause { bus_name })?,
        "Next" => system_media_action(call, |bus_name| SystemAction::Next { bus_name })?,
        "Previous" => system_media_action(call, |bus_name| SystemAction::Previous { bus_name })?,
        "SetVolume" => {
            expect_arity(call, 7)?;
            let percent = u8::try_from(required_u32(&call.args[3], "percent")?)
                .map_err(|_| invalid("percent exceeds u8"))?;
            if percent > 150 {
                return Err(invalid("percent must be in 0..=150"));
            }
            (
                SystemAction::SetVolume(AudioVolumeAction {
                    target: audio_target(
                        required_string(&call.args[0], "targetKind")?,
                        required_string(&call.args[1], "endpointKind")?,
                        required_u32(&call.args[2], "id")?,
                    )?,
                    percent,
                    generation: required_u64(&call.args[4], "generation")?,
                    server_generation: required_u64(&call.args[5], "serverGeneration")?,
                }),
                required_u64(&call.args[6], "expectedRevision")?,
            )
        }
        "SetMute" => {
            expect_arity(call, 7)?;
            (
                SystemAction::SetMute(AudioMuteAction {
                    target: audio_target(
                        required_string(&call.args[0], "targetKind")?,
                        required_string(&call.args[1], "endpointKind")?,
                        required_u32(&call.args[2], "id")?,
                    )?,
                    muted: call.args[3]
                        .as_bool()
                        .ok_or_else(|| invalid("muted must be bool"))?,
                    generation: required_u64(&call.args[4], "generation")?,
                    server_generation: required_u64(&call.args[5], "serverGeneration")?,
                }),
                required_u64(&call.args[6], "expectedRevision")?,
            )
        }
        _ => {
            return Err(ControlError {
                name: flamewm_control_core::error_name(ErrorCode::NotFound),
                code: ErrorCode::NotFound,
                message: format!("unknown Flame Control system method {member}"),
            });
        }
    };
    Ok(ControlRequest::SystemAction {
        action,
        expected_revision,
    })
}

pub(crate) fn system_simple_action(
    call: &WireCall,
    action: SystemAction,
) -> Result<(SystemAction, u64), ControlError> {
    expect_arity(call, 1)?;
    Ok((action, required_u64(&call.args[0], "expectedRevision")?))
}

pub(crate) fn system_media_action(
    call: &WireCall,
    action: impl FnOnce(String) -> SystemAction,
) -> Result<(SystemAction, u64), ControlError> {
    expect_arity(call, 2)?;
    Ok((
        action(required_string(&call.args[0], "busName")?.to_owned()),
        required_u64(&call.args[1], "expectedRevision")?,
    ))
}

pub(crate) fn system_action_call(action: &SystemAction, expected_revision: u64) -> WireCall {
    let (member, mut args) = match action {
        SystemAction::SetWifiEnabled(enabled) => {
            ("SetWifiEnabled", vec![WireValue::Bool(*enabled)])
        }
        SystemAction::ConnectKnown { access_point_path } => (
            "ConnectKnown",
            vec![WireValue::String(access_point_path.clone())],
        ),
        SystemAction::ConnectWifi {
            access_point_path,
            generation,
        } => (
            "ConnectWifi",
            vec![
                WireValue::String(access_point_path.clone()),
                WireValue::U64(*generation),
            ],
        ),
        SystemAction::SubmitNetworkSecret {
            request_id,
            generation,
            secret,
        } => (
            "SubmitNetworkSecret",
            vec![
                WireValue::U64(*request_id),
                WireValue::U64(*generation),
                WireValue::String(secret.clone()),
            ],
        ),
        SystemAction::CancelNetworkSecret {
            request_id,
            generation,
        } => (
            "CancelNetworkSecret",
            vec![WireValue::U64(*request_id), WireValue::U64(*generation)],
        ),
        SystemAction::Disconnect => ("Disconnect", Vec::new()),
        SystemAction::Scan => ("Scan", Vec::new()),
        SystemAction::BatteryMenuOpened => ("BatteryMenuOpened", Vec::new()),
        SystemAction::BatteryMenuClosed => ("BatteryMenuClosed", Vec::new()),
        SystemAction::Play { bus_name } => ("Play", vec![WireValue::String(bus_name.clone())]),
        SystemAction::Pause { bus_name } => ("Pause", vec![WireValue::String(bus_name.clone())]),
        SystemAction::PlayPause { bus_name } => {
            ("PlayPause", vec![WireValue::String(bus_name.clone())])
        }
        SystemAction::Next { bus_name } => ("Next", vec![WireValue::String(bus_name.clone())]),
        SystemAction::Previous { bus_name } => {
            ("Previous", vec![WireValue::String(bus_name.clone())])
        }
        SystemAction::SetVolume(action) => (
            "SetVolume",
            audio_action_args(
                action.target,
                Some(action.percent),
                None,
                action.generation,
                action.server_generation,
            ),
        ),
        SystemAction::SetMute(action) => (
            "SetMute",
            audio_action_args(
                action.target,
                None,
                Some(action.muted),
                action.generation,
                action.server_generation,
            ),
        ),
    };
    args.push(WireValue::U64(expected_revision));
    WireCall::new(IFACE_SYSTEM, member, args)
}

pub(crate) fn audio_action_args(
    target: AudioTarget,
    percent: Option<u8>,
    muted: Option<bool>,
    generation: u64,
    server_generation: u64,
) -> Vec<WireValue> {
    let (target_kind, endpoint_kind, id) = match target {
        AudioTarget::Endpoint { id, kind } => ("endpoint", audio_endpoint_kind_name(kind), id),
        AudioTarget::Stream { id } => ("stream", "", id),
    };
    let mut args = vec![
        WireValue::String(target_kind.to_owned()),
        WireValue::String(endpoint_kind.to_owned()),
        WireValue::U32(id),
    ];
    if let Some(percent) = percent {
        args.push(WireValue::U32(u32::from(percent)));
    }
    if let Some(muted) = muted {
        args.push(WireValue::Bool(muted));
    }
    args.push(WireValue::U64(generation));
    args.push(WireValue::U64(server_generation));
    args
}

pub(crate) fn expect_arity(call: &WireCall, expected: usize) -> Result<(), ControlError> {
    if call.args.len() == expected {
        Ok(())
    } else {
        Err(invalid(&format!(
            "{} expects {expected} arguments",
            call.member
        )))
    }
}

pub(crate) fn required_u64(value: &WireValue, name: &str) -> Result<u64, ControlError> {
    value
        .as_u64()
        .ok_or_else(|| invalid(&format!("{name} must be uint64")))
}

pub(crate) fn required_u32(value: &WireValue, name: &str) -> Result<u32, ControlError> {
    value
        .as_u32()
        .ok_or_else(|| invalid(&format!("{name} must be uint32")))
}

pub(crate) fn required_bool(value: &WireValue, name: &str) -> Result<bool, ControlError> {
    value
        .as_bool()
        .ok_or_else(|| invalid(&format!("{name} must be bool")))
}

pub(crate) fn required_string<'a>(
    value: &'a WireValue,
    name: &str,
) -> Result<&'a str, ControlError> {
    value
        .as_str()
        .ok_or_else(|| invalid(&format!("{name} must be string")))
}

pub(crate) fn required_dict<'a>(
    value: &'a WireValue,
    name: &str,
) -> Result<&'a WireDict, ControlError> {
    value
        .as_dict()
        .ok_or_else(|| invalid(&format!("{name} must be a{{sv}}")))
}

pub(crate) fn parse_edge(value: &str) -> Result<PanelEdge, ControlError> {
    match value.to_ascii_lowercase().as_str() {
        "bottom" => Ok(PanelEdge::Bottom),
        "top" => Ok(PanelEdge::Top),
        "left" => Ok(PanelEdge::Left),
        "right" => Ok(PanelEdge::Right),
        _ => Err(invalid("edge must be bottom/top/left/right")),
    }
}

#[must_use]
pub const fn edge_name(edge: PanelEdge) -> &'static str {
    match edge {
        PanelEdge::Bottom => "bottom",
        PanelEdge::Top => "top",
        PanelEdge::Left => "left",
        PanelEdge::Right => "right",
    }
}

pub(crate) fn nullary(
    call: &WireCall,
    request: ControlRequest,
) -> Result<ControlRequest, ControlError> {
    expect_arity(call, 0).map(|()| request)
}

pub(crate) fn nullary_call(interface: &str, member: &str) -> WireCall {
    WireCall::new(interface, member, Vec::new())
}

pub(crate) fn invalid(message: &str) -> ControlError {
    ControlError {
        name: flamewm_control_core::error_name(ErrorCode::InvalidArgument),
        code: ErrorCode::InvalidArgument,
        message: message.to_owned(),
    }
}

pub(crate) fn u32_saturating(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

pub(crate) fn i32_saturating(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

use super::*;

pub fn encode_reply(
    call: &WireCall,
    response: &ControlResponse,
) -> Result<WireReply, ControlError> {
    let values = match (call.interface.as_str(), call.member.as_str(), response) {
        (IFACE_ROOT, "Ping", ControlResponse::Pong) => vec![WireValue::String("pong".to_owned())],
        (IFACE_ROOT, "GetVersion", ControlResponse::Version(version)) => {
            vec![WireValue::String(version.to_string())]
        }
        (IFACE_ROOT, "GetCapabilities", ControlResponse::Capabilities(items)) => {
            vec![WireValue::StringArray(items.clone())]
        }
        (IFACE_ROOT, "GetShellBootstrap", ControlResponse::ShellBootstrap(snapshot)) => {
            vec![WireValue::Dict(shell_bootstrap_dict(snapshot))]
        }
        (IFACE_WINDOWS, "GetWindows", ControlResponse::Windows(windows)) => {
            vec![WireValue::Array(
                windows.iter().map(window_wire_value).collect(),
            )]
        }
        (IFACE_APPLICATIONS, "GetApplications", ControlResponse::Applications(applications)) => {
            vec![WireValue::Array(
                applications.iter().map(application_wire_value).collect(),
            )]
        }
        (IFACE_SETTINGS, "GetSnapshot", ControlResponse::Settings(snapshot)) => {
            vec![WireValue::Dict(settings_dict(snapshot))]
        }
        (IFACE_SETTINGS, "Apply" | "ResetSection", ControlResponse::Settings(snapshot)) => {
            vec![WireValue::Dict(settings_dict(snapshot))]
        }
        (IFACE_WORKSPACES, "GetSnapshot", ControlResponse::Workspaces(snapshot)) => {
            vec![WireValue::Dict(workspace_dict(snapshot))]
        }
        (IFACE_DISPLAYS, "GetSnapshot", ControlResponse::Displays(snapshot)) => {
            vec![WireValue::Dict(display_dict(snapshot))]
        }
        (IFACE_DISPLAYS, "BeginModeChange", ControlResponse::Transaction(transaction)) => {
            vec![WireValue::U64(transaction.0)]
        }
        (IFACE_SHORTCUTS, "GetSnapshot", ControlResponse::Shortcuts(snapshot)) => {
            vec![WireValue::Dict(shortcut_dict(snapshot))]
        }
        (IFACE_SHORTCUTS, "BeginCapture", ControlResponse::CaptureLease(lease)) => {
            vec![WireValue::U64(*lease)]
        }
        (IFACE_PANELS, "GetSnapshot", ControlResponse::Panels(snapshot)) => {
            vec![WireValue::Dict(panels_dict(snapshot))]
        }
        (IFACE_SESSION, "GetCapabilities", ControlResponse::SessionCapabilities(capabilities)) => {
            vec![WireValue::StringArray(session_capability_names(
                *capabilities,
            ))]
        }
        (IFACE_WEATHER, "GetSnapshot", ControlResponse::WeatherFeed(snapshot)) => {
            vec![WireValue::Dict(weather_feed_dict(snapshot))]
        }
        (IFACE_SYSTEM, "GetSnapshot", ControlResponse::System(snapshot)) => {
            vec![WireValue::Dict(system_dict(snapshot))]
        }
        (_, _, ControlResponse::Unit | ControlResponse::Changed(_)) => Vec::new(),
        _ => {
            return Err(ControlError {
                name: flamewm_control_core::error_name(ErrorCode::InternalFailure),
                code: ErrorCode::InternalFailure,
                message: format!("response does not match {}.{}", call.interface, call.member),
            });
        }
    };
    Ok(WireReply { values })
}

/// Decode the reply for the exact request call. This is the client-side inverse of `encode_reply`.
pub fn decode_reply(call: &WireCall, reply: &WireReply) -> Result<ControlResponse, ControlError> {
    match (call.interface.as_str(), call.member.as_str()) {
        (IFACE_ROOT, "Ping") => {
            let pong = required_string(single_reply(reply)?, "reply")?;
            if pong != "pong" {
                return Err(invalid("Ping reply must be pong"));
            }
            Ok(ControlResponse::Pong)
        }
        (IFACE_ROOT, "GetVersion") => {
            let version = Version::parse(required_string(single_reply(reply)?, "version")?)
                .ok_or_else(|| invalid("version is not canonical major.minor.patch"))?;
            Ok(ControlResponse::Version(version))
        }
        (IFACE_ROOT, "GetCapabilities") => Ok(ControlResponse::Capabilities(
            required_string_array(single_reply(reply)?, "capabilities")?.to_vec(),
        )),
        (IFACE_ROOT, "GetShellBootstrap") => Ok(ControlResponse::ShellBootstrap(
            shell_bootstrap_snapshot(required_dict(single_reply(reply)?, "snapshot")?)?,
        )),
        (IFACE_WINDOWS, "GetWindows") => Ok(ControlResponse::Windows(
            required_array(single_reply(reply)?, "windows")?
                .iter()
                .map(window_snapshot)
                .collect::<Result<Vec<_>, _>>()?,
        )),
        (IFACE_APPLICATIONS, "GetApplications") => Ok(ControlResponse::Applications(
            required_array(single_reply(reply)?, "applications")?
                .iter()
                .map(application_snapshot)
                .collect::<Result<Vec<_>, _>>()?,
        )),
        (IFACE_SETTINGS, "GetSnapshot" | "Apply" | "ResetSection") => {
            Ok(ControlResponse::Settings(settings_snapshot(
                required_dict(single_reply(reply)?, "snapshot")?,
            )?))
        }
        (IFACE_WORKSPACES, "GetSnapshot") => Ok(ControlResponse::Workspaces(workspace_snapshot(
            required_dict(single_reply(reply)?, "snapshot")?,
        )?)),
        (IFACE_DISPLAYS, "GetSnapshot") => Ok(ControlResponse::Displays(display_snapshot(
            required_dict(single_reply(reply)?, "snapshot")?,
        )?)),
        (IFACE_DISPLAYS, "BeginModeChange") => Ok(ControlResponse::Transaction(TransactionId(
            required_u64(single_reply(reply)?, "transaction")?,
        ))),
        (IFACE_SHORTCUTS, "BeginCapture") => Ok(ControlResponse::CaptureLease(required_u64(
            single_reply(reply)?,
            "lease",
        )?)),
        (IFACE_SHORTCUTS, "GetSnapshot") => Ok(ControlResponse::Shortcuts(shortcut_snapshot(
            required_dict(single_reply(reply)?, "snapshot")?,
        )?)),
        (IFACE_PANELS, "GetSnapshot") => Ok(ControlResponse::Panels(panels_snapshot(
            required_dict(single_reply(reply)?, "snapshot")?,
        )?)),
        (IFACE_SESSION, "GetCapabilities") => Ok(ControlResponse::SessionCapabilities(
            session_capabilities(required_string_array(single_reply(reply)?, "capabilities")?),
        )),
        (IFACE_WEATHER, "GetSnapshot") => Ok(ControlResponse::WeatherFeed(weather_feed_snapshot(
            required_dict(single_reply(reply)?, "snapshot")?,
        )?)),
        (IFACE_SYSTEM, "GetSnapshot") => Ok(ControlResponse::System(system_snapshot(
            required_dict(single_reply(reply)?, "snapshot")?,
        )?)),
        (
            IFACE_SYSTEM,
            "SetWifiEnabled"
            | "ConnectKnown"
            | "ConnectWifi"
            | "SubmitNetworkSecret"
            | "CancelNetworkSecret"
            | "Disconnect"
            | "Scan"
            | "Play"
            | "Pause"
            | "PlayPause"
            | "Next"
            | "Previous"
            | "SetVolume"
            | "SetMute"
            | "BatteryMenuOpened"
            | "BatteryMenuClosed",
        )
        | (IFACE_WORKSPACES, "Activate" | "InsertAfter" | "Remove")
        | (IFACE_DISPLAYS, "Keep" | "Revert" | "SetShellScale")
        | (IFACE_WEATHER, "Publish")
        | (IFACE_SHORTCUTS, "SetBinding" | "ClearBinding" | "ResetBinding" | "EndCapture")
        | (IFACE_PANELS, "SetEdge" | "SetSize" | "Pin" | "Unpin" | "Reorder")
        | (
            IFACE_SESSION,
            "Lock" | "LockPreview" | "Logout" | "Suspend" | "Reboot" | "Shutdown"
            | "ScreensaverPreview",
        )
        | (IFACE_WINDOWS, "ActivateWindow" | "MinimizeWindow" | "RestoreWindow" | "CloseWindow")
        | (IFACE_APPLICATIONS, "LaunchApplication") => {
            expect_reply_arity(reply, 0)?;
            Ok(ControlResponse::Unit)
        }
        _ => Err(ControlError {
            name: flamewm_control_core::error_name(ErrorCode::NotFound),
            code: ErrorCode::NotFound,
            message: format!(
                "unknown Flame Control reply {}.{}",
                call.interface, call.member
            ),
        }),
    }
}

use super::*;

pub fn decode_call(call: &WireCall) -> Result<ControlRequest, ControlError> {
    match (call.interface.as_str(), call.member.as_str()) {
        (IFACE_ROOT, "Ping") => nullary(call, ControlRequest::Ping),
        (IFACE_ROOT, "GetVersion") => nullary(call, ControlRequest::GetVersion),
        (IFACE_ROOT, "GetCapabilities") => nullary(call, ControlRequest::GetCapabilities),
        (IFACE_ROOT, "GetShellBootstrap") => nullary(call, ControlRequest::GetShellBootstrap),
        (IFACE_WINDOWS, "GetWindows") => nullary(call, ControlRequest::GetWindows),
        (IFACE_APPLICATIONS, "GetApplications") => nullary(call, ControlRequest::GetApplications),
        (IFACE_APPLICATIONS, "LaunchApplication") => {
            expect_arity(call, 3)?;
            Ok(ControlRequest::LaunchApplication {
                app: DesktopAppId::new(required_string(&call.args[0], "appId")?),
                options: ApplicationLaunchOptions {
                    extra_args: required_string_array(&call.args[1], "extraArgs")?.to_vec(),
                    uris: required_string_array(&call.args[2], "uris")?.to_vec(),
                },
            })
        }
        (
            IFACE_WINDOWS,
            member @ ("ActivateWindow" | "MinimizeWindow" | "RestoreWindow" | "CloseWindow"),
        ) => {
            expect_arity(call, 2)?;
            let window = WindowRef::new(
                required_u64(&call.args[0], "windowId")?,
                required_u64(&call.args[1], "generation")?,
            );
            Ok(match member {
                "ActivateWindow" => ControlRequest::ActivateWindow(window),
                "MinimizeWindow" => ControlRequest::MinimizeWindow(window),
                "RestoreWindow" => ControlRequest::RestoreWindow(window),
                _ => ControlRequest::CloseWindow(window),
            })
        }
        (IFACE_SETTINGS, "GetSnapshot") => nullary(call, ControlRequest::GetSettings),
        (IFACE_SETTINGS, "Apply") => {
            expect_arity(call, 2)?;
            let expected_revision = required_u64(&call.args[0], "expectedRevision")?;
            let changes = required_dict(&call.args[1], "changes")?;
            let changes = changes
                .iter()
                .map(|(key, value)| {
                    setting_value_for_key(key, value).map(|value| SettingsChange {
                        key: key.clone(),
                        value: Some(value),
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ControlRequest::ApplySettings(SettingsTransaction {
                expected_revision,
                changes,
                reset_section: None,
            }))
        }
        (IFACE_SETTINGS, "ResetSection") => {
            expect_arity(call, 2)?;
            Ok(ControlRequest::ApplySettings(SettingsTransaction {
                expected_revision: required_u64(&call.args[0], "expectedRevision")?,
                changes: Vec::new(),
                reset_section: Some(required_string(&call.args[1], "section")?.to_owned()),
            }))
        }
        (IFACE_WORKSPACES, "GetSnapshot") => nullary(call, ControlRequest::GetWorkspaces),
        (IFACE_WORKSPACES, "Activate") => {
            workspace_index_request(call, |index, expected_revision| {
                ControlRequest::ActivateWorkspace {
                    index,
                    expected_revision,
                }
            })
        }
        (IFACE_WORKSPACES, "InsertAfter") => {
            workspace_index_request(call, |index, expected_revision| {
                ControlRequest::InsertWorkspaceAfter {
                    index,
                    expected_revision,
                }
            })
        }
        (IFACE_WORKSPACES, "Remove") => {
            workspace_index_request(call, |index, expected_revision| {
                ControlRequest::RemoveWorkspace {
                    index,
                    expected_revision,
                }
            })
        }
        (IFACE_DISPLAYS, "GetSnapshot") => nullary(call, ControlRequest::GetDisplays),
        (IFACE_DISPLAYS, "BeginModeChange") => {
            expect_arity(call, 3)?;
            Ok(ControlRequest::BeginDisplayMode {
                output: OutputId::new(required_string(&call.args[0], "outputKey")?),
                mode: ModeId(required_u64(&call.args[1], "modeId")?),
                expected_generation: required_u64(&call.args[2], "generation")?,
                now_ms: call.now_ms,
            })
        }
        (IFACE_DISPLAYS, "Keep") => {
            expect_arity(call, 1)?;
            Ok(ControlRequest::KeepDisplayMode {
                transaction: TransactionId(required_u64(&call.args[0], "transaction")?),
                now_ms: call.now_ms,
            })
        }
        (IFACE_DISPLAYS, "Revert") => {
            expect_arity(call, 1)?;
            Ok(ControlRequest::RevertDisplayMode {
                transaction: TransactionId(required_u64(&call.args[0], "transaction")?),
            })
        }
        (IFACE_DISPLAYS, "SetShellScale") => {
            expect_arity(call, 3)?;
            let percent = required_u32(&call.args[1], "percent")?;
            Ok(ControlRequest::SetShellScale {
                output: OutputId::new(required_string(&call.args[0], "outputKey")?),
                percent: u16::try_from(percent).map_err(|_| invalid("percent exceeds u16"))?,
                expected_revision: required_u64(&call.args[2], "expectedRevision")?,
            })
        }
        (IFACE_SHORTCUTS, "GetSnapshot") => nullary(call, ControlRequest::GetShortcuts),
        (IFACE_SHORTCUTS, "BeginCapture") => {
            shortcut_action_request(call, |action, expected_revision| {
                ControlRequest::BeginHotkeyCapture {
                    action,
                    expected_revision,
                }
            })
        }
        (IFACE_SHORTCUTS, "EndCapture") => {
            expect_arity(call, 1)?;
            Ok(ControlRequest::EndHotkeyCapture {
                lease: required_u64(&call.args[0], "lease")?,
            })
        }
        (IFACE_SHORTCUTS, "SetBinding") => {
            expect_arity(call, 3)?;
            Ok(ControlRequest::SetShortcut {
                action: required_string(&call.args[0], "action")?.to_owned(),
                binding: required_string(&call.args[1], "binding")?.to_owned(),
                expected_revision: required_u64(&call.args[2], "expectedRevision")?,
            })
        }
        (IFACE_SHORTCUTS, "ClearBinding") => {
            shortcut_action_request(call, |action, expected_revision| {
                ControlRequest::ClearShortcut {
                    action,
                    expected_revision,
                }
            })
        }
        (IFACE_SHORTCUTS, "ResetBinding") => {
            shortcut_action_request(call, |action, expected_revision| {
                ControlRequest::ResetShortcut {
                    action,
                    expected_revision,
                }
            })
        }
        (IFACE_PANELS, "GetSnapshot") => nullary(call, ControlRequest::GetPanels),
        (IFACE_WEATHER, "GetSnapshot") => nullary(call, ControlRequest::GetWeatherFeed),
        (IFACE_WEATHER, "Publish") => {
            expect_arity(call, 1)?;
            Ok(ControlRequest::PublishWeatherFeed {
                payload: required_string(&call.args[0], "payload")?.to_owned(),
            })
        }
        (IFACE_PANELS, "SetEdge") => {
            expect_arity(call, 3)?;
            Ok(ControlRequest::SetPanelEdge {
                output: OutputId::new(required_string(&call.args[0], "outputKey")?),
                edge: parse_edge(required_string(&call.args[1], "edge")?)?,
                expected_revision: required_u64(&call.args[2], "expectedRevision")?,
            })
        }
        (IFACE_PANELS, "SetSize") => {
            expect_arity(call, 3)?;
            Ok(ControlRequest::SetPanelSize {
                output: OutputId::new(required_string(&call.args[0], "outputKey")?),
                logical_size: u16::try_from(required_u32(&call.args[1], "size")?)
                    .map_err(|_| invalid("size exceeds u16"))?,
                expected_revision: required_u64(&call.args[2], "expectedRevision")?,
            })
        }
        (IFACE_PANELS, "Pin") => {
            panel_app_request(call, |app, expected_revision| ControlRequest::PinApp {
                app,
                expected_revision,
            })
        }
        (IFACE_PANELS, "Unpin") => {
            panel_app_request(call, |app, expected_revision| ControlRequest::UnpinApp {
                app,
                expected_revision,
            })
        }
        (IFACE_PANELS, "Reorder") => {
            expect_arity(call, 3)?;
            Ok(ControlRequest::ReorderTask {
                entry_id: TaskEntryId::new(required_string(&call.args[0], "entryId")?),
                index: usize::try_from(required_u32(&call.args[1], "index")?)
                    .map_err(|_| invalid("index does not fit usize"))?,
                expected_revision: required_u64(&call.args[2], "expectedRevision")?,
            })
        }
        (IFACE_SESSION, "GetCapabilities") => nullary(call, ControlRequest::GetSessionCapabilities),
        (IFACE_SESSION, "Lock") => session_action(call, SessionAction::Lock),
        (IFACE_SESSION, "LockPreview") => session_action(call, SessionAction::LockPreview),
        (IFACE_SESSION, "Logout") => session_action(call, SessionAction::Logout),
        (IFACE_SESSION, "Suspend") => session_action(call, SessionAction::Suspend),
        (IFACE_SESSION, "Reboot") => session_action(call, SessionAction::Reboot),
        (IFACE_SESSION, "Shutdown") => session_action(call, SessionAction::Shutdown),
        (IFACE_SESSION, "ScreensaverPreview") => {
            session_action(call, SessionAction::ScreensaverPreview)
        }
        (IFACE_SYSTEM, "GetSnapshot") => nullary(call, ControlRequest::GetSystem),
        (IFACE_SYSTEM, member) => system_action_request(call, member),
        _ => Err(ControlError {
            name: flamewm_control_core::error_name(ErrorCode::NotFound),
            code: ErrorCode::NotFound,
            message: format!(
                "unknown Flame Control method {}.{}",
                call.interface, call.member
            ),
        }),
    }
}

/// Encode typed client request arguments for the D-Bus adapter.
pub fn encode_call(request: &ControlRequest) -> Result<WireCall, ControlError> {
    let call = match request {
        ControlRequest::Ping => nullary_call(IFACE_ROOT, "Ping"),
        ControlRequest::GetVersion => nullary_call(IFACE_ROOT, "GetVersion"),
        ControlRequest::GetCapabilities => nullary_call(IFACE_ROOT, "GetCapabilities"),
        ControlRequest::GetShellBootstrap => nullary_call(IFACE_ROOT, "GetShellBootstrap"),
        ControlRequest::GetWindows => nullary_call(IFACE_WINDOWS, "GetWindows"),
        ControlRequest::GetApplications => nullary_call(IFACE_APPLICATIONS, "GetApplications"),
        ControlRequest::LaunchApplication { app, options } => WireCall::new(
            IFACE_APPLICATIONS,
            "LaunchApplication",
            vec![
                WireValue::String(app.as_str().to_owned()),
                WireValue::StringArray(options.extra_args.clone()),
                WireValue::StringArray(options.uris.clone()),
            ],
        ),
        ControlRequest::ActivateWindow(window) => window_call("ActivateWindow", *window),
        ControlRequest::MinimizeWindow(window) => window_call("MinimizeWindow", *window),
        ControlRequest::RestoreWindow(window) => window_call("RestoreWindow", *window),
        ControlRequest::CloseWindow(window) => window_call("CloseWindow", *window),
        ControlRequest::GetSettings => nullary_call(IFACE_SETTINGS, "GetSnapshot"),
        ControlRequest::ApplySettings(transaction) => match &transaction.reset_section {
            Some(section) => WireCall::new(
                IFACE_SETTINGS,
                "ResetSection",
                vec![
                    WireValue::U64(transaction.expected_revision),
                    WireValue::String(section.clone()),
                ],
            ),
            None => WireCall::new(
                IFACE_SETTINGS,
                "Apply",
                vec![
                    WireValue::U64(transaction.expected_revision),
                    WireValue::Dict(
                        transaction
                            .changes
                            .iter()
                            .filter_map(|change| {
                                change
                                    .value
                                    .as_ref()
                                    .map(|value| (change.key.clone(), setting_wire_value(value)))
                            })
                            .collect(),
                    ),
                ],
            ),
        },
        ControlRequest::GetWorkspaces => nullary_call(IFACE_WORKSPACES, "GetSnapshot"),
        ControlRequest::ActivateWorkspace {
            index,
            expected_revision,
        } => workspace_call("Activate", *index, *expected_revision),
        ControlRequest::InsertWorkspaceAfter {
            index,
            expected_revision,
        } => workspace_call("InsertAfter", *index, *expected_revision),
        ControlRequest::RemoveWorkspace {
            index,
            expected_revision,
        } => workspace_call("Remove", *index, *expected_revision),
        ControlRequest::GetDisplays => nullary_call(IFACE_DISPLAYS, "GetSnapshot"),
        ControlRequest::BeginDisplayMode {
            output,
            mode,
            expected_generation,
            ..
        } => WireCall::new(
            IFACE_DISPLAYS,
            "BeginModeChange",
            vec![
                WireValue::String(output.as_str().to_owned()),
                WireValue::U64(mode.0),
                WireValue::U64(*expected_generation),
            ],
        ),
        ControlRequest::KeepDisplayMode { transaction, .. } => {
            WireCall::new(IFACE_DISPLAYS, "Keep", vec![WireValue::U64(transaction.0)])
        }
        ControlRequest::RevertDisplayMode { transaction } => WireCall::new(
            IFACE_DISPLAYS,
            "Revert",
            vec![WireValue::U64(transaction.0)],
        ),
        ControlRequest::SetShellScale {
            output,
            percent,
            expected_revision,
        } => WireCall::new(
            IFACE_DISPLAYS,
            "SetShellScale",
            vec![
                WireValue::String(output.as_str().to_owned()),
                WireValue::U32(u32::from(*percent)),
                WireValue::U64(*expected_revision),
            ],
        ),
        ControlRequest::GetShortcuts => nullary_call(IFACE_SHORTCUTS, "GetSnapshot"),
        ControlRequest::BeginHotkeyCapture {
            action,
            expected_revision,
        } => named_call(IFACE_SHORTCUTS, "BeginCapture", action, *expected_revision),
        ControlRequest::EndHotkeyCapture { lease } => {
            WireCall::new(IFACE_SHORTCUTS, "EndCapture", vec![WireValue::U64(*lease)])
        }
        ControlRequest::SetShortcut {
            action,
            binding,
            expected_revision,
        } => WireCall::new(
            IFACE_SHORTCUTS,
            "SetBinding",
            vec![
                WireValue::String(action.clone()),
                WireValue::String(binding.clone()),
                WireValue::U64(*expected_revision),
            ],
        ),
        ControlRequest::ClearShortcut {
            action,
            expected_revision,
        } => named_call(IFACE_SHORTCUTS, "ClearBinding", action, *expected_revision),
        ControlRequest::ResetShortcut {
            action,
            expected_revision,
        } => named_call(IFACE_SHORTCUTS, "ResetBinding", action, *expected_revision),
        ControlRequest::GetPanels => nullary_call(IFACE_PANELS, "GetSnapshot"),
        ControlRequest::GetWeatherFeed => nullary_call(IFACE_WEATHER, "GetSnapshot"),
        ControlRequest::PublishWeatherFeed { payload } => WireCall::new(
            IFACE_WEATHER,
            "Publish",
            vec![WireValue::String(payload.clone())],
        ),
        ControlRequest::SetPanelEdge {
            output,
            edge,
            expected_revision,
        } => WireCall::new(
            IFACE_PANELS,
            "SetEdge",
            vec![
                WireValue::String(output.as_str().to_owned()),
                WireValue::String(edge_name(*edge).to_owned()),
                WireValue::U64(*expected_revision),
            ],
        ),
        ControlRequest::SetPanelSize {
            output,
            logical_size,
            expected_revision,
        } => WireCall::new(
            IFACE_PANELS,
            "SetSize",
            vec![
                WireValue::String(output.as_str().to_owned()),
                WireValue::U32(u32::from(*logical_size)),
                WireValue::U64(*expected_revision),
            ],
        ),
        ControlRequest::PinApp {
            app,
            expected_revision,
        } => named_call(IFACE_PANELS, "Pin", app.as_str(), *expected_revision),
        ControlRequest::UnpinApp {
            app,
            expected_revision,
        } => named_call(IFACE_PANELS, "Unpin", app.as_str(), *expected_revision),
        ControlRequest::ReorderTask {
            entry_id,
            index,
            expected_revision,
        } => WireCall::new(
            IFACE_PANELS,
            "Reorder",
            vec![
                WireValue::String(entry_id.as_str().to_owned()),
                WireValue::U32(u32_saturating(*index)),
                WireValue::U64(*expected_revision),
            ],
        ),
        ControlRequest::GetSessionCapabilities => nullary_call(IFACE_SESSION, "GetCapabilities"),
        ControlRequest::GetSystem => nullary_call(IFACE_SYSTEM, "GetSnapshot"),
        ControlRequest::SystemAction {
            action,
            expected_revision,
        } => system_action_call(action, *expected_revision),
        ControlRequest::SessionAction(action) => {
            WireCall::new(IFACE_SESSION, session_method(*action), Vec::new())
        }
        ControlRequest::ApplyShortcuts { .. } => {
            return Err(invalid("ApplyShortcuts has no D-Bus method"));
        }
    };
    Ok(call)
}

pub(crate) fn workspace_call(member: &str, index: usize, revision: u64) -> WireCall {
    WireCall::new(
        IFACE_WORKSPACES,
        member,
        vec![
            WireValue::U32(u32_saturating(index)),
            WireValue::U64(revision),
        ],
    )
}

pub(crate) fn named_call(interface: &str, member: &str, name: &str, revision: u64) -> WireCall {
    WireCall::new(
        interface,
        member,
        vec![WireValue::String(name.to_owned()), WireValue::U64(revision)],
    )
}

pub(crate) fn window_call(member: &str, window: WindowRef) -> WireCall {
    WireCall::new(
        IFACE_WINDOWS,
        member,
        vec![WireValue::U64(window.id), WireValue::U64(window.generation)],
    )
}

pub(crate) const fn session_method(action: SessionAction) -> &'static str {
    match action {
        SessionAction::Lock => "Lock",
        SessionAction::LockPreview => "LockPreview",
        SessionAction::Logout => "Logout",
        SessionAction::Suspend => "Suspend",
        SessionAction::Reboot => "Reboot",
        SessionAction::Shutdown => "Shutdown",
        SessionAction::ScreensaverPreview => "ScreensaverPreview",
    }
}

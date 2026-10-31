use super::*;

/// One row of the Alt+Tab switcher as the shell renders it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitcherEntry {
    pub title: String,
    pub app_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlSignal {
    WindowsChanged {
        revision: u64,
    },
    WindowsSnapshotChanged {
        revision: u64,
        windows: Vec<WindowSnapshot>,
    },
    SettingsChanged {
        revision: u64,
        keys: Vec<String>,
    },
    SettingsSnapshotChanged {
        snapshot: SettingsSnapshot,
    },
    WorkspacesChanged {
        revision: u64,
    },
    WorkspacesSnapshotChanged {
        snapshot: WorkspaceSnapshot,
    },
    TopologyChanged {
        generation: u64,
    },
    DisplaysSnapshotChanged {
        snapshot: DisplaySnapshot,
    },
    DisplayTransactionChanged {
        transaction: u64,
        state: String,
        deadline_ms: u64,
    },
    StartMenuToggleRequested,
    LauncherToggleRequested,
    /// Alt+Tab switcher state, pushed by the WM on open, selection change and close. `entries`
    /// are in MRU order; `selected` indexes them. `immediate` asks for the overlay to show
    /// without the hold delay (a second Tab press arrived). The shell only renders this.
    WindowSwitcherChanged {
        open: bool,
        immediate: bool,
        selected: u32,
        entries: Vec<SwitcherEntry>,
    },
    ShortcutsChanged {
        revision: u64,
    },
    PanelsChanged {
        revision: u64,
    },
    PanelsSnapshotChanged {
        snapshot: PanelsSnapshot,
    },
    SystemSnapshotChanged {
        snapshot: SystemSnapshot,
    },
    SystemChanged {
        revision: u64,
    },
    WeatherFeedChanged {
        snapshot: flamewm_api::weather::WeatherFeedSnapshot,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireSignal {
    pub interface: String,
    pub member: String,
    pub args: Vec<WireValue>,
}

#[must_use]
pub fn encode_signal(signal: &ControlSignal) -> WireSignal {
    match signal {
        ControlSignal::WindowsChanged { revision } => {
            revision_signal(IFACE_WINDOWS, "WindowsChanged", *revision)
        }
        ControlSignal::WindowsSnapshotChanged { revision, windows } => WireSignal {
            interface: IFACE_WINDOWS.to_owned(),
            member: "WindowsSnapshotChanged".to_owned(),
            args: vec![
                WireValue::U64(*revision),
                WireValue::Array(windows.iter().map(window_wire_value).collect()),
            ],
        },
        ControlSignal::WeatherFeedChanged { snapshot } => WireSignal {
            interface: IFACE_WEATHER.to_owned(),
            member: "FeedChanged".to_owned(),
            args: vec![WireValue::Dict(weather_feed_dict(snapshot))],
        },
        ControlSignal::SystemChanged { revision } => WireSignal {
            interface: IFACE_SYSTEM.to_owned(),
            member: "SystemChanged".to_owned(),
            args: vec![WireValue::U64(*revision)],
        },
        ControlSignal::SystemSnapshotChanged { snapshot } => WireSignal {
            interface: IFACE_SYSTEM.to_owned(),
            member: "SystemSnapshotChanged".to_owned(),
            args: vec![WireValue::Dict(system_dict(snapshot))],
        },
        ControlSignal::SettingsChanged { revision, keys } => WireSignal {
            interface: IFACE_SETTINGS.to_owned(),
            member: "SettingsChanged".to_owned(),
            args: vec![
                WireValue::U64(*revision),
                WireValue::StringArray(keys.clone()),
            ],
        },
        ControlSignal::SettingsSnapshotChanged { snapshot } => WireSignal {
            interface: IFACE_SETTINGS.to_owned(),
            member: "SettingsSnapshotChanged".to_owned(),
            args: vec![WireValue::Dict(settings_dict(snapshot))],
        },
        ControlSignal::WorkspacesChanged { revision } => {
            revision_signal(IFACE_WORKSPACES, "WorkspacesChanged", *revision)
        }
        ControlSignal::WorkspacesSnapshotChanged { snapshot } => WireSignal {
            interface: IFACE_WORKSPACES.to_owned(),
            member: "WorkspacesSnapshotChanged".to_owned(),
            args: vec![WireValue::Dict(workspace_dict(snapshot))],
        },
        ControlSignal::TopologyChanged { generation } => {
            revision_signal(IFACE_DISPLAYS, "TopologyChanged", *generation)
        }
        ControlSignal::DisplaysSnapshotChanged { snapshot } => WireSignal {
            interface: IFACE_DISPLAYS.to_owned(),
            member: "DisplaysSnapshotChanged".to_owned(),
            args: vec![WireValue::Dict(display_dict(snapshot))],
        },
        ControlSignal::StartMenuToggleRequested => WireSignal {
            interface: IFACE_ROOT.to_owned(),
            member: "StartMenuToggleRequested".to_owned(),
            args: Vec::new(),
        },
        ControlSignal::LauncherToggleRequested => WireSignal {
            interface: IFACE_ROOT.to_owned(),
            member: "LauncherToggleRequested".to_owned(),
            args: Vec::new(),
        },
        ControlSignal::WindowSwitcherChanged {
            open,
            immediate,
            selected,
            entries,
        } => WireSignal {
            interface: IFACE_ROOT.to_owned(),
            member: "WindowSwitcherChanged".to_owned(),
            args: vec![
                WireValue::Bool(*open),
                WireValue::Bool(*immediate),
                WireValue::U32(*selected),
                WireValue::Array(entries.iter().map(switcher_entry_wire_value).collect()),
            ],
        },
        ControlSignal::ShortcutsChanged { revision } => {
            revision_signal(IFACE_SHORTCUTS, "ShortcutsChanged", *revision)
        }
        ControlSignal::PanelsChanged { revision } => {
            revision_signal(IFACE_PANELS, "PanelsChanged", *revision)
        }
        ControlSignal::PanelsSnapshotChanged { snapshot } => WireSignal {
            interface: IFACE_PANELS.to_owned(),
            member: "PanelsSnapshotChanged".to_owned(),
            args: vec![WireValue::Dict(panels_dict(snapshot))],
        },
        ControlSignal::DisplayTransactionChanged {
            transaction,
            state,
            deadline_ms,
        } => WireSignal {
            interface: IFACE_DISPLAYS.to_owned(),
            member: "DisplayTransactionChanged".to_owned(),
            args: vec![
                WireValue::U64(*transaction),
                WireValue::String(state.clone()),
                WireValue::U64(*deadline_ms),
            ],
        },
    }
}

pub fn decode_signal(signal: &WireSignal) -> Result<ControlSignal, ControlError> {
    let call = WireCall::new(&signal.interface, &signal.member, signal.args.clone());
    match (signal.interface.as_str(), signal.member.as_str()) {
        (IFACE_WINDOWS, "WindowsChanged") => Ok(ControlSignal::WindowsChanged {
            revision: signal_revision(&call)?,
        }),
        (IFACE_WINDOWS, "WindowsSnapshotChanged") => {
            expect_arity(&call, 2)?;
            Ok(ControlSignal::WindowsSnapshotChanged {
                revision: required_u64(&call.args[0], "revision")?,
                windows: required_array(&call.args[1], "windows")?
                    .iter()
                    .map(window_snapshot)
                    .collect::<Result<Vec<_>, _>>()?,
            })
        }
        (IFACE_WEATHER, "FeedChanged") => {
            expect_arity(&call, 1)?;
            Ok(ControlSignal::WeatherFeedChanged {
                snapshot: weather_feed_snapshot(required_dict(&call.args[0], "snapshot")?)?,
            })
        }
        (IFACE_SYSTEM, "SystemChanged") => Ok(ControlSignal::SystemChanged {
            revision: signal_revision(&call)?,
        }),
        (IFACE_SYSTEM, "SystemSnapshotChanged") => {
            expect_arity(&call, 1)?;
            Ok(ControlSignal::SystemSnapshotChanged {
                snapshot: system_snapshot(required_dict(&call.args[0], "snapshot")?)?,
            })
        }
        (IFACE_SETTINGS, "SettingsChanged") => {
            expect_arity(&call, 2)?;
            Ok(ControlSignal::SettingsChanged {
                revision: required_u64(&call.args[0], "revision")?,
                keys: required_string_array(&call.args[1], "keys")?.to_vec(),
            })
        }
        (IFACE_SETTINGS, "SettingsSnapshotChanged") => {
            expect_arity(&call, 1)?;
            Ok(ControlSignal::SettingsSnapshotChanged {
                snapshot: settings_snapshot(required_dict(&call.args[0], "snapshot")?)?,
            })
        }
        (IFACE_WORKSPACES, "WorkspacesChanged") => Ok(ControlSignal::WorkspacesChanged {
            revision: signal_revision(&call)?,
        }),
        (IFACE_WORKSPACES, "WorkspacesSnapshotChanged") => {
            expect_arity(&call, 1)?;
            Ok(ControlSignal::WorkspacesSnapshotChanged {
                snapshot: workspace_snapshot(required_dict(&call.args[0], "snapshot")?)?,
            })
        }
        (IFACE_DISPLAYS, "TopologyChanged") => Ok(ControlSignal::TopologyChanged {
            generation: signal_revision(&call)?,
        }),
        (IFACE_DISPLAYS, "DisplaysSnapshotChanged") => {
            expect_arity(&call, 1)?;
            Ok(ControlSignal::DisplaysSnapshotChanged {
                snapshot: display_snapshot(required_dict(&call.args[0], "snapshot")?)?,
            })
        }
        (IFACE_ROOT, "StartMenuToggleRequested") => {
            expect_arity(&call, 0)?;
            Ok(ControlSignal::StartMenuToggleRequested)
        }
        (IFACE_ROOT, "LauncherToggleRequested") => {
            expect_arity(&call, 0)?;
            Ok(ControlSignal::LauncherToggleRequested)
        }
        (IFACE_ROOT, "WindowSwitcherChanged") => {
            expect_arity(&call, 4)?;
            Ok(ControlSignal::WindowSwitcherChanged {
                open: required_bool(&call.args[0], "open")?,
                immediate: required_bool(&call.args[1], "immediate")?,
                selected: required_u32(&call.args[2], "selected")?,
                entries: required_array(&call.args[3], "entries")?
                    .iter()
                    .map(switcher_entry)
                    .collect::<Result<Vec<_>, _>>()?,
            })
        }
        (IFACE_SHORTCUTS, "ShortcutsChanged") => Ok(ControlSignal::ShortcutsChanged {
            revision: signal_revision(&call)?,
        }),
        (IFACE_PANELS, "PanelsChanged") => Ok(ControlSignal::PanelsChanged {
            revision: signal_revision(&call)?,
        }),
        (IFACE_PANELS, "PanelsSnapshotChanged") => {
            expect_arity(&call, 1)?;
            Ok(ControlSignal::PanelsSnapshotChanged {
                snapshot: panels_snapshot(required_dict(&call.args[0], "snapshot")?)?,
            })
        }
        _ => Err(invalid("unknown Flame Control signal")),
    }
}

pub(crate) fn switcher_entry_wire_value(entry: &SwitcherEntry) -> WireValue {
    WireValue::Dict(dict_of([
        ("title", WireValue::String(entry.title.clone())),
        ("appId", WireValue::String(entry.app_id.clone())),
    ]))
}

pub(crate) fn switcher_entry(value: &WireValue) -> Result<SwitcherEntry, ControlError> {
    let dict = required_dict(value, "switcher entry")?;
    Ok(SwitcherEntry {
        title: dict_string(dict, "title")?.to_owned(),
        app_id: dict_string(dict, "appId")?.to_owned(),
    })
}

pub(crate) fn weather_feed_dict(snapshot: &flamewm_api::weather::WeatherFeedSnapshot) -> WireDict {
    dict_of([
        ("revision", WireValue::U64(snapshot.revision)),
        ("payload", WireValue::String(snapshot.payload.clone())),
    ])
}

pub(crate) fn weather_feed_snapshot(
    dict: &WireDict,
) -> Result<flamewm_api::weather::WeatherFeedSnapshot, ControlError> {
    Ok(flamewm_api::weather::WeatherFeedSnapshot {
        revision: dict_u64(dict, "revision")?,
        payload: dict_string(dict, "payload")?.to_owned(),
    })
}

pub(crate) fn revision_signal(interface: &str, member: &str, revision: u64) -> WireSignal {
    WireSignal {
        interface: interface.to_owned(),
        member: member.to_owned(),
        args: vec![WireValue::U64(revision)],
    }
}

pub(crate) fn signal_revision(call: &WireCall) -> Result<u64, ControlError> {
    expect_arity(call, 1)?;
    required_u64(&call.args[0], "revision")
}

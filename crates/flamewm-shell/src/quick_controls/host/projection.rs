use super::*;

pub(crate) fn project_snapshot(
    runtime: &mut SurfaceRuntime,
    surface: SurfaceHandle,
    snapshot: &QuickControlSnapshot,
    kind: QuickControlKind,
) -> Result<(), String> {
    let shell = crate::runtime::ShellSnapshot {
        displays: flamewm_api::display::DisplaySnapshot {
            generation: 0,
            outputs: Vec::new(),
            pending: None,
        },
        panels: flamewm_api::panels::PanelsSnapshot {
            revision: 0,
            panels: Vec::new(),
            tasks: Vec::new(),
            pinned_apps: Vec::new(),
        },
        windows: Vec::new(),
        applications: Vec::new(),
        workspaces: None,
        session: flamewm_api::session::SessionCapabilities::default(),
        system: snapshot.system.clone(),
    };
    runtime
        .with_document(surface, |document| {
            let root = match kind {
                QuickControlKind::Audio => "audio-popup",
                QuickControlKind::Network => "wifi-popup",
                QuickControlKind::Calendar => "clock-popup",
                QuickControlKind::Battery => "battery-popup",
                QuickControlKind::Weather => "weather-popup",
            };
            crate::settings_projection::project_menu_background(
                document,
                &snapshot.settings,
                root,
            )?;
            match kind {
                QuickControlKind::Audio => crate::projection::project_audio_full(
                    document,
                    &shell,
                    snapshot.audio_tab,
                    snapshot.audio_settings,
                    snapshot.audio_scroll_offset,
                ),
                QuickControlKind::Network => {
                    let query = snapshot.network_query.clone();
                    crate::projection::project_network_full(
                        document,
                        &shell,
                        &query,
                        snapshot.network_scroll_offset,
                    )?;
                    crate::projection::project_network_secret(
                        document,
                        &"*".repeat(snapshot.network_secret_masked_len),
                    )?;
                    document.text(
                        "network-secret-error",
                        if snapshot.network_secret_error {
                            "Could not connect. Check the password and try again.".to_owned()
                        } else {
                            String::new()
                        },
                    )
                }
                QuickControlKind::Calendar => crate::projection::project_calendar(
                    document,
                    &snapshot.calendar,
                    crate::projection::local_date(),
                ),
                QuickControlKind::Battery => {
                    crate::projection::project_battery_menu(document, &snapshot.system.power, 268.0)
                }
                QuickControlKind::Weather => project_weather_popup(
                    document,
                    &snapshot.weather.state,
                    &snapshot.weather.settings,
                    &snapshot.weather_search,
                    crate::settings_projection::surface_background(&snapshot.settings),
                ),
            }
        })
        .map_err(|error| format!("{error:?}"))
}

/// Selects the audio tab named by a released tab action and resets the list
/// window. Returns true when the projected tab or window changed, so the open
/// popup must be reprojected to show the new selection.
pub(crate) fn select_audio_tab(snapshot: &mut QuickControlSnapshot, action: &str) -> bool {
    let tab = match action {
        "audio.tab.devices" => AudioTab::Devices,
        "audio.tab.apps" => AudioTab::Applications,
        _ => return false,
    };
    let changed = snapshot.audio_tab != tab || snapshot.audio_scroll_offset != 0;
    snapshot.audio_tab = tab;
    snapshot.audio_scroll_offset = 0;
    changed
}

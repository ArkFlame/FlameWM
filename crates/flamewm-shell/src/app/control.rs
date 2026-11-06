//! Control-request dispatch: applying control results, context-menu system requests, transient-state teardown.

use super::*;

impl ShellLoop {
    /// Shared transient close transaction and semantic state update.
    pub(crate) fn close_transients(
        &mut self,
        runtime: &mut SurfaceRuntime,
    ) -> Result<(), UiBackendError> {
        self.surfaces
            .close_transients(runtime)
            .map_err(UiBackendError::Renderer)?;
        self.surfaces
            .close_launcher(runtime)
            .map_err(UiBackendError::Renderer)?;
        if let Some(lease) = self.popup_session.active().cloned() {
            if !is_quick_popup_kind(&lease.kind) {
                let transition = self.popup_session.close(lease.sequence);
                self.popup_session.commit(&transition);
            }
        }
        self.clear_transient_state();
        self.active_start_launch = None;
        Ok(())
    }

    pub(super) fn clear_transient_state(&mut self) {
        clear_transient_state_fields(
            &mut self.start_open,
            &mut self.start_query,
            &mut self.context_menu,
        );
    }

    /// Single owner of the Start session end. Every Start close path routes
    /// here so the search query never survives into the next open. Other
    /// popups are left alone.
    pub(crate) fn end_start_session(&mut self) {
        end_start_session_fields(&mut self.start_open, &mut self.start_query);
    }

    pub(super) fn select_control_target(
        &mut self,
        requested: Option<flamewm_api::OutputId>,
        interaction: Option<&flamewm_shell::runtime::InteractionContext>,
    ) -> Option<flamewm_shell_core::popup_session::PopupTarget> {
        let snapshot = self.shell.snapshot();
        if let Some(context) = interaction {
            if !interaction_target_is_current(
                requested.as_ref(),
                &context.output,
                context.display_generation,
                context.panels_revision,
                snapshot.displays.generation,
                snapshot.panels.revision,
            ) || self.surfaces.input_output(context.source_surface).as_ref()
                != Some(&context.output)
            {
                return None;
            }
        }
        let panels = self.surfaces.panel_outputs();
        let panel_outputs: Vec<_> = panels.iter().map(|(_, output)| output.clone()).collect();
        let connected_outputs: Vec<_> = snapshot
            .displays
            .outputs
            .iter()
            .filter(|output| output.connected)
            .map(|output| output.id.clone())
            .collect();
        let primary_outputs: Vec<_> = snapshot
            .displays
            .outputs
            .iter()
            .filter(|output| output.connected && output.primary)
            .map(|output| output.id.clone())
            .collect();
        let focused_output = snapshot
            .windows
            .iter()
            .find(|window| window.focused)
            .map(|window| window.output.clone());
        let pointer_output = self.pointer_position.and_then(|(x, y)| {
            flamewm_shell::runtime::output_at_root_point(
                &snapshot.displays.outputs,
                flamewm_api::Point { x, y },
            )
        });
        let output = resolve_control_output(
            requested.as_ref(),
            &panel_outputs,
            &connected_outputs,
            focused_output.as_ref(),
            pointer_output.as_ref(),
            &primary_outputs,
        );
        let output = output?;
        let display_generation = snapshot.displays.generation;
        let panels_revision = snapshot.panels.revision;
        self.shell.select_panel_output(output.clone());
        if !self.surfaces.select_panel_output(&output) {
            return None;
        }
        Some(flamewm_shell_core::popup_session::PopupTarget {
            output,
            topology_generation: display_generation,
            panels_revision,
        })
    }

    pub(crate) fn apply_controls(
        &mut self,
        runtime: &mut SurfaceRuntime,
    ) -> Result<(), UiBackendError> {
        let controls: Vec<_> = self.shell.drain_controls().collect();
        for control in controls {
            match control {
                ShellControl::ToggleStart {
                    output,
                    interaction,
                } => {
                    let was_open = self.surfaces.is_start_group_open();
                    let open_output = was_open
                        .then(|| self.surfaces.input_output(self.surfaces.start))
                        .flatten();
                    let Some(target) = self.select_control_target(output, interaction.as_ref())
                    else {
                        continue;
                    };
                    let target_output = target.output.clone();
                    let action =
                        start_toggle_action(was_open, open_output.as_ref(), &target_output);
                    // C07: start open owns one total around the open path.
                    // F10: read current context generation; refuse when
                    // edge/geometry is inconsistent.
                    self.note_popup_generation("start");
                    if self.refuse_popup_on_inconsistent_geometry().is_some() {
                        continue;
                    }
                    let transition = if action == StartToggleAction::Close {
                        self.popup_session
                            .toggle(flamewm_shell_core::popup_session::PopupKind::Start, target)
                    } else {
                        if self.quick_kind_open.is_some() {
                            self.close_quick_control_nonfatal();
                        }
                        self.popup_session
                            .open(flamewm_shell_core::popup_session::PopupKind::Start, target)
                    };
                    let Ok(transition) = transition else { continue };
                    if action == StartToggleAction::Close {
                        let _close = shell_stage!("shell.start.open.close");
                        match self.surfaces.toggle_start(runtime) {
                            Ok(()) => {
                                self.popup_session.commit(&transition);
                                self.end_start_session();
                            }
                            Err(error) if is_popup_refusal(&error) => {}
                            Err(error) => return Err(UiBackendError::Renderer(error)),
                        }
                    } else {
                        self.close_launcher(runtime)?;
                        let started = Instant::now();
                        let _total = shell_stage!("shell.start.open.total");
                        if action == StartToggleAction::Open
                            || action == StartToggleAction::Transfer
                        {
                            // Reopening the menu clears any stale launch error.
                            self.start_error = None;
                            self.active_start_launch = None;
                        } else {
                            self.surfaces
                                .close_transients(runtime)
                                .map_err(UiBackendError::Renderer)?;
                        }
                        if self.open_start_surface(runtime, &target_output, started)? {
                            self.start_open = true;
                            self.start_selection.reset();
                            self.start_scroll = 0;
                            self.popup_session.commit(&transition);
                        }
                    }
                }
                ShellControl::ToggleLauncher {
                    output,
                    interaction,
                } => {
                    self.toggle_launcher(runtime, output, interaction.as_ref())?;
                }
                ShellControl::CloseStart | ShellControl::ClosePopovers => {
                    self.close_transients(runtime)?;
                }
                ShellControl::SelectStartCategory {
                    category,
                    output,
                    interaction,
                } => {
                    let Some(target) = self.select_control_target(output, interaction.as_ref())
                    else {
                        continue;
                    };
                    self.start_category = category;
                    self.start_open = true;
                    self.start_selection.reset();
                    self.start_scroll = 0;
                    // F10: read current context generation; refuse when
                    // edge/geometry is inconsistent.
                    self.note_popup_generation("start");
                    if self.refuse_popup_on_inconsistent_geometry().is_some() {
                        continue;
                    }
                    if self.quick_kind_open.is_some() {
                        self.close_quick_control_nonfatal();
                    }
                    let same_session = self.popup_session.active().is_some_and(|lease| {
                        lease.kind == flamewm_shell_core::popup_session::PopupKind::Start
                            && lease.output == target.output
                    });
                    if !same_session {
                        self.active_start_launch = None;
                        self.start_error = None;
                    }
                    let Ok(transition) = self.popup_session.open(
                        flamewm_shell_core::popup_session::PopupKind::Start,
                        target.clone(),
                    ) else {
                        continue;
                    };
                    // C07: SelectStartCategory owns the start total.
                    let started = Instant::now();
                    let _total = shell_stage!("shell.start.open.total");
                    if self.open_start_surface(runtime, &target.output, started)? {
                        self.popup_session.commit(&transition);
                    }
                }
                ShellControl::OpenPopover {
                    name,
                    output,
                    interaction,
                } => {
                    let Some(target) = self.select_control_target(output, interaction.as_ref())
                    else {
                        continue;
                    };
                    // J07: media stays in-process; audio/network/clock/battery
                    // forward to the supervisor, then return immediately.
                    if matches!(
                        name.as_str(),
                        "volume" | "audio" | "network" | "clock" | "battery" | "weather"
                    ) {
                        self.open_quick_control(runtime, &name, target);
                        self.end_start_session();
                        continue;
                    }
                    self.close_quick_control_nonfatal();
                    // F10: read current context generation; refuse when
                    // edge/geometry is inconsistent.
                    self.note_popup_generation("media");
                    if self.refuse_popup_on_inconsistent_geometry().is_some() {
                        continue;
                    }
                    self.project_status_content(runtime, &name)?;
                    let Ok(transition) = self.popup_session.open(
                        flamewm_shell_core::popup_session::PopupKind::Media,
                        target.clone(),
                    ) else {
                        continue;
                    };
                    match self
                        .surfaces
                        .open_status_for_output(runtime, &name, &target.output)
                    {
                        Ok(()) => {
                            self.popup_session.commit(&transition);
                        }
                        Err(error) if is_popup_refusal(&error) => {}
                        Err(error) => return Err(UiBackendError::Renderer(error)),
                    }
                    self.end_start_session();
                }
                ShellControl::SystemAction {
                    action,
                    expected_revision,
                } => {
                    self.submit(ControlRequest::SystemAction {
                        action,
                        expected_revision,
                    });
                }
                ShellControl::OpenContextMenu(state) => {
                    let Some(output) = state.output.clone() else {
                        continue;
                    };
                    let Some(target) =
                        self.select_control_target(Some(output), state.interaction.as_ref())
                    else {
                        continue;
                    };
                    // C04: task/shell context menus invalidate the helper
                    // anchor. F10: refuse when edge/geometry is inconsistent.
                    if self.refuse_popup_on_inconsistent_geometry().is_some() {
                        continue;
                    }
                    let started = Instant::now();
                    let _total = shell_stage!("shell.context.open.total");
                    self.close_quick_control_nonfatal();
                    let Ok(transition) = self.popup_session.open(
                        flamewm_shell_core::popup_session::PopupKind::TaskContext,
                        target,
                    ) else {
                        continue;
                    };
                    let settings = self
                        .settings_cache
                        .clone()
                        .unwrap_or_else(|| flamewm_api::settings::SettingsSnapshot::new(0));
                    {
                        let _project = shell_stage!("shell.context.open.project");
                        runtime.with_document(self.surfaces.task_menu, |document| {
                            projection::project_context_menu(
                                document,
                                self.shell.snapshot(),
                                &settings,
                                Some(&state),
                            )
                        })?;
                    }
                    let backdrop =
                        crate::entry::settings_projection::start_backdrop_percents(&settings);
                    self.context_menu = Some(state.clone());
                    let _measure = shell_stage!("shell.context.open.measure");
                    let _place = shell_stage!("shell.context.open.place");
                    let _prepare = shell_stage!("shell.context.open.prepare");
                    let opened = self
                        .surfaces
                        .open_context_menu(runtime, &state, self.shell.snapshot(), backdrop)
                        .map_err(UiBackendError::Renderer);
                    let _present = shell_stage!("shell.context.open.present");
                    shell_slow_note("context", started);
                    opened?;
                    self.popup_session.commit(&transition);
                }
                ShellControl::CloseContextMenu => {
                    let _close = shell_stage!("shell.context.open.close");
                    self.close_transients(runtime)?;
                }
                ShellControl::SessionAction(action) => {
                    self.submit(ControlRequest::SessionAction(action));
                    self.close_transients(runtime)?;
                    self.end_start_session();
                }
                ShellControl::Launch(_)
                | ShellControl::ActivateWindow(_)
                | ShellControl::MinimizeWindow(_)
                | ShellControl::RestoreWindow(_) => {}
            }
        }
        Ok(())
    }

    pub(crate) fn context_system_request(&mut self, action: &str) -> Option<ControlRequest> {
        if action == "audio.tab.devices" {
            self.audio_tab = AudioTab::Devices;
            return None;
        }
        if action == "audio.tab.apps" {
            self.audio_tab = AudioTab::Applications;
            return None;
        }
        if action == "audio.raise.toggle" {
            self.audio_settings.raise_maximum = !self.audio_settings.raise_maximum;
            let raise = self.audio_settings.raise_maximum;
            let revision = self.settings_revision;
            self.submit(ControlRequest::ApplySettings(SettingsTransaction {
                expected_revision: revision,
                changes: vec![flamewm_api::settings::SettingsChange {
                    key: AudioSettings::SETTINGS_KEY.to_owned(),
                    value: Some(SettingValue::Boolean(raise)),
                }],
                reset_section: None,
            }));
            return None;
        }
        if action == "network.filter.clear" {
            self.network_query.filter.clear();
            return None;
        }
        let snapshot = self.shell.snapshot();
        if let Some(command) = action.strip_prefix("media.") {
            return media_system_request(&snapshot.system.media, snapshot.system.revision, command);
        }
        let audio_row = if matches!(action, "volume.mute" | "volume.down" | "volume.up") {
            flamewm_shell::taskbar::status::audio::default_endpoint_row(&snapshot.system.audio)
        } else if let Some((kind_slot, _)) = action
            .strip_prefix("audio.")
            .and_then(|value| value.rsplit_once('.'))
        {
            let (_, slot) = kind_slot.rsplit_once('.')?;
            let slot = slot.parse::<usize>().ok()?.checked_sub(1)?;
            flamewm_shell::taskbar::status::audio::popover_view(&snapshot.system)
                .and_then(|popover| popover.visible_rows.get(slot).cloned())
                .map(|row| flamewm_shell::taskbar::status::audio::RowSource {
                    kind: row.kind,
                    id: row.id,
                    label: row.label,
                    volume_percent: row.volume_percent,
                    muted: row.muted,
                    selected: row.selected,
                })
        } else {
            None
        };
        if let Some(row) = audio_row {
            let command = action.rsplit('.').next()?;
            let cap = self.audio_settings.max_percent();
            return match command {
                "up" => self.shell.audio_system_action_for_row(
                    &row.id,
                    Some(
                        self.audio_settings
                            .clamp_percent(row.volume_percent.saturating_add(5)),
                    ),
                    None,
                ),
                "down" => self.shell.audio_system_action_for_row(
                    &row.id,
                    Some(row.volume_percent.saturating_sub(5).min(cap)),
                    None,
                ),
                "mute" => self
                    .shell
                    .audio_system_action_for_row(&row.id, None, Some(!row.muted)),
                _ => None,
            };
        }
        if action == "network.wifi.toggle" {
            return Some(ControlRequest::SystemAction {
                action: flamewm_api::system::SystemAction::SetWifiEnabled(
                    !snapshot.system.network.wifi_enabled,
                ),
                expected_revision: snapshot.system.revision,
            });
        }
        if action == "network.scan" {
            return self.shell.network_system_action_for_path("", true, false);
        }
        if action == "network.disconnect" {
            return self.shell.network_system_action_for_path("", false, true);
        }
        if let Some(slot) = action
            .strip_prefix("network.connect.")
            .and_then(|value| value.parse::<usize>().ok())
            .and_then(|slot| slot.checked_sub(1))
        {
            let path = flamewm_shell::taskbar::status::network::popover_view(&snapshot.system)
                .and_then(|popover| popover.visible_rows.get(slot).map(|row| row.path.clone()))?;
            return self
                .shell
                .network_system_action_for_path(&path, false, false);
        }
        if action == "network.secret.cancel" {
            return self.network_secret_request(false);
        }
        if action == "network.secret.submit" {
            return self.network_secret_request(true);
        }
        None
    }
}

/// Output rect and UI scale for the shell's active output: the shown panel's output, else the
/// snapshot's primary, connected or first output. Falls back to the work area at 100%.
pub(super) fn active_output_target(
    surfaces: &ShellSurfaces,
    snapshot: &ShellSnapshot,
) -> (flamewm_api::Rect, f32) {
    let panel = surfaces.placement().output.clone();
    let panel = flamewm_shell::osd::OsdOutput {
        geometry: panel.geometry,
        scale_percent: panel.shell_scale_percent,
        primary: panel.primary,
        connected: panel.connected,
    };
    let outputs = snapshot
        .displays
        .outputs
        .iter()
        .map(|output| flamewm_shell::osd::OsdOutput {
            geometry: output.geometry,
            scale_percent: output.shell_scale_percent,
            primary: output.primary,
            connected: output.connected,
        })
        .collect::<Vec<_>>();
    match flamewm_shell::osd::target_output(Some(panel), &outputs) {
        Some(target) => (target.geometry, f32::from(target.scale_percent) / 100.0),
        None => (surfaces.work_area(), 1.0),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn control_output_prefers_keyboard_focus_and_rejects_invalid_explicit_targets() {
        use flamewm_api::OutputId;
        let a = OutputId::new("A");
        let b = OutputId::new("B");
        let panel_outputs = [a.clone(), b.clone()];
        let connected = panel_outputs.clone();
        let primary = [a.clone()];
        assert_eq!(
            super::resolve_control_output(
                None,
                &panel_outputs,
                &connected,
                Some(&b),
                Some(&a),
                &primary
            ),
            Some(b.clone()),
            "global keyboard action follows focused window before pointer history"
        );
        assert_eq!(
            super::resolve_control_output(
                Some(&OutputId::new("gone")),
                &panel_outputs,
                &connected,
                Some(&b),
                Some(&a),
                &primary
            ),
            None,
            "an explicit stale target must not fall back to another panel"
        );
        assert_eq!(
            super::resolve_control_output(
                None,
                &panel_outputs,
                &connected,
                None,
                Some(&b),
                &primary
            ),
            Some(b),
            "pointer is the fallback when no window is focused"
        );
    }

    #[test]
    fn interaction_context_rejects_output_and_snapshot_drift() {
        let a = flamewm_api::OutputId::new("A");
        let b = flamewm_api::OutputId::new("B");
        assert!(super::interaction_target_is_current(
            Some(&b),
            &b,
            7,
            11,
            7,
            11
        ));
        assert!(!super::interaction_target_is_current(
            Some(&a),
            &b,
            7,
            11,
            7,
            11
        ));
        assert!(!super::interaction_target_is_current(
            Some(&b),
            &b,
            6,
            11,
            7,
            11
        ));
        assert!(!super::interaction_target_is_current(
            Some(&b),
            &b,
            7,
            10,
            7,
            11
        ));
    }

    #[test]
    fn media_node_actions_route_to_active_player_with_revision() {
        let media = flamewm_api::system::MediaSnapshot {
            active_bus_name: "org.mpris.MediaPlayer2.test".to_owned(),
            ..Default::default()
        };
        let bus = "org.mpris.MediaPlayer2.test";
        let cases = [
            (
                "previous",
                flamewm_api::system::SystemAction::Previous {
                    bus_name: bus.to_owned(),
                },
            ),
            (
                "playpause",
                flamewm_api::system::SystemAction::PlayPause {
                    bus_name: bus.to_owned(),
                },
            ),
            (
                "next",
                flamewm_api::system::SystemAction::Next {
                    bus_name: bus.to_owned(),
                },
            ),
        ];
        for (command, expected) in cases {
            match super::media_system_request(&media, 42, command) {
                Some(super::ControlRequest::SystemAction {
                    action,
                    expected_revision,
                }) => {
                    assert_eq!(format!("{action:?}"), format!("{expected:?}"), "{command}");
                    assert_eq!(expected_revision, 42, "{command}");
                }
                _ => panic!("media.{command} must produce a SystemAction request"),
            }
        }
    }

    #[test]
    fn media_node_actions_without_player_or_unknown_command_are_dropped() {
        let no_player = flamewm_api::system::MediaSnapshot::default();
        for command in ["previous", "playpause", "next"] {
            assert!(
                super::media_system_request(&no_player, 1, command).is_none(),
                "media.{command} without an active player must be dropped"
            );
        }
        let media = flamewm_api::system::MediaSnapshot {
            active_bus_name: "org.mpris.MediaPlayer2.test".to_owned(),
            ..Default::default()
        };
        assert!(super::media_system_request(&media, 1, "stop").is_none());
    }

    #[test]
    fn escape_close_clears_start_and_transient_state() {
        let mut start_open = true;
        let mut start_query = "fire".to_owned();
        let mut context_menu = Some(flamewm_shell::ContextMenuState {
            kind: flamewm_shell::ContextMenuKind::Workspace { index: 0 },
            revision: 1,
            output: None,
            interaction: None,
            anchor: None,
        });

        super::clear_transient_state_fields(&mut start_open, &mut start_query, &mut context_menu);

        assert!(!start_open);
        assert!(start_query.is_empty(), "close clears the search query");
        assert!(context_menu.is_none());
    }
}

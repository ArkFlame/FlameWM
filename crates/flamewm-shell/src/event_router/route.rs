use super::overflow_keys::{
    menu_row_actions, overflow_activation_index, overflow_menu_closes_on, overflow_menu_step,
};
use super::*;

impl ShellLoop {
    /// Applies root presses seen by the quick-popup observer. A press outside every
    /// quick popup dismisses the open quick popup and latches its group, so the
    /// opener release of the same click is consumed instead of reopening it.
    pub(crate) fn drain_raw_press_observations(&mut self, runtime: &mut SurfaceRuntime) {
        for observation in runtime.take_raw_press_observations() {
            let Some(kind) = self.quick_kind_open else {
                break;
            };
            if !raw_press_is_outside_quick_popups(observation.child_window_title.as_deref()) {
                continue;
            }
            self.dismiss_latch = Some(DismissLatch {
                group: quick_popup_group(kind),
                at: Instant::now(),
            });
            self.close_quick_control_nonfatal();
        }
    }

    pub(crate) fn route_event(
        &mut self,
        event: SurfaceEvent,
        runtime: &mut SurfaceRuntime,
    ) -> Result<(), UiBackendError> {
        if Some(event.surface) == self.dock_surface {
            return self.route_dock_event(runtime, &event);
        }
        if Some(event.surface) == self.dock_stage.reveal {
            self.route_dock_reveal(runtime, &event);
            return Ok(());
        }
        if Some(event.surface) == self.dock_menu {
            return self.route_dock_menu_event(runtime, &event);
        }
        let output = match resolve_interaction_origin(self.surfaces.input_output(event.surface)) {
            InteractionOrigin::Ignore => return Ok(()),
            InteractionOrigin::Output(output) => output,
        };
        if !interaction_origin_is_connected(
            &output,
            self.shell
                .snapshot()
                .displays
                .outputs
                .iter()
                .map(|candidate| (&candidate.id, candidate.connected)),
        ) || !self.surfaces.select_panel_output(&output)
        {
            return Ok(());
        }
        let snapshot = self.shell.snapshot();
        let interaction = InteractionContext {
            source_surface: event.surface,
            output: output.clone(),
            display_generation: snapshot.displays.generation,
            panels_revision: snapshot.panels.revision,
        };
        self.shell.select_panel_output(output.clone());
        // Presses are queued during the same reactor step, ahead of this release.
        self.drain_raw_press_observations(runtime);
        let action = event.action.action.as_str();
        // never activate on Secondary.
        let button = PointerButton::from_raw_x(event.action.button);
        if event.action.phase == ActionPhase::Press {
            if let Some(slot) = slot_of(action, "task.slot.") {
                self.capture_task_target_for_output(slot, &output);
            } else {
                self.task_press_target = None;
            }
        } else if !release_keeps_task_press(
            event.action.phase,
            event.action.inside,
            action,
            self.task_drag.is_some(),
        ) {
            self.task_press_target = None;
        }
        // Wheel ticks arrive as the hit node's action name plus the wheel button (any phase);
        // the separate "scroll" event for the same tick matches neither branch below.
        if matches!(button, PointerButton::WheelUp | PointerButton::WheelDown)
            && self.start_open
            && slot_of(action, "start.app.").is_some()
        {
            // Start application list: 3 rows per tick, clamped by the list itself.
            let delta = if button == PointerButton::WheelUp {
                -crate::projection::START_WHEEL_ROWS
            } else {
                crate::projection::START_WHEEL_ROWS
            };
            self.wheel_start_list(runtime, delta)?;
            return Ok(());
        }
        if matches!(button, PointerButton::WheelUp | PointerButton::WheelDown) {
            if let Some(slot) = slot_of(action, "task.slot.") {
                // Per-app volume: one notch over a task button steps that app's playback stream.
                let up = button == PointerButton::WheelUp;
                let cap = self.audio_settings.max_percent();
                let snapshot = self.shell.snapshot();
                let request = self
                    .shell
                    .task_target_for_slot_on_output(slot, &output)
                    .and_then(|target| {
                        flamewm_shell::taskbar::tasks::app_volume::app_volume_request(
                            &snapshot.system,
                            target.app_id.0.as_str(),
                            cap,
                            up,
                        )
                    });
                if let Some(request) = request {
                    self.submit(request);
                }
                return Ok(());
            }
            // One step per wheel tick: the node-targeted event is delivered once per X tick.
            // The request is enqueued on the control dispatcher; the OSD follows the snapshot.
            if action == TRAY_VOLUME_ACTION {
                let cap = self.audio_settings.max_percent();
                let up = button == PointerButton::WheelUp;
                let snapshot = self.shell.snapshot();
                if let Some(request) = flamewm_shell::taskbar::status::audio::wheel_volume_request(
                    &snapshot.system,
                    cap,
                    up,
                ) {
                    self.submit(request);
                }
            }
            return Ok(());
        }
        if self.route_start_app_drag(&event, runtime)? {
            return Ok(());
        }
        let release_inside = event.action.phase == ActionPhase::Release && event.action.inside;
        if release_inside && matches!(button, PointerButton::Secondary) {
            return self.route_secondary(&event, runtime, &interaction);
        }
        if self.route_task_drag(&event, runtime, &output) {
            return Ok(());
        }
        // ESC-V3: keyboard events carry button 0; gating them as
        // non-primary pointer releases drops every real Escape press
        // before the keyboard arms below. Exempt them here.
        let is_keyboard = action == "keyboard.input";
        if non_primary_release_is_dropped(
            action,
            event.action.phase == ActionPhase::Release,
            button,
        ) {
            return Ok(());
        }
        if action == "keyboard.input" && event.action.phase == ActionPhase::Release {
            let input = event.action.text.as_deref().unwrap_or_default();
            if self.surfaces.is_polkit_open() {
                return self.route_polkit_text(runtime, input);
            }
            if self.surfaces.is_bluetooth_open() {
                return self.route_bluetooth_text(runtime, input);
            }
            // Start with a query: the Start branch below clears it; the generic close must not run first.
            if input == "\u{1b}" && !escape_clears_start_query(self.start_open, &self.start_query) {
                if let Some(request) = self.network_secret_request(false) {
                    self.submit(request);
                }
                self.clear_network_secret(runtime)?;
                self.start_query.clear();
                self.start_scroll = 0;
                self.close_quick_control_nonfatal();
                return self.close_transients(runtime);
            }
            if self
                .shell
                .snapshot()
                .system
                .network
                .pending_secret
                .is_some()
            {
                match input {
                    "\u{8}" => {
                        self.network_secret.pop();
                    }
                    "\n" => {
                        if let Some(request) = self.network_secret_request(true) {
                            self.submit(request);
                            self.clear_network_secret(runtime)?;
                        }
                    }
                    value if !value.is_empty() => self.network_secret.push_str(value),
                    _ => {}
                }
                self.project_network_secret(runtime)?;
                return Ok(());
            }
        }
        let typed = (is_keyboard && event.action.phase == ActionPhase::Release)
            .then_some(event.action.text.as_deref())
            .flatten();
        let menu_rows = menu_row_actions(self.context_menu.as_ref(), self.shell.snapshot());
        let overflow_rows = menu_rows.len();
        if action == "__flamewm.ui.key" && event.action.phase == ActionPhase::Press {
            let payload = event.action.text.as_deref().unwrap_or_default();
            if overflow_menu_closes_on(payload, overflow_rows) {
                return self.close_transients(runtime);
            }
            if let Some(next) = overflow_menu_step(payload, overflow_rows, self.overflow_row) {
                self.overflow_row = next;
                return self.project_overflow_menu(runtime);
            }
        }
        if !self.surfaces.is_media_open() {
            self.media_focus = None;
        }
        if self.surfaces.is_media_open() {
            // Media card focus: Tab walks the transport stops; Enter or Space on a focused stop
            // activates it and keeps focus there. Escape closes the card on the generic path.
            if action == "__flamewm.ui.key" && event.action.phase == ActionPhase::Press {
                let payload = event.action.text.as_deref().unwrap_or_default();
                if let Some(next) = super::media_keys::media_focus_after_tab(
                    payload,
                    self.media_focus,
                    super::media_keys::MEDIA_FOCUS_STOPS,
                ) {
                    self.media_focus = Some(next);
                    return self.project_media_focus(runtime);
                }
            }
            if let Some(media_action) = super::media_keys::media_activation(typed, self.media_focus)
            {
                if let Some(request) = self.context_system_request(media_action) {
                    self.submit(request);
                }
                return Ok(());
            }
        }
        if self.surfaces.is_usage_open() {
            // Usage card focus: Tab walks its stops; Enter or Space on a focused stop closes it.
            if action == "__flamewm.ui.key" && event.action.phase == ActionPhase::Press {
                let payload = event.action.text.as_deref().unwrap_or_default();
                if let Some(next) = super::usage_keys::usage_focus_after_tab(
                    payload,
                    self.usage_focus,
                    super::usage_keys::USAGE_FOCUS_STOPS,
                ) {
                    self.usage_focus = Some(next);
                    return self.project_usage_card(runtime);
                }
            }
            if super::usage_keys::usage_activates(typed, self.usage_focus) {
                self.usage_focus = None;
                return self.close_transients(runtime);
            }
        }
        if let Some(index) = overflow_activation_index(typed, overflow_rows, self.overflow_row) {
            self.activate_menu_row(runtime, &menu_rows[index])?;
            return self.apply_controls(runtime);
        }
        let decision = outside_release_decision(
            action,
            event.action.phase == ActionPhase::Release,
            is_keyboard,
            event.action.inside,
            self.quick_audio_open,
        );
        if event.action.phase == ActionPhase::Release
            && event.action.inside
            && latch_consumes_opener(&mut self.dismiss_latch, action, Instant::now())
        {
            return Ok(());
        }
        match decision {
            OutsideReleaseDecision::Ignore => {}
            // Toggle-closed and close share the same teardown; the early
            // return is what prevents the same-turn OpenPopover reopen.
            OutsideReleaseDecision::Close | OutsideReleaseDecision::ToggleTrayVolumeClosed => {
                self.dismiss_latch = self.open_popup_group().map(|group| DismissLatch {
                    group,
                    at: Instant::now(),
                });
                self.shell.dispatch("popover.close");
                // C04: geometry-invalidating or menu-opening paths also drop
                // any helper popup on the same turn (nonfatal, loop survives).
                self.close_quick_control_nonfatal();
                return self.apply_controls(runtime);
            }
        }

        if event.action.phase == ActionPhase::Release && event.action.inside {
            if action.starts_with("polkit.") && self.surfaces.is_polkit_open() {
                return self.route_polkit_action(runtime, action, event.action.text.as_deref());
            }
            if action.starts_with("bluetooth.") && self.surfaces.is_bluetooth_open() {
                return self.route_bluetooth_action(runtime, action);
            }
            if action == crate::assistant_popover::ACTION {
                self.shell.dispatch("popover.close");
                self.close_quick_control_nonfatal();
                if let Some(panel) = self.surfaces.panel_for_output(&output) {
                    if let Err(error) = crate::assistant_popover::launch(runtime, panel) {
                        eprintln!("flamewm-shell: {error}");
                    }
                }
                return self.apply_controls(runtime);
            }
            if let Some(request) = self.context_system_request(action) {
                self.submit(request);
                if action == "network.secret.submit" || action == "network.secret.cancel" {
                    self.clear_network_secret(runtime)?;
                }
                return Ok(());
            }
            if self.activate_menu_row(runtime, action)? {
                return self.apply_controls(runtime);
            }
        }

        if action == "start.search"
            && event.action.phase == ActionPhase::Release
            && event.action.inside
        {
            // C07: start open via the search affordance owns the same
            // start total as the ToggleStart path. F10: read current
            // context generation; refuse on inconsistency.
            self.note_popup_generation("start");
            if self.refuse_popup_on_inconsistent_geometry().is_some() {
                return Ok(());
            }
            self.start_open = true;
            self.start_selection.reset();
            self.start_scroll = 0;
            let started = Instant::now();
            let _total = shell_stage!("shell.start.open.total");
            self.project_start(runtime)?;
            let _measure = shell_stage!("shell.start.open.measure");
            let _place = shell_stage!("shell.start.open.place");
            let _prepare = shell_stage!("shell.start.open.prepare");
            let opened = self
                .surfaces
                .open_start_group_for_output(runtime, &interaction.output);
            let _present = shell_stage!("shell.start.open.present");
            shell_slow_note("start", started);
            match opened {
                Ok(()) => {}
                Err(error) if is_popup_refusal(&error) => {}
                Err(error) => return Err(UiBackendError::Renderer(error)),
            }
            return Ok(());
        }

        // Launcher owns text, navigation and row activation while open. Escape is
        // handled above by the generic keyboard path (close_transients).
        if action == "keyboard.input"
            && event.action.phase == ActionPhase::Release
            && event.action.inside
            && self
                .route_launcher_text(runtime, event.action.text.as_deref().unwrap_or_default())?
        {
            return Ok(());
        }
        if action == "__flamewm.ui.key" && event.action.phase == ActionPhase::Press {
            let payload = event.action.text.as_deref().unwrap_or_default();
            if self.route_launcher_key(runtime, payload)? {
                return Ok(());
            }
        }
        if action == "__flamewm.ui.key"
            && event.action.phase == ActionPhase::Press
            && self.start_open
        {
            let payload = event.action.text.as_deref().unwrap_or_default();
            if let Some((modifiers, key)) = flamewm_shell::launcher::parse_typed_key(payload) {
                if key == "Return" || key == "KP_Enter" {
                    self.launch_start_slot(
                        runtime,
                        self.start_selection.highlighted().unwrap_or(0),
                    )?;
                    return Ok(());
                }
                if let Some(delta) = flamewm_shell::launcher::navigation_delta(modifiers, key) {
                    self.move_start_selection(runtime, delta)?;
                    return Ok(());
                }
            }
        }
        if let Some(slot) = slot_of(action, "launcher.app.") {
            if event.action.inside {
                match event.action.phase {
                    ActionPhase::Release => self.click_launcher_row(runtime, slot)?,
                    ActionPhase::Hover => self.route_launcher_hover(runtime, slot)?,
                    _ => {}
                }
            } else if event.action.phase == ActionPhase::Hover {
                self.route_launcher_leave(runtime)?;
            }
            return Ok(());
        }

        if action == "keyboard.input"
            && event.action.phase == ActionPhase::Release
            && event.action.inside
            && self.start_open
        {
            let input = event.action.text.as_deref().unwrap_or_default();
            match input {
                "\u{8}" => {
                    self.start_query.pop();
                    self.start_selection.reset();
                    self.start_scroll = 0;
                    self.project_start(runtime)?;
                }
                "\u{1b}" => {
                    if self.start_query.is_empty() {
                        self.close_transients(runtime)?;
                    } else {
                        self.start_query.clear();
                        self.start_selection.reset();
                        self.start_scroll = 0;
                        self.project_start(runtime)?;
                    }
                }
                // Return launches on the typed-key path above; this text copy must not launch again.
                "\n" => {}
                value if !value.is_empty() => {
                    self.start_query.push_str(value);
                    self.start_selection.reset();
                    self.start_scroll = 0;
                    self.project_start(runtime)?;
                }
                _ => {}
            }
            return Ok(());
        }

        if action == "desktop.show" {
            if event.action.phase == ActionPhase::Release && event.action.inside {
                let (requests, saved) = flamewm_shell::taskbar::show_desktop::plan(
                    &self.shell.snapshot().windows,
                    &self.desktop_hidden,
                );
                self.desktop_hidden = saved;
                for request in requests {
                    self.submit(request);
                }
            }
            return Ok(());
        }

        if let Some(slot) = slot_of(action, "start.app.") {
            if event.action.inside {
                match event.action.phase {
                    ActionPhase::Release => self.launch_start_slot(runtime, slot)?,
                    ActionPhase::Hover => self.hover_start_slot(runtime, slot)?,
                    _ => {}
                }
            } else if event.action.phase == ActionPhase::Hover {
                self.reset_start_selection(runtime)?;
            }
            return Ok(());
        }

        if action == "task.overflow" {
            if event.action.phase == ActionPhase::Release && event.action.inside {
                if let Some(pointer) = self.surfaces.root_pointer_for_output(
                    runtime,
                    &output,
                    event.action.x,
                    event.action.y,
                ) {
                    self.shell.open_task_overflow_menu(Some(pointer));
                    self.close_quick_control_nonfatal();
                    return self.apply_controls(runtime);
                }
            }
            return Ok(());
        }

        if let Some(slot) = slot_of(action, "task.slot.") {
            if event.action.phase == ActionPhase::Release && event.action.inside {
                self.click_task_slot(runtime, slot)?;
            }
            return Ok(());
        }

        if action.starts_with("workspace-") || action.starts_with("workspace.") {
            if event.action.phase == ActionPhase::Release && event.action.inside {
                // C07: workspace action owns one operation. The normal path
                // performs no control fetch; the fetch counter stays 0.
                let started = Instant::now();
                {
                    let _submit = shell_stage!("shell.workspace.action.submit");
                    if let Some(request) = self.shell.control_request_for_action(action) {
                        self.submit(request);
                    }
                }
                {
                    let _apply = shell_stage!("shell.workspace.action.signal.apply");
                    // C04: workspace change invalidates the helper anchor.
                    self.close_quick_control_nonfatal();
                }
                {
                    let _project = shell_stage!("shell.workspace.action.project");
                }
                shell_slow_note("workspace", started);
            }
            return Ok(());
        }

        let category_hover = event.action.phase == ActionPhase::Hover
            && event.action.inside
            && action.starts_with("start.category.");
        if category_hover {
            // Moving from the app list onto the category rail leaves the list.
            self.reset_start_selection(runtime)?;
            // Same-view hovers are free; only a real target change updates
            // state and reprojects ONE unified document.
            let Some(target) = flamewm_shell::start::category_for_action(action) else {
                return Ok(());
            };
            if !self.start_view.note_start_view(target, &self.start_query) {
                return Ok(());
            }
            self.start_scroll = 0;
            let transition = flamewm_profiler::ProfilePoint::new("shell.category_transition");
            let _s = transition.start();
            self.shell
                .dispatch_with_context(action, interaction.clone());
            self.apply_controls(runtime)?;
            return Ok(());
        }
        if event.action.phase == ActionPhase::Release && event.action.inside {
            self.shell
                .dispatch_with_context(action, interaction.clone());
            self.apply_controls(runtime)?;
        }
        Ok(())
    }

    /// Runs one open-menu row action the way a mouse release on that row does: launcher rows,
    /// the start favorite toggle, tray dbusmenu rows, then the row's typed control. `false` when
    /// no branch owns the action (the caller keeps its own fall-through).
    pub(super) fn activate_menu_row(
        &mut self,
        runtime: &mut SurfaceRuntime,
        action: &str,
    ) -> Result<bool, UiBackendError> {
        if let Some(program) = general_settings_launch(action) {
            if let Err(error) = std::process::Command::new(program).spawn() {
                eprintln!("flamewm-shell: spawn {program}: {error}");
            }
            self.shell.dispatch("popover.close");
            return Ok(true);
        }
        if action == START_MENU_FAVORITE_ACTION {
            if let Some(flamewm_shell::ContextMenuKind::StartApp(app)) =
                self.context_menu.as_ref().map(|menu| menu.kind.clone())
            {
                self.toggle_start_favorite(runtime, &app)?;
            }
            self.shell.dispatch("context.menu.close");
            return Ok(true);
        }
        if action.starts_with(flamewm_shell::tray_host::TRAY_MENU_ACTION_PREFIX) {
            let target = self
                .context_menu
                .as_ref()
                .and_then(|menu| flamewm_shell::tray_host::menu_row_target(&menu.kind, action));
            if let (Some((key, menu_path, id)), Some(tray)) = (target, self.tray.as_ref()) {
                tray.borrow().send_menu_event(&key, &menu_path, id);
                self.shell.dispatch("context.menu.close");
                return Ok(true);
            }
        }
        if let Some(request) = self
            .shell
            .context_menu_request_for_action(action, self.context_menu.as_ref())
        {
            self.submit(request);
            self.shell.dispatch("popover.close");
            return Ok(true);
        }
        Ok(false)
    }

    pub(super) fn route_secondary(
        &mut self,
        event: &SurfaceEvent,
        runtime: &mut SurfaceRuntime,
        interaction: &InteractionContext,
    ) -> Result<(), UiBackendError> {
        let action = event.action.action.as_str();
        if let Some(slot) = slot_of(action, "start.app.") {
            // Start application row: Secondary opens the favorite toggle menu.
            if !self.start_open {
                return Ok(());
            }
            // The event is local to the Start popup surface, not to the panel.
            let Ok(origin) = runtime.surface_device_rect(event.surface) else {
                return Ok(());
            };
            let pointer = flamewm_api::Point::new(
                origin.x as i32 + event.action.x as i32,
                origin.y as i32 + event.action.y as i32,
            );
            if let Some(app) = self.start_app_for_slot(slot) {
                self.shell
                    .open_start_app_context_scoped(app, Some(pointer), interaction.clone());
                self.close_quick_control_nonfatal();
                return self.apply_controls(runtime);
            }
            return Ok(());
        }
        if action == "start.search" {
            return Ok(());
        }
        if let Some(slot) = slot_of(action, "task.slot.") {
            let Some(pointer) = self.surfaces.root_pointer_for_output(
                runtime,
                &interaction.output,
                event.action.x,
                event.action.y,
            ) else {
                return Ok(());
            };
            let target = secondary_task_target(self.task_press_target.take(), slot, || {
                self.shell
                    .task_target_for_slot_on_output(slot, &interaction.output)
            });
            if let Some(target) = target {
                self.shell.open_task_context_for_target_scoped(
                    &target,
                    Some(pointer),
                    Some(interaction.clone()),
                );
            }
            self.close_quick_control_nonfatal();
            return self.apply_controls(runtime);
        }
        if let Some(cell) = slot_of(action, "workspace.") {
            let Some(pointer) = self.surfaces.root_pointer_for_output(
                runtime,
                &interaction.output,
                event.action.x,
                event.action.y,
            ) else {
                return Ok(());
            };
            self.shell.open_workspace_context_at_cell_scoped(
                cell,
                Some(pointer),
                interaction.clone(),
            );
            self.close_quick_control_nonfatal();
            return self.apply_controls(runtime);
        }
        if action == TASKBAR_SURFACE_ACTION {
            let Some(pointer) = self.surfaces.root_pointer_for_output(
                runtime,
                &interaction.output,
                event.action.x,
                event.action.y,
            ) else {
                return Ok(());
            };
            self.shell
                .open_taskbar_context_at_scoped(Some(pointer), interaction.clone());
            self.close_quick_control_nonfatal();
            return self.apply_controls(runtime);
        }
        Ok(())
    }
}

/// Escape with a non-empty Start query clears the query and keeps Start open (Start branch below);
/// with an empty query it closes. Precedent: WAI-ARIA APG combobox (Escape clears the edit first).
pub(super) fn escape_clears_start_query(start_open: bool, query: &str) -> bool {
    start_open && !query.is_empty()
}

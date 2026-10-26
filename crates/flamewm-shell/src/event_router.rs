use std::time::{Duration, Instant};
use std::{fs, path::PathBuf};

use flamewm_ui_x11::{
    ActionPhase, PointerButton, SurfaceEvent, SurfaceRuntime, UiBackendError, UiDocumentAccess,
};

use crate::entry::app::{is_popup_refusal, shell_slow_note, ShellLoop};
use flamewm_shell::quick_controls::host::is_quick_popup_title;
use flamewm_shell::quick_controls::QuickControlKind;
use flamewm_shell::runtime::InteractionContext;

/// C07 stage helper: open a static profiler span. Static labels only.
macro_rules! shell_stage {
    ($label:expr) => {{
        let __point = flamewm_profiler::ProfilePoint::new($label);
        __point.start()
    }};
}

const OUTSIDE_RELEASE_ACTION: &str = "surface.outside.release";
/// Tray volume source node; its release toggles the audio popup.
const TRAY_VOLUME_ACTION: &str = "volume.open";
const TASKBAR_SURFACE_ACTION: &str = "taskbar.surface";
const TASKBAR_MENU_SETTINGS_ACTION: &str = "taskbar.menu.settings";
const GENERAL_SETTINGS_PROGRAM: &str = "flamewm-settings";
const START_DRAG_THRESHOLD_PX: f32 = 6.0;

#[derive(Debug, Clone)]
pub(crate) struct StartAppDrag {
    app_id: flamewm_api::DesktopAppId,
    origin_x: f32,
    origin_y: f32,
    dragging: bool,
}

impl StartAppDrag {
    fn press(app_id: flamewm_api::DesktopAppId, x: f32, y: f32) -> Self {
        Self {
            app_id,
            origin_x: x,
            origin_y: y,
            dragging: false,
        }
    }

    fn motion(&mut self, x: f32, y: f32) -> bool {
        if !self.dragging && (x - self.origin_x).hypot(y - self.origin_y) > START_DRAG_THRESHOLD_PX
        {
            self.dragging = true;
        }
        self.dragging
    }
}

fn create_app_shortcut(app_id: &flamewm_api::DesktopAppId) -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set".to_owned())?;
    let user_dirs = fs::read_to_string(home.join(".config/user-dirs.dirs")).ok();
    let desktop =
        flamewm_desktop_core::model::DesktopDirResolver::resolve(&home, user_dirs.as_deref());
    let catalog = flamewm_applications::ApplicationCatalog::discover()
        .map_err(|error| format!("discover application catalog: {error}"))?;
    let entry = catalog
        .find(app_id)
        .map_err(|error| format!("find application {}: {error}", app_id.as_str()))?;
    let source = entry.path();
    let file_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|_| {
            source
                .extension()
                .is_some_and(|extension| extension == "desktop")
        })
        .ok_or_else(|| "application has no desktop-entry file".to_owned())?;
    fs::create_dir_all(&desktop)
        .map_err(|error| format!("create desktop directory {}: {error}", desktop.display()))?;
    let destination = flamewm_desktop_core::file_actions::shortcut_destination(&desktop, file_name);
    fs::copy(source, &destination).map_err(|error| {
        format!(
            "copy application shortcut to {}: {error}",
            destination.display()
        )
    })?;
    Ok(destination)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum InteractionOrigin {
    Ignore,
    Output(flamewm_api::OutputId),
}

fn resolve_interaction_origin(output: Option<flamewm_api::OutputId>) -> InteractionOrigin {
    output.map_or(InteractionOrigin::Ignore, InteractionOrigin::Output)
}

fn interaction_origin_is_connected<'a>(
    output: &flamewm_api::OutputId,
    mut outputs: impl Iterator<Item = (&'a flamewm_api::OutputId, bool)>,
) -> bool {
    outputs.any(|(candidate, connected)| connected && candidate == output)
}

fn task_drag_stays_on_origin(
    target: Option<&flamewm_shell::runtime::TaskSlotTarget>,
    output: &flamewm_api::OutputId,
) -> bool {
    target.is_some_and(|target| &target.output == output)
}

fn task_drag_cleanup_output(
    target: Option<&flamewm_shell::runtime::TaskSlotTarget>,
) -> Option<&flamewm_api::OutputId> {
    target.map(|target| &target.output)
}

fn task_drag_can_start(
    target: Option<&(usize, flamewm_shell::runtime::TaskSlotTarget)>,
    slot: usize,
    output: &flamewm_api::OutputId,
) -> bool {
    target.is_some_and(|(pressed_slot, target)| *pressed_slot == slot && &target.output == output)
}

/// Program to spawn for a taskbar-menu row action; `None` for other actions.
fn general_settings_launch(action: &str) -> Option<&'static str> {
    (action == TASKBAR_MENU_SETTINGS_ACTION).then_some(GENERAL_SETTINGS_PROGRAM)
}

/// Slot number (1-based in the action id) after `prefix`; `None` for other actions.
fn slot_of(action: &str, prefix: &str) -> Option<usize> {
    action
        .strip_prefix(prefix)
        .and_then(|value| value.parse::<usize>().ok())
        .and_then(|slot| slot.checked_sub(1))
}

/// A release keeps the press-captured task target while a drag owns it; a
/// release inside a task slot also keeps it for the click path.
fn release_keeps_task_press(
    phase: ActionPhase,
    inside: bool,
    action: &str,
    drag_armed: bool,
) -> bool {
    phase != ActionPhase::Release
        || drag_armed
        || (inside && slot_of(action, "task.slot.").is_some())
}

/// Secondary release target: the press capture when it matches the slot,
/// otherwise the task currently shown there. Non-primary presses are not
/// dispatched, so a right-click has no capture to consume.
fn secondary_task_target(
    captured: Option<(usize, flamewm_shell::runtime::TaskSlotTarget)>,
    slot: usize,
    current: impl FnOnce() -> Option<flamewm_shell::runtime::TaskSlotTarget>,
) -> Option<flamewm_shell::runtime::TaskSlotTarget> {
    captured
        .filter(|(pressed_slot, _)| *pressed_slot == slot)
        .map(|(_, target)| target)
        .or_else(current)
}

fn root_pointer_for_surface(
    runtime: &mut SurfaceRuntime,
    event: &SurfaceEvent,
) -> Option<flamewm_api::Point> {
    let rect = runtime.surface_device_rect(event.surface).ok()?;
    let scale = runtime.surface_ui_scale(event.surface).ok()?;
    Some(flamewm_api::Point::new(
        (rect.x + event.action.x * scale).round() as i32,
        (rect.y + event.action.y * scale).round() as i32,
    ))
}

/// Non-primary releases are dropped, except an outside release: a right-button press outside an
/// open shell popup must dismiss it (the right-click that opens another menu replaces this one).
fn non_primary_release_is_dropped(action: &str, is_release: bool, button: PointerButton) -> bool {
    is_release
        && !matches!(button, PointerButton::Primary)
        && action != "keyboard.input"
        && action != OUTSIDE_RELEASE_ACTION
}

/// What a release outside the active popup does to the quick-control popups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutsideReleaseDecision {
    /// Not an outside release: the normal routing continues.
    Ignore,
    /// Close the popup; the release is consumed.
    Close,
    /// The tray volume node toggles an open audio popup closed. The release is
    /// consumed, so the same click cannot reopen it on this turn.
    ToggleTrayVolumeClosed,
}

/// Pure decision for release handling. Non-Escape keyboard releases never
/// close popups (Escape is handled before this point). A release on the tray
/// volume node is an outside release for the popup, but only closes an open
/// audio popup; with no audio popup open it falls through to the open path.
pub(crate) fn outside_release_decision(
    action: &str,
    is_release: bool,
    is_keyboard: bool,
    inside: bool,
    audio_popup_open: bool,
) -> OutsideReleaseDecision {
    if !is_release || is_keyboard {
        return OutsideReleaseDecision::Ignore;
    }
    if action != OUTSIDE_RELEASE_ACTION && inside {
        return OutsideReleaseDecision::Ignore;
    }
    if action == TRAY_VOLUME_ACTION {
        return if audio_popup_open {
            OutsideReleaseDecision::ToggleTrayVolumeClosed
        } else {
            OutsideReleaseDecision::Ignore
        };
    }
    OutsideReleaseDecision::Close
}

/// Window in which the opener release that follows an outside-press dismissal
/// is still treated as the same click.
pub(crate) const DISMISS_LATCH_TTL: Duration = Duration::from_millis(300);

/// Popup group closed by the latest outside-press dismissal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DismissLatch {
    pub(crate) group: &'static str,
    pub(crate) at: Instant,
}

/// Popup group an opener action toggles; `None` for non-opener actions.
pub(crate) fn opener_group(action: &str) -> Option<&'static str> {
    match action {
        "start.toggle" => Some("start"),
        TRAY_VOLUME_ACTION => Some("audio"),
        "network.open" => Some("network"),
        "battery.open" => Some("battery"),
        "weather.open" => Some("weather"),
        "clock.open" => Some("calendar"),
        "media.open" | "media.toggle" => Some("media"),
        _ => None,
    }
}

/// Popup group a quick-control kind occupies; matches its opener's `opener_group`.
pub(crate) fn quick_popup_group(kind: QuickControlKind) -> &'static str {
    match kind {
        QuickControlKind::Audio => "audio",
        QuickControlKind::Network => "network",
        QuickControlKind::Calendar => "calendar",
        QuickControlKind::Battery => "battery",
        QuickControlKind::Weather => "weather",
    }
}

/// True when an observed root press lands outside every quick popup. `title` is
/// the WM_NAME of the top-level window under the pointer.
pub(crate) fn raw_press_is_outside_quick_popups(title: Option<&str>) -> bool {
    !title.is_some_and(is_quick_popup_title)
}

/// Consumes the opener release that is the same click as the dismissal that set
/// `latch`: true means the opener must be a no-op. The latch survives unrelated
/// events and expires after `DISMISS_LATCH_TTL`.
pub(crate) fn latch_consumes_opener(
    latch: &mut Option<DismissLatch>,
    action: &str,
    now: Instant,
) -> bool {
    let Some(pending) = *latch else {
        return false;
    };
    if now.saturating_duration_since(pending.at) > DISMISS_LATCH_TTL {
        *latch = None;
        return false;
    }
    if opener_group(action) == Some(pending.group) {
        *latch = None;
        return true;
    }
    false
}

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
        if matches!(button, PointerButton::WheelUp | PointerButton::WheelDown) {
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
            if input == "\u{1b}" {
                if let Some(request) = self.network_secret_request(false) {
                    self.submit(request);
                }
                self.clear_network_secret(runtime)?;
                self.start_query.clear();
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
            if let Some(program) = general_settings_launch(action) {
                if let Err(error) = std::process::Command::new(program).spawn() {
                    eprintln!("flamewm-shell: spawn {program}: {error}");
                }
                self.shell.dispatch("popover.close");
                return self.apply_controls(runtime);
            }
            if let Some(request) = self.context_system_request(action) {
                self.submit(request);
                if action == "network.secret.submit" || action == "network.secret.cancel" {
                    self.clear_network_secret(runtime)?;
                }
                return Ok(());
            }
            if let Some(request) = self
                .shell
                .context_menu_request_for_action(action, self.context_menu.as_ref())
            {
                self.submit(request);
                self.shell.dispatch("popover.close");
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
            && self.route_launcher_text(
                runtime,
                event.action.text.as_deref().unwrap_or_default(),
            )?
        {
            return Ok(());
        }
        if action == "__flamewm.ui.key" && event.action.phase == ActionPhase::Press {
            let payload = event.action.text.as_deref().unwrap_or_default();
            if self.route_launcher_key(runtime, payload)? {
                return Ok(());
            }
        }
        if let Some(slot) = slot_of(action, "launcher.app.") {
            if event.action.inside {
                match event.action.phase {
                    ActionPhase::Release => self.click_launcher_row(runtime, slot)?,
                    ActionPhase::Hover => self.route_launcher_hover(runtime, slot)?,
                    _ => {}
                }
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
                    self.project_start(runtime)?;
                }
                "\u{1b}" => {
                    if self.start_query.is_empty() {
                        self.close_transients(runtime)?;
                    } else {
                        self.start_query.clear();
                        self.project_start(runtime)?;
                    }
                }
                "\n" => self.launch_start_slot(runtime, 0)?,
                value if !value.is_empty() => {
                    self.start_query.push_str(value);
                    self.project_start(runtime)?;
                }
                _ => {}
            }
            return Ok(());
        }

        if let Some(slot) = slot_of(action, "start.app.") {
            if event.action.phase == ActionPhase::Release && event.action.inside {
                self.launch_start_slot(runtime, slot)?;
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
            // Same-view hovers are free; only a real target change updates
            // state and reprojects ONE unified document.
            let Some(target) = flamewm_shell::start::category_for_action(action) else {
                return Ok(());
            };
            if !self.start_view.note_start_view(target, &self.start_query) {
                return Ok(());
            }
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

    /// Task-slot drag gesture. Returns true when the event is consumed; a
    /// short press returns false so the normal release click still runs.
    fn route_task_drag(
        &mut self,
        event: &SurfaceEvent,
        runtime: &mut SurfaceRuntime,
        event_output: &flamewm_api::OutputId,
    ) -> bool {
        use flamewm_shell::taskbar::tasks::drag::{TaskDrag, TaskDragRelease};
        let action = event.action.action.as_str();
        if self.task_drag.is_some()
            && action == "keyboard.input"
            && event.action.text.as_deref() == Some("\u{1b}")
        {
            self.cancel_task_drag(runtime);
            return true;
        }
        if self.task_drag.is_some()
            && !task_drag_stays_on_origin(
                self.task_press_target.as_ref().map(|(_, target)| target),
                event_output,
            )
        {
            self.cancel_task_drag(runtime);
            return true;
        }
        let slot = slot_of(action, "task.slot.");
        match event.action.phase {
            ActionPhase::Press => {
                let primary = matches!(
                    PointerButton::from_raw_x(event.action.button),
                    PointerButton::Primary
                );
                self.task_drag = slot
                    .filter(|slot| {
                        primary
                            && task_drag_can_start(
                                self.task_press_target.as_ref(),
                                *slot,
                                event_output,
                            )
                    })
                    .map(|slot| TaskDrag::press(slot, event.action.x));
                false
            }
            ActionPhase::Motion => {
                let count = self.shell.task_slot_count();
                let Some(drag) = self.task_drag.as_mut() else {
                    return false;
                };
                if drag.motion(event.action.x) {
                    let target = drag.target(event.action.x, count);
                    self.set_task_insert_marker(runtime, event_output, Some(target));
                }
                true
            }
            ActionPhase::Release => {
                let Some(drag) = self.task_drag.take() else {
                    return false;
                };
                self.set_task_insert_marker(runtime, event_output, None);
                let count = self.shell.task_slot_count();
                match drag.release_on_strip(event.action.x, event.action.y, count) {
                    TaskDragRelease::Click => false,
                    TaskDragRelease::Noop => {
                        self.task_press_target = None;
                        true
                    }
                    TaskDragRelease::Reorder { to, .. } => {
                        let target = self.task_press_target.take().map(|(_, target)| target);
                        if let Some(request) = target
                            .as_ref()
                            .and_then(|target| self.reorder_task_target(target, to))
                        {
                            self.submit(request);
                        }
                        true
                    }
                }
            }
            ActionPhase::Hover => false,
        }
    }

    fn route_start_app_drag(
        &mut self,
        event: &SurfaceEvent,
        runtime: &mut SurfaceRuntime,
    ) -> Result<bool, UiBackendError> {
        let action = event.action.action.as_str();
        match event.action.phase {
            ActionPhase::Press => {
                let primary = matches!(
                    PointerButton::from_raw_x(event.action.button),
                    PointerButton::Primary
                );
                self.start_drag = slot_of(action, "start.app.")
                    .filter(|_| primary && self.start_open)
                    .and_then(|slot| {
                        crate::entry::projection::start_application_for_slot(
                            &self.start_model,
                            self.start_category,
                            &self.start_query,
                            slot,
                        )
                    })
                    .map(|app_id| StartAppDrag::press(app_id, event.action.x, event.action.y));
                Ok(false)
            }
            ActionPhase::Motion => {
                let Some(drag) = self.start_drag.as_mut() else {
                    return Ok(false);
                };
                Ok(drag.motion(event.action.x, event.action.y))
            }
            ActionPhase::Release => {
                let Some(drag) = self.start_drag.take() else {
                    return Ok(false);
                };
                if !drag.dragging {
                    return Ok(false);
                }
                if let Some(point) = root_pointer_for_surface(runtime, event) {
                    let surface_rect = runtime.surface_device_rect(event.surface).ok();
                    let outside_start = surface_rect.is_none_or(|rect| {
                        !flamewm_api::Rect::new(
                            rect.x.round() as i32,
                            rect.y.round() as i32,
                            rect.width.round() as i32,
                            rect.height.round() as i32,
                        )
                        .contains(point)
                    });
                    if outside_start && self.is_desktop_drop_target(point) {
                        if let Err(error) = create_app_shortcut(&drag.app_id) {
                            eprintln!(
                                "flamewm-shell: create desktop shortcut for {}: {error}",
                                drag.app_id.as_str()
                            );
                        }
                    }
                }
                self.close_transients(runtime)?;
                Ok(true)
            }
            ActionPhase::Hover => Ok(false),
        }
    }

    fn is_desktop_drop_target(&self, point: flamewm_api::Point) -> bool {
        let snapshot = self.shell.snapshot();
        if !snapshot
            .displays
            .outputs
            .iter()
            .any(|output| output.connected && output.geometry.contains(point))
            || snapshot
                .panels
                .panels
                .iter()
                .any(|panel| panel.visible && panel.geometry.contains(point))
        {
            return false;
        }
        let active_workspace = snapshot
            .workspaces
            .as_ref()
            .map(|workspaces| workspaces.active_index);
        !snapshot.windows.iter().any(|window| {
            window.is_visible()
                && (window.sticky
                    || active_workspace.is_none_or(|active| {
                        usize::try_from(window.workspace.index).ok() == Some(active)
                    }))
                && window.outer_geometry.contains(point)
        })
    }

    /// Show the 2 px accent insertion marker before slot `target`, or hide it
    /// (`None`). Only the marker node is written, so the panel repaints once.
    fn set_task_insert_marker(
        &mut self,
        runtime: &mut SurfaceRuntime,
        output: &flamewm_api::OutputId,
        target: Option<usize>,
    ) {
        if let Some(panel) = self.surfaces.panel_for_output(output) {
            self.set_task_insert_marker_on_panel(runtime, panel, target);
        }
    }

    fn cancel_task_drag(&mut self, runtime: &mut SurfaceRuntime) {
        let panel = self
            .task_press_target
            .as_ref()
            .and_then(|(_, target)| task_drag_cleanup_output(Some(target)))
            .and_then(|output| self.surfaces.panel_for_output(output));
        self.task_drag = None;
        self.task_press_target = None;
        if let Some(panel) = panel {
            self.set_task_insert_marker_on_panel(runtime, panel, None);
        }
    }

    fn set_task_insert_marker_on_panel(
        &mut self,
        runtime: &mut SurfaceRuntime,
        panel: flamewm_ui_x11::SurfaceHandle,
        target: Option<usize>,
    ) {
        use flamewm_shell::taskbar::tasks::drag::insertion_marker_left;
        let result = runtime.with_document(panel, |document| match target {
            Some(index) => {
                document.position("task-slot-insert", insertion_marker_left(index), 6.0)?;
                document.size("task-slot-insert", 2.0, 32.0)
            }
            None => document.size("task-slot-insert", 0.0, 0.0),
        });
        if let Err(error) = result {
            eprintln!("flamewm-shell: task insertion marker projection failed: {error:?}");
        }
    }

    fn route_secondary(
        &mut self,
        event: &SurfaceEvent,
        runtime: &mut SurfaceRuntime,
        interaction: &InteractionContext,
    ) -> Result<(), UiBackendError> {
        let action = event.action.action.as_str();
        if action == "start.search" || action.starts_with("start.app.") {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitor_scoped_interaction_origin_requires_a_registered_surface() {
        assert_eq!(resolve_interaction_origin(None), InteractionOrigin::Ignore);
        let output = flamewm_api::OutputId::new("B");
        assert_eq!(
            resolve_interaction_origin(Some(output.clone())),
            InteractionOrigin::Output(output),
        );
    }

    #[test]
    fn stale_or_disconnected_origin_is_rejected_without_fallback() {
        let output = flamewm_api::OutputId::new("B");
        let connected = flamewm_api::OutputId::new("A");
        let outputs = [(&connected, true), (&output, false)];
        assert!(!interaction_origin_is_connected(
            &output,
            outputs.into_iter()
        ));
        assert!(!interaction_origin_is_connected(
            &flamewm_api::OutputId::new("gone"),
            [(&connected, true)].into_iter()
        ));
        assert!(interaction_origin_is_connected(
            &connected,
            [(&connected, true)].into_iter()
        ));
    }

    #[test]
    fn task_drag_is_cancelled_when_release_origin_differs_from_pressed_task() {
        let origin = flamewm_api::OutputId::new("A");
        let other = flamewm_api::OutputId::new("B");
        let window = flamewm_api::WindowRef::new(7, 1);
        let target = flamewm_shell::runtime::TaskSlotTarget {
            id: flamewm_api::TaskEntryId::window(window),
            app_id: flamewm_api::DesktopAppId::new("org.example.editor"),
            window: Some(window),
            output: origin.clone(),
        };

        assert!(task_drag_stays_on_origin(Some(&target), &origin));
        assert!(!task_drag_stays_on_origin(Some(&target), &other));
        assert!(!task_drag_stays_on_origin(None, &origin));
        assert_eq!(task_drag_cleanup_output(Some(&target)), Some(&origin));
        assert_eq!(task_drag_cleanup_output(None), None);
        let captured = Some((2, target));
        assert!(task_drag_can_start(captured.as_ref(), 2, &origin));
        assert!(!task_drag_can_start(captured.as_ref(), 1, &origin));
        assert!(!task_drag_can_start(captured.as_ref(), 2, &other));
        assert!(!task_drag_can_start(None, 2, &origin));
    }

    #[test]
    fn release_keeps_the_pressed_task_target_only_for_clicks_or_armed_drags() {
        // Cross-slot drag release: the drag still owns the capture.
        assert!(release_keeps_task_press(
            ActionPhase::Release,
            false,
            "task.slot.3",
            true
        ));
        // Plain release outside the pressed node with no drag clears it.
        assert!(!release_keeps_task_press(
            ActionPhase::Release,
            false,
            "task.slot.3",
            false
        ));
        // Release inside a task slot keeps it for the click path.
        assert!(release_keeps_task_press(
            ActionPhase::Release,
            true,
            "task.slot.3",
            false
        ));
        assert!(!release_keeps_task_press(
            ActionPhase::Release,
            true,
            "taskbar.surface",
            false
        ));
        assert!(release_keeps_task_press(
            ActionPhase::Press,
            false,
            "taskbar.surface",
            false
        ));
    }

    #[test]
    fn secondary_release_without_press_capture_uses_the_slot_task() {
        let output = flamewm_api::OutputId::new("A");
        let window = flamewm_api::WindowRef::new(9, 1);
        let target = flamewm_shell::runtime::TaskSlotTarget {
            id: flamewm_api::TaskEntryId::window(window),
            app_id: flamewm_api::DesktopAppId::new("org.example.terminal"),
            window: Some(window),
            output,
        };
        // Right-click: no primary press capture exists.
        assert_eq!(
            secondary_task_target(None, 2, || Some(target.clone())),
            Some(target.clone())
        );
        // A matching capture wins over the currently shown task.
        assert_eq!(
            secondary_task_target(Some((2, target.clone())), 2, || None),
            Some(target.clone())
        );
        // A stale capture from another slot never redirects the release.
        assert_eq!(
            secondary_task_target(Some((0, target.clone())), 2, || Some(target.clone())),
            Some(target.clone())
        );
        assert_eq!(secondary_task_target(None, 2, || None), None);
    }

    #[test]
    fn monitor_scoped_start_transfer_and_unknown_surface_are_not_fail_open() {
        let app = include_str!("app.rs");
        let controls = app
            .split("ShellControl::ToggleStart {")
            .nth(1)
            .and_then(|tail| tail.split("ShellControl::CloseStart").next())
            .expect("ToggleStart control branch exists");
        let router = include_str!("event_router.rs");
        let route = router
            .split("pub(crate) fn route_event(")
            .nth(1)
            .and_then(|tail| tail.split("// Presses are queued").next())
            .expect("route_event origin handling exists");

        let mut missing_contracts = Vec::new();
        if !controls.contains("open_start_surface(runtime, &target_output, started)") {
            missing_contracts
                .push("an already-open Start must transfer to the newly requested output");
        }
        if route.contains("select_input_output(event.surface)") {
            missing_contracts
                .push("an unknown/stale surface must be rejected before routing actions");
        }
        if !route.contains("interaction_origin_is_connected")
            || !router.contains("source_surface: event.surface")
            || !router.contains("dispatch_with_context(action, interaction.clone())")
        {
            missing_contracts.push("pointer controls must use a connected, explicit source output");
        }

        assert!(
            missing_contracts.is_empty(),
            "missing monitor-scoped contracts: {missing_contracts:?}"
        );
    }

    #[test]
    fn taskbar_settings_row_maps_to_general_settings_program_only() {
        assert_eq!(
            general_settings_launch("taskbar.menu.settings"),
            Some("flamewm-settings")
        );
        assert_eq!(general_settings_launch("taskbar.menu.new-desktop"), None);
        assert_eq!(general_settings_launch("task.menu.close"), None);
    }

    #[test]
    fn taskbar_menu_settings_markup_action_is_the_routed_settings_action() {
        let html = include_str!("../../../ui/shell/task-menu.html");
        assert!(html.contains("data-action=\"taskbar.menu.settings\""));
        assert!(general_settings_launch("taskbar.menu.settings").is_some());
    }

    #[test]
    fn tray_release_while_audio_open_toggles_closed_without_reopen() {
        assert_eq!(
            outside_release_decision(TRAY_VOLUME_ACTION, true, false, false, true),
            OutsideReleaseDecision::ToggleTrayVolumeClosed
        );
    }

    #[test]
    fn tray_release_with_no_audio_popup_falls_through_to_open() {
        assert_eq!(
            outside_release_decision(TRAY_VOLUME_ACTION, true, false, false, false),
            OutsideReleaseDecision::Ignore
        );
    }

    #[test]
    fn non_escape_keyboard_release_never_closes_popups() {
        assert_eq!(
            outside_release_decision("keyboard.input", true, true, false, true),
            OutsideReleaseDecision::Ignore
        );
    }

    #[test]
    fn genuine_outside_release_closes_and_inside_release_is_kept() {
        assert_eq!(
            outside_release_decision(OUTSIDE_RELEASE_ACTION, true, false, false, true),
            OutsideReleaseDecision::Close
        );
        assert_eq!(
            outside_release_decision("network.open", true, false, false, true),
            OutsideReleaseDecision::Close
        );
        assert_eq!(
            outside_release_decision("network.open", true, false, true, true),
            OutsideReleaseDecision::Ignore
        );
        assert_eq!(
            outside_release_decision("network.open", false, false, false, true),
            OutsideReleaseDecision::Ignore
        );
    }

    #[test]
    fn right_button_outside_release_reaches_dismissal_but_other_secondary_releases_drop() {
        assert!(!non_primary_release_is_dropped(
            OUTSIDE_RELEASE_ACTION,
            true,
            PointerButton::Secondary
        ));
        assert!(!non_primary_release_is_dropped(
            OUTSIDE_RELEASE_ACTION,
            true,
            PointerButton::Primary
        ));
        assert!(non_primary_release_is_dropped(
            "network.open",
            true,
            PointerButton::Secondary
        ));
        assert!(!non_primary_release_is_dropped(
            "keyboard.input",
            true,
            PointerButton::Other(0)
        ));
        assert!(!non_primary_release_is_dropped(
            OUTSIDE_RELEASE_ACTION,
            false,
            PointerButton::Secondary
        ));
    }

    fn latch_for(group: &'static str, at: Instant) -> Option<DismissLatch> {
        Some(DismissLatch { group, at })
    }

    #[test]
    fn opener_release_after_its_own_dismiss_is_consumed_for_every_popup_group() {
        let now = Instant::now();
        for (group, opener) in [
            ("start", "start.toggle"),
            ("audio", TRAY_VOLUME_ACTION),
            ("network", "network.open"),
            ("battery", "battery.open"),
            ("calendar", "clock.open"),
        ] {
            let mut latch = latch_for(group, now);
            assert!(latch_consumes_opener(&mut latch, opener, now), "{opener}");
            assert_eq!(latch, None, "{opener} consumes the latch");
        }
    }

    #[test]
    fn closed_popup_opener_is_not_consumed_and_opens() {
        let now = Instant::now();
        let mut latch = None;
        assert!(!latch_consumes_opener(&mut latch, "start.toggle", now));
        assert_eq!(latch, None);
    }

    #[test]
    fn other_group_opener_is_not_consumed_and_latch_survives() {
        let now = Instant::now();
        let mut latch = latch_for("start", now);
        assert!(!latch_consumes_opener(&mut latch, "network.open", now));
        assert_eq!(latch, latch_for("start", now));
    }

    #[test]
    fn latch_expires_so_a_later_genuine_click_reopens() {
        let now = Instant::now();
        let inside = now + DISMISS_LATCH_TTL - Duration::from_millis(1);
        let expired = now + DISMISS_LATCH_TTL + Duration::from_millis(1);
        let mut latch = latch_for("start", now);
        assert!(latch_consumes_opener(&mut latch, "start.toggle", inside));
        let mut latch = latch_for("start", now);
        assert!(!latch_consumes_opener(&mut latch, "start.toggle", expired));
        assert_eq!(latch, None);
    }

    /// Replays one click as the shell sees it: the outside release closes `group`
    /// (recording the latch when enabled), then the same click's opener release arrives.
    /// Returns whether the popup is open afterwards.
    fn replay_dismiss_then_opener(latch_enabled: bool, group: &'static str, opener: &str) -> bool {
        let at = Instant::now();
        let mut latch = latch_enabled.then_some(DismissLatch { group, at });
        let mut open = false;
        open |= !latch_consumes_opener(&mut latch, opener, at + Duration::from_millis(5));
        open
    }

    #[test]
    fn replayed_opener_after_outside_dismiss_leaves_popup_closed() {
        for (group, opener) in [
            ("start", "start.toggle"),
            ("audio", TRAY_VOLUME_ACTION),
            ("network", "network.open"),
            ("battery", "battery.open"),
            ("calendar", "clock.open"),
            ("media", "media.open"),
            ("media", "media.toggle"),
        ] {
            assert!(!replay_dismiss_then_opener(true, group, opener), "{opener}");
        }
    }

    #[test]
    fn opener_group_covers_quick_popups_and_ignores_other_actions() {
        assert_eq!(opener_group("start.toggle"), Some("start"));
        assert_eq!(opener_group(TRAY_VOLUME_ACTION), Some("audio"));
        assert_eq!(opener_group("network.open"), Some("network"));
        assert_eq!(opener_group("battery.open"), Some("battery"));
        assert_eq!(opener_group("clock.open"), Some("calendar"));
        assert_eq!(opener_group("media.open"), Some("media"));
        assert_eq!(opener_group("media.toggle"), Some("media"));
        assert_eq!(opener_group("start.category.apps"), None);
    }

    #[test]
    fn observed_outside_press_latches_each_quick_group_and_its_opener_is_consumed_once() {
        let now = Instant::now();
        for (kind, opener) in [
            (QuickControlKind::Audio, TRAY_VOLUME_ACTION),
            (QuickControlKind::Network, "network.open"),
            (QuickControlKind::Calendar, "clock.open"),
            (QuickControlKind::Battery, "battery.open"),
            (QuickControlKind::Weather, "weather.open"),
        ] {
            let group = quick_popup_group(kind);
            assert_eq!(opener_group(opener), Some(group), "{opener}");
            let mut latch = Some(DismissLatch { group, at: now });
            assert!(latch_consumes_opener(&mut latch, opener, now), "{opener}");
            assert!(
                !latch_consumes_opener(&mut latch, opener, now),
                "{opener} second release opens"
            );
        }
    }

    #[test]
    fn press_classification_treats_only_quick_popup_titles_as_inside() {
        for title in [
            "FlameWM audio",
            "FlameWM network",
            "FlameWM calendar",
            "FlameWM battery",
            "FlameWM weather",
            "FlameWM media",
        ] {
            assert!(
                !raw_press_is_outside_quick_popups(Some(title)),
                "{title} is inside"
            );
        }
        assert!(
            raw_press_is_outside_quick_popups(None),
            "no window under pointer"
        );
        assert!(
            raw_press_is_outside_quick_popups(Some("FlameWM Render 0.0.9")),
            "panel"
        );
        assert!(
            raw_press_is_outside_quick_popups(Some("FlameWM audio2")),
            "prefix is not a match"
        );
    }
}

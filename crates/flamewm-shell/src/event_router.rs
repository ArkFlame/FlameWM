use std::time::{Duration, Instant};

use flamewm_ui_x11::{
    ActionPhase, PointerButton, SurfaceEvent, SurfaceRuntime, UiBackendError, UiDocumentAccess,
};

use crate::app::{is_popup_refusal, shell_slow_note, ShellLoop};
use flamewm_shell::quick_controls::host::is_quick_popup_title;
use flamewm_shell::quick_controls::QuickControlKind;

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

/// Program to spawn for a taskbar-menu row action; `None` for other actions.
fn general_settings_launch(action: &str) -> Option<&'static str> {
    (action == TASKBAR_MENU_SETTINGS_ACTION).then_some(GENERAL_SETTINGS_PROGRAM)
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
        // Presses are queued during the same reactor step, ahead of this release.
        self.drain_raw_press_observations(runtime);
        let action = event.action.action.as_str();
        // never activate on Secondary.
        let button = PointerButton::from_raw_x(event.action.button);
        if matches!(button, PointerButton::WheelUp | PointerButton::WheelDown) {
            self.apply_wheel_scroll(&event, runtime);
            return Ok(());
        }
        let release_inside = event.action.phase == ActionPhase::Release && event.action.inside;
        if release_inside && matches!(button, PointerButton::Secondary) {
            return self.route_secondary(&event, runtime);
        }
        if self.route_task_drag(&event, runtime) {
            return Ok(());
        }
        // ESC-V3: keyboard events carry button 0; gating them as
        // non-primary pointer releases drops every real Escape press
        // before the keyboard arms below. Exempt them here.
        let is_keyboard = action == "keyboard.input";
        if event.action.phase == ActionPhase::Release
            && !matches!(button, PointerButton::Primary)
            && !is_keyboard
        {
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

        // context menus here.
        if false {
            if let Some(slot) = action
                .strip_prefix("task.slot.")
                .and_then(|value| value.parse::<usize>().ok())
                .and_then(|slot| slot.checked_sub(1))
            {
                self.shell.open_task_context_at_slot(
                    slot,
                    Some(
                        self.surfaces
                            .root_pointer(runtime, event.action.x, event.action.y),
                    ),
                );
                return self.apply_controls(runtime);
            }
            if let Some(slot) = action
                .strip_prefix("workspace.")
                .and_then(|value| value.parse::<usize>().ok())
                .and_then(|slot| slot.checked_sub(1))
            {
                self.shell.open_workspace_context_at_slot(
                    slot,
                    Some(
                        self.surfaces
                            .root_pointer(runtime, event.action.x, event.action.y),
                    ),
                );
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
            let opened = self.surfaces.open_start_group(runtime);
            let _present = shell_stage!("shell.start.open.present");
            shell_slow_note("start", started);
            match opened {
                Ok(()) => {}
                Err(error) if is_popup_refusal(&error) => {}
                Err(error) => return Err(UiBackendError::Renderer(error)),
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

        if let Some(slot) = action
            .strip_prefix("start.app.")
            .and_then(|value| value.parse::<usize>().ok())
            .and_then(|slot| slot.checked_sub(1))
        {
            if event.action.phase == ActionPhase::Release && event.action.inside {
                self.launch_start_slot(runtime, slot)?;
            }
            return Ok(());
        }

        if let Some(slot) = action
            .strip_prefix("task.slot.")
            .and_then(|value| value.parse::<usize>().ok())
            .and_then(|slot| slot.checked_sub(1))
        {
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
            self.shell.dispatch(action);
            self.apply_controls(runtime)?;
            return Ok(());
        }
        if event.action.phase == ActionPhase::Release && event.action.inside {
            self.shell.dispatch(action);
            self.apply_controls(runtime)?;
        }
        Ok(())
    }

    /// Task-slot drag gesture. Returns true when the event is consumed; a
    /// short press returns false so the normal release click still runs.
    fn route_task_drag(&mut self, event: &SurfaceEvent, runtime: &mut SurfaceRuntime) -> bool {
        use flamewm_shell::taskbar::tasks::drag::{TaskDrag, TaskDragRelease};
        let action = event.action.action.as_str();
        if self.task_drag.is_some()
            && action == "keyboard.input"
            && event.action.text.as_deref() == Some("\u{1b}")
        {
            self.task_drag = None;
            self.set_task_insert_marker(runtime, None);
            return true;
        }
        let slot = action
            .strip_prefix("task.slot.")
            .and_then(|value| value.parse::<usize>().ok())
            .and_then(|slot| slot.checked_sub(1));
        match event.action.phase {
            ActionPhase::Press => {
                let primary = matches!(
                    PointerButton::from_raw_x(event.action.button),
                    PointerButton::Primary
                );
                self.task_drag = slot
                    .filter(|_| primary)
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
                    self.set_task_insert_marker(runtime, Some(target));
                }
                true
            }
            ActionPhase::Release => {
                let Some(drag) = self.task_drag.take() else {
                    return false;
                };
                self.set_task_insert_marker(runtime, None);
                let count = self.shell.task_slot_count();
                match drag.release_on_strip(event.action.x, event.action.y, count) {
                    TaskDragRelease::Click => false,
                    TaskDragRelease::Noop => true,
                    TaskDragRelease::Reorder { from, to } => {
                        if let Some(request) = self.shell.task_reorder_request(from, to) {
                            self.submit(request);
                        }
                        true
                    }
                }
            }
            ActionPhase::Hover => false,
        }
    }

    /// Show the 2 px accent insertion marker before slot `target`, or hide it
    /// (`None`). Only the marker node is written, so the panel repaints once.
    fn set_task_insert_marker(&mut self, runtime: &mut SurfaceRuntime, target: Option<usize>) {
        use flamewm_shell::taskbar::tasks::drag::insertion_marker_left;
        let panel = self.surfaces.panel();
        let result = runtime.with_document(panel, |document| match target {
            Some(index) => {
                document.position(
                    "task-slot-insert",
                    insertion_marker_left(index),
                    6.0,
                )?;
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
    ) -> Result<(), UiBackendError> {
        let action = event.action.action.as_str();
        if action == "start.search" || action.starts_with("start.app.") {
            return Ok(());
        }
        if let Some(slot) = action
            .strip_prefix("task.slot.")
            .and_then(|value| value.parse::<usize>().ok())
            .and_then(|slot| slot.checked_sub(1))
        {
            self.shell.open_task_context_at_slot(
                slot,
                Some(
                    self.surfaces
                        .root_pointer(runtime, event.action.x, event.action.y),
                ),
            );
            self.close_quick_control_nonfatal();
            return self.apply_controls(runtime);
        }
        if let Some(slot) = action
            .strip_prefix("workspace.")
            .and_then(|value| value.parse::<usize>().ok())
            .and_then(|slot| slot.checked_sub(1))
        {
            self.shell.open_workspace_context_at_slot(
                slot,
                Some(
                    self.surfaces
                        .root_pointer(runtime, event.action.x, event.action.y),
                ),
            );
            self.close_quick_control_nonfatal();
            return self.apply_controls(runtime);
        }
        if action == TASKBAR_SURFACE_ACTION {
            self.shell.open_taskbar_context_at(Some(
                self.surfaces
                    .root_pointer(runtime, event.action.x, event.action.y),
            ));
            self.close_quick_control_nonfatal();
            return self.apply_controls(runtime);
        }
        Ok(())
    }

    fn apply_wheel_scroll(&mut self, event: &SurfaceEvent, _runtime: &mut SurfaceRuntime) {
        let button = PointerButton::from_raw_x(event.action.button);
        let delta = match button {
            PointerButton::WheelUp => -48.0,
            PointerButton::WheelDown => 48.0,
            _ => return,
        };
        let action = event.action.action.as_str();
        let node: u32 = match action {
            value if value.starts_with("audio.") || value == "popup.surface" => 1,
            value if value.starts_with("network.") => 2,
            value if value.starts_with("start.") => 3,
            _ => return,
        };
        let _ = (delta, node);
        let _ = flamewm_ui_core::range::RangeSpec::new(0.0, 100.0, 1.0, 50.0);
        let _ = flamewm_ui_core::scroll::ScrollStore::default();
        let _ = flamewm_ui_core::virtual_list::VirtualListModel {
            row_count: 0,
            row_height: 1,
            viewport_height: 1,
            scroll_offset: 0,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            assert!(!latch_consumes_opener(&mut latch, opener, now), "{opener} second release opens");
        }
    }

    #[test]
    fn press_classification_treats_only_quick_popup_titles_as_inside() {
        for title in ["FlameWM audio", "FlameWM network", "FlameWM calendar", "FlameWM battery", "FlameWM weather", "FlameWM media"] {
            assert!(!raw_press_is_outside_quick_popups(Some(title)), "{title} is inside");
        }
        assert!(raw_press_is_outside_quick_popups(None), "no window under pointer");
        assert!(raw_press_is_outside_quick_popups(Some("FlameWM Render 0.0.9")), "panel");
        assert!(raw_press_is_outside_quick_popups(Some("FlameWM audio2")), "prefix is not a match");
    }
}

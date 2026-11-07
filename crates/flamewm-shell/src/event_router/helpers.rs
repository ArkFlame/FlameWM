use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum InteractionOrigin {
    Ignore,
    Output(flamewm_api::OutputId),
}

pub(super) fn resolve_interaction_origin(output: Option<flamewm_api::OutputId>) -> InteractionOrigin {
    output.map_or(InteractionOrigin::Ignore, InteractionOrigin::Output)
}

pub(super) fn interaction_origin_is_connected<'a>(
    output: &flamewm_api::OutputId,
    mut outputs: impl Iterator<Item = (&'a flamewm_api::OutputId, bool)>,
) -> bool {
    outputs.any(|(candidate, connected)| connected && candidate == output)
}

pub(super) fn task_drag_stays_on_origin(
    target: Option<&flamewm_shell::runtime::TaskSlotTarget>,
    output: &flamewm_api::OutputId,
) -> bool {
    target.is_some_and(|target| &target.output == output)
}

pub(super) fn task_drag_cleanup_output(
    target: Option<&flamewm_shell::runtime::TaskSlotTarget>,
) -> Option<&flamewm_api::OutputId> {
    target.map(|target| &target.output)
}

pub(super) fn task_drag_can_start(
    target: Option<&(usize, flamewm_shell::runtime::TaskSlotTarget)>,
    slot: usize,
    output: &flamewm_api::OutputId,
) -> bool {
    target.is_some_and(|(pressed_slot, target)| *pressed_slot == slot && &target.output == output)
}

/// Program to spawn for a taskbar-menu row action; `None` for other actions.
pub(super) fn general_settings_launch(action: &str) -> Option<&'static str> {
    match action {
        TASKBAR_MENU_SETTINGS_ACTION => Some(GENERAL_SETTINGS_PROGRAM),
        TASKBAR_MENU_TASK_MANAGER_ACTION => Some(TASK_MANAGER_PROGRAM),
        _ => None,
    }
}

/// Slot number (1-based in the action id) after `prefix`; `None` for other actions.
pub(super) fn slot_of(action: &str, prefix: &str) -> Option<usize> {
    action
        .strip_prefix(prefix)
        .and_then(|value| value.parse::<usize>().ok())
        .and_then(|slot| slot.checked_sub(1))
}

/// A release keeps the press-captured task target while a drag owns it; a
/// release inside a task slot also keeps it for the click path.
pub(super) fn release_keeps_task_press(
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
pub(super) fn secondary_task_target(
    captured: Option<(usize, flamewm_shell::runtime::TaskSlotTarget)>,
    slot: usize,
    current: impl FnOnce() -> Option<flamewm_shell::runtime::TaskSlotTarget>,
) -> Option<flamewm_shell::runtime::TaskSlotTarget> {
    captured
        .filter(|(pressed_slot, _)| *pressed_slot == slot)
        .map(|(_, target)| target)
        .or_else(current)
}

pub(super) fn root_pointer_for_surface(
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
pub(super) fn non_primary_release_is_dropped(action: &str, is_release: bool, button: PointerButton) -> bool {
    is_release
        && !matches!(button, PointerButton::Primary)
        && action != "keyboard.input"
        && action != OUTSIDE_RELEASE_ACTION
}

//! Pure shell helpers: output resolution, settings readers, popup-state field resets, media and pager mapping, wake scheduling.

use super::*;

pub(super) fn resolve_control_output(
    requested: Option<&flamewm_api::OutputId>,
    panel_outputs: &[flamewm_api::OutputId],
    connected_outputs: &[flamewm_api::OutputId],
    focused_output: Option<&flamewm_api::OutputId>,
    pointer_output: Option<&flamewm_api::OutputId>,
    primary_outputs: &[flamewm_api::OutputId],
) -> Option<flamewm_api::OutputId> {
    let valid = |output: &flamewm_api::OutputId| {
        panel_outputs.contains(output) && connected_outputs.contains(output)
    };
    if let Some(requested) = requested {
        return valid(requested).then(|| requested.clone());
    }
    focused_output
        .filter(|output| valid(output))
        .or_else(|| pointer_output.filter(|output| valid(output)))
        .or_else(|| primary_outputs.iter().find(|output| valid(output)))
        .or_else(|| {
            panel_outputs
                .iter()
                .filter(|output| connected_outputs.contains(output))
                .min()
        })
        .cloned()
}

pub(super) fn interaction_target_is_current(
    requested: Option<&flamewm_api::OutputId>,
    origin: &flamewm_api::OutputId,
    display_generation: u64,
    panels_revision: u64,
    current_display_generation: u64,
    current_panels_revision: u64,
) -> bool {
    requested == Some(origin)
        && display_generation == current_display_generation
        && panels_revision == current_panels_revision
}

/// Flat `taskbarStyle` / `taskbarFloating*` keys as the platform reads them (defaults match
/// `ProductSettings::default()`).
pub(super) fn taskbar_frame(settings: &SettingsSnapshot) -> TaskbarFrame {
    let floating = matches!(
        settings.values.get("taskbarStyle"),
        Some(SettingValue::Text(style)) if style == "floating"
    );
    let logical = |key: &str, default: u16| match settings.values.get(key) {
        Some(SettingValue::Integer(value)) => u16::try_from(*value).unwrap_or(default),
        _ => default,
    };
    TaskbarFrame {
        floating,
        margin: logical("taskbarFloatingMargin", 8),
        radius: logical("taskbarFloatingRadius", 12),
    }
}

/// Whether an open Start document must be re-projected this refresh. A stale theme
/// (`icons_stale`) is consumed by the projection it triggers, so later refreshes do not
/// re-project the open Start again; an application-set change always projects while open.
pub(super) fn start_icon_refresh_due(
    icons_stale: &mut bool,
    applications_changed: bool,
    start_open: bool,
) -> bool {
    if !start_open {
        return false;
    }
    let stale = std::mem::take(icons_stale);
    applications_changed || stale
}

pub(super) fn end_start_session_fields(start_open: &mut bool, start_query: &mut String) {
    *start_open = false;
    start_query.clear();
}

pub(super) fn clear_transient_state_fields(
    start_open: &mut bool,
    start_query: &mut String,
    context_menu: &mut Option<flamewm_shell::ContextMenuState>,
) {
    end_start_session_fields(start_open, start_query);
    *context_menu = None;
}

/// Opening a quick popup releases the shell context-menu slot. Returns true
/// when a context menu was open and its surface must be closed.
pub(super) fn take_context_menu_for_popup(
    context_menu: &mut Option<flamewm_shell::ContextMenuState>,
) -> bool {
    context_menu.take().is_some()
}

/// `IconGeometrySink` over the `SurfaceRuntime` facade. Write failures are
/// nonfatal diagnostics; a vanished client window is not an error.
pub(super) struct RuntimeIconGeometrySink<'a> {
    pub(super) runtime: &'a mut SurfaceRuntime,
}

impl IconGeometrySink for RuntimeIconGeometrySink<'_> {
    fn set(&mut self, window: u64, rect: IconRect) {
        if let Err(error) = self.runtime.set_window_icon_geometry(window, rect) {
            eprintln!("flamewm-shell: icon geometry write failed (stage=x11-property): {error:?}");
        }
    }

    fn clear(&mut self, window: u64) {
        if let Err(error) = self.runtime.clear_window_icon_geometry(window) {
            eprintln!("flamewm-shell: icon geometry clear failed (stage=x11-property): {error:?}");
        }
    }
}

/// Pure mapping for one pager cell: root-space node rect `(x, y, w, h)`
/// minus the panel window origin, rounded to whole device pixels.
/// Negative offsets clamp to zero.
pub(super) fn pager_cell_window_rect(node: (f32, f32, f32, f32), origin: (f32, f32)) -> [u64; 4] {
    let (x, y, width, height) = node;
    [
        (x - origin.0).round().max(0.0) as u64,
        (y - origin.1).round().max(0.0) as u64,
        width.round().max(0.0) as u64,
        height.round().max(0.0) as u64,
    ]
}

/// Maps a `media.*` node action to an MPRIS system request for the active
/// player. The host checks the live generation, so only the bus name and the
/// snapshot revision are carried.
pub(super) fn media_system_request(
    media: &flamewm_api::system::MediaSnapshot,
    expected_revision: u64,
    command: &str,
) -> Option<ControlRequest> {
    if media.active_bus_name.is_empty() {
        return None;
    }
    let bus_name = media.active_bus_name.clone();
    let action = match command {
        "previous" => flamewm_api::system::SystemAction::Previous { bus_name },
        "playpause" => flamewm_api::system::SystemAction::PlayPause { bus_name },
        "next" => flamewm_api::system::SystemAction::Next { bus_name },
        _ => return None,
    };
    Some(ControlRequest::SystemAction {
        action,
        expected_revision,
    })
}

/// Whether the visible panel document must be re-projected for this change.
/// System-only changes (media, etc.) project the panel too.
pub(super) fn panel_projection_needed(
    changed: flamewm_shell::runtime::ChangedDomains,
    resynced: bool,
) -> bool {
    changed.windows || changed.workspaces || changed.system || (changed.panels && !resynced)
}

/// Whether an open media popup must close because media is no longer
/// shown in the tray (`tray-media` hidden). Uses the same visibility owner
/// as the panel slot (`status::media::project`).
pub(super) fn media_popup_must_close(
    system: &flamewm_api::system::SystemSnapshot,
    media_open: bool,
) -> bool {
    media_open && !flamewm_shell::taskbar::status::media::project(system).visible
}

/// Chrono's local-zone cache refresh window (chrono 0.4.45 `local/unix.rs`).
pub(super) const ZONE_SETTLE: std::time::Duration = std::time::Duration::from_millis(1100);

/// The single shared reactor wake slot takes the earliest pending owner deadline, or none.
#[must_use]
pub(super) fn earliest_wake(
    deadlines: [Option<std::time::Instant>; 5],
) -> Option<std::time::Instant> {
    deadlines.into_iter().flatten().min()
}

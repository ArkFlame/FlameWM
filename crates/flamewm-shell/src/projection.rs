use std::fmt::Write as _;

#[cfg(test)]
use chrono::Local;
use flamewm_api::applications::DesktopApplication;
use flamewm_api::session::SessionCapabilities;
use flamewm_api::settings::SettingsSnapshot;
use flamewm_integrations_linux::icons::{IconResolver, IconSize};
use flamewm_ui_x11::{RuntimeImage, UiColor, UiDocumentAccess};

use crate::start::{StartCategory, PRESENTATION_CATEGORIES};
use crate::taskbar::workspaces::{
    cell_left, pager_cell, pager_columns, pager_width, project as project_workspaces,
    WorkspaceView, PAGER_CELLS, PAGER_CELL_GAP, PAGER_CELL_HEIGHT, PAGER_CELL_WIDTH, PAGER_COLUMNS,
};
use crate::taskbar_scale::{self, scaled};
use crate::ShellSnapshot;
use flamewm_shell_core::start_menu::{bucket_for_application, StartBucket};
use flamewm_shell_core::{task_visual_states_on_output_and_workspace, StartModel};

#[cfg(test)]
macro_rules! forward {
    ($($method:ident($($arg:ident: $ty:ty),*)),* $(,)?) => {
        $(fn $method(&mut self, $($arg: $ty),*) -> Result<(), String> {
            UiDocumentAccess::$method(self.document, $($arg),*)
        })*
    };
}

mod audio;
mod bluetooth;
mod calendar;
mod clock;
mod context_menu;
mod media;
mod network;
mod panel;
mod polkit;
mod start;
mod task_menu;
mod usage;
mod weather;

pub use audio::{project_audio, project_audio_focus, project_audio_full};
pub use bluetooth::project_bluetooth;
pub use calendar::project_calendar;
use clock::month_name;
pub use clock::{clock_time_text, local_date, project_clock, project_clock_at, ClockDisplay};
pub use context_menu::project_context_menu;
pub use media::{project_media, project_media_focus, MEDIA_STOP_IDS};
pub use network::{
    bluetooth_row_status, bluetooth_trust_action, project_bluetooth_row, project_network,
    project_network_details, project_network_focus, project_network_full, project_network_secret,
    project_network_status, project_vpn_rows, vpn_toggle_action,
};
#[cfg(test)]
pub(crate) use panel::pager_active_fill;
pub(crate) use panel::taskbar_surface;
pub use panel::{
    brightness_step_action, for_each_panel_record, power_profile_cycle_action, project,
    project_battery_menu, project_battery_tray, project_brightness_row, project_input_row,
    project_panel, project_power_profile_row, reapply_element_backgrounds, set_task_accent,
    set_taskbar_surface, task_icon_device_px, task_icon_px, taskbar_css_height, taskbar_css_width,
    PanelView, TaskbarItems, BATTERY_PROGRESS_HEIGHT,
};
pub(crate) use panel::{
    project_element_backgrounds, project_pager, project_pager_visibility, project_status_tray,
    project_task_line, project_task_overflow, project_task_progress, project_task_slots_extent,
    project_taskbar_height, project_tray_button, set_taskbar_border, task_accent, task_label,
    task_slot_capacity_for_panel, task_slot_capacity_for_view, task_slot_split,
    taskbar_is_vertical, tray_any_visible, workspace_pager_visible, TASK_ATTENTION,
    TASK_NOT_RESPONDING,
};
pub use polkit::project_polkit;
pub use start::{
    project_start, project_start_error, start_application_for_slot,
    start_application_name_for_slot, start_scroll_max, start_scroll_step, start_visible_results,
    START_STATUS_NODE, START_WHEEL_ROWS,
};
pub use usage::{project_usage, project_usage_focus};
pub use weather::{project_weather_popup, project_weather_tray};

#[must_use]
pub(crate) fn session_available(session: SessionCapabilities) -> bool {
    session.lock || session.logout || session.suspend || session.reboot || session.shutdown
}

fn resolve_icon(
    resolver: &mut IconResolver,
    icon_name: &str,
    size: IconSize,
) -> Result<RuntimeImage, String> {
    let raster = resolver.prepare_name(icon_name, size).or_else(|error| {
        resolver.prepare_name("", size).map_err(|fallback| {
            format!("icon '{icon_name}' failed: {error}; generic fallback failed: {fallback}")
        })
    })?;
    Ok(RuntimeImage::from(raster))
}

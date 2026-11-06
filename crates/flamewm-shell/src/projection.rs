use std::fmt::Write as _;

#[cfg(test)]
use chrono::Local;
use flamewm_api::applications::DesktopApplication;
use flamewm_api::session::SessionCapabilities;
use flamewm_api::settings::SettingsSnapshot;
use flamewm_integrations_linux::icons::{IconResolver, IconSize, Rgb8Raster};
use flamewm_ui_x11::{RuntimeImage, UiColor, UiDocumentAccess};

use crate::ShellSnapshot;
use crate::start::{PRESENTATION_CATEGORIES, StartCategory};
use crate::taskbar::workspaces::{
    PAGER_CELL_GAP, PAGER_CELL_HEIGHT, PAGER_CELL_WIDTH, PAGER_CELLS, PAGER_COLUMNS, WorkspaceView,
    cell_left, pager_cell, pager_columns, pager_width, project as project_workspaces,
};
use crate::taskbar_scale::{self, scaled};
use flamewm_shell_core::start_menu::{StartBucket, bucket_for_application};
use flamewm_shell_core::{StartModel, task_visual_states_on_output_and_workspace};

#[cfg(test)]
macro_rules! forward {
    ($($method:ident($($arg:ident: $ty:ty),*)),* $(,)?) => {
        $(fn $method(&mut self, $($arg: $ty),*) -> Result<(), String> {
            UiDocumentAccess::$method(self.document, $($arg),*)
        })*
    };
}

mod audio;
mod calendar;
mod clock;
mod context_menu;
mod media;
mod network;
mod panel;
mod start;
mod task_menu;
mod weather;

pub use audio::{project_audio, project_audio_full};
pub use calendar::project_calendar;
use clock::month_name;
pub use clock::{ClockDisplay, clock_time_text, local_date, project_clock, project_clock_at};
pub use context_menu::project_context_menu;
pub use media::project_media;
pub use network::{
    project_network, project_network_details, project_network_full, project_network_secret,
    project_network_status,
};
#[cfg(test)]
pub(crate) use panel::pager_active_fill;
pub use panel::{
    BATTERY_PROGRESS_HEIGHT, PanelView, TaskbarItems, for_each_panel_record, project,
    project_battery_menu, project_battery_tray, project_panel, reapply_element_backgrounds,
    set_task_accent, set_taskbar_surface, task_icon_device_px, task_icon_px, taskbar_css_height,
    taskbar_css_width,
};
pub(crate) use panel::{
    project_element_backgrounds, project_pager, project_pager_visibility, project_status_tray,
    project_task_line, project_task_progress, project_task_slots_extent, project_taskbar_height,
    project_tray_button, set_taskbar_border, task_accent, task_slots_rendered, taskbar_is_vertical,
    tray_any_visible, workspace_pager_visible,
};
pub use start::{
    START_STATUS_NODE, START_WHEEL_ROWS, project_start, project_start_error,
    start_application_for_slot, start_application_name_for_slot, start_scroll_max,
    start_scroll_step, start_visible_results,
};
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
    Ok(runtime_image(raster))
}

fn runtime_image(raster: Rgb8Raster) -> RuntimeImage {
    RuntimeImage {
        source: raster.source,
        width: raster.width,
        height: raster.height,
        pixels: raster.pixels,
    }
}

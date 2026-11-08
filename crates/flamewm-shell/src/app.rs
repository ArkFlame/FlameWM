use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use chrono::{Datelike, Local};
use flamewm_api::settings::{SettingValue, SettingsSnapshot, SettingsTransaction};
use flamewm_control_core::ControlRequest;
use flamewm_platform::panel::TaskbarFrame;
use flamewm_shell::async_projection::{self, StartViewNote};
use flamewm_shell::icon_loader::{DependentSurface, IconLoader, IconTarget};
use flamewm_shell::projection::PanelView;
use flamewm_shell::quick_controls::protocol::OpenResult;
use flamewm_shell::start::StartCategory;
use flamewm_shell::taskbar::tasks::icon_geometry::{
    self, IconGeometrySink, IconGeometryTracker, IconRect,
};
use flamewm_shell::{projection, ShellControl, ShellRuntime, ShellSnapshot, ShellSurfaces};
use flamewm_shell_core::clock::ClockDateTracker;
use flamewm_shell_core::popup::PopupRefusal;
use flamewm_shell_core::status::{AudioSettings, AudioTab, NetworkQuery};
use flamewm_ui_x11::{SurfaceRuntime, UiBackendError};

use crate::entry::event_router::quick_popup_group;

/// Minimum gap between clock-driven heap trims; churn is handed back at most this often.
const CLOCK_HEAP_TRIM_INTERVAL: Duration = Duration::from_secs(600);

thread_local! {
    /// When the last clock-driven heap trim ran (UI thread only).
    static LAST_CLOCK_HEAP_TRIM: std::cell::Cell<Option<Instant>> =
        const { std::cell::Cell::new(None) };
}

/// C07 stage helper: open a static profiler span. Static labels only.
macro_rules! shell_stage {
    ($label:expr) => {{
        let __point = flamewm_profiler::ProfilePoint::new($label);
        __point.start()
    }};
}

mod control;
mod dock;
mod helpers;
mod icons;
mod input;
mod ipc;
mod launcher_session;
mod panels;
mod popups;
mod progress_gate;
mod quick_popups;
mod settings;
mod slow_op;
mod start_session;
mod start_view;
mod timers;
mod tray;
mod weather;

use helpers::{
    clear_transient_state_fields, earliest_wake, end_start_session_fields,
    interaction_target_is_current, media_popup_must_close, media_system_request,
    pager_cell_window_rect, panel_projection_needed, resolve_control_output,
    start_icon_refresh_due, take_context_menu_for_popup, taskbar_enabled, taskbar_frame,
    RuntimeIconGeometrySink, ZONE_SETTLE,
};
use quick_popups::{
    apply_quick_open_result, is_quick_popup_kind, quick_popup_kind, QuickOpenResolution,
};
pub(crate) use slow_op::{is_popup_refusal, shell_slow_note};
use start_session::{
    start_launch_session_matches, start_toggle_action, StartLaunchContext, StartToggleAction,
};

pub(crate) struct ShellLoop {
    pub(crate) shell: ShellRuntime,
    pub(crate) surfaces: ShellSurfaces,
    pub(crate) start_model: flamewm_shell_core::StartModel,
    pub(crate) dispatcher: Option<std::sync::Arc<flamewm_control_dbus::MutationDispatcher>>,
    pub(crate) start_category: StartCategory,
    pub(crate) start_query: String,
    pub(crate) start_selection: flamewm_shell_core::selection::ListSelection,
    /// First visible Start application row (rows, not pixels); clamped at use.
    pub(crate) start_scroll: usize,
    pub(crate) desktop_hidden: Vec<flamewm_api::WindowRef>,
    pub(crate) launcher: flamewm_shell::launcher::LauncherState,
    pub(crate) task_drag: Option<flamewm_shell::taskbar::tasks::drag::TaskDrag>,
    pub(crate) task_press_target: Option<(usize, flamewm_shell::runtime::TaskSlotTarget)>,
    pub(crate) start_drag: Option<crate::entry::event_router::StartAppDrag>,
    pub(crate) start_open: bool,
    /// Start error line for a failed launch; cleared on success or reopen.
    pub(crate) start_error: Option<String>,
    /// Invocation currently owned by the visible Start-menu session.
    pub(crate) active_start_launch: Option<StartLaunchContext>,
    pub(crate) context_menu: Option<flamewm_shell::ContextMenuState>,
    pub(crate) network_secret: String,
    pub(crate) network_secret_request_id: Option<u64>,
    pub(crate) local_date: (i32, u8, u8),
    pub(crate) date_tracker: ClockDateTracker,
    pub(crate) audio_tab: AudioTab,
    pub(crate) audio_settings: AudioSettings,
    cursor_theme: flamewm_shell::cursor_theme::CursorThemeSync,
    pub(crate) network_query: NetworkQuery,
    pub(crate) settings_revision: u64,
    pub(crate) settings_cache: Option<SettingsSnapshot>,
    pub(crate) dock_surface: Option<flamewm_ui_x11::SurfaceHandle>,
    pub(crate) dock_entries: Vec<flamewm_shell_core::dock::DockEntry>,
    pub(crate) dock_hover: Option<usize>,
    pub(crate) dock_last: Option<dock::DockSync>,
    pub(crate) dock_catalog: Option<Vec<flamewm_api::applications::DesktopApplication>>,
    pub(crate) dock_drag: Option<dock::DockDrag>,
    pub(crate) dock_menu: Option<flamewm_ui_x11::SurfaceHandle>,
    pub(crate) dock_menu_slot: Option<usize>,
    pub(crate) dock_menu_items: Vec<flamewm_shell_core::dock::DockMenuAction>,
    pub(crate) settings_projection_pending: bool,
    pub(crate) icon_loader: Option<IconLoader>,
    pub(crate) task_icon_cache: async_projection::TaskIconCache,
    pub(crate) loader_root: std::path::PathBuf,
    pub(crate) start_view: StartViewNote,
    pub(crate) quick_control: flamewm_shell::quick_controls::QuickControlSupervisor,
    pub(crate) popup_session: flamewm_shell_core::popup_session::PopupSession,
    job_view_provider: Option<flamewm_integrations_linux::job_view::KioJobViewProvider>,
    launcher_entry_provider:
        Option<flamewm_integrations_linux::launcher_entry::LauncherEntryProvider>,
    task_progress: Vec<flamewm_api::panels::TaskProgressEntry>,
    task_progress_pending: bool,
    task_progress_gate: progress_gate::ProgressPaintGate,
    task_progress_deadline: Option<Instant>,
    /// Shell-side record that the audio popup was last opened and not yet
    /// closed by the shell. Drives the tray-volume toggle decision.
    pub(crate) quick_audio_open: bool,
    /// Kind of the quick-control popup currently open, if any.
    pub(crate) quick_kind_open: Option<flamewm_shell::quick_controls::QuickControlKind>,
    /// Group closed by the latest outside-press dismissal; consumes its opener.
    pub(crate) dismiss_latch: Option<crate::entry::event_router::DismissLatch>,
    /// One-shot: task-slot icons re-requested once the loader exists.
    pub(crate) task_icon_prime_armed: bool,
    /// `StatusNotifier` tray host; `None` when another watcher owns the bus name.
    pub(crate) tray: Option<std::rc::Rc<std::cell::RefCell<flamewm_shell::tray_host::TrayHost>>>,
    /// Last `_NET_WM_ICON_GEOMETRY` written per task client window. Per shell
    /// run: a restart starts empty.
    pub(crate) icon_geometry: IconGeometryTracker,
    /// Volume OSD: one retained overlay, driven by the audio snapshot.
    pub(crate) osd: flamewm_shell::osd::OsdController,
    pub(crate) switcher: flamewm_shell::switcher::SwitcherController,
    /// Weather scheduler: the only owner of weather fetches. Pushes its feed to the popup host.
    pub(crate) weather: flamewm_shell::weather_host::WeatherHost,
    /// Set when the tray or the popup host must receive the current weather feed.
    pub(crate) weather_feed_pending: bool,
    /// Automatic location lookup cadence (opt-in via `autoLocationEnabled`).
    pub(crate) auto_lookup: flamewm_shell::weather_auto::AutoLookup,
    /// The reactor's single shared wake slot; the tick publishes the earliest pending deadline.
    pub(crate) wake: Option<flamewm_reactor::WakeDeadline>,
    /// Zone this process applied from the automatic time zone; `None` means the system zone.
    pub(crate) auto_time_zone_applied: Option<String>,
    /// Set when the process zone changed; the clock and date are re-projected on the next turn.
    pub(crate) time_zone_changed: bool,
    /// Second clock projection after a zone change: chrono may still serve the old zone
    /// for up to a second, so the first projection can be stale.
    pub(crate) zone_settle_at: Option<std::time::Instant>,
    /// Set when the icon theme changed: open Start re-projects, and the next
    /// Start open re-requests its icons under the new theme.
    pub(crate) start_icons_stale: bool,
    pointer_position: Option<(i32, i32)>,
}

impl ShellLoop {
    pub(crate) fn new(
        shell: ShellRuntime,
        surfaces: ShellSurfaces,
        start_model: flamewm_shell_core::StartModel,
        dispatcher: Option<std::sync::Arc<flamewm_control_dbus::MutationDispatcher>>,
        loader_root: std::path::PathBuf,
        quick_program: String,
    ) -> Self {
        let quick_control =
            flamewm_shell::quick_controls::QuickControlSupervisor::new(quick_program);
        Self {
            shell,
            surfaces,
            start_model,
            dispatcher,
            start_category: StartCategory::All,
            start_query: String::new(),
            start_selection: flamewm_shell_core::selection::ListSelection::default(),
            start_scroll: 0,
            desktop_hidden: Vec::new(),
            launcher: Default::default(),
            task_drag: None,
            task_press_target: None,
            start_drag: None,
            start_open: false,
            start_error: None,
            active_start_launch: None,
            quick_audio_open: false,
            quick_kind_open: None,
            dismiss_latch: None,
            context_menu: None,
            network_secret: String::new(),
            network_secret_request_id: None,
            local_date: projection::local_date(),
            date_tracker: ClockDateTracker::new(projection::local_date()),
            audio_tab: AudioTab::default(),
            audio_settings: AudioSettings::default(),
            cursor_theme: flamewm_shell::cursor_theme::CursorThemeSync::default(),
            network_query: NetworkQuery::default(),
            settings_revision: 0,
            settings_cache: None,
            dock_surface: None,
            dock_entries: Vec::new(),
            dock_hover: None,
            dock_last: None,
            dock_catalog: None,
            dock_drag: None,
            dock_menu: None,
            dock_menu_slot: None,
            dock_menu_items: Vec::new(),
            settings_projection_pending: false,
            icon_loader: None,
            task_icon_cache: Default::default(),
            loader_root,
            start_view: StartViewNote::default(),
            quick_control,
            popup_session: flamewm_shell_core::popup_session::PopupSession::default(),
            job_view_provider: None,
            launcher_entry_provider: None,
            task_progress: Vec::new(),
            task_progress_pending: false,
            task_progress_gate: progress_gate::ProgressPaintGate::default(),
            task_progress_deadline: None,
            task_icon_prime_armed: true,
            tray: None,
            icon_geometry: IconGeometryTracker::default(),
            osd: flamewm_shell::osd::OsdController::default(),
            switcher: flamewm_shell::switcher::SwitcherController::default(),
            weather: flamewm_shell::weather_host::WeatherHost::new(),
            weather_feed_pending: true,
            auto_lookup: flamewm_shell::weather_auto::AutoLookup::default(),
            wake: None,
            auto_time_zone_applied: None,
            time_zone_changed: false,
            zone_settle_at: None,
            start_icons_stale: false,
            pointer_position: None,
        }
    }
}

#[cfg(test)]
mod tests;

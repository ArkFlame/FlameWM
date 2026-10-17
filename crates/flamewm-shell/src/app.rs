use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use chrono::{Datelike, Local};
use flamewm_api::settings::{SettingValue, SettingsSnapshot, SettingsTransaction};
use flamewm_control_core::ControlRequest;
use flamewm_platform::panel::TaskbarFrame;
use flamewm_shell::async_projection::{self, StartViewNote};
use flamewm_shell::icon_loader::{DependentSurface, IconLoader, IconTarget};
use flamewm_shell::projection::PanelView;
use flamewm_shell::start::StartCategory;
use flamewm_shell::taskbar::tasks::icon_geometry::{
    self, IconGeometrySink, IconGeometryTracker, IconRect,
};
use flamewm_shell::{projection, ShellControl, ShellRuntime, ShellSnapshot, ShellSurfaces};
use flamewm_shell_core::clock::ClockDateTracker;
use flamewm_shell_core::popup::PopupRefusal;
use flamewm_shell_core::status::{AudioSettings, AudioTab, NetworkQuery};
use flamewm_ui_x11::{SurfaceRuntime, UiBackendError};

use crate::event_router::quick_popup_group;

/// C07: one shell operation owns exactly one total span; nested totals must
/// not double-own. `operation_total_active` on the `ShellLoop` event turn
/// marks the outer owner; inner paths check it and skip their own total.
pub(crate) const SHELL_SLOW_TOTAL_NS: u128 = 16_670_000;
pub(crate) const SHELL_SLOW_COOLDOWN_MS: u64 = 500;

/// C07 slow-operation note: when `started`->now exceeds one 60Hz frame,
/// emit `shell.operation.slow` debug with a 500ms per-op cooldown.
pub(crate) fn shell_slow_note(op: &'static str, started: Instant) {
    if started.elapsed().as_nanos() <= SHELL_SLOW_TOTAL_NS {
        return;
    }
    fn last_for(op: &str) -> Option<&'static std::sync::atomic::AtomicU64> {
        use std::sync::atomic::AtomicU64;
        static START: AtomicU64 = AtomicU64::new(0);
        static CONTEXT: AtomicU64 = AtomicU64::new(0);
        static WORKSPACE: AtomicU64 = AtomicU64::new(0);
        match op {
            "start" => Some(&START),
            "context" => Some(&CONTEXT),
            "workspace" => Some(&WORKSPACE),
            _ => None,
        }
    }
    let wall_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    if let Some(last) = last_for(op) {
        let prev = last.load(std::sync::atomic::Ordering::Relaxed);
        if wall_ms.saturating_sub(prev) < SHELL_SLOW_COOLDOWN_MS {
            return;
        }
        last.store(wall_ms, std::sync::atomic::Ordering::Relaxed);
    }
    flamewm_debug::emit(
        flamewm_debug::DebugEventId("shell.operation.slow"),
        Duration::from_millis(SHELL_SLOW_COOLDOWN_MS),
        || format!("op={op} slow"),
    );
}

/// C07 stage helper: open a static profiler span. Static labels only.
macro_rules! shell_stage {
    ($label:expr) => {{
        let __point = flamewm_profiler::ProfilePoint::new($label);
        __point.start()
    }};
}

/// Popup measurement refusal: layout pending, anchor pending, or transient
/// pointer-grab contention. Callers swallow these (state untouched).
pub(crate) fn is_popup_refusal(error: &str) -> bool {
    error == PopupRefusal::PendingLayout.to_string()
        || error == PopupRefusal::PendingAnchor.to_string()
        || error == PopupRefusal::PointerGrabRefused.to_string()
}

pub(crate) struct ShellLoop {
    pub(crate) shell: ShellRuntime,
    pub(crate) surfaces: ShellSurfaces,
    pub(crate) start_model: flamewm_shell_core::StartModel,
    pub(crate) dispatcher: Option<std::sync::Arc<flamewm_control_dbus::MutationDispatcher>>,
    pub(crate) start_category: StartCategory,
    pub(crate) start_query: String,
    pub(crate) task_drag: Option<flamewm_shell::taskbar::tasks::drag::TaskDrag>,
    pub(crate) start_open: bool,
    /// Start error line for a failed launch; cleared on success or reopen.
    pub(crate) start_error: Option<String>,
    /// Invocation currently owned by the visible Start-menu session.
    pub(crate) active_start_launch: Option<u64>,
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
    pub(crate) settings_projection_pending: bool,
    pub(crate) icon_loader: Option<IconLoader>,
    pub(crate) task_icon_cache: async_projection::TaskIconCache,
    pub(crate) loader_root: std::path::PathBuf,
    pub(crate) start_view: StartViewNote,
    pub(crate) quick_control: flamewm_shell::quick_controls::QuickControlSupervisor,
    /// Shell-side record that the audio popup was last opened and not yet
    /// closed by the shell. Drives the tray-volume toggle decision.
    pub(crate) quick_audio_open: bool,
    /// Kind of the quick-control popup currently open, if any.
    pub(crate) quick_kind_open: Option<flamewm_shell::quick_controls::QuickControlKind>,
    /// Group closed by the latest outside-press dismissal; consumes its opener.
    pub(crate) dismiss_latch: Option<crate::event_router::DismissLatch>,
    /// One-shot: task-slot icons re-requested once the loader exists.
    pub(crate) task_icon_prime_armed: bool,
    /// Last `_NET_WM_ICON_GEOMETRY` written per task client window. Per shell
    /// run: a restart starts empty.
    pub(crate) icon_geometry: IconGeometryTracker,
    /// Volume OSD: one retained overlay, driven by the audio snapshot.
    pub(crate) osd: flamewm_shell::osd::OsdController,
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
            task_drag: None,
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
            settings_projection_pending: false,
            icon_loader: None,
            task_icon_cache: Default::default(),
            loader_root,
            start_view: StartViewNote::default(),
            quick_control,
            task_icon_prime_armed: true,
            icon_geometry: IconGeometryTracker::default(),
            osd: flamewm_shell::osd::OsdController::default(),
            weather: flamewm_shell::weather_host::WeatherHost::new(),
            weather_feed_pending: true,
            auto_lookup: flamewm_shell::weather_auto::AutoLookup::default(),
            wake: None,
            auto_time_zone_applied: None,
            time_zone_changed: false,
            zone_settle_at: None,
            start_icons_stale: false,
        }
    }

    /// J07: start the helper asynchronously after panel show. Spawn failure
    /// is nonfatal: the shell keeps running with in-process media only.
    pub(crate) fn start_helper_after_panel(&mut self) {
        if let Err(error) = self.quick_control.start_after_panel() {
            eprintln!("flamewm-shell: quick-control helper spawn failed (nonfatal): {error}");
        }
    }

    pub(crate) fn note_signal(&mut self, signal: &flamewm_control_wire::ControlSignal) {
        if let flamewm_control_wire::ControlSignal::SettingsSnapshotChanged { snapshot } = signal {
            self.apply_settings_snapshot(snapshot.clone());
        }
        self.shell.mark_signal(signal);
    }

    pub(crate) fn apply_settings_snapshot(&mut self, snapshot: SettingsSnapshot) {
        if self.settings_cache.as_ref().is_some_and(|current| snapshot.revision <= current.revision) {
            return;
        }
        self.settings_revision = snapshot.revision;
        self.cursor_theme.sync_snapshot(&snapshot);
        self.weather.apply_settings(
            flamewm_shell_core::weather::WeatherSettings::from_snapshot(&snapshot),
        );
        self.weather_feed_pending = true;
        let theme = crate::settings_projection::icon_theme_name(Some(&snapshot));
        if let Some(loader) = self.icon_loader.as_mut() {
            if loader.set_theme(&theme) {
                // Re-prime task slots so they re-request under the new theme.
                self.task_icon_prime_armed = true;
                // Drop rasters cached under the old theme; Start re-requests.
                async_projection::invalidate_icon_cache();
                self.start_icons_stale = true;
            }
        }
        if let Some(SettingValue::Boolean(raise)) = snapshot.values.get(AudioSettings::SETTINGS_KEY)
        {
            self.audio_settings.raise_maximum = *raise;
        }
        // A hidden taskbar item closes its open quick-control popup.
        let items = crate::settings_projection::taskbar_items(Some(&snapshot));
        if self.quick_kind_open.is_some_and(|kind| !quick_kind_shown(kind, items)) {
            self.close_quick_control_nonfatal();
        }
        let desired_zone = flamewm_shell::weather_auto::desired_time_zone(&snapshot);
        self.settings_cache = Some(snapshot);
        self.settings_projection_pending = true;
        self.set_auto_time_zone(desired_zone);
        // This runs on the settings signal, not an X event: wake the loop so the tick
        // projects the change now instead of at the next pointer or clock event.
        self.request_tick();
    }

    /// Makes the reactor return from dispatch immediately so the next tick runs.
    fn request_tick(&self) {
        if let Some(wake) = &self.wake {
            wake.set(Some(std::time::Instant::now()));
        }
    }

    /// Makes `desired` the process zone when it differs from what this shell applied. `None`
    /// restores the system zone. A failed zone file leaves the current zone unchanged.
    fn set_auto_time_zone(&mut self, desired: Option<String>) {
        if desired == self.auto_time_zone_applied {
            return;
        }
        match desired {
            Some(zone) => {
                if !flamewm_shell::weather_auto::apply_time_zone(&zone) {
                    eprintln!("flamewm-shell: time zone not applied (zone file missing or invalid)");
                    return;
                }
                self.auto_time_zone_applied = Some(zone);
            }
            None => {
                flamewm_shell::weather_auto::restore_system_time_zone();
                self.auto_time_zone_applied = None;
            }
        }
        self.time_zone_changed = true;
        self.zone_settle_at = Some(std::time::Instant::now() + ZONE_SETTLE);
    }

    /// Automatic location tick: one lookup on the cadence, then the fix is written as the
    /// weather location and cached zone. Failures keep the last values and are not fatal.
    fn tick_auto_location(&mut self) {
        let Some(enabled) = self
            .settings_cache
            .as_ref()
            .map(|s| flamewm_shell::weather_auto::flag(s, flamewm_shell::weather_auto::AUTO_LOCATION_KEY))
        else {
            return;
        };
        let online = self.shell.snapshot().system.network.availability
            == flamewm_api::system::ServiceAvailability::Available;
        let Some(result) = self.auto_lookup.tick(
            std::time::Instant::now(),
            enabled,
            online,
            flamewm_shell::weather_auto::spawn_ip_lookup,
        ) else {
            return;
        };
        match result {
            Ok(fix) => self.apply_auto_fix(fix),
            Err(error) => eprintln!("flamewm-shell: automatic location unavailable: {error:?}"),
        }
    }

    fn apply_auto_fix(&mut self, fix: flamewm_shell::weather_auto::IpFix) {
        use flamewm_shell::weather_auto as auto;
        let Some(snapshot) = self.settings_cache.clone() else {
            return;
        };
        // Re-check: a manual city pick may have turned automatic location off meanwhile.
        if !auto::flag(&snapshot, auto::AUTO_LOCATION_KEY) {
            return;
        }
        let current = flamewm_shell_core::weather::WeatherSettings::from_snapshot(&snapshot);
        let mut changes: Vec<flamewm_api::settings::SettingsChange> =
            auto::location_change(&current, &fix).into_iter().collect();
        if let Some(zone) = fix.time_zone.as_ref() {
            if snapshot.values.get(auto::AUTO_TIME_ZONE_KEY)
                != Some(&SettingValue::Text(zone.clone()))
            {
                changes.push(flamewm_api::settings::SettingsChange {
                    key: auto::AUTO_TIME_ZONE_KEY.to_owned(),
                    value: Some(SettingValue::Text(zone.clone())),
                });
            }
        }
        if !changes.is_empty() {
            self.submit(ControlRequest::ApplySettings(SettingsTransaction {
                expected_revision: self.settings_revision,
                changes,
                reset_section: None,
            }));
        }
        if let Some(zone) = fix.time_zone.filter(|_| auto::flag(&snapshot, auto::AUTO_TIME_ZONE_ENABLED_KEY)) {
            self.set_auto_time_zone(Some(zone));
        }
    }

    /// J07 nonblocking submit: enqueue onto the mutation queue and return
    /// immediately. Full/closed queue is a nonfatal diagnostic.
    pub(crate) fn submit(&self, request: ControlRequest) {
        flamewm_shell::control_actions::enqueue_action(self.dispatcher.as_ref(), request);
    }

    /// Shared transient close transaction and semantic state update.
    pub(crate) fn close_transients(
        &mut self,
        runtime: &mut SurfaceRuntime,
    ) -> Result<(), UiBackendError> {
        self.surfaces.close_transients(runtime).map_err(UiBackendError::Renderer)?;
        self.clear_transient_state();
        self.active_start_launch = None;
        Ok(())
    }

    fn clear_transient_state(&mut self) {
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

    pub(crate) fn apply_controls(
        &mut self,
        runtime: &mut SurfaceRuntime,
    ) -> Result<(), UiBackendError> {
        let controls: Vec<_> = self.shell.drain_controls().collect();
        for control in controls {
            match control {
                ShellControl::ToggleStart => {
                    // C07: start open owns one total around the open path.
                    // F10: read current context generation; refuse when
                    // edge/geometry is inconsistent.
                    self.note_popup_generation("start");
                    if self.refuse_popup_on_inconsistent_geometry().is_some() {
                        continue;
                    }
                    let was_open = self.surfaces.is_start_group_open();
                    if !was_open {
                        let started = Instant::now();
                        let _total = shell_stage!("shell.start.open.total");
                        // Reopening the menu clears any stale launch error.
                        self.start_error = None;
                        self.active_start_launch = None;
                        self.project_start(runtime)?;
                        let _measure = shell_stage!("shell.start.open.measure");
                        let _place = shell_stage!("shell.start.open.place");
                        let _prepare = shell_stage!("shell.start.open.prepare");
                        self.apply_start_backdrop(runtime);
                        self.apply_start_slide(runtime);
                        let opened = self.surfaces.toggle_start(runtime);
                        let _present = shell_stage!("shell.start.open.present");
                        shell_slow_note("start", started);
                        match opened {
                            Ok(()) => {
                                self.start_open = !was_open;
                            }
                            Err(error) if is_popup_refusal(&error) => {}
                            Err(error) => return Err(UiBackendError::Renderer(error)),
                        }
                    } else {
                        let _close = shell_stage!("shell.start.open.close");
                        match self.surfaces.toggle_start(runtime) {
                            Ok(()) => self.end_start_session(),
                            Err(error) if is_popup_refusal(&error) => {}
                            Err(error) => return Err(UiBackendError::Renderer(error)),
                        }
                    }
                }
                ShellControl::CloseStart | ShellControl::ClosePopovers => {
                    self.close_transients(runtime)?;
                }
                ShellControl::SelectStartCategory(category) => {
                    self.start_category = category;
                    self.start_open = true;
                    // F10: read current context generation; refuse when
                    // edge/geometry is inconsistent.
                    self.note_popup_generation("start");
                    if self.refuse_popup_on_inconsistent_geometry().is_some() {
                        continue;
                    }
                    // C07: SelectStartCategory owns the start total.
                    let started = Instant::now();
                    let _total = shell_stage!("shell.start.open.total");
                    self.project_start(runtime)?;
                    let _measure = shell_stage!("shell.start.open.measure");
                    let _place = shell_stage!("shell.start.open.place");
                    let _prepare = shell_stage!("shell.start.open.prepare");
                    self.apply_start_backdrop(runtime);
                    self.apply_start_slide(runtime);
                    let opened = self.surfaces.open_start_group(runtime);
                    let _present = shell_stage!("shell.start.open.present");
                    shell_slow_note("start", started);
                    match opened {
                        Ok(()) => {}
                        Err(error) if is_popup_refusal(&error) => {}
                        Err(error) => return Err(UiBackendError::Renderer(error)),
                    }
                }
                ShellControl::OpenPopover(name) => {
                    // J07: media stays in-process; audio/network/clock/battery
                    // forward to the supervisor, then return immediately.
                    if matches!(
                        name.as_str(),
                        "volume" | "audio" | "network" | "clock" | "battery" | "weather"
                    ) {
                        self.open_quick_control(runtime, &name);
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
                    match self.surfaces.open_status(runtime, &name) {
                        Ok(()) => {}
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
                    // C04: task/shell context menus invalidate the helper
                    // anchor. F10: refuse when edge/geometry is inconsistent.
                    if self.refuse_popup_on_inconsistent_geometry().is_some() {
                        continue;
                    }
                    let started = Instant::now();
                    let _total = shell_stage!("shell.context.open.total");
                    self.close_quick_control_nonfatal();
                    {
                        let _project = shell_stage!("shell.context.open.project");
                        let settings = self
                            .settings_cache
                            .clone()
                            .unwrap_or_else(|| flamewm_api::settings::SettingsSnapshot::new(0));
                        runtime.with_document(self.surfaces.task_menu, |document| {
                            projection::project_context_menu(
                                document,
                                self.shell.snapshot(),
                                &settings,
                                Some(&state),
                            )
                        })?;
                        // Translucent fill needs the blurred root under the popup: the popup window is
                        // shape-backed (no alpha), so without this the fill composites over black.
                        let (opacity, blur) =
                            crate::settings_projection::start_backdrop_percents(&settings);
                        runtime.set_popup_wallpaper(self.surfaces.task_menu, self.popup_wallpaper_canvas(&settings))?;
                        runtime.set_popup_backdrop(self.surfaces.task_menu, Some((opacity, blur)))?;
                    }
                    self.context_menu = Some(state.clone());
                    let _measure = shell_stage!("shell.context.open.measure");
                    let _place = shell_stage!("shell.context.open.place");
                    let _prepare = shell_stage!("shell.context.open.prepare");
                    let opened = self
                        .surfaces
                        .open_context_menu(runtime, &state, self.shell.snapshot())
                        .map_err(UiBackendError::Renderer);
                    let _present = shell_stage!("shell.context.open.present");
                    shell_slow_note("context", started);
                    opened?;
                }
                ShellControl::CloseContextMenu => {
                    let _close = shell_stage!("shell.context.open.close");
                    self.surfaces.close_transients(runtime).map_err(UiBackendError::Renderer)?;
                    self.context_menu = None;
                }
                ShellControl::SessionAction(action) => {
                    self.submit(ControlRequest::SessionAction(action));
                    self.surfaces.close_transients(runtime).map_err(UiBackendError::Renderer)?;
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
            return self.shell.network_system_action_for_path(&path, false, false);
        }
        if action == "network.secret.cancel" {
            return self.network_secret_request(false);
        }
        if action == "network.secret.submit" {
            return self.network_secret_request(true);
        }
        None
    }

    pub(crate) fn network_secret_request(&self, submit: bool) -> Option<ControlRequest> {
        let snapshot = self.shell.snapshot();
        let secret = snapshot.system.network.pending_secret.as_ref()?;
        let action = if submit {
            flamewm_api::system::SystemAction::SubmitNetworkSecret {
                request_id: secret.request_id,
                generation: snapshot.system.network.generation,
                secret: self.network_secret.clone(),
            }
        } else {
            flamewm_api::system::SystemAction::CancelNetworkSecret {
                request_id: secret.request_id,
                generation: snapshot.system.network.generation,
            }
        };
        Some(ControlRequest::SystemAction {
            action,
            expected_revision: snapshot.system.revision,
        })
    }

    pub(crate) fn project_network_secret(
        &self,
        _runtime: &mut SurfaceRuntime,
    ) -> Result<(), UiBackendError> {
        // J07: the secret editor lives in the helper process; the parent
        // keeps the masked-length-free no-op so keyboard paths stay total.
        Ok(())
    }

    pub(crate) fn clear_network_secret(
        &mut self,
        _runtime: &mut SurfaceRuntime,
    ) -> Result<(), UiBackendError> {
        self.network_secret.clear();
        Ok(())
    }

    pub(crate) fn project_status_content(
        &mut self,
        runtime: &mut SurfaceRuntime,
        name: &str,
    ) -> Result<(), UiBackendError> {
        // Project-first: the measured open reads intrinsic size from the
        // already-projected document, so content must land before measure.
        // J07: only media projects in-process; helper kinds are unreachable
        // here (routed to the supervisor before this call).
        let snapshot = self.shell.snapshot();
        match name {
            "media" => runtime.with_document(self.surfaces.media, |document| {
                projection::project_media(document, snapshot)
            })?,
            _ => {}
        }
        Ok(())
    }

    /// C04: send quick-control CLOSE; close signals are owned in main
    /// (workspace change, menus, media open, outside release, geometry
    /// invalidation). Best-effort and nonfatal: the loop always survives.
    pub(crate) fn close_quick_control_nonfatal(&mut self) {
        self.quick_audio_open = false;
        self.quick_kind_open = None;
        if let Err(error) = self.quick_control.close() {
            let message = error.to_string();
            // NotRunning = no helper/harmless; log the rest as diagnostics.
            if !message.contains("not running") {
                eprintln!("flamewm-shell: quick-control close failed (nonfatal): {message}");
            }
        }
    }

    /// J07: retained anchor + work area + edge -> supervisor.open, then
    /// return immediately. Failures are nonfatal (logged, loop survives).
    /// F10: always reads the current context generation first and
    /// refuses when edge/geometry is inconsistent.
    pub(crate) fn open_quick_control(&mut self, runtime: &mut SurfaceRuntime, name: &str) {
        use flamewm_shell::quick_controls::QuickControlKind;
        self.note_popup_generation("quick");
        if self.refuse_popup_on_inconsistent_geometry().is_some() {
            return;
        }
        // One visible menu at a time: a quick popup dismisses an open shell context menu.
        if take_context_menu_for_popup(&mut self.context_menu) {
            if let Err(error) = self.surfaces.close_transients(runtime) {
                eprintln!("flamewm-shell: close context menu for quick popup failed: {error}");
            }
        }
        let kind = match name {
            "volume" | "audio" => QuickControlKind::Audio,
            "network" => QuickControlKind::Network,
            "clock" => QuickControlKind::Calendar,
            "battery" => QuickControlKind::Battery,
            "weather" => QuickControlKind::Weather,
            _ => return,
        };
        let source_id = match kind {
            QuickControlKind::Audio => "tray-volume",
            QuickControlKind::Network => "tray-network",
            QuickControlKind::Calendar => "clock-button",
            QuickControlKind::Battery => "tray-battery",
            QuickControlKind::Weather => "tray-weather",
        };
        // Retained anchor preferred; snapshot geometry is the sole
        // pre-map fallback (same contract as the in-process path).
        let anchor =
            self.surfaces
                .quick_anchor(runtime, &self.pending_anchor_fallback(), source_id);
        // Panel-aware work area: quick popups pin right to the output and fit
        // vertically inside it, so they never cover the panel strip.
        let work_area = self.surfaces.work_area();
        let panel_edge = self.surfaces.panel_edge();
        // Retained panel rect: the helper clips slide transit frames against the dock.
        let dock = runtime
            .surface_device_rect(self.surfaces.panel())
            .ok()
            .map(|r| flamewm_api::Rect::new(r.x as i32, r.y as i32, r.width as i32, r.height as i32));
        if kind == QuickControlKind::Weather {
            // Fetch on this open; the feed goes out before OPEN on the same ordered channel.
            self.weather.request_open_fetch();
            self.push_weather_feed();
        }
        flamewm_shell::quick_controls::host::open_via_supervisor_nonfatal(
            &mut self.quick_control,
            kind,
            anchor,
            work_area,
            panel_edge,
            dock,
        );
        self.quick_audio_open = kind == QuickControlKind::Audio;
        self.quick_kind_open = Some(kind);
        runtime.arm_raw_press_observer();
    }

    /// Popup group that an outside-press dismissal would close right now.
    pub(crate) fn open_popup_group(&self) -> Option<&'static str> {
        if self.surfaces.is_start_group_open() {
            return Some("start");
        }
        if self.surfaces.is_media_open() {
            return Some("media");
        }
        self.quick_kind_open.map(quick_popup_group)
    }

    /// Pre-map anchor fallback: the panel snapshot geometry. Used only
    /// when no retained layout exists yet (same sole-fallback contract as
    /// the in-process popup path).
    pub(crate) fn pending_anchor_fallback(&self) -> flamewm_api::Rect {
        self.shell
            .snapshot()
            .panels
            .panels
            .first()
            .map(|panel| panel.geometry)
            .unwrap_or(flamewm_api::Rect::new(0, 0, 1, 1))
    }

    pub(crate) fn project_start(
        &mut self,
        runtime: &mut SurfaceRuntime,
    ) -> Result<(), UiBackendError> {
        // C07: project stage only. The caller owns the operation total, so
        // nested totals never double-own one operation.
        let _project = shell_stage!("shell.start.open.project");
        // This projection re-requests every visible icon under the current theme.
        self.start_icons_stale = false;
        let start_model = self.start_model.clone();
        let category = self.start_category;
        let query = self.start_query.clone();
        let session = self.shell.snapshot().session.clone();
        self.ensure_loader();
        let loader = self.icon_loader.as_mut();
        // ONE unified document on the single Start surface: categories +
        // search + app rows + power rows project in one pass, so one
        // intrinsic union, one move_resize, one show/present follow.
        runtime.with_document(self.surfaces.start, |document| {
            async_projection::project_start_unified(
                document,
                &start_model,
                category,
                &query,
                &session,
                loader,
            )
        })?;
        self.project_start_error_line(runtime);
        Ok(())
    }

    /// Publishes each pager cell's window-relative rect on the panel X
    /// window (`FLAMEWM_PAGER_CELLS`), read from the panel's retained layout
    /// so the canary never hard-codes screen offsets. Nonfatal on failure.
    pub(crate) fn publish_pager_cells(&self, runtime: &mut SurfaceRuntime) {
        let panel = self.surfaces.panel();
        let Ok(origin) = runtime.surface_device_rect(panel) else {
            eprintln!("flamewm-shell: pager cells skipped (stage=panel-origin)");
            return;
        };
        let mut cells: Vec<[u64; 4]> =
            Vec::with_capacity(flamewm_shell_core::workspaces::PAGE_SIZE);
        for index in 0..flamewm_shell_core::workspaces::PAGE_SIZE {
            let id = format!("workspace-{}", index + 1);
            let Some(rect) = runtime.node_device_rect_by_id(panel, &id) else {
                eprintln!("flamewm-shell: pager cells skipped (stage=layout, node={id})");
                return;
            };
            cells.push(pager_cell_window_rect(
                (rect.x, rect.y, rect.width, rect.height),
                (origin.x, origin.y),
            ));
        }
        if let Err(error) = runtime.set_pager_cells(panel, &cells) {
            eprintln!("flamewm-shell: pager cells publish failed (stage=x11-property): {error:?}");
        }
    }

    pub(crate) fn ensure_loader(&mut self) {
        if self.icon_loader.is_none() {
            let theme = crate::settings_projection::icon_theme_name(self.settings_cache.as_ref());
            self.icon_loader = Some(IconLoader::spawn(self.loader_root.clone(), theme));
        }
    }

    /// Canonical IconService wake FD for the reactor loop. Ensures the
    /// loader exists (single owner) and returns the service descriptor;
    /// the reactor only wakes on it, `drain_icons` drains on the tick.
    pub(crate) fn icon_wake_fd(&mut self) -> std::os::unix::io::RawFd {
        self.ensure_loader();
        self.icon_loader
            .as_ref()
            .map(|loader| loader.wake_fd())
            .expect("icon loader present after ensure")
    }

    /// FD wake path: drain the wake pipe only. Result payloads stay queued
    /// for the tick (`drain_icons`), which owns document mutation.
    pub(crate) fn drain_icons_pending(&mut self) {
        self.ensure_loader();
        if let Some(loader) = self.icon_loader.as_mut() {
            loader.wake_drain();
        }
    }

    pub(crate) fn drain_icons(&mut self, runtime: &mut SurfaceRuntime) {
        self.ensure_loader();
        let results = self
            .icon_loader
            .as_mut()
            .map(|loader| loader.drain_ready())
            .unwrap_or_default();
        let loader = self.icon_loader.as_ref();
        if loader.map_or(0, |l| l.queue_full_drops) > 0 || loader.map_or(0, |l| l.stale_drops) > 0 {
            if let Some(loader) = self.icon_loader.as_ref() {
                async_projection::note_loader_stats(loader);
            }
        }
        if results.is_empty() {
            return;
        }
        // Exact-target completion: worker result -> wake -> reactor ->
        // verify (key + generation + view epoch) -> cache -> mutate the
        // matching image node on every per-output panel, then redraw each
        // surface. The slot binding is shared because each output projects
        // the same task model and image key.
        let mut need_panel = false;
        let mut need_submenu = false;
        for res in results {
            let Some(raster) = res.raster else { continue };
            let image = flamewm_ui_x11::RuntimeImage {
                source: raster.source.clone(),
                width: raster.width,
                height: raster.height,
                pixels: raster.pixels,
            };
            async_projection::note_icon_result(
                &res.name,
                raster.width,
                raster.height,
                image.clone(),
            );
            let surface = match res.target {
                IconTarget::StartSlot(_) if !self.start_open => {
                    continue;
                }
                target => IconLoader::dependent_surface(target),
            };
            let apply = |document: &mut flamewm_ui_x11::UiDocumentView<'_>| {
                async_projection::apply_icon_result(
                    document,
                    res.target,
                    &res.name,
                    res.width,
                    res.height,
                    res.generation,
                    image.clone(),
                )
            };
            match surface {
                DependentSurface::Panel => {
                    for handle in self.surfaces.panel_handles() {
                        let applied = runtime.with_document(handle, |document| {
                            match apply(document)? {
                                true => Ok(true),
                                false => {
                                    async_projection::apply_cached_binding(document, res.target)
                                }
                            }
                        });
                        if matches!(applied, Ok(true)) {
                            need_panel = true;
                        }
                    }
                }
                DependentSurface::Start => {
                    if matches!(runtime.with_document(self.surfaces.start, apply), Ok(true)) {
                        need_submenu = true;
                    }
                }
            }
        }
        if need_panel {
            for handle in self.surfaces.panel_handles() {
                let _ = runtime.redraw(handle);
            }
        }
        if need_submenu {
            let _ = runtime.redraw(self.surfaces.start);
        }
    }

    pub(crate) fn launch_start_slot(
        &mut self,
        runtime: &mut SurfaceRuntime,
        slot: usize,
    ) -> Result<(), UiBackendError> {
        let Some(app_id) = projection::start_application_for_slot(
            &self.start_model,
            self.start_category,
            &self.start_query,
            slot,
        ) else {
            return Ok(());
        };
        let app_name = projection::start_application_name_for_slot(
            &self.start_model,
            self.start_category,
            &self.start_query,
            slot,
        )
        .unwrap_or_else(|| "application".to_owned());
        self.start_error = None;
        self.project_start_error_line(runtime);
        // The menu stays open until the launch result arrives: success closes
        // it in `apply_start_launch_outcomes`, failure keeps it open with the
        // Start error line. Submission itself never blocks.
        self.active_start_launch = Some(flamewm_shell::control_actions::enqueue_start_launch(
            self.dispatcher.as_ref(),
            app_name,
            ControlRequest::LaunchApplication {
                app: app_id,
                options: flamewm_api::applications::ApplicationLaunchOptions::default(),
            },
        ));
        Ok(())
    }

    /// Apply Start launch results drained from the mutation result source.
    /// A failure sets the Start error line and keeps the menu open; a success
    /// clears the line and closes the menu that launched it.
    fn apply_start_launch_outcomes(
        &mut self,
        runtime: &mut SurfaceRuntime,
    ) -> Result<(), UiBackendError> {
        use flamewm_shell::control_actions::{start_error_after_launch, StartLaunchOutcome};
        let outcomes = flamewm_shell::control_actions::take_launch_outcomes();
        let Some(invocation) = self.active_start_launch else {
            return Ok(());
        };
        let Some(outcome) =
            flamewm_shell::control_actions::matching_launch_outcome(outcomes, invocation)
        else {
            return Ok(());
        };
        if !self.start_open {
            self.active_start_launch = None;
            return Ok(());
        }
        self.start_error = start_error_after_launch(&outcome);
        if matches!(outcome, StartLaunchOutcome::Started { .. }) {
            self.active_start_launch = None;
            return self.close_transients(runtime);
        }
        self.active_start_launch = None;
        self.project_start_error_line(runtime);
        runtime.redraw(self.surfaces.start)?;
        Ok(())
    }

    /// Project the Start error line into the Start surface document. Errors
    /// are nonfatal diagnostics so a projection fault never breaks the loop.
    fn project_start_error_line(&mut self, runtime: &mut SurfaceRuntime) {
        let message = self.start_error.clone();
        let result = runtime.with_document(self.surfaces.start, |document| {
            projection::project_start_error(document, message.as_deref())
        });
        if let Err(error) = result {
            eprintln!("flamewm-shell: start error line projection failed: {error:?}");
        }
    }

    pub(crate) fn click_task_slot(
        &mut self,
        _runtime: &mut SurfaceRuntime,
        slot: usize,
    ) -> Result<(), UiBackendError> {
        let Some(requests) = self.shell.task_click_requests_for_slot(slot) else {
            return Ok(());
        };
        for request in requests {
            self.submit(request);
        }
        Ok(())
    }

    /// Signal-driven dynamic refresh (§6): reconcile snapshot domains flagged
    /// by `ControlSignal` delivery, then project only the surfaces each
    /// changed domain owns.
    pub(crate) fn refresh_dynamic(&mut self, runtime: &mut SurfaceRuntime) {
        use flamewm_shell::runtime::ProjectionKind;
        let changed = self.shell.reconcile_dirty();
        // F10: display geometry change may arrive without a panels dirty
        // flag, so check generation/revision/geometry drift even when no
        // changed domain fired. Sync happens BEFORE next open/projection.
        // The drift check borrows the snapshot; clone only on the resync path.
        let resynced = if self.panel_geometry_drifted(self.shell.snapshot()) {
            let snapshot = self.shell.snapshot().clone();
            self.resync_panel_geometry(runtime, &snapshot)
        } else {
            false
        };
        if !changed.any() && !resynced {
            return;
        }
        // Domain projectors retain their own semantic defaults. Reapply the
        // already-authoritative settings payload after them so a later task
        // or workspace update cannot erase the active presentation accent.
        if self.settings_cache.is_some() {
            self.settings_projection_pending = true;
        }
        let request_id = self
            .shell
            .snapshot()
            .system
            .network
            .pending_secret
            .as_ref()
            .map(|request| request.request_id);
        if request_id != self.network_secret_request_id {
            self.network_secret.clear();
            self.network_secret_request_id = request_id;
        }
        let snapshot = self.shell.snapshot().clone();
        let start_model = self.start_model.clone();
        let category = self.start_category;
        let query = self.start_query.clone();
        let session = snapshot.session.clone();
        self.ensure_loader();
        // Windows or panels or workspaces own the visible panel document.
        if panel_projection_needed(changed, resynced) {
            // C04: workspace/output/panel geometry changes invalidate the
            // helper anchor. F10: when the pre-projection resync above did
            // not fire but panels changed, resync now.
            if changed.panels && self.resync_panel_geometry(runtime, &snapshot) {
                if changed.workspaces || changed.panels {
                    self.close_quick_control_nonfatal();
                }
            } else {
                let loader = &mut self.icon_loader;
                let icon_cache = &mut self.task_icon_cache;
                let items = crate::settings_projection::taskbar_items(self.settings_cache.as_ref());
                let panel_outputs = self.surfaces.panel_outputs();
                let projected = projection::for_each_panel_record(
                    &snapshot,
                    panel_outputs.clone(),
                    |handle, view| {
                        runtime.with_document(handle, |document| {
                            async_projection::project_panel_for_changed(
                                document,
                                view.with_items(items),
                                loader.as_mut(),
                                icon_cache,
                                changed,
                            )
                        })
                    },
                );
                if let Err(error) = &projected {
                    eprintln!(
                        "flamewm-shell: panel projection failed (stage=tick-partial): {error:?}"
                    );
                }
                if projected.is_ok() {
                    flamewm_shell::runtime::projection_counter(ProjectionKind::Partial).increment();
                    for (handle, _) in &panel_outputs {
                        let _ = runtime.redraw(*handle);
                    }
                    self.publish_pager_cells(runtime);
                }
            }
        }
        // Media popup closes when its tray slot is hidden (player gone or idle).
        let media_gone = changed.system
            && media_popup_must_close(&snapshot.system, self.surfaces.is_media_open());
        if media_gone {
            if let Err(error) = self.close_transients(runtime) {
                eprintln!("flamewm-shell: media popup close failed (nonfatal): {error:?}");
            }
        }
        // System owns only the currently open in-process status popup.
        if changed.system {
            let open = self.open_popup_name();
            let projected = match open {
                Some("media") => runtime.with_document(self.surfaces.media, |document| {
                    projection::project_media(document, &snapshot)
                }),
                _ => Ok(()),
            };
            if projected.is_ok() && open.is_some() {
                flamewm_shell::runtime::projection_counter(ProjectionKind::Popup).increment();
                if let Some("media") = open {
                    let _ = runtime.redraw(self.surfaces.media);
                }
            }
        }
        // Applications own the unified Start document, only when the app
        // set changed and Start is open; hidden Start stays untouched.
        if (changed.applications || self.start_icons_stale) && self.start_open {
            let loader = self.icon_loader.as_mut();
            let projected = runtime.with_document(self.surfaces.start, |document| {
                async_projection::project_start_unified(
                    document,
                    &start_model,
                    category,
                    &query,
                    &session,
                    loader,
                )
            });
            if projected.is_ok() {
                flamewm_shell::runtime::projection_counter(ProjectionKind::Start).increment();
                let _ = runtime.redraw(self.surfaces.start);
            }
        }
        if changed.windows || changed.workspaces || changed.panels || resynced {
            self.sync_icon_geometry(runtime, &snapshot);
        }
    }

    /// Publishes `_NET_WM_ICON_GEOMETRY` on each task slot's client window
    /// from the slot's retained root-space rect. Called only after a layout
    /// or task change; the tracker writes and clears just the deltas.
    pub(crate) fn sync_icon_geometry(
        &mut self,
        runtime: &mut SurfaceRuntime,
        snapshot: &ShellSnapshot,
    ) {
        let slots: Vec<Option<u64>> = flamewm_shell_core::task_visual_states_on_output(
            &snapshot.panels,
            &snapshot.windows,
            &snapshot.applications,
            snapshot.panels.panels.first().map(|panel| &panel.output),
        )
        .iter()
        .map(|state| state.window.map(|window| window.id))
        .collect();
        let panels: Vec<_> = self
            .surfaces
            .panel_outputs()
            .into_iter()
            .map(|(handle, _)| handle)
            .collect();
        let desired = icon_geometry::desired_slot_geometry(&slots, |index| {
            let node = format!("task-slot-{}", index + 1);
            panels
                .iter()
                .find_map(|&panel| runtime.node_device_rect_by_id(panel, &node))
                .map(|rect| (rect.x, rect.y, rect.width, rect.height))
        });
        let mut sink = RuntimeIconGeometrySink { runtime };
        self.icon_geometry.sync(&desired, &mut sink);
    }

    /// F10: panel/display geometry resync. Close popup/quick helper,
    /// sync the placement context BEFORE the next open/projection,
    /// move_resize the panel native surface when geometry changed,
    /// then reproject the panel. Returns true when a resync happened.
    /// Pure drift check against a borrowed snapshot; no clone, no mutation.
    fn panel_geometry_drifted(&self, snapshot: &ShellSnapshot) -> bool {
        let (display_generation, panels_revision) = self.surfaces.placement_generation();
        snapshot.displays.generation != display_generation
            || snapshot.panels.revision != panels_revision
            || snapshot
                .panels
                .panels
                .first()
                .is_some_and(|panel| panel.geometry != self.surfaces.panel_geometry())
            || self.surfaces.output_scale_drifted(snapshot)
    }

    pub(crate) fn resync_panel_geometry(
        &mut self,
        runtime: &mut SurfaceRuntime,
        snapshot: &ShellSnapshot,
    ) -> bool {
        if !self.panel_geometry_drifted(snapshot) {
            return false;
        }
        // Close popup/quick helper first so no open surface keeps a
        // stale anchor while the context swaps underneath it.
        let _ = self.surfaces.close_transients(runtime);
        self.close_quick_control_nonfatal();
        self.end_start_session();
        self.context_menu = None;
        // Sync BEFORE next open/projection; refusal keeps old context.
        match self.surfaces.sync_panel_context(runtime, snapshot) {
            Ok(true) => self.apply_taskbar_frame(runtime),
            Ok(false) => {
                eprintln!("flamewm-shell: panel context sync refused (missing output/panel)");
                return false;
            }
            Err(error) => {
                eprintln!("flamewm-shell: panel surface sync failed: {error}");
                return false;
            }
        }
        if let Err(error) = self.surfaces.sync_panel_ui_scale(runtime) {
            eprintln!("flamewm-shell: panel ui scale resync failed (nonfatal): {error}");
        }
        if let Err(error) = self.surfaces.move_panel_to_synced(runtime) {
            eprintln!("flamewm-shell: panel move_resize failed (nonfatal): {error}");
        }
        self.apply_taskbar_backdrop(runtime);
        let loader = &mut self.icon_loader;
        let icon_cache = &mut self.task_icon_cache;
        let changed = flamewm_shell::runtime::ChangedDomains {
            panels: true,
            ..Default::default()
        };
        let items = crate::settings_projection::taskbar_items(self.settings_cache.as_ref());
        let panel_outputs = self.surfaces.panel_outputs();
        let projected =
            projection::for_each_panel_record(snapshot, panel_outputs.clone(), |handle, view| {
                runtime.with_document(handle, |document| {
                    async_projection::project_panel_for_changed(
                        document,
                        view.with_items(items),
                        loader.as_mut(),
                        icon_cache,
                        changed,
                    )
                })
            });
        if let Err(error) = &projected {
            eprintln!("flamewm-shell: panel projection failed (stage=resync-partial): {error:?}");
        }
        if projected.is_ok() {
            flamewm_shell::runtime::projection_counter(
                flamewm_shell::runtime::ProjectionKind::Partial,
            )
            .increment();
            for (handle, _) in &panel_outputs {
                let _ = runtime.redraw(*handle);
            }
            self.publish_pager_cells(runtime);
        }
        flamewm_debug::emit(
            flamewm_debug::DebugEventId("shell.panel.resync"),
            Duration::from_millis(500),
            || {
                let (generation, revision) = self.surfaces.placement_generation();
                format!(
                    "F10 resync generation={generation} revision={revision} edge={:?}",
                    self.surfaces.panel_edge()
                )
            },
        );
        true
    }

    /// F10: edge/geometry gate shared by every popup open path.
    pub(crate) fn refuse_popup_on_inconsistent_geometry(&self) -> Option<String> {
        match self.surfaces.check_edge_consistent() {
            Ok(()) => None,
            Err(diagnostic) => {
                debug_assert!(false, "{diagnostic}");
                eprintln!("flamewm-shell: popup open refused: {diagnostic}");
                Some(diagnostic)
            }
        }
    }

    /// F10: read the current context generation on every Start/Quick
    /// open so the open provably uses the post-sync context.
    pub(crate) fn note_popup_generation(&self, what: &'static str) {
        let (generation, revision) = self.surfaces.placement_generation();
        flamewm_debug::emit(
            flamewm_debug::DebugEventId("shell.popup.open.generation"),
            Duration::from_millis(500),
            || format!("F10 {what} generation={generation} revision={revision}"),
        );
    }

    /// Name of the currently open in-process status popup, if any.
    /// Helper-owned popups are never in-process, so only media qualifies.
    pub(crate) fn open_popup_name(&self) -> Option<&'static str> {
        if self.surfaces.is_media_open() {
            return Some("media");
        }
        None
    }

    pub(crate) fn tick(
        &mut self,
        runtime: &mut SurfaceRuntime,
        clock_due: &std::sync::atomic::AtomicBool,
    ) -> Result<(), UiBackendError> {
        self.drain_raw_press_observations(runtime);
        if self.quick_kind_open.is_none() {
            runtime.disarm_raw_press_observer();
        }
        // Event-driven retry: a blank taskbar snapshot is recaptured when root children change.
        if runtime.take_root_changed() && self.surfaces.panel_capture_pending() {
            self.apply_taskbar_backdrop(runtime);
        }
        // Startup panel projection runs with no loader (main.rs
        // project_panel_critical), so task-slot icons were recorded as empty
        // placeholders with no request. Re-project task slots once with the
        // loader now that it exists so the requests are actually submitted.
        // Stays armed while the snapshot has no task states (the first tick
        // often precedes any managed window), and disarms only after a run
        // with task states and a loader projected successfully.
        if self.task_icon_prime_armed {
            self.ensure_loader();
            let snapshot = self.shell.snapshot();
            let task_state_count = flamewm_shell_core::task_visual_states_on_output(
                &snapshot.panels,
                &snapshot.windows,
                &snapshot.applications,
                snapshot.panels.panels.first().map(|panel| &panel.output),
            )
            .len();
            let loader_present = self.icon_loader.is_some();
            if async_projection::task_icon_prime_should_run(
                self.task_icon_prime_armed,
                task_state_count,
                loader_present,
            ) {
                let mut loader = self.icon_loader.as_mut();
                let icon_cache = &mut self.task_icon_cache;
                let changed = flamewm_shell::runtime::ChangedDomains {
                    windows: true,
                    ..Default::default()
                };
                let items = crate::settings_projection::taskbar_items(self.settings_cache.as_ref());
                let panel_outputs = self.surfaces.panel_outputs();
                let mut projection_ok = true;
                for (handle, _) in panel_outputs {
                    let result = runtime.with_document(handle, |document| {
                        async_projection::project_panel_for_changed(
                            document,
                            PanelView::first(snapshot).with_items(items),
                            loader.as_deref_mut(),
                            icon_cache,
                            changed,
                        )
                    });
                    if let Err(error) = result {
                        eprintln!(
                            "flamewm-shell: task icon prime projection failed (stage=task-slots, states={task_state_count}): {error:?}"
                        );
                        projection_ok = false;
                    }
                }
                eprintln!(
                    "flamewm-shell: task icon prime ran (stage=task-slots, states={task_state_count}, ok={projection_ok})"
                );
                self.task_icon_prime_armed = async_projection::task_icon_prime_still_armed(
                    self.task_icon_prime_armed,
                    task_state_count,
                    loader_present,
                    projection_ok,
                );
            }
        }
        self.drain_icons(runtime);
        // J07: nonblocking helper poll; restart is bounded, failures are
        // nonfatal diagnostics, the shell loop always survives.
        flamewm_shell::quick_controls::host::poll_supervisor_nonfatal(&mut self.quick_control);
        self.tick_auto_location();
        let settled = self.zone_settle_at.is_some_and(|at| std::time::Instant::now() >= at);
        if settled {
            self.zone_settle_at = None;
        }
        let clock_fired = clock_due.swap(false, Ordering::Acquire) | std::mem::take(&mut self.time_zone_changed) | settled;
        if clock_fired {
            let now = Local::now();
            let clock = self
                .settings_cache
                .as_ref()
                .map(crate::settings_projection::clock_display)
                .unwrap_or_default();
            for (handle, _) in self.surfaces.panel_outputs() {
                runtime.with_document(handle, |document| {
                    projection::project_clock_at(document, now, clock)
                })?;
                runtime.redraw(handle)?;
            }

            let date = (now.year(), now.month() as u8, now.day() as u8);
            if self.date_tracker.date_changed(date) {
                self.local_date = date;
            }
        }
        // Steady state never polls: reconcile signal-flagged domains at most
        // once per tick, after the clock projection above.
        // Start launch results arrive via the mutation result drain; apply
        // them here so a failed launch shows its error on this turn.
        self.apply_start_launch_outcomes(runtime)?;
        self.refresh_dynamic(runtime);
        self.project_settings(runtime)?;
        self.tick_weather(runtime);
        // Signal-queued controls (StartMenuToggleRequested -> ToggleStart) arrive
        // on the control-signal fd, not an X event, so no event route drains
        // them. Drain through the same apply_controls the click path uses, then
        // flush damage: the reactor only redraws dirty surfaces when X events
        // were pumped on this turn.
        if self.shell.has_pending_controls() {
            self.apply_controls(runtime)?;
            runtime.redraw_dirty()?;
        }
        self.tick_osd(runtime);
        self.publish_wake(std::time::Instant::now());
        Ok(())
    }

    /// Wires the reactor wake slot. OSD, weather and auto-location share it, so the tick publishes
    /// the earliest of their deadlines in one place (a per-owner set would clobber the other).
    pub(crate) fn attach_wake(&mut self, wake: flamewm_reactor::WakeDeadline) {
        self.wake = Some(wake);
    }

    fn publish_wake(&self, now: std::time::Instant) {
        let Some(wake) = &self.wake else { return };
        wake.set(earliest_wake([
            self.osd.deadline(),
            self.weather.poll_deadline(now),
            self.auto_lookup.poll_deadline(now),
            self.zone_settle_at,
        ]));
    }

    /// Volume OSD: shows the default endpoint's snapshot level when it changes
    /// and hides at the single deadline. Nonfatal: a failure never stops the loop.
    fn tick_osd(&mut self, runtime: &mut SurfaceRuntime) {
        let snapshot = self.shell.snapshot();
        let level =
            flamewm_shell::taskbar::status::audio::default_endpoint_row(&snapshot.system.audio)
                .map(|row| flamewm_shell::osd::Level::new(row.volume_percent, row.muted));
        // Same output source as the panel scale: the shown panel's placement output.
        let panel = self.surfaces.placement().output.clone();
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
        let (output, scale) = match flamewm_shell::osd::target_output(Some(panel), &outputs) {
            Some(target) => (target.geometry, f32::from(target.scale_percent) / 100.0),
            None => (self.surfaces.work_area(), 1.0),
        };
        let now = Instant::now();
        let settings = self.settings_cache.as_ref();
        if let Err(error) = self.osd.update(runtime, level, output, scale, settings, now) {
            eprintln!("flamewm-shell: volume OSD failed (nonfatal): {error:?}");
        }
    }

    /// Applies the Start slide for the next show/hide. Derived from the cached
    /// settings and the current panel edge, so it runs before each open and
    /// after each settings projection. A rejected slide is nonfatal and
    /// falls back to the immediate show path.
    fn apply_start_slide(&mut self, runtime: &mut SurfaceRuntime) {
        // Magic Lamp folds into the panel's Start button; without its layout the show stays immediate.
        let start_button = runtime
            .node_device_rect_by_id(
                self.surfaces.panel(),
                flamewm_shell::popup_role::PopupRole::Start.source_node_id(),
            )
            .filter(|rect| rect.width > 0.0 && rect.height > 0.0)
            .map(|rect| {
                (
                    rect.x as i32,
                    rect.y as i32,
                    rect.width.max(1.0) as u32,
                    rect.height.max(1.0) as u32,
                )
            });
        let slide = self.settings_cache.as_ref().and_then(|snapshot| {
            crate::settings_projection::start_slide(
                snapshot,
                self.surfaces.panel_edge(),
                start_button,
            )
        });
        self.surfaces.set_start_motion(slide.is_some());
        if let Err(error) = runtime.set_slide(self.surfaces.start, slide) {
            eprintln!("flamewm-shell: start slide setup failed (nonfatal): {error:?}");
            let _ = runtime.set_slide(self.surfaces.start, None);
        }
        // Same Start preference and speed for task menu, media, and status popups. Magic Lamp
        // has no popup target, so the popup slide never takes the start_button path.
        let popup_slide = self.settings_cache.as_ref().and_then(|snapshot| {
            crate::settings_projection::start_slide(snapshot, self.surfaces.panel_edge(), None)
        });
        self.surfaces.set_popup_slide(popup_slide);
    }

    /// Installs the taskbar pseudo-transparency backdrop from the current opacity and blur.
    fn apply_taskbar_backdrop(&mut self, runtime: &mut SurfaceRuntime) {
        let Some(settings) = self.settings_cache.as_ref() else {
            return;
        };
        let (opacity, blur) = crate::settings_projection::taskbar_backdrop_percents(settings);
        let text = |key: &str| {
            settings
                .values
                .get(key)
                .map(|value| format!("{value:?}"))
                .unwrap_or_default()
        };
        let wallpaper_path = text_setting(settings, "wallpaper");
        let fit_name = text_setting(settings, "wallpaperFit");
        let key = match runtime.surface_device_rect(self.surfaces.panel()) {
            Ok(rect) => flamewm_shell::surface_backdrop::CaptureKey {
                wallpaper: text("wallpaper"),
                fit: text("wallpaperFit"),
                rect: (
                    rect.x as i32,
                    rect.y as i32,
                    rect.width as u32,
                    rect.height as u32,
                ),
            },
            Err(error) => {
                eprintln!("flamewm-shell: taskbar backdrop rect unavailable (nonfatal): {error:?}");
                return;
            }
        };
        let rect = key.rect;
        let wallpaper = || {
            let (width, height) = flamewm_ui_x11::backdrop::root_geometry().ok()?;
            flamewm_shell::surface_backdrop::wallpaper_source(
                &wallpaper_path,
                flamewm_image_core::wallpaper::parse(&fit_name),
                (width, height),
                rect,
                &crate::packaged_root(),
            )
        };
        if let Err(error) = self.surfaces.refresh_panel_backdrop(runtime, key, wallpaper) {
            eprintln!("flamewm-shell: taskbar backdrop capture failed (nonfatal): {error}");
        }
        if let Err(error) = self.surfaces.apply_panel_backdrop(runtime, opacity, blur) {
            eprintln!("flamewm-shell: taskbar backdrop failed (nonfatal): {error}");
        }
    }

    /// Root-sized wallpaper canvas the popup backdrop crops from, so a popup over the taskbar blurs the
    /// wallpaper like the taskbar does, not the dark bar. `None` falls back to a root capture.
    fn popup_wallpaper_canvas(&self, settings: &flamewm_api::settings::SettingsSnapshot) -> Option<(u32, u32, Vec<u8>)> {
        let text = |key: &str| text_setting(settings, key);
        let (width, height) = flamewm_ui_x11::backdrop::root_geometry().ok()?;
        let canvas = flamewm_shell::surface_backdrop::wallpaper_source(
            &text("wallpaper"),
            flamewm_image_core::wallpaper::parse(&text("wallpaperFit")),
            (width, height),
            (0, 0, width, height),
            &crate::packaged_root(),
        )?;
        Some((canvas.width, canvas.height, canvas.pixels))
    }

    /// Installs the Start popup backdrop percents; the snapshot itself is captured on each first show.
    fn apply_start_backdrop(&self, runtime: &mut SurfaceRuntime) {
        let Some(settings) = self.settings_cache.as_ref() else {
            return;
        };
        let percents = crate::settings_projection::start_backdrop_percents(settings);
        if let Err(error) = runtime.set_popup_backdrop(self.surfaces.start, Some(percents)) {
            eprintln!("flamewm-shell: start backdrop failed (nonfatal): {error:?}");
        }
    }

    /// Publishes the taskbar dock frame (strut edge and corner radius); a no-op while unchanged.
    fn apply_taskbar_frame(&self, runtime: &mut SurfaceRuntime) {
        let Some(settings) = self.settings_cache.as_ref() else {
            return;
        };
        let frame = taskbar_frame(settings);
        if let Err(error) = self.surfaces.apply_dock_frame(runtime, frame) {
            eprintln!("flamewm-shell: {error}");
        }
    }

    /// Weather tick: no timers and no fetch while disabled. Projects the tray and pushes
    /// the feed only when the scheduler changed or a settings snapshot arrived.
    fn tick_weather(&mut self, runtime: &mut SurfaceRuntime) {
        let changed = self.weather.tick(
            std::time::Instant::now(),
            flamewm_shell::weather_host::spawn_weather_fetch,
        );
        if !(changed || self.weather_feed_pending) {
            return;
        }
        let feed = self.weather.feed();
        let units = feed.settings.units;
        let shown = crate::settings_projection::taskbar_items(self.settings_cache.as_ref()).weather;
        let projected = runtime.with_document(self.surfaces.panel(), |document| {
            projection::project_weather_tray(document, &feed.state, units, shown)
        });
        // Pending clears only after a successful tray write; failures stay loud and retry.
        if let Err(error) = &projected {
            eprintln!("flamewm-shell: weather tray projection failed: {error:?}");
        }
        let tray_ok = projected.is_ok();
        let _ = runtime.redraw(self.surfaces.panel());
        self.push_weather_feed();
        if !tray_ok {
            self.weather_feed_pending = true;
        }
    }

    /// Nonblocking feed push. A full pipe re-arms the pending flag for the next tick;
    /// no running popup host is not an error (its first OPEN carries the feed).
    fn push_weather_feed(&mut self) {
        match self.quick_control.push_weather(self.weather.feed()) {
            Ok(()) => self.weather_feed_pending = false,
            Err(flamewm_shell::quick_controls::SupervisorError::WouldBlock) => {
                self.weather_feed_pending = true;
            }
            Err(error) => eprintln!("flamewm-shell: weather feed not sent (nonfatal): {error}"),
        }
    }

    fn project_settings(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), UiBackendError> {
        if !self.settings_projection_pending {
            return Ok(());
        }
        let Some(snapshot) = self.settings_cache.as_ref() else {
            self.settings_projection_pending = false;
            return Ok(());
        };
        let snapshot = snapshot.clone();
        runtime.with_document(self.surfaces.panel(), |document| {
            crate::settings_projection::project(
                document,
                &snapshot,
                crate::settings_projection::SurfaceKind::Panel,
            )
        })?;
        for (surface, kind) in [
            (
                self.surfaces.start,
                crate::settings_projection::SurfaceKind::Start,
            ),
            (
                self.surfaces.task_menu,
                crate::settings_projection::SurfaceKind::TaskMenu,
            ),
            (
                self.surfaces.media,
                crate::settings_projection::SurfaceKind::Media,
            ),
        ] {
            runtime.with_document(surface, |document| {
                crate::settings_projection::project(document, &snapshot, kind)
            })?;
        }
        // The focused task underline takes its colour from the settings accent; re-project the task slots
        // now so a settings change (or a first settings arrival after the last task update) cannot leave it stale.
        let shell_snapshot = self.shell.snapshot();
        // Taskbar item settings (tray, clock, pager visibility) follow the same reprojection.
        let task_changed = flamewm_shell::runtime::ChangedDomains {
            windows: true,
            workspaces: true,
            system: true,
            ..Default::default()
        };
        let loader = &mut self.icon_loader;
        let items = crate::settings_projection::taskbar_items(self.settings_cache.as_ref());
        let icon_cache = &mut self.task_icon_cache;
        let _ = projection::for_each_panel_record(
            &shell_snapshot,
            self.surfaces.panel_outputs(),
            |handle, view| {
                runtime.with_document(handle, |document| {
                    async_projection::project_panel_for_changed(
                        document,
                        view.with_items(items),
                        loader.as_mut(),
                        icon_cache,
                        task_changed,
                    )
                })
            },
        );
        self.apply_start_backdrop(runtime);
        self.apply_start_slide(runtime);
        self.apply_taskbar_backdrop(runtime);
        let _ = runtime.redraw(self.surfaces.panel());
        if self.surfaces.is_start_group_open() {
            let _ = runtime.redraw(self.surfaces.start);
        }
        if self.context_menu.is_some() {
            let _ = runtime.redraw(self.surfaces.task_menu);
        }
        if self.surfaces.is_media_open() {
            let _ = runtime.redraw(self.surfaces.media);
        }
        self.settings_projection_pending = false;
        self.apply_taskbar_frame(runtime);
        Ok(())
    }
}

/// Whether a quick-control popup's taskbar item is still shown under `items`.
fn quick_kind_shown(
    kind: flamewm_shell::quick_controls::QuickControlKind,
    items: flamewm_shell::projection::TaskbarItems,
) -> bool {
    use flamewm_shell::quick_controls::QuickControlKind;
    match kind {
        QuickControlKind::Audio => items.volume,
        QuickControlKind::Network => items.network,
        QuickControlKind::Battery => items.battery,
        QuickControlKind::Weather => items.weather,
        QuickControlKind::Calendar => items.clock,
    }
}

/// Text value of one setting, or empty when it is absent or not text.
fn text_setting(settings: &SettingsSnapshot, key: &str) -> String {
    match settings.values.get(key) {
        Some(SettingValue::Text(value)) => value.clone(),
        _ => String::new(),
    }
}

/// Flat `taskbarStyle` / `taskbarFloating*` keys as the platform reads them (defaults match
/// `ProductSettings::default()`).
fn taskbar_frame(settings: &SettingsSnapshot) -> TaskbarFrame {
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

fn end_start_session_fields(start_open: &mut bool, start_query: &mut String) {
    *start_open = false;
    start_query.clear();
}

fn clear_transient_state_fields(
    start_open: &mut bool,
    start_query: &mut String,
    context_menu: &mut Option<flamewm_shell::ContextMenuState>,
) {
    end_start_session_fields(start_open, start_query);
    *context_menu = None;
}

/// Opening a quick popup releases the shell context-menu slot. Returns true
/// when a context menu was open and its surface must be closed.
fn take_context_menu_for_popup(context_menu: &mut Option<flamewm_shell::ContextMenuState>) -> bool {
    context_menu.take().is_some()
}

/// `IconGeometrySink` over the `SurfaceRuntime` facade. Write failures are
/// nonfatal diagnostics; a vanished client window is not an error.
struct RuntimeIconGeometrySink<'a> {
    runtime: &'a mut SurfaceRuntime,
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
fn pager_cell_window_rect(node: (f32, f32, f32, f32), origin: (f32, f32)) -> [u64; 4] {
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
fn media_system_request(
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
fn panel_projection_needed(
    changed: flamewm_shell::runtime::ChangedDomains,
    resynced: bool,
) -> bool {
    changed.windows || changed.workspaces || changed.system || (changed.panels && !resynced)
}

/// Whether an open media popup must close because media is no longer
/// shown in the tray (`tray-media` hidden). Uses the same visibility owner
/// as the panel slot (`status::media::project`).
fn media_popup_must_close(system: &flamewm_api::system::SystemSnapshot, media_open: bool) -> bool {
    media_open && !flamewm_shell::taskbar::status::media::project(system).visible
}

/// Chrono's local-zone cache refresh window (chrono 0.4.45 `local/unix.rs`).
const ZONE_SETTLE: std::time::Duration = std::time::Duration::from_millis(1100);

/// The single shared reactor wake slot takes the earliest pending owner deadline, or none.
#[must_use]
fn earliest_wake(
    deadlines: [Option<std::time::Instant>; 4],
) -> Option<std::time::Instant> {
    deadlines.into_iter().flatten().min()
}

#[cfg(test)]
mod tests {
    #[test]
    fn shared_wake_slot_takes_the_earliest_owner_deadline() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let soon = t0 + Duration::from_millis(100);
        assert_eq!(super::earliest_wake([None, None, None, None]), None, "idle loop");
        assert_eq!(super::earliest_wake([Some(t0), None, Some(soon), None]), Some(t0));
        assert_eq!(super::earliest_wake([None, Some(soon), None, None]), Some(soon));
        assert_eq!(super::earliest_wake([None, None, None, Some(soon)]), Some(soon));
    }

    fn production_source() -> &'static str {
        include_str!("app.rs")
            .split_once("\n#[cfg(test)]\nmod tests")
            .map(|(source, _)| source)
            .expect("app tests must have a production section")
    }

    #[test]
    fn open_media_popup_closes_when_media_unavailable_or_idle() {
        use flamewm_api::system::{PlaybackState, ServiceAvailability, SystemSnapshot};
        let mut system = SystemSnapshot::default();
        system.media.availability = ServiceAvailability::Unavailable;
        system.media.playback = PlaybackState::Playing;
        assert!(
            super::media_popup_must_close(&system, true),
            "player gone must close the open media popup"
        );
        system.media.availability = ServiceAvailability::Available;
        system.media.playback = PlaybackState::Unavailable;
        assert!(
            super::media_popup_must_close(&system, true),
            "no playing/paused media must close the open media popup"
        );
        system.media.playback = PlaybackState::Playing;
        assert!(
            !super::media_popup_must_close(&system, true),
            "playing media keeps the popup open"
        );
        assert!(
            !super::media_popup_must_close(&system, false),
            "closed popup is never closed again"
        );
    }

    #[test]
    fn panel_projection_needed_true_for_system_only_change() {
        let changed = flamewm_shell::runtime::ChangedDomains {
            system: true,
            ..Default::default()
        };
        assert!(super::panel_projection_needed(changed, false));
        assert!(super::panel_projection_needed(changed, true));
    }

    #[test]
    fn panel_projection_needed_panels_only_when_not_resynced() {
        let changed = flamewm_shell::runtime::ChangedDomains {
            panels: true,
            ..Default::default()
        };
        assert!(super::panel_projection_needed(changed, false));
        assert!(!super::panel_projection_needed(changed, true));
    }

    #[test]
    fn panel_projection_needed_false_without_projection_domains() {
        let changed = flamewm_shell::runtime::ChangedDomains {
            applications: true,
            ..Default::default()
        };
        assert!(!super::panel_projection_needed(changed, false));
    }

    #[test]
    fn start_slide_pass_also_arms_popup_slide_without_start_button_target() {
        let source = production_source();
        let body = source
            .split_once("fn apply_start_slide(")
            .and_then(|(_, rest)| rest.split_once("\n    }\n"))
            .map(|(body, _)| body)
            .expect("apply_start_slide must exist");
        assert!(
            body.contains("self.surfaces.set_popup_slide(popup_slide);"),
            "apply_start_slide must arm the popup slide on every Start-slide refresh"
        );
        assert!(
            body.contains("crate::settings_projection::start_slide(snapshot, self.surfaces.panel_edge(), None)"),
            "popup slide must use the shared start_slide owner with no Magic Lamp target"
        );
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
    fn pager_cell_rect_follows_panel_resize_scale() {
        // Panel origin at (0, 1000) on a 1080 px output: 44 px bar at 100%,
        // 55 px bar at 125%. Cell 1 sits at window y=6 in both cases.
        let origin = (0.0, 1000.0);
        assert_eq!(
            super::pager_cell_window_rect((1166.0, 1006.0, 22.0, 16.0), origin),
            [1166, 6, 22, 16]
        );
        // 125% layout: cell y and height scale by 1.25, rounded.
        assert_eq!(
            super::pager_cell_window_rect((1457.5, 1007.5, 27.5, 20.0), origin),
            [1458, 8, 28, 20]
        );
        // Negative offset clamps to zero.
        assert_eq!(
            super::pager_cell_window_rect((5.0, 990.0, 10.0, 10.0), origin),
            [5, 0, 10, 10]
        );
    }

    #[test]
    fn escape_close_clears_start_and_transient_state() {
        let mut start_open = true;
        let mut start_query = "fire".to_owned();
        let mut context_menu = Some(flamewm_shell::ContextMenuState {
            kind: flamewm_shell::ContextMenuKind::Workspace { index: 0 },
            revision: 1,
            anchor: None,
        });

        super::clear_transient_state_fields(&mut start_open, &mut start_query, &mut context_menu);

        assert!(!start_open);
        assert!(start_query.is_empty(), "close clears the search query");
        assert!(context_menu.is_none());
    }

    #[test]
    fn quick_popup_open_takes_and_closes_open_context_menu_only() {
        let mut context_menu = Some(flamewm_shell::ContextMenuState {
            kind: flamewm_shell::ContextMenuKind::Workspace { index: 0 },
            revision: 1,
            anchor: None,
        });
        assert!(super::take_context_menu_for_popup(&mut context_menu));
        assert!(context_menu.is_none(), "popup open releases the menu slot");
        assert!(
            !super::take_context_menu_for_popup(&mut context_menu),
            "no surface close is issued when no context menu is open"
        );
    }

    #[test]
    fn red_t06_settings_toggle_has_no_follow_up_get_settings_fetch() {
        // RED(T06): the current toggle submits ApplySettings, then performs a
        // blocking UI-thread GetSettings roundtrip instead of using its result.
        let source = production_source();
        assert!(
            !source.contains("self.control.call(&ControlRequest::GetSettings"),
            "T06 RED: settings mutation must not be followed by UI GetSettings"
        );
        assert!(
            !source.contains("refresh_settings_revision"),
            "T06 RED: remove the blocking settings refresh seam"
        );
    }

    #[test]
    fn red_t08_shell_loop_has_no_steady_state_control_client_field() {
        // RED(T08): the current ShellLoop retains a blocking ControlClient
        // solely for steady-state reconciliation.
        let source = production_source();
        let shell_loop = source
            .split_once("pub(crate) struct ShellLoop {")
            .map(|(_, rest)| rest)
            .and_then(|rest| rest.split_once("\n}\n\nimpl ShellLoop"))
            .map(|(body, _)| body)
            .expect("ShellLoop definition must remain discoverable");
        assert!(
            !shell_loop.contains("ControlClient"),
            "T08 RED: ShellLoop steady state must not own a blocking ControlClient"
        );
    }
}

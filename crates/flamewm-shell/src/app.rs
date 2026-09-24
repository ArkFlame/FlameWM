use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use chrono::{Datelike, Local};
use flamewm_api::settings::{SettingValue, SettingsSnapshot, SettingsTransaction};
use flamewm_control_core::ControlRequest;
use flamewm_shell::async_projection::{self, StartViewNote};
use flamewm_shell::icon_loader::{DependentSurface, IconLoader, IconTarget};
use flamewm_shell::start::StartCategory;
use flamewm_shell::{projection, start, ShellControl, ShellRuntime, ShellSnapshot, ShellSurfaces};
use flamewm_shell_core::clock::ClockDateTracker;
use flamewm_shell_core::popup::PopupRefusal;
use flamewm_shell_core::status::{AudioSettings, AudioTab, NetworkQuery};
use flamewm_ui_x11::{SurfaceRuntime, UiBackendError};

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
pub(crate) use shell_stage;

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
    pub(crate) start_open: bool,
    pub(crate) context_menu: Option<flamewm_shell::ContextMenuState>,
    pub(crate) network_secret: String,
    pub(crate) network_secret_request_id: Option<u64>,
    pub(crate) local_date: (i32, u8, u8),
    pub(crate) date_tracker: ClockDateTracker,
    pub(crate) audio_tab: AudioTab,
    pub(crate) audio_settings: AudioSettings,
    pub(crate) network_query: NetworkQuery,
    pub(crate) settings_revision: u64,
    pub(crate) settings_cache: Option<SettingsSnapshot>,
    pub(crate) settings_projection_pending: bool,
    pub(crate) icon_loader: Option<IconLoader>,
    pub(crate) loader_root: std::path::PathBuf,
    pub(crate) start_view: StartViewNote,
    pub(crate) quick_control: flamewm_shell::quick_controls::QuickControlSupervisor,
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
            start_open: false,
            context_menu: None,
            network_secret: String::new(),
            network_secret_request_id: None,
            local_date: projection::local_date(),
            date_tracker: ClockDateTracker::new(projection::local_date()),
            audio_tab: AudioTab::default(),
            audio_settings: AudioSettings::default(),
            network_query: NetworkQuery::default(),
            settings_revision: 0,
            settings_cache: None,
            settings_projection_pending: false,
            icon_loader: None,
            loader_root,
            start_view: StartViewNote::default(),
            quick_control,
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
        if self
            .settings_cache
            .as_ref()
            .is_some_and(|current| snapshot.revision <= current.revision)
        {
            return;
        }
        self.settings_revision = snapshot.revision;
        if let Some(SettingValue::Boolean(raise)) = snapshot.values.get(AudioSettings::SETTINGS_KEY)
        {
            self.audio_settings.raise_maximum = *raise;
        }
        self.settings_cache = Some(snapshot);
        self.settings_projection_pending = true;
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
        self.surfaces
            .close_transients(runtime)
            .map_err(UiBackendError::Renderer)?;
        self.clear_transient_state();
        Ok(())
    }

    fn clear_transient_state(&mut self) {
        clear_transient_state_fields(&mut self.start_open, &mut self.context_menu);
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
                        self.project_start(runtime)?;
                        let _measure = shell_stage!("shell.start.open.measure");
                        let _place = shell_stage!("shell.start.open.place");
                        let _prepare = shell_stage!("shell.start.open.prepare");
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
                            Ok(()) => {
                                self.start_open = !was_open;
                            }
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
                    // J07: media stays in-process; audio/network/clock
                    // forward to the supervisor, then return immediately.
                    if matches!(name.as_str(), "volume" | "audio" | "network" | "clock") {
                        self.open_quick_control(runtime, &name);
                        self.start_open = false;
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
                    self.start_open = false;
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
                        runtime.with_document(self.surfaces.task_menu, |document| {
                            projection::project_context_menu(
                                document,
                                self.shell.snapshot(),
                                Some(&state),
                            )
                        })?;
                    }
                    self.context_menu = Some(state.clone());
                    let snapshot = self.shell.snapshot().clone();
                    let _measure = shell_stage!("shell.context.open.measure");
                    let _place = shell_stage!("shell.context.open.place");
                    let _prepare = shell_stage!("shell.context.open.prepare");
                    let opened = self
                        .surfaces
                        .open_context_menu(runtime, &state, &snapshot)
                        .map_err(UiBackendError::Renderer);
                    let _present = shell_stage!("shell.context.open.present");
                    shell_slow_note("context", started);
                    opened?;
                }
                ShellControl::CloseContextMenu => {
                    let _close = shell_stage!("shell.context.open.close");
                    self.surfaces
                        .close_transients(runtime)
                        .map_err(UiBackendError::Renderer)?;
                    self.context_menu = None;
                }
                ShellControl::SessionAction(action) => {
                    self.submit(ControlRequest::SessionAction(action));
                    self.surfaces
                        .close_transients(runtime)
                        .map_err(UiBackendError::Renderer)?;
                    self.start_open = false;
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
        let snapshot = self.shell.snapshot().clone();
        match name {
            "media" => runtime.with_document(self.surfaces.media, |document| {
                projection::project_media(document, &snapshot)
            })?,
            _ => {}
        }
        Ok(())
    }

    /// C04: send quick-control CLOSE; close signals are owned in main
    /// (workspace change, menus, media open, outside release, geometry
    /// invalidation). Best-effort and nonfatal: the loop always survives.
    pub(crate) fn close_quick_control_nonfatal(&mut self) {
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
    pub(crate) fn open_quick_control(&mut self, runtime: &SurfaceRuntime, name: &str) {
        use flamewm_shell::quick_controls::QuickControlKind;
        self.note_popup_generation("quick");
        if self.refuse_popup_on_inconsistent_geometry().is_some() {
            return;
        }
        let kind = match name {
            "volume" | "audio" => QuickControlKind::Audio,
            "network" => QuickControlKind::Network,
            "clock" => QuickControlKind::Calendar,
            _ => return,
        };
        let source_id = match kind {
            QuickControlKind::Audio => "tray-volume",
            QuickControlKind::Network => "tray-network",
            QuickControlKind::Calendar => "clock-button",
        };
        // Retained anchor preferred; snapshot geometry is the sole
        // pre-map fallback (same contract as the in-process path).
        let anchor =
            self.surfaces
                .quick_anchor(runtime, &self.pending_anchor_fallback(), source_id);
        let work_area = self.surfaces.work_area();
        let panel_edge = self.surfaces.panel_edge();
        flamewm_shell::quick_controls::host::open_via_supervisor_nonfatal(
            &mut self.quick_control,
            kind,
            anchor,
            work_area,
            panel_edge,
        );
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
        Ok(())
    }

    pub(crate) fn ensure_loader(&mut self) {
        if self.icon_loader.is_none() {
            self.icon_loader = Some(IconLoader::spawn(self.loader_root.clone()));
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
        if self
            .icon_loader
            .as_ref()
            .map(|l| l.queue_full_drops)
            .unwrap_or(0)
            > 0
            || self
                .icon_loader
                .as_ref()
                .map(|l| l.stale_drops)
                .unwrap_or(0)
                > 0
        {
            if let Some(loader) = self.icon_loader.as_ref() {
                async_projection::note_loader_stats(loader);
            }
        }
        if results.is_empty() {
            return;
        }
        // Exact-target completion: worker result -> wake -> reactor ->
        // verify (key + generation + view epoch) -> cache -> mutate the
        // one image node only, then redraw its surface.
        let mut need_panel = false;
        let mut need_submenu = false;
        for res in &results {
            let raster = match &res.raster {
                Some(r) => r.clone(),
                None => continue,
            };
            let image = flamewm_ui_x11::RuntimeImage {
                source: raster.source.clone(),
                width: raster.width,
                height: raster.height,
                pixels: raster.pixels.clone(),
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
            let applied = match surface {
                DependentSurface::Panel => runtime.with_document(self.surfaces.panel, |document| {
                    async_projection::apply_icon_result(
                        document,
                        res.target,
                        &res.name,
                        res.width,
                        res.height,
                        res.generation,
                        image.clone(),
                    )
                }),
                DependentSurface::Start => runtime.with_document(self.surfaces.start, |document| {
                    async_projection::apply_icon_result(
                        document,
                        res.target,
                        &res.name,
                        res.width,
                        res.height,
                        res.generation,
                        image.clone(),
                    )
                }),
            };
            let mutated = match applied {
                Ok(true) => true,
                Ok(false) => continue,
                Err(_) => continue,
            };
            if !mutated {
                continue;
            }
            match surface {
                DependentSurface::Panel => need_panel = true,
                DependentSurface::Start => need_submenu = true,
            }
        }
        if need_panel {
            let _ = runtime.redraw(self.surfaces.panel);
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
        self.submit(ControlRequest::LaunchApplication {
            app: app_id,
            options: flamewm_api::applications::ApplicationLaunchOptions::default(),
        });
        self.surfaces
            .close_transients(runtime)
            .map_err(UiBackendError::Renderer)?;
        self.start_open = false;
        Ok(())
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
        let mut changed = self.shell.reconcile_dirty();
        // F10: display geometry change may arrive without a panels dirty
        // flag, so check generation/revision/geometry drift even when no
        // changed domain fired. Sync happens BEFORE next open/projection.
        let mut snapshot = self.shell.snapshot().clone();
        let resynced = self.resync_panel_geometry(runtime, &snapshot);
        if resynced {
            snapshot = self.shell.snapshot().clone();
        }
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
        let mut snapshot = self.shell.snapshot().clone();
        let start_model = self.start_model.clone();
        let category = self.start_category;
        let query = self.start_query.clone();
        let session = snapshot.session.clone();
        self.ensure_loader();
        // Windows or panels or workspaces own the visible panel document.
        if changed.windows || changed.workspaces || (changed.panels && !resynced) {
            // C04: workspace/output/panel geometry changes invalidate the
            // helper anchor. F10: when the pre-projection resync above did
            // not fire but panels changed, resync now.
            if changed.panels && self.resync_panel_geometry(runtime, &snapshot) {
                if changed.workspaces || changed.panels {
                    self.close_quick_control_nonfatal();
                }
            } else {
                let loader = self.icon_loader.as_mut();
                if runtime
                    .with_document(self.surfaces.panel, |document| {
                        async_projection::project_panel_for_changed(
                            document, &snapshot, loader, changed,
                        )
                    })
                    .is_ok()
                {
                    flamewm_shell::runtime::projection_counter(ProjectionKind::Partial).increment();
                    let _ = runtime.redraw(self.surfaces.panel);
                }
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
                let surface = match open {
                    Some("media") => Some(self.surfaces.media),
                    _ => None,
                };
                if let Some(surface) = surface {
                    let _ = runtime.redraw(surface);
                }
            }
        }
        // Applications own the unified Start document, only when the app
        // set changed and Start is open; hidden Start stays untouched.
        if changed.applications && self.start_open {
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
    }

    /// F10: panel/display geometry resync. Close popup/quick helper,
    /// sync the placement context BEFORE the next open/projection,
    /// move_resize the panel native surface when geometry changed,
    /// then reproject the panel. Returns true when a resync happened.
    pub(crate) fn resync_panel_geometry(
        &mut self,
        runtime: &mut SurfaceRuntime,
        snapshot: &ShellSnapshot,
    ) -> bool {
        let (display_generation, panels_revision) = self.surfaces.placement_generation();
        let geometry_changed = snapshot.displays.generation != display_generation
            || snapshot.panels.revision != panels_revision
            || snapshot
                .panels
                .panels
                .first()
                .map(|panel| panel.geometry != self.surfaces.panel_geometry())
                .unwrap_or(false);
        if !geometry_changed {
            return false;
        }
        // Close popup/quick helper first so no open surface keeps a
        // stale anchor while the context swaps underneath it.
        let _ = self.surfaces.close_transients(runtime);
        self.close_quick_control_nonfatal();
        self.start_open = false;
        self.context_menu = None;
        // Sync BEFORE next open/projection; refusal keeps old context.
        if !self.surfaces.sync_panel_context(snapshot) {
            eprintln!("flamewm-shell: panel context sync refused (missing output/panel)");
            return false;
        }
        if let Err(error) = self.surfaces.move_panel_to_synced(runtime) {
            eprintln!("flamewm-shell: panel move_resize failed (nonfatal): {error}");
        }
        let loader = self.icon_loader.as_mut();
        let changed = flamewm_shell::runtime::ChangedDomains {
            panels: true,
            ..Default::default()
        };
        if runtime
            .with_document(self.surfaces.panel, |document| {
                async_projection::project_panel_for_changed(document, snapshot, loader, changed)
            })
            .is_ok()
        {
            flamewm_shell::runtime::projection_counter(
                flamewm_shell::runtime::ProjectionKind::Partial,
            )
            .increment();
            let _ = runtime.redraw(self.surfaces.panel);
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
        self.drain_icons(runtime);
        // J07: nonblocking helper poll; restart is bounded, failures are
        // nonfatal diagnostics, the shell loop always survives.
        flamewm_shell::quick_controls::host::poll_supervisor_nonfatal(&mut self.quick_control);
        let clock_fired = clock_due.swap(false, Ordering::Acquire);
        if clock_fired {
            let now = Local::now();
            runtime.with_document(self.surfaces.panel, |document| {
                projection::project_clock_at(document, now)
            })?;
            runtime.redraw(self.surfaces.panel)?;

            let date = (now.year(), now.month() as u8, now.day() as u8);
            if self.date_tracker.date_changed(date) {
                self.local_date = date;
            }
        }
        // Steady state never polls: reconcile signal-flagged domains at most
        // once per tick, after the clock projection above.
        self.refresh_dynamic(runtime);
        self.project_settings(runtime)?;
        Ok(())
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
        runtime.with_document(self.surfaces.panel, |document| {
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
        let _ = runtime.redraw(self.surfaces.panel);
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
        Ok(())
    }
}

fn clear_transient_state_fields(
    start_open: &mut bool,
    context_menu: &mut Option<flamewm_shell::ContextMenuState>,
) {
    *start_open = false;
    *context_menu = None;
}

#[cfg(test)]
mod tests {
    fn production_source() -> &'static str {
        include_str!("app.rs")
            .split_once("\n#[cfg(test)]\nmod tests")
            .map(|(source, _)| source)
            .expect("app tests must have a production section")
    }

    #[test]
    fn escape_close_clears_start_and_transient_state() {
        let mut start_open = true;
        let mut context_menu = Some(flamewm_shell::ContextMenuState {
            kind: flamewm_shell::ContextMenuKind::Workspace { index: 0 },
            revision: 1,
            anchor: None,
        });

        super::clear_transient_state_fields(&mut start_open, &mut context_menu);

        assert!(!start_open);
        assert!(context_menu.is_none());
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

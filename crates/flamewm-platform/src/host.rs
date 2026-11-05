use std::path::PathBuf;

use flamewm_api::applications::{ApplicationLaunchOptions, DesktopApplication};
use flamewm_api::capabilities::Capabilities;
use flamewm_api::display::DisplaySnapshot;
use flamewm_api::panels::PanelsSnapshot;
use flamewm_api::ports::EnginePorts;
use flamewm_api::session::{SessionAction, SessionCapabilities};
use flamewm_api::settings::{SettingsSnapshot, SettingsTransaction};
use flamewm_api::shortcuts::{KeyBinding, ShortcutSnapshot};
use flamewm_api::system::{SystemAction, SystemSnapshot};
use flamewm_api::wm_features::{FullscreenMonitorSpan, InteractiveMoveResize};
use flamewm_api::workspace::WorkspaceSnapshot;
use flamewm_api::{
    DesktopAppId, ErrorCode, FlameError, FlameResult, ModeId, OutputId, PanelEdge, TaskEntryId,
    TransactionId, WindowRef,
};

use crate::panel::TaskbarFrame;
use crate::scale::is_supported_scale;
use crate::services::applications::ApplicationService;
use crate::services::background::BackgroundService;
use crate::services::display::DisplayService;
use crate::services::panels::PanelService;
use crate::services::session::SessionService;
use crate::services::settings::SettingsService;
use crate::services::shortcuts::{ShortcutService, effective_registry};
use crate::services::weather::WeatherFeedService;
use crate::services::windows::WindowService;
use crate::services::workspaces::{WorkspaceService, count_store_path};
use crate::shortcuts::ShortcutRegistry;
use crate::system::{SystemActionHandler, SystemService};

/// Process composition root for `FlameWM` product state.
///
/// The engine adapter is owned once. Product services never retain pointers/references into it;
/// every command borrows the required port for exactly the duration of that command. This avoids
/// the native shutdown/UAF class where views outlived raw service/engine pointers.
pub struct PlatformHost<E: EnginePorts> {
    engine: E,
    started: bool,
    generation: u64,
    windows: WindowService,
    workspaces: WorkspaceService,
    displays: DisplayService,
    panels: PanelService,
    settings: SettingsService,
    shortcuts: ShortcutService,
    applications: ApplicationService,
    session: SessionService,
    background: BackgroundService,
    system: SystemService,
    weather: WeatherFeedService,
}

impl<E: EnginePorts> PlatformHost<E> {
    #[must_use]
    pub fn new(engine: E, settings_path: PathBuf) -> Self {
        Self {
            engine,
            started: false,
            generation: 0,
            windows: WindowService::default(),
            workspaces: WorkspaceService::new(count_store_path(&settings_path)),
            displays: DisplayService::default(),
            panels: PanelService::default(),
            settings: SettingsService::new(settings_path),
            shortcuts: ShortcutService::default(),
            applications: ApplicationService::default(),
            session: SessionService,
            background: BackgroundService::default(),
            system: SystemService::default(),
            weather: WeatherFeedService::default(),
        }
    }

    pub fn start(&mut self) -> FlameResult<()> {
        if self.started {
            return Ok(());
        }
        match std::fs::read_to_string(self.settings.path()) {
            Ok(text) => self.settings.load_text(&text)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(FlameError::new(ErrorCode::IoFailure, error.to_string()));
            }
        }
        self.windows.refresh(&self.engine)?;
        self.displays.refresh(&mut self.engine)?;
        self.seed_persisted_shell_scales()?;
        let displays = self.displays.cached().cloned().ok_or_else(|| {
            FlameError::new(ErrorCode::InternalFailure, "display snapshot missing")
        })?;
        self.panels.sync_outputs(&displays);
        self.seed_persisted_panel_edges();
        self.panels
            .reconcile_taskbar_height(self.settings.current().taskbar_height);
        self.panels
            .reconcile_taskbar_frame(TaskbarFrame::from_settings(self.settings.current()));
        self.started = true;
        self.generation = self.generation.saturating_add(1).max(1);
        Ok(())
    }

    /// The legacy single-output settings key is `default`. `XWayland`'s `RandR` output is
    /// named `wayland-0`; resolve this alias only when exactly one output is connected.
    /// Never guess an output on multi-monitor systems.
    fn persisted_output_id(&self, name: &str) -> OutputId {
        if name == "default" {
            if let Some(snapshot) = self.displays.cached() {
                let mut connected = snapshot.outputs.iter().filter(|out| out.connected);
                if let Some(only) = connected.next() {
                    if connected.next().is_none() {
                        return only.id.clone();
                    }
                }
            }
        }
        OutputId::new(name.to_owned())
    }

    /// Re-applies persisted panel edges after panels exist for the connected outputs. An entry for a
    /// disconnected output is skipped with a stderr diagnostic so it cannot block startup.
    fn seed_persisted_panel_edges(&mut self) {
        for (output, edge) in &self.settings.current().panel_edges {
            let revision = self.panels.revision();
            let target = self.persisted_output_id(output);
            if let Err(error) = self.panels.set_edge(&target, *edge, revision) {
                eprintln!("flamewm: persisted panel edge for {output} not applied: {error}");
            }
        }
    }

    /// Applies persisted per-output shell scales to the display snapshot at startup.
    ///
    /// Entries for outputs that are not connected are skipped (`NotFound`) with a one-line stderr diagnostic. Entries rejected by the
    /// setter as invalid (`InvalidArgument`) are skipped so a bad persisted value cannot block startup.
    /// The generation is re-read per entry because each applied scale bumps it.
    fn seed_persisted_shell_scales(&mut self) -> FlameResult<()> {
        for (output, scale) in &self.settings.current().per_output_scale {
            let generation = self.cached_generation();
            let target = self.persisted_output_id(output);
            match self.displays.set_shell_scale(&target, *scale, generation) {
                Ok(_) => {}
                Err(error) if error.code == ErrorCode::NotFound => {
                    let connected = self.displays.cached().map_or_else(
                        || "none (display snapshot not cached)".to_owned(),
                        |snapshot| {
                            snapshot
                                .outputs
                                .iter()
                                .filter(|candidate| candidate.connected)
                                .map(|candidate| candidate.id.to_string())
                                .collect::<Vec<_>>()
                                .join(", ")
                        },
                    );
                    eprintln!(
                        "flamewm: persisted scale.{output} matches no connected output; connected outputs: {connected}"
                    );
                }
                Err(error) if error.code == ErrorCode::InvalidArgument => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    /// Pushes per-output scale changes from a committed settings transaction into the display snapshot.
    ///
    /// Runs only after the settings are persisted. Outputs whose scale was removed (or reset by the
    /// `displays` section) return to 100. Failures are diagnosed on stderr rather than failing the
    /// already-committed transaction; when no display snapshot is cached yet, startup seeding applies it.
    fn sync_runtime_shell_scales(
        &mut self,
        before: &std::collections::BTreeMap<String, u16>,
        after: &std::collections::BTreeMap<String, u16>,
    ) {
        let mut targets: Vec<(&String, u16)> = after
            .iter()
            .filter(|(output, scale)| before.get(*output) != Some(*scale))
            .map(|(output, scale)| (output, *scale))
            .collect();
        targets.extend(
            before
                .keys()
                .filter(|output| !after.contains_key(*output))
                .map(|output| (output, 100)),
        );
        for (output, scale) in targets {
            let Some(generation) = self.displays.cached().map(|snapshot| snapshot.generation)
            else {
                return;
            };
            match self
                .displays
                .set_shell_scale(&OutputId::new(output.clone()), scale, generation)
            {
                Ok(_) => {}
                Err(error) if error.code == ErrorCode::NotFound => {
                    eprintln!("flamewm: scale.{output} matches no connected output");
                }
                Err(error) => {
                    eprintln!("flamewm: failed to apply scale.{output}: {}", error.message);
                }
            }
        }
    }

    /// Generation of the cached display snapshot; 0 before the first display refresh.
    fn cached_generation(&self) -> u64 {
        self.displays
            .cached()
            .map_or(0, |snapshot| snapshot.generation)
    }

    pub fn stop(&mut self) {
        if !self.started {
            return;
        }
        self.panels.close_start();
        self.started = false;
        self.generation = self.generation.saturating_add(1).max(1);
    }

    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub fn capabilities(&self) -> Capabilities {
        Capabilities::current()
    }

    pub fn refresh_windows(&mut self) -> FlameResult<bool> {
        let changed = self.windows.refresh(&self.engine)?;
        if changed {
            let _ = self.panels.reconcile_windows(self.windows.cached());
        }
        Ok(changed)
    }

    pub fn activate_window(&mut self, window: WindowRef) -> FlameResult<()> {
        self.windows.activate(&mut self.engine, window)
    }

    pub fn minimize_window(&mut self, window: WindowRef) -> FlameResult<()> {
        self.windows.minimize(&mut self.engine, window)
    }

    pub fn restore_window(&mut self, window: WindowRef) -> FlameResult<()> {
        self.windows.restore(&mut self.engine, window)
    }

    pub fn close_window(&mut self, window: WindowRef) -> FlameResult<()> {
        self.windows.close(&mut self.engine, window)
    }

    pub fn workspace_snapshot(&self) -> FlameResult<WorkspaceSnapshot> {
        self.workspaces.snapshot(&self.engine)
    }

    /// The persisted workspace count, when the session has one.
    ///
    /// A compositor that owns its own topology (Wayland) needs the count before it can build a
    /// snapshot, so this reads the store rather than requiring a working [`WorkspacePort`].
    #[must_use]
    pub fn workspace_count(&self) -> Option<usize> {
        self.workspaces.load_count()
    }

    pub fn activate_workspace(&mut self, index: usize, expected_revision: u64) -> FlameResult<()> {
        self.workspaces
            .activate(&mut self.engine, index, expected_revision)
    }

    pub fn insert_workspace_after(
        &mut self,
        index: usize,
        expected_revision: u64,
    ) -> FlameResult<()> {
        self.workspaces
            .insert_after(&mut self.engine, index, expected_revision)
    }

    pub fn remove_workspace(&mut self, index: usize, expected_revision: u64) -> FlameResult<()> {
        self.workspaces
            .remove(&mut self.engine, index, expected_revision)
    }

    pub fn showing_desktop(&self) -> FlameResult<bool> {
        self.windows.showing_desktop(&self.engine)
    }

    pub fn set_showing_desktop(&mut self, show: bool) -> FlameResult<()> {
        self.windows.set_showing_desktop(&mut self.engine, show)?;
        let _ = self.refresh_windows()?;
        Ok(())
    }

    pub fn set_fullscreen_monitors(
        &mut self,
        window: WindowRef,
        span: FullscreenMonitorSpan,
    ) -> FlameResult<()> {
        self.windows
            .set_fullscreen_monitors(&mut self.engine, window, span)
    }

    pub fn begin_interactive_move_resize(
        &mut self,
        request: InteractiveMoveResize,
    ) -> FlameResult<()> {
        self.windows
            .begin_interactive_move_resize(&mut self.engine, request)
    }

    pub fn cancel_interactive_move_resize(&mut self, window: WindowRef) -> FlameResult<()> {
        self.windows
            .cancel_interactive_move_resize(&mut self.engine, window)
    }

    pub fn display_snapshot(&mut self) -> FlameResult<DisplaySnapshot> {
        Ok(self.displays.refresh(&mut self.engine)?.clone())
    }

    /// Reconcile per-output panel surfaces/reservations after a display topology refresh.
    pub fn resync_panels_for_displays(&mut self, displays: &DisplaySnapshot) -> FlameResult<bool> {
        let previous_revision = self.panels.revision();
        self.panels.sync_outputs(displays);
        // A detached and reattached output creates a fresh OutputPanel. Reapply
        // persisted presentation policy, otherwise the panel silently reverts
        // to bottom/44px/classic after RandR hotplug or monitor rearrangement.
        self.seed_persisted_panel_edges();
        self.panels
            .reconcile_taskbar_height(self.settings.current().taskbar_height);
        self.panels
            .reconcile_taskbar_frame(TaskbarFrame::from_settings(self.settings.current()));
        Ok(self.panels.revision() != previous_revision)
    }

    pub fn begin_display_mode(
        &mut self,
        output: &OutputId,
        mode: ModeId,
        expected_generation: u64,
        now_ms: u64,
    ) -> FlameResult<TransactionId> {
        self.displays
            .begin_mode(&mut self.engine, output, mode, expected_generation, now_ms)
    }

    pub fn keep_display_mode(
        &mut self,
        transaction: TransactionId,
        now_ms: u64,
    ) -> FlameResult<()> {
        self.displays.keep(&mut self.engine, transaction, now_ms)
    }

    pub fn revert_display_mode(&mut self, transaction: TransactionId) -> FlameResult<()> {
        self.displays.revert(&mut self.engine, transaction)
    }

    pub fn tick_display_transactions(&mut self, now_ms: u64) -> FlameResult<bool> {
        self.displays.tick(&mut self.engine, now_ms)
    }

    /// Revert deadline of the pending display mode transaction, if any (cached snapshot).
    #[must_use]
    pub fn display_transaction_deadline_ms(&self) -> Option<u64> {
        self.displays
            .cached()
            .and_then(|snapshot| snapshot.pending.as_ref())
            .map(|pending| pending.deadline_ms)
    }

    pub fn set_shell_scale(
        &mut self,
        output: &OutputId,
        percent: u16,
        expected_generation: u64,
    ) -> FlameResult<bool> {
        let previous = self
            .displays
            .cached()
            .and_then(|snapshot| {
                snapshot
                    .outputs
                    .iter()
                    .find(|candidate| candidate.connected && candidate.id == *output)
            })
            .map(|candidate| candidate.shell_scale_percent);
        let changed = self
            .displays
            .set_shell_scale(output, percent, expected_generation)?;
        if changed {
            // Persist before the panel resync so a failed write leaves no runtime-only scale behind.
            if let Err(error) = self.settings.persist_scale(output.as_str(), percent) {
                if let Some(previous) = previous {
                    let generation = self.cached_generation();
                    if let Err(restore_error) =
                        self.displays.set_shell_scale(output, previous, generation)
                    {
                        eprintln!(
                            "flamewm: shell scale rollback failed after persistence error: {}",
                            restore_error.message
                        );
                    }
                }
                return Err(error);
            }
            // Panel thickness is derived from the output scale; resync it the same way
            // apply_settings does so the panel geometry follows the new display scale.
            if let Some(displays) = self.displays.cached().cloned() {
                self.panels.sync_outputs(&displays);
            }
        }
        Ok(changed)
    }

    /// Last display snapshot already cached by the platform, without refreshing from the engine.
    #[must_use]
    pub fn cached_display_snapshot(&self) -> Option<DisplaySnapshot> {
        self.displays.cached().cloned()
    }

    #[must_use]
    pub fn settings_snapshot(&self) -> SettingsSnapshot {
        self.settings.snapshot()
    }

    pub fn apply_settings(
        &mut self,
        transaction: &SettingsTransaction,
    ) -> FlameResult<SettingsSnapshot> {
        // Validate scale values before any side effect so an invalid scale rejects the whole transaction.
        validate_requested_scales(transaction)?;
        let scales_before = self.settings.current().per_output_scale.clone();
        let snapshot = self.settings.apply(&mut self.engine, transaction)?;
        // Settings are now committed; push the resulting per-output scales into the display snapshot.
        let scales_after = self.settings.current().per_output_scale.clone();
        self.sync_runtime_shell_scales(&scales_before, &scales_after);
        // Panel thickness is derived from the display scale at sync time, so re-run the same
        // reconcile used at startup (sync_outputs, then taskbar height) once scales have moved.
        if let Some(displays) = self.displays.cached().cloned() {
            self.panels.sync_outputs(&displays);
        }
        if let Some(flamewm_api::settings::SettingValue::Integer(logical_size)) =
            snapshot.values.get("taskbarHeight")
        {
            let logical_size = (*logical_size).try_into().map_err(|_| {
                FlameError::new(ErrorCode::InvalidArgument, "taskbarHeight is out of range")
            })?;
            self.panels.reconcile_taskbar_height(logical_size);
        }
        self.panels
            .reconcile_taskbar_frame(TaskbarFrame::from_settings(self.settings.current()));
        Ok(snapshot)
    }

    #[must_use]
    pub fn shortcut_snapshot(&self) -> ShortcutSnapshot {
        self.shortcuts.snapshot()
    }

    pub fn set_shortcut(
        &mut self,
        action: &str,
        binding: &str,
        expected_revision: u64,
    ) -> FlameResult<()> {
        let checkpoint = self.shortcuts.checkpoint();
        self.shortcuts
            .set_binding(&mut self.engine, action, binding, expected_revision)?;
        let persisted = self
            .shortcuts
            .snapshot()
            .bindings
            .get(action)
            .and_then(|entry| entry.0.clone())
            .unwrap_or_default();
        self.persist_shortcut(checkpoint, action, Some(&persisted))
    }

    pub fn clear_shortcut(&mut self, action: &str, expected_revision: u64) -> FlameResult<()> {
        let checkpoint = self.shortcuts.checkpoint();
        self.shortcuts
            .clear_binding(&mut self.engine, action, expected_revision)?;
        self.persist_shortcut(checkpoint, action, Some(""))
    }

    pub fn reset_shortcut(&mut self, action: &str, expected_revision: u64) -> FlameResult<()> {
        let checkpoint = self.shortcuts.checkpoint();
        self.shortcuts
            .reset_binding(&mut self.engine, action, expected_revision)?;
        self.persist_shortcut(checkpoint, action, None)
    }

    /// Persists one hotkey entry after its grabs committed. If persistence fails, the checkpoint
    /// registry and grabs are restored so the WM and the settings file never diverge.
    fn persist_shortcut(
        &mut self,
        checkpoint: (ShortcutRegistry, u64),
        action: &str,
        binding: Option<&str>,
    ) -> FlameResult<()> {
        let Err(error) = self.settings.persist_hotkey(action, binding) else {
            return Ok(());
        };
        if let Err(restore_error) = self.shortcuts.restore(&mut self.engine, checkpoint) {
            eprintln!("flamewm: hotkey rollback failed after persistence error: {restore_error}");
        }
        Err(error)
    }

    pub fn apply_shortcuts(
        &mut self,
        bindings: &std::collections::BTreeMap<String, KeyBinding>,
        expected_revision: u64,
    ) -> FlameResult<()> {
        self.shortcuts
            .apply(&mut self.engine, bindings, expected_revision)
    }

    /// Suspends every shortcut grab for a Settings capture lease. See `ShortcutService::begin_capture`.
    pub fn begin_hotkey_capture(
        &mut self,
        action: &str,
        expected_revision: u64,
    ) -> FlameResult<u64> {
        self.shortcuts
            .begin_capture(&mut self.engine, action, expected_revision)
    }

    pub fn end_hotkey_capture(&mut self, lease: u64) -> FlameResult<()> {
        self.shortcuts.end_capture(&mut self.engine, lease)
    }

    /// Grabs the effective shortcut set (persisted hotkeys over defaults) after `start()` loaded
    /// settings. A persisted set that fails validation logs once and grabs the defaults instead.
    pub fn commit_startup_shortcuts(&mut self) -> FlameResult<()> {
        let current = self.settings.current();
        let registry = match effective_registry(&current.hotkeys, current.assistant_enabled) {
            Ok(registry) => registry,
            Err(error) => {
                eprintln!("flamewm: persisted hotkeys rejected, grabbing defaults: {error}");
                ShortcutRegistry::default()
            }
        };
        self.shortcuts.commit_startup(&mut self.engine, registry)
    }

    #[must_use]
    pub fn panels_snapshot(&self) -> PanelsSnapshot {
        self.panels.snapshot()
    }

    /// Physical output geometry and panel-reserved work area for WM geometry decisions.
    #[must_use]
    pub fn output_work_areas(&self) -> Vec<(OutputId, flamewm_api::Rect, flamewm_api::Rect, bool)> {
        self.panels.work_area_view()
    }

    pub fn set_panel_edge(
        &mut self,
        output: &OutputId,
        edge: PanelEdge,
        expected_revision: u64,
    ) -> FlameResult<()> {
        let previous = self.panels.manager().panel(output).map(|panel| panel.edge);
        self.panels.set_edge(output, edge, expected_revision)?;
        let Err(error) = self.settings.persist_panel_edge(output.as_str(), edge) else {
            return Ok(());
        };
        // Roll the panel back so the live edge and the settings file never diverge.
        if let Some(previous) = previous {
            let revision = self.panels.revision();
            if let Err(restore_error) = self.panels.set_edge(output, previous, revision) {
                eprintln!(
                    "flamewm: panel edge rollback failed after persistence error: {restore_error}"
                );
            }
        }
        Err(error)
    }

    pub fn set_panel_size(
        &mut self,
        output: &OutputId,
        logical_size: u16,
        expected_revision: u64,
    ) -> FlameResult<()> {
        self.panels
            .set_size(output, logical_size, expected_revision)
    }

    pub fn pin_app(&mut self, app: DesktopAppId, expected_revision: u64) -> FlameResult<bool> {
        self.panels.pin_app(app, expected_revision)
    }

    pub fn unpin_app(&mut self, app: &DesktopAppId, expected_revision: u64) -> FlameResult<bool> {
        self.panels.unpin_app(app, expected_revision)
    }

    pub fn reorder_task_entry(
        &mut self,
        entry_id: &TaskEntryId,
        to: usize,
        expected_revision: u64,
    ) -> FlameResult<bool> {
        self.panels.reorder_entry(entry_id, to, expected_revision)
    }

    pub fn toggle_start(&mut self) -> bool {
        self.panels.toggle_start()
    }

    pub fn replace_applications(&mut self, applications: Vec<DesktopApplication>) -> bool {
        self.applications.replace_catalog(applications)
    }

    #[must_use]
    pub fn search_applications(&self, query: &str) -> Vec<DesktopApplication> {
        self.applications.search(query)
    }

    pub fn launch_application(
        &mut self,
        app: &DesktopAppId,
        options: &ApplicationLaunchOptions,
    ) -> FlameResult<()> {
        self.applications.launch(&mut self.engine, app, options)
    }

    #[must_use]
    pub fn session_capabilities(&self) -> SessionCapabilities {
        self.session.capabilities(&self.engine)
    }

    pub fn session_action(&mut self, action: SessionAction) -> FlameResult<()> {
        self.session.perform(&mut self.engine, action)
    }

    /// Latest weather payload published by the shell (revision 0 when none was published).
    #[must_use]
    pub fn weather_feed_snapshot(&self) -> flamewm_api::weather::WeatherFeedSnapshot {
        self.weather.snapshot()
    }

    /// Caches a shell-published weather payload; `true` when the cached payload changed.
    pub fn publish_weather_feed(&mut self, payload: String) -> FlameResult<bool> {
        self.weather.publish(payload)
    }

    #[must_use]
    pub fn system_snapshot(&self) -> SystemSnapshot {
        self.system.snapshot().clone()
    }

    pub fn set_system_action_handler(&mut self, handler: SystemActionHandler) {
        self.system.set_action_handler(handler);
    }

    /// Dispatches a media key to the default output sink through the system action path.
    /// `cap` is the live volume cap from the shared "raise maximum volume" setting.
    pub fn media_audio_key(
        &mut self,
        key: crate::system::MediaAudioKey,
        cap: u8,
    ) -> FlameResult<()> {
        self.system.media_audio_key(key, cap)
    }

    /// Dispatches a brightness key to the panel backlight through the system action path.
    pub fn media_brightness_key(
        &mut self,
        key: crate::system::MediaBrightnessKey,
    ) -> FlameResult<()> {
        self.system.media_brightness_key(key)
    }

    pub fn system_action(
        &mut self,
        action: SystemAction,
        expected_revision: u64,
    ) -> FlameResult<()> {
        self.system.perform_action(action, expected_revision)
    }

    #[must_use]
    pub fn system(&self) -> &SystemService {
        &self.system
    }

    pub fn system_mut(&mut self) -> &mut SystemService {
        &mut self.system
    }

    #[must_use]
    pub fn background(&self) -> &BackgroundService {
        &self.background
    }

    #[must_use]
    pub fn engine(&self) -> &E {
        &self.engine
    }

    pub fn engine_mut(&mut self) -> &mut E {
        &mut self.engine
    }
}

impl<E: EnginePorts> Drop for PlatformHost<E> {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Rejects `scale.<output>` values outside the supported bucket set before the transaction is committed.
/// Uses the same validator as `DisplayService::set_shell_scale`, so no state is touched on failure.
fn validate_requested_scales(transaction: &SettingsTransaction) -> FlameResult<()> {
    for change in &transaction.changes {
        if !change.key.starts_with("scale.") {
            continue;
        }
        if let Some(flamewm_api::settings::SettingValue::Integer(scale)) = change.value.as_ref() {
            let supported = u16::try_from(*scale).is_ok_and(is_supported_scale);
            if !supported {
                return Err(FlameError::new(
                    ErrorCode::InvalidArgument,
                    "shell scale must be 100..=200 in 5% steps",
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

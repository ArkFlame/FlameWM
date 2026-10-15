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
use flamewm_api::wm_features::{
    FeatureAction, FullscreenMonitorSpan, InteractiveMoveResize, WindowFeature,
};
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
use crate::services::windows::WindowService;
use crate::services::workspaces::WorkspaceService;
use crate::shortcuts::ShortcutRegistry;
use crate::system::{SystemActionHandler, SystemService};

/// Process composition root for FlameWM product state.
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
}

impl<E: EnginePorts> PlatformHost<E> {
    #[must_use]
    pub fn new(engine: E, settings_path: PathBuf) -> Self {
        Self {
            engine,
            started: false,
            generation: 0,
            windows: WindowService::default(),
            workspaces: WorkspaceService,
            displays: DisplayService::default(),
            panels: PanelService::default(),
            settings: SettingsService::new(settings_path),
            shortcuts: ShortcutService::default(),
            applications: ApplicationService::default(),
            session: SessionService,
            background: BackgroundService::default(),
            system: SystemService::default(),
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

    /// Re-applies persisted panel edges after panels exist for the connected outputs. An entry for a
    /// disconnected output is skipped with a stderr diagnostic so it cannot block startup.
    fn seed_persisted_panel_edges(&mut self) {
        let entries: Vec<(String, PanelEdge)> = self
            .settings
            .current()
            .panel_edges
            .iter()
            .map(|(output, edge)| (output.clone(), *edge))
            .collect();
        for (output, edge) in entries {
            let revision = self.panels.revision();
            if let Err(error) = self
                .panels
                .set_edge(&OutputId::new(output.clone()), edge, revision)
            {
                eprintln!("flamewm: persisted panel edge for {output} not applied: {error}");
            }
        }
    }

    /// Applies persisted per-output shell scales to the display snapshot at startup.
    ///
    /// Entries for outputs that are not connected are skipped (NotFound) with a one-line stderr diagnostic. Entries rejected by the
    /// setter as invalid (InvalidArgument) are skipped so a bad persisted value cannot block startup.
    /// The generation is re-read per entry because each applied scale bumps it.
    fn seed_persisted_shell_scales(&mut self) -> FlameResult<()> {
        let entries: Vec<(String, u16)> = self
            .settings
            .current()
            .per_output_scale
            .iter()
            .map(|(output, scale)| (output.clone(), *scale))
            .collect();
        for (output, scale) in entries {
            let generation = self
                .displays
                .cached()
                .map_or(0, |snapshot| snapshot.generation);
            match self
                .displays
                .set_shell_scale(&OutputId::new(output.clone()), scale, generation)
            {
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
        let mut targets: Vec<(String, u16)> = after
            .iter()
            .filter(|(output, scale)| before.get(*output) != Some(*scale))
            .map(|(output, scale)| (output.clone(), *scale))
            .collect();
        targets.extend(
            before
                .keys()
                .filter(|output| !after.contains_key(*output))
                .map(|output| (output.clone(), 100)),
        );
        for (output, scale) in targets {
            let Some(generation) = self.displays.cached().map(|snapshot| snapshot.generation) else {
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

    pub fn stop(&mut self) {
        if !self.started {
            return;
        }
        self.panels.close_start();
        self.started = false;
        self.generation = self.generation.saturating_add(1).max(1);
    }

    #[must_use]
    pub const fn is_started(&self) -> bool {
        self.started
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

    pub fn move_window_to_workspace(
        &mut self,
        window: WindowRef,
        target: usize,
    ) -> FlameResult<()> {
        self.workspaces
            .move_window(&mut self.engine, window, target)
    }

    pub fn apply_window_feature(
        &mut self,
        window: WindowRef,
        feature: WindowFeature,
        action: FeatureAction,
    ) -> FlameResult<()> {
        self.windows
            .apply_feature(&mut self.engine, window, feature, action)
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
        Ok(self.panels.sync_outputs(displays))
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
                    let generation = self
                        .displays
                        .cached()
                        .map_or(0, |snapshot| snapshot.generation);
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
    pub fn begin_hotkey_capture(&mut self, action: &str, expected_revision: u64) -> FlameResult<u64> {
        self.shortcuts
            .begin_capture(&mut self.engine, action, expected_revision)
    }

    pub fn end_hotkey_capture(&mut self, lease: u64) -> FlameResult<()> {
        self.shortcuts.end_capture(&mut self.engine, lease)
    }

    /// Grabs the effective shortcut set (persisted hotkeys over defaults) after `start()` loaded
    /// settings. A persisted set that fails validation logs once and grabs the defaults instead.
    pub fn commit_startup_shortcuts(&mut self) -> FlameResult<()> {
        let registry = match effective_registry(&self.settings.current().hotkeys) {
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

    #[must_use]
    pub fn system_snapshot(&self) -> SystemSnapshot {
        self.system.snapshot().clone()
    }

    pub fn set_system_action_handler(&mut self, handler: SystemActionHandler) {
        self.system.set_action_handler(handler);
    }

    /// Dispatches a media key to the default output sink through the system action path.
    pub fn media_audio_key(&mut self, key: crate::system::MediaAudioKey) -> FlameResult<()> {
        self.system.media_audio_key(key)
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
                    "shell scale must be 100/125/150/175/200",
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use flamewm_api::display::{DisplaySnapshot, OutputSnapshot};
    use flamewm_api::geometry::Rect;
    use flamewm_api::input::PointerPosition;
    use flamewm_api::ports::{
        ApplicationPort, DisplayPort, InputPort, SessionPort, ShortcutPort, WindowPort,
        WorkspacePort,
    };
    use flamewm_api::window::WindowSnapshot;
    use flamewm_api::workspace::{WorkspacePlan, WorkspaceSnapshot};

    use super::*;

    struct FakeEngine {
        display: DisplaySnapshot,
        reverted: Vec<TransactionId>,
    }

    impl WindowPort for FakeEngine {
        fn get(&self, _window: WindowRef) -> FlameResult<WindowSnapshot> {
            Err(FlameError::new(ErrorCode::NotFound, "no windows"))
        }
        fn snapshot(&self) -> FlameResult<Vec<WindowSnapshot>> {
            Ok(Vec::new())
        }
        fn activate(&mut self, _window: WindowRef) -> FlameResult<()> {
            Ok(())
        }
        fn minimize(&mut self, _window: WindowRef) -> FlameResult<()> {
            Ok(())
        }
        fn maximize(&mut self, _window: WindowRef) -> FlameResult<()> {
            Ok(())
        }
        fn restore(&mut self, _window: WindowRef) -> FlameResult<()> {
            Ok(())
        }
        fn close(&mut self, _window: WindowRef) -> FlameResult<()> {
            Ok(())
        }
        fn set_outer_geometry(&mut self, _window: WindowRef, _geometry: Rect) -> FlameResult<()> {
            Ok(())
        }
        fn work_area(&self, _window: WindowRef) -> FlameResult<Rect> {
            Err(FlameError::new(ErrorCode::NotFound, "no windows"))
        }
        fn output(&self, _window: WindowRef) -> FlameResult<OutputId> {
            Err(FlameError::new(ErrorCode::NotFound, "no windows"))
        }
    }

    impl WorkspacePort for FakeEngine {
        fn workspace_snapshot(&self) -> FlameResult<WorkspaceSnapshot> {
            Err(FlameError::new(ErrorCode::Unsupported, "fake workspace"))
        }
        fn activate_workspace(&mut self, _index: usize, _expected_revision: u64) -> FlameResult<()> {
            Err(FlameError::new(ErrorCode::Unsupported, "fake workspace"))
        }
        fn move_window_to_workspace(
            &mut self,
            _window: WindowRef,
            _target: usize,
        ) -> FlameResult<()> {
            Err(FlameError::new(ErrorCode::Unsupported, "fake workspace"))
        }
        fn switch_workspace_with_window(
            &mut self,
            _window: WindowRef,
            _target: usize,
            _expected_revision: u64,
        ) -> FlameResult<()> {
            Err(FlameError::new(ErrorCode::Unsupported, "fake workspace"))
        }
        fn apply_workspace_plan(&mut self, _plan: &WorkspacePlan) -> FlameResult<()> {
            Err(FlameError::new(ErrorCode::Unsupported, "fake workspace"))
        }
    }

    impl DisplayPort for FakeEngine {
        fn display_snapshot(&self) -> FlameResult<DisplaySnapshot> {
            Ok(self.display.clone())
        }
        fn apply_mode(&mut self, _output: &OutputId, _mode: ModeId) -> FlameResult<TransactionId> {
            Ok(TransactionId(7))
        }
        fn keep_mode(&mut self, _transaction: TransactionId) -> FlameResult<()> {
            Ok(())
        }
        fn revert_mode(&mut self, transaction: TransactionId) -> FlameResult<()> {
            self.reverted.push(transaction);
            Ok(())
        }
    }

    impl ShortcutPort for FakeEngine {
        fn prepare_shortcuts(&mut self, _desired: &BTreeMap<String, KeyBinding>) -> FlameResult<()> {
            Ok(())
        }
        fn commit_shortcuts(&mut self) -> FlameResult<()> {
            Ok(())
        }
        fn rollback_shortcuts(&mut self) {}
    }

    impl InputPort for FakeEngine {
        fn root_pointer(&self) -> FlameResult<PointerPosition> {
            Err(FlameError::new(ErrorCode::Unsupported, "fake input"))
        }
    }

    impl ApplicationPort for FakeEngine {
        fn launch(
            &mut self,
            _app: &DesktopAppId,
            _options: &ApplicationLaunchOptions,
        ) -> FlameResult<()> {
            Ok(())
        }
        fn launch_uri(&mut self, _uri: &str) -> FlameResult<()> {
            Ok(())
        }
    }

    impl SessionPort for FakeEngine {
        fn session_capabilities(&self) -> SessionCapabilities {
            SessionCapabilities::default()
        }
        fn perform_session_action(&mut self, _action: SessionAction) -> FlameResult<()> {
            Ok(())
        }
    }

    fn fake_display(shell_scale_percent: u16) -> DisplaySnapshot {
        DisplaySnapshot {
            generation: 1,
            outputs: vec![OutputSnapshot {
                id: OutputId::new("default"),
                connector: "default".to_owned(),
                edid_identity: String::new(),
                connected: true,
                primary: true,
                geometry: Rect {
                    x: 0,
                    y: 0,
                    width: 1920,
                    height: 1080,
                },
                current_mode: ModeId(1),
                modes: Vec::new(),
                shell_scale_percent,
            }],
            pending: None,
        }
    }

    #[test]
    fn unconfirmed_display_mode_reverts_only_after_deadline_tick() {
        let path = std::env::temp_dir().join(format!(
            "flamewm-host-display-tick-test-{}.conf",
            std::process::id()
        ));
        std::fs::write(&path, "").expect("write settings fixture");
        let mut display = fake_display(100);
        display.outputs[0].modes = vec![
            flamewm_api::display::DisplayMode {
                id: ModeId(1),
                resolution: flamewm_api::geometry::Size {
                    width: 1920,
                    height: 1080,
                },
                refresh_millihz: 60_000,
                preferred: true,
            },
            flamewm_api::display::DisplayMode {
                id: ModeId(5),
                resolution: flamewm_api::geometry::Size {
                    width: 1280,
                    height: 720,
                },
                refresh_millihz: 60_000,
                preferred: false,
            },
        ];
        let mut host = PlatformHost::new(
            FakeEngine {
                display,
                reverted: Vec::new(),
            },
            path.clone(),
        );
        host.start().expect("host starts");
        let generation = host
            .display_snapshot()
            .expect("display snapshot")
            .generation;
        let begun = host.begin_display_mode(&OutputId::new("default"), ModeId(5), generation, 0);
        let _ = std::fs::remove_file(&path);
        assert_eq!(begun.expect("mode change begins"), TransactionId(7));

        // Before the 15s confirmation deadline the tick must not revert.
        assert!(!host.tick_display_transactions(14_999).expect("tick"));
        assert!(host.engine().reverted.is_empty());

        // At the deadline the pending transaction is reverted and cleared.
        assert!(host.tick_display_transactions(15_000).expect("tick"));
        assert_eq!(host.engine().reverted, vec![TransactionId(7)]);
        let snapshot = host.display_snapshot().expect("display snapshot");
        assert!(snapshot.pending.is_none());
    }

    #[test]
    fn start_applies_persisted_per_output_scale_to_display_snapshot() {
        let path = std::env::temp_dir().join(format!(
            "flamewm-host-scale-test-{}.conf",
            std::process::id()
        ));
        std::fs::write(&path, "scale.default=125\n").expect("write settings fixture");
        let mut host = PlatformHost::new(
            FakeEngine {
                display: fake_display(100),
                reverted: Vec::new(),
            },
            path.clone(),
        );
        let started = host.start();
        let _ = std::fs::remove_file(&path);
        started.expect("host starts");
        let scale = host
            .displays
            .cached()
            .and_then(|snapshot| snapshot.outputs.first())
            .map(|output| output.shell_scale_percent);
        assert_eq!(scale, Some(125));
    }

    #[test]
    fn startup_seeded_scale_sizes_panel_like_live_scale_change() {
        use crate::panel::logical_to_physical;

        let path = std::env::temp_dir().join(format!(
            "flamewm-host-startup-panel-scale-test-{}.conf",
            std::process::id()
        ));
        std::fs::write(&path, "scale.default=125\ntaskbarHeight=72\ntaskbarStyle=classic\n")
            .expect("write settings fixture");
        let mut display = fake_display(100);
        display.outputs[0].geometry = Rect {
            x: 0,
            y: 0,
            width: 1350,
            height: 641,
        };
        let mut host = PlatformHost::new(
            FakeEngine {
                display,
                reverted: Vec::new(),
            },
            path.clone(),
        );
        let started = host.start();
        let _ = std::fs::remove_file(&path);
        started.expect("host starts");

        let panel = host.panels.snapshot().panels[0].clone();
        assert_eq!(panel.logical_size, 72);
        assert_eq!(logical_to_physical(72, 125), 90);
        assert_eq!(panel.geometry.height, 90);
        assert_eq!(panel.geometry.width, 1350);
    }

    #[test]
    fn runtime_scale_setting_resyncs_panel_thickness() {
        use flamewm_api::settings::{SettingValue, SettingsChange, SettingsTransaction};

        let path = std::env::temp_dir().join(format!(
            "flamewm-host-runtime-panel-scale-test-{}.conf",
            std::process::id()
        ));
        std::fs::write(&path, "").expect("write settings fixture");
        let mut host = PlatformHost::new(
            FakeEngine {
                display: fake_display(100),
                reverted: Vec::new(),
            },
            path.clone(),
        );
        host.start().expect("host starts");
        assert_eq!(host.panels.snapshot().panels[0].geometry.height, 44);

        let revision = host.settings_snapshot().revision;
        let applied = host.apply_settings(&SettingsTransaction {
            expected_revision: revision,
            changes: vec![SettingsChange {
                key: "scale.default".to_owned(),
                value: Some(SettingValue::Integer(125)),
            }],
            reset_section: None,
        });
        let _ = std::fs::remove_file(&path);
        applied.expect("valid runtime scale applies");

        let panels = host.panels.snapshot();
        assert_eq!(panels.panels[0].logical_size, 44);
        assert_eq!(panels.panels[0].geometry.height, 55);
    }

    #[test]
    fn runtime_scale_setting_updates_display_snapshot_and_rejects_invalid_values() {
        use flamewm_api::settings::{SettingValue, SettingsChange, SettingsTransaction};

        let path = std::env::temp_dir().join(format!(
            "flamewm-host-runtime-scale-test-{}.conf",
            std::process::id()
        ));
        std::fs::write(&path, "").expect("write settings fixture");
        let mut host = PlatformHost::new(
            FakeEngine {
                display: fake_display(100),
                reverted: Vec::new(),
            },
            path.clone(),
        );
        host.start().expect("host starts");
        let scale_of = |host: &PlatformHost<FakeEngine>| {
            host.displays
                .cached()
                .and_then(|snapshot| snapshot.outputs.first())
                .map(|output| output.shell_scale_percent)
        };
        let transaction = |revision: u64, scale: i64| SettingsTransaction {
            expected_revision: revision,
            changes: vec![SettingsChange {
                key: "scale.default".to_owned(),
                value: Some(SettingValue::Integer(scale)),
            }],
            reset_section: None,
        };

        let revision = host.settings_snapshot().revision;
        let applied = host.apply_settings(&transaction(revision, 125));
        assert!(applied.is_ok(), "valid scale applies: {applied:?}");
        assert_eq!(scale_of(&host), Some(125));

        let revision = host.settings_snapshot().revision;
        let rejected = host.apply_settings(&transaction(revision, 133));
        let _ = std::fs::remove_file(&path);
        assert_eq!(
            rejected.expect_err("invalid scale is rejected").code,
            ErrorCode::InvalidArgument
        );
        assert_eq!(scale_of(&host), Some(125));
    }

    #[test]
    fn set_shell_scale_resyncs_panel_thickness() {
        let path = std::env::temp_dir().join(format!(
            "flamewm-host-set-shell-scale-panel-test-{}.conf",
            std::process::id()
        ));
        std::fs::write(&path, "").expect("write settings fixture");
        let mut host = PlatformHost::new(
            FakeEngine {
                display: fake_display(100),
                reverted: Vec::new(),
            },
            path.clone(),
        );
        host.start().expect("host starts");
        assert_eq!(host.panels.snapshot().panels[0].geometry.height, 44);

        let generation = host
            .display_snapshot()
            .expect("display snapshot")
            .generation;
        let changed = host
            .set_shell_scale(&OutputId::new("default"), 125, generation)
            .expect("shell scale applies");
        let _ = std::fs::remove_file(&path);
        assert!(changed);

        // 44 logical px at 125% rounds to 55 physical px via logical_to_physical.
        let panels = host.panels.snapshot();
        assert_eq!(panels.panels[0].logical_size, 44);
        assert_eq!(panels.panels[0].geometry.height, 55);
    }

    #[test]
    fn set_shell_scale_persists_scale_across_restart() {
        let path = std::env::temp_dir().join(format!(
            "flamewm-host-set-shell-scale-persist-test-{}.conf",
            std::process::id()
        ));
        std::fs::write(&path, "").expect("write settings fixture");
        let mut host = PlatformHost::new(
            FakeEngine {
                display: fake_display(100),
                reverted: Vec::new(),
            },
            path.clone(),
        );
        host.start().expect("host starts");
        let revision_before = host.settings_snapshot().revision;
        let generation = host
            .display_snapshot()
            .expect("display snapshot")
            .generation;
        let changed = host
            .set_shell_scale(&OutputId::new("default"), 125, generation)
            .expect("shell scale applies");
        assert!(changed);
        assert_eq!(
            host.settings_snapshot().revision,
            revision_before,
            "scale persistence must not make Settings clients stale"
        );
        drop(host);

        // A WM restart re-seeds the display snapshot from the settings file, as at first start.
        let mut restarted = PlatformHost::new(
            FakeEngine {
                display: fake_display(100),
                reverted: Vec::new(),
            },
            path.clone(),
        );
        let started = restarted.start();
        let persisted = std::fs::read_to_string(&path);
        let _ = std::fs::remove_file(&path);
        started.expect("host restarts");
        let scale = restarted.cached_display_snapshot().and_then(|snapshot| {
            snapshot
                .outputs
                .first()
                .map(|output| output.shell_scale_percent)
        });
        assert_eq!(scale, Some(125));
        let persisted = persisted.expect("settings file is written");
        assert!(persisted.contains("scale.default=125\n"), "{persisted}");
    }

    #[test]
    fn failed_shell_scale_persistence_restores_display_scale() {
        // The settings parent becomes a regular file after start, so persistence cannot create it.
        let dir = hotkey_fixture("scale-rollback-dir");
        std::fs::create_dir_all(&dir).expect("create settings dir");
        let path = dir.join("settings.conf");
        let mut host = PlatformHost::new(
            FakeEngine {
                display: fake_display(100),
                reverted: Vec::new(),
            },
            path.clone(),
        );
        host.start().expect("host starts");
        std::fs::remove_dir_all(&dir).expect("remove settings dir");
        std::fs::write(&dir, "").expect("replace settings dir with a file");

        let generation = host
            .display_snapshot()
            .expect("display snapshot")
            .generation;
        let error = host
            .set_shell_scale(&OutputId::new("default"), 125, generation)
            .expect_err("persistence failure is surfaced");
        assert_ne!(error.code, ErrorCode::StaleRevision);
        let scale = host.cached_display_snapshot().and_then(|snapshot| {
            snapshot
                .outputs
                .first()
                .map(|output| output.shell_scale_percent)
        });
        assert_eq!(scale, Some(100));
        let _ = std::fs::remove_file(&dir);
    }

    fn hotkey_fixture(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "flamewm-host-hotkey-{name}-{}.conf",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    /// Starts a host on `path` and grabs its effective shortcut set, as a WM restart would.
    fn started_hotkey_host(path: &std::path::Path) -> PlatformHost<FakeEngine> {
        let mut host = PlatformHost::new(
            FakeEngine {
                display: fake_display(100),
                reverted: Vec::new(),
            },
            path.to_path_buf(),
        );
        host.start().expect("host starts");
        host.commit_startup_shortcuts().expect("startup grabs");
        host
    }

    // CONTRACT-REGRESSION: a Panels.SetEdge value persists across a WM restart (docs/TODO.md taskbar
    // edge persistence) without moving the settings revision, so open Settings clients stay current.
    // TRIGGER: set the top edge, drop the host, start a fresh host from the same settings file.
    // OBSERVABLE: the reloaded panel reports Top and the settings file holds panel.edge.default=top.
    // MUTATION: skip persist_panel_edge or the startup seed and the reloaded panel reports Bottom.
    #[test]
    fn set_panel_edge_persists_edge_across_restart_without_settings_revision_bump() {
        let path = std::env::temp_dir().join(format!(
            "flamewm-host-panel-edge-persist-test-{}.conf",
            std::process::id()
        ));
        std::fs::write(&path, "").expect("write settings fixture");
        let engine = || FakeEngine {
            display: fake_display(100),
            reverted: Vec::new(),
        };
        let mut host = PlatformHost::new(engine(), path.clone());
        host.start().expect("host starts");
        let settings_revision = host.settings_snapshot().revision;
        let panels_revision = host.panels_snapshot().revision;
        host.set_panel_edge(&OutputId::new("default"), PanelEdge::Top, panels_revision)
            .expect("edge applies");
        assert_eq!(host.settings_snapshot().revision, settings_revision);
        drop(host);

        let persisted = std::fs::read_to_string(&path).expect("edge is persisted");
        assert!(persisted.contains("panel.edge.default=top\n"), "{persisted}");
        let mut restarted = PlatformHost::new(engine(), path.clone());
        restarted.start().expect("restart starts");
        assert_eq!(restarted.panels_snapshot().panels[0].edge, PanelEdge::Top);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn cleared_hotkey_persists_and_stays_unassigned_after_restart() {
        use flamewm_api::shortcuts::ACTION_TOGGLE_START_MENU;

        let path = hotkey_fixture("clear");
        let mut host = started_hotkey_host(&path);
        let revision = host.shortcut_snapshot().revision;
        host.clear_shortcut(ACTION_TOGGLE_START_MENU, revision)
            .expect("clear applies");
        // Idempotent: clearing an already-unassigned action succeeds without a new revision.
        host.clear_shortcut(ACTION_TOGGLE_START_MENU, revision + 1)
            .expect("repeated clear applies");
        let persisted = std::fs::read_to_string(&path).expect("clear is persisted");
        assert!(
            persisted.contains("hotkey.ToggleStartMenu=\n"),
            "{persisted}"
        );

        let reloaded = started_hotkey_host(&path);
        assert_eq!(
            reloaded
                .shortcut_snapshot()
                .bindings
                .get(ACTION_TOGGLE_START_MENU),
            Some(&KeyBinding::unassigned())
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn set_shortcut_persists_binding_across_restart() {
        use flamewm_api::shortcuts::ACTION_WINDOW_CLOSE;

        let path = hotkey_fixture("set");
        let mut host = started_hotkey_host(&path);
        let revision = host.shortcut_snapshot().revision;
        host.set_shortcut(ACTION_WINDOW_CLOSE, "Super+F8", revision)
            .expect("set applies");
        let persisted = std::fs::read_to_string(&path).expect("set is persisted");
        assert!(
            persisted.contains("hotkey.WindowClose=Super+F8\n"),
            "{persisted}"
        );

        let reloaded = started_hotkey_host(&path);
        assert_eq!(
            reloaded
                .shortcut_snapshot()
                .bindings
                .get(ACTION_WINDOW_CLOSE),
            Some(&KeyBinding::assigned("Super+F8"))
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn reset_shortcut_removes_persisted_override_and_restores_default() {
        use flamewm_api::shortcuts::ACTION_WINDOW_CLOSE;

        let path = hotkey_fixture("reset");
        std::fs::write(&path, "hotkey.WindowClose=Super+F8\n").expect("write override fixture");
        let mut host = started_hotkey_host(&path);
        assert_eq!(
            host.shortcut_snapshot().bindings.get(ACTION_WINDOW_CLOSE),
            Some(&KeyBinding::assigned("Super+F8"))
        );
        let revision = host.shortcut_snapshot().revision;
        host.reset_shortcut(ACTION_WINDOW_CLOSE, revision)
            .expect("reset applies");
        let persisted = std::fs::read_to_string(&path).expect("reset is persisted");
        assert!(!persisted.contains("hotkey.WindowClose="), "{persisted}");

        let reloaded = started_hotkey_host(&path);
        let defaults = ShortcutRegistry::default();
        assert_eq!(
            reloaded
                .shortcut_snapshot()
                .bindings
                .get(ACTION_WINDOW_CLOSE),
            defaults.binding(ACTION_WINDOW_CLOSE)
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn stale_hotkey_revision_is_refused_without_persisting() {
        use flamewm_api::shortcuts::ACTION_TOGGLE_START_MENU;

        let path = hotkey_fixture("stale");
        let mut host = started_hotkey_host(&path);
        let stale = host.shortcut_snapshot().revision + 1;
        let error = host
            .clear_shortcut(ACTION_TOGGLE_START_MENU, stale)
            .expect_err("stale revision is refused");
        assert_eq!(error.code, ErrorCode::StaleRevision);
        assert!(!path.exists());
        assert!(host.shortcut_snapshot().bindings[ACTION_TOGGLE_START_MENU].is_assigned());
    }

    #[test]
    fn failed_hotkey_persistence_rolls_back_the_shortcut_mutation() {
        use flamewm_api::shortcuts::ACTION_TOGGLE_START_MENU;

        // The settings parent becomes a regular file after start, so persistence cannot create it.
        let dir = hotkey_fixture("rollback-dir");
        std::fs::create_dir_all(&dir).expect("create settings dir");
        let path = dir.join("settings.conf");
        let mut host = started_hotkey_host(&path);
        let before = host.shortcut_snapshot();
        std::fs::remove_dir_all(&dir).expect("remove settings dir");
        std::fs::write(&dir, "").expect("replace settings dir with a file");

        let error = host
            .clear_shortcut(ACTION_TOGGLE_START_MENU, before.revision)
            .expect_err("persistence failure is surfaced");
        assert_ne!(error.code, ErrorCode::StaleRevision);
        assert_eq!(host.shortcut_snapshot(), before);
        let _ = std::fs::remove_file(&dir);
    }
}

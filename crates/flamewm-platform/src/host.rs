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
    FeatureAction, FullscreenMonitorSpan, InteractiveMoveResize, RestackMode, WindowFeature,
    WindowFeatureSnapshot,
};
use flamewm_api::workspace::WorkspaceSnapshot;
use flamewm_api::{
    DesktopAppId, ErrorCode, FlameError, FlameResult, ModeId, OutputId, PanelEdge, TaskEntryId,
    TransactionId, WindowRef,
};

use crate::services::applications::ApplicationService;
use crate::services::background::BackgroundService;
use crate::services::display::DisplayService;
use crate::services::panels::PanelService;
use crate::services::session::SessionService;
use crate::services::settings::SettingsService;
use crate::services::shortcuts::ShortcutService;
use crate::services::windows::WindowService;
use crate::services::workspaces::{NavigationDirection, WorkspaceService};
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
        let displays = self.displays.refresh(&mut self.engine)?.clone();
        self.panels.sync_outputs(&displays);
        self.panels
            .reconcile_taskbar_height(self.settings.current().taskbar_height);
        self.started = true;
        self.generation = self.generation.saturating_add(1).max(1);
        Ok(())
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

    pub fn navigate_workspace(
        &mut self,
        direction: NavigationDirection,
        expected_revision: u64,
    ) -> FlameResult<()> {
        self.workspaces
            .navigate(&mut self.engine, direction, expected_revision)
    }

    pub fn move_window_to_workspace(
        &mut self,
        window: WindowRef,
        target: usize,
    ) -> FlameResult<()> {
        self.workspaces
            .move_window(&mut self.engine, window, target)
    }

    pub fn window_feature_snapshot(&self, window: WindowRef) -> FlameResult<WindowFeatureSnapshot> {
        self.windows.feature_snapshot(&self.engine, window)
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

    pub fn restack_window(
        &mut self,
        window: WindowRef,
        mode: RestackMode,
        sibling: Option<WindowRef>,
    ) -> FlameResult<()> {
        self.windows
            .restack(&mut self.engine, window, mode, sibling)
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

    pub fn keep_display_mode(&mut self, transaction: TransactionId) -> FlameResult<()> {
        self.displays.keep(&mut self.engine, transaction)
    }

    pub fn revert_display_mode(&mut self, transaction: TransactionId) -> FlameResult<()> {
        self.displays.revert(&mut self.engine, transaction)
    }

    pub fn tick_display_transactions(&mut self, now_ms: u64) -> FlameResult<bool> {
        self.displays.tick(&mut self.engine, now_ms)
    }

    pub fn set_shell_scale(
        &mut self,
        output: &OutputId,
        percent: u16,
        expected_generation: u64,
    ) -> FlameResult<bool> {
        self.displays
            .set_shell_scale(output, percent, expected_generation)
    }

    #[must_use]
    pub fn settings_snapshot(&self) -> SettingsSnapshot {
        self.settings.snapshot()
    }

    pub fn apply_settings(
        &mut self,
        transaction: &SettingsTransaction,
    ) -> FlameResult<SettingsSnapshot> {
        let snapshot = self.settings.apply(&mut self.engine, transaction)?;
        if let Some(flamewm_api::settings::SettingValue::Integer(logical_size)) =
            snapshot.values.get("taskbarHeight")
        {
            let logical_size = (*logical_size).try_into().map_err(|_| {
                FlameError::new(ErrorCode::InvalidArgument, "taskbarHeight is out of range")
            })?;
            self.panels.reconcile_taskbar_height(logical_size);
        }
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
        self.shortcuts
            .set_binding(&mut self.engine, action, binding, expected_revision)
    }

    pub fn clear_shortcut(&mut self, action: &str, expected_revision: u64) -> FlameResult<()> {
        self.shortcuts
            .clear_binding(&mut self.engine, action, expected_revision)
    }

    pub fn reset_shortcut(&mut self, action: &str, expected_revision: u64) -> FlameResult<()> {
        self.shortcuts
            .reset_binding(&mut self.engine, action, expected_revision)
    }

    pub fn apply_shortcuts(
        &mut self,
        bindings: &std::collections::BTreeMap<String, KeyBinding>,
        expected_revision: u64,
    ) -> FlameResult<()> {
        self.shortcuts
            .apply(&mut self.engine, bindings, expected_revision)
    }

    #[must_use]
    pub fn panels_snapshot(&self) -> PanelsSnapshot {
        self.panels.snapshot()
    }

    pub fn set_panel_edge(
        &mut self,
        output: &OutputId,
        edge: PanelEdge,
        expected_revision: u64,
    ) -> FlameResult<()> {
        self.panels.set_edge(output, edge, expected_revision)
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

    pub fn reorder_task(
        &mut self,
        from: usize,
        to: usize,
        expected_revision: u64,
    ) -> FlameResult<bool> {
        self.panels.reorder(from, to, expected_revision)
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

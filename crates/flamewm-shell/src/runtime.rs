use std::collections::VecDeque;
use std::rc::Rc;

pub mod launcher_surface;
pub mod start_surface;
pub mod status_surface;
use crate::popup_controller::POPUP_GAP;
use crate::start::{self, StartCategory};
use flamewm_api::applications::{ApplicationLaunchOptions, DesktopApplication};
use flamewm_api::display::DisplaySnapshot;
use flamewm_api::panels::PanelsSnapshot;
use flamewm_api::session::{SessionAction, SessionCapabilities};
use flamewm_api::system::{SystemAction, SystemSnapshot};
use flamewm_api::window::WindowSnapshot;
use flamewm_api::workspace::WorkspaceSnapshot;
use flamewm_api::PanelEdge;
use flamewm_control_core::{ControlRequest, ControlResponse};
use flamewm_control_dbus::ControlClient;
use flamewm_control_wire::ControlSignal;
use flamewm_platform::panel::{OutputPanel, TaskbarFrame};
use flamewm_shell_core::popup::{
    anchor_popover_rect, context_menu_rect, context_menu_size, panel_anchor, work_area_for_panel,
    PopupRefusal,
};
use flamewm_shell_core::start_surface_layout;
use flamewm_ui_core::style::ShellMetrics;
use flamewm_ui_x11::backdrop::DesktopSource;
use flamewm_ui_x11::{
    decode_document, DockEdge, GeometryTrace, SlideConfig, SurfaceConfig, SurfaceHandle,
    SurfaceInputMode, SurfaceRole, SurfaceRuntime, UiBackendError, UiDocumentAccess, UiTemplate,
};

const PANEL_ARTIFACT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-panel.rwr"));
const START_ARTIFACT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-start.rwr"));
const TASK_MENU_ARTIFACT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-task-menu.rwr"));
const MEDIA_ARTIFACT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-media.rwr"));
const LAUNCHER_ARTIFACT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-launcher.rwr"));
// J07: audio/network/calendar artifacts live in the helper role
// (`quick_controls::host`), not in the parent runtime.

/// Complete renderer input. No UI component owns a competing copy of this state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellSnapshot {
    pub displays: DisplaySnapshot,
    pub panels: PanelsSnapshot,
    pub windows: Vec<WindowSnapshot>,
    pub applications: Vec<DesktopApplication>,
    pub workspaces: Option<WorkspaceSnapshot>,
    pub session: SessionCapabilities,
    pub system: SystemSnapshot,
}

#[cfg(test)]
impl Default for ShellSnapshot {
    fn default() -> Self {
        Self {
            displays: DisplaySnapshot {
                generation: 0,
                outputs: Vec::new(),
                pending: None,
            },
            panels: PanelsSnapshot {
                revision: 0,
                panels: Vec::new(),
                tasks: Vec::new(),
                pinned_apps: Vec::new(),
            },
            windows: Vec::new(),
            applications: Vec::new(),
            workspaces: None,
            session: SessionCapabilities::default(),
            system: SystemSnapshot {
                revision: 0,
                network: Default::default(),
                media: Default::default(),
                audio: Default::default(),
                power: Default::default(),
            },
        }
    }
}

impl ShellSnapshot {
    /// Single-roundtrip startup: exactly one `GetShellBootstrap` call. The
    /// aggregate carries every domain the panel/start surfaces need;
    /// steady-state updates arrive via `ControlSignal` delivery, never by
    /// re-calling this loader.
    pub fn load(client: &ControlClient) -> Result<Self, String> {
        let bootstrap = match client.call(&ControlRequest::GetShellBootstrap) {
            Ok(ControlResponse::ShellBootstrap(snapshot)) => snapshot,
            Ok(response) => return Err(format!("GetShellBootstrap returned {response:?}")),
            Err(error) => return Err(format!("GetShellBootstrap failed: {}", error.message)),
        };

        Ok(Self {
            displays: bootstrap.displays,
            panels: bootstrap.panels,
            windows: bootstrap.windows,
            applications: bootstrap.applications,
            workspaces: Some(bootstrap.workspaces),
            session: bootstrap.session,
            system: bootstrap.system,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractionContext {
    pub source_surface: SurfaceHandle,
    pub output: flamewm_api::OutputId,
    pub display_generation: u64,
    pub panels_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellControl {
    ToggleStart {
        output: Option<flamewm_api::OutputId>,
        interaction: Option<InteractionContext>,
    },
    /// Top-center Launcher; `output: None` targets the panel output the same way `ToggleStart` does.
    ToggleLauncher {
        output: Option<flamewm_api::OutputId>,
        interaction: Option<InteractionContext>,
    },
    CloseStart,
    SelectStartCategory {
        category: StartCategory,
        output: Option<flamewm_api::OutputId>,
        interaction: Option<InteractionContext>,
    },
    Launch(String),
    ActivateWindow(flamewm_api::WindowRef),
    MinimizeWindow(flamewm_api::WindowRef),
    RestoreWindow(flamewm_api::WindowRef),
    SystemAction {
        action: SystemAction,
        expected_revision: u64,
    },
    SessionAction(SessionAction),
    OpenPopover {
        name: String,
        output: Option<flamewm_api::OutputId>,
        interaction: Option<InteractionContext>,
    },
    ClosePopovers,
    OpenContextMenu(ContextMenuState),
    CloseContextMenu,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextMenuKind {
    Task(flamewm_api::TaskEntryId),
    Workspace {
        index: usize,
    },
    /// Taskbar background (Secondary on `taskbar.surface`); one settings row.
    Taskbar,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextMenuState {
    pub kind: ContextMenuKind,
    pub revision: u64,
    pub output: Option<flamewm_api::OutputId>,
    pub interaction: Option<InteractionContext>,
    /// Root-coordinate pointer anchor (`ActionEvent` local x/y resolved
    /// against the source surface global rect). `None` only when pointer
    /// data was absent; that is the sole output-origin fallback case.
    pub anchor: Option<flamewm_api::Point>,
}

/// Identity rendered into one task slot. Slot positions can change after a
/// snapshot refresh, so pointer releases validate this token before acting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSlotTarget {
    pub id: flamewm_api::TaskEntryId,
    pub app_id: flamewm_api::DesktopAppId,
    pub window: Option<flamewm_api::WindowRef>,
    pub output: flamewm_api::OutputId,
}

#[derive(Debug, Clone)]
pub struct ShellRuntime {
    snapshot: Rc<ShellSnapshot>,
    selected_panel_output: Option<flamewm_api::OutputId>,
    controls: VecDeque<ShellControl>,
    dirty: ShellDirty,
    /// Latest Alt+Tab switcher snapshot not yet taken by the shell loop.
    switcher: Option<crate::switcher::SwitcherFrame>,
}

/// Domain dirty flags set by `ControlSignal` delivery and reconciled at most
/// once per turn. Each flag preserves single ownership: windows, workspaces,
/// panels, system, and applications segments map to one fetch each.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ShellDirty {
    pub windows: bool,
    pub workspaces: bool,
    pub panels: bool,
    pub system: bool,
    pub applications: bool,
}

impl ShellDirty {
    #[must_use]
    pub fn any(self) -> bool {
        self.windows || self.workspaces || self.panels || self.system || self.applications
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Per-domain change set from `reconcile_dirty`. The shell loop projects
/// only the surfaces each changed domain owns; unchanged domains (and
/// hidden surfaces) are never reprojected.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChangedDomains {
    pub windows: bool,
    pub workspaces: bool,
    pub panels: bool,
    pub system: bool,
    pub applications: bool,
}

impl ChangedDomains {
    #[must_use]
    pub fn any(self) -> bool {
        self.windows || self.workspaces || self.panels || self.system || self.applications
    }
}

#[cfg(test)]
impl Default for ShellRuntime {
    fn default() -> Self {
        Self::new(ShellSnapshot::default())
    }
}

impl ShellRuntime {
    #[must_use]
    pub fn new(snapshot: ShellSnapshot) -> Self {
        Self {
            snapshot: Rc::new(snapshot),
            selected_panel_output: None,
            controls: VecDeque::new(),
            dirty: ShellDirty::default(),
            switcher: None,
        }
    }

    #[must_use]
    pub fn dirty(&self) -> ShellDirty {
        self.dirty
    }

    /// Record a delivered `ControlSignal` by applying snapshot payloads
    /// directly. No fetch happens here and legacy revision-only signals
    /// (`WindowsChanged` / `WorkspacesChanged` / `PanelsChanged` /
    /// `SystemChanged`) are ignored for normal refresh; the shell loop
    /// projects from the already-applied snapshot once per turn.
    pub fn mark_signal(&mut self, signal: &ControlSignal) {
        match signal {
            ControlSignal::WindowsSnapshotChanged { windows, .. } => {
                let _s = crate::runtime::shell_span("shell.signal_to_visual.windows").start();
                if *windows != self.snapshot.windows {
                    Rc::make_mut(&mut self.snapshot).windows.clone_from(windows);
                    self.dirty.windows = true;
                }
            }
            ControlSignal::WorkspacesSnapshotChanged { snapshot } => {
                let _s = crate::runtime::shell_span("shell.signal_to_visual.workspaces").start();
                if self.snapshot.workspaces.as_ref() != Some(snapshot) {
                    Rc::make_mut(&mut self.snapshot).workspaces = Some(snapshot.clone());
                    self.dirty.workspaces = true;
                }
            }
            ControlSignal::PanelsSnapshotChanged { snapshot } => {
                if *snapshot != self.snapshot.panels {
                    Rc::make_mut(&mut self.snapshot).panels = snapshot.clone();
                    self.dirty.panels = true;
                }
            }
            ControlSignal::DisplaysSnapshotChanged { snapshot } => {
                // Output shell scale lives only in the display snapshot. Refreshing it here lets
                // resync_panel_geometry see the scale drift; panels are marked dirty so the
                // frame loop runs the panel projection path this turn.
                if *snapshot != self.snapshot.displays {
                    Rc::make_mut(&mut self.snapshot).displays = snapshot.clone();
                    self.dirty.panels = true;
                }
            }
            ControlSignal::SystemSnapshotChanged { snapshot } => {
                let _s = crate::runtime::shell_span("shell.signal_to_visual.system").start();
                if snapshot != &self.snapshot.system {
                    Rc::make_mut(&mut self.snapshot).system = snapshot.clone();
                    self.dirty.system = true;
                }
            }
            ControlSignal::SystemChanged { .. } => {}
            // Same path as the `start.toggle` dispatch: one queued ToggleStart control.
            ControlSignal::StartMenuToggleRequested => {
                self.controls.push_back(ShellControl::ToggleStart {
                    output: None,
                    interaction: None,
                });
            }
            // Same shape as the Start toggle: one queued control per request.
            ControlSignal::LauncherToggleRequested => {
                self.controls.push_back(ShellControl::ToggleLauncher {
                    output: None,
                    interaction: None,
                });
            }
            // Latest snapshot wins; the overlay renders it and never decides the switch.
            ControlSignal::WindowSwitcherChanged {
                open,
                immediate,
                selected,
                entries,
            } => {
                self.switcher = Some(crate::switcher::SwitcherFrame {
                    open: *open,
                    immediate: *immediate,
                    selected: usize::try_from(*selected).unwrap_or(0),
                    entries: entries.clone(),
                });
            }
            _ => {}
        }
    }

    /// Reconcile already-applied snapshot domains into a change set without
    /// any fetch. Every steady-state domain is supplied by a snapshot signal;
    /// this method only consumes dirty flags and never owns a control client.
    pub fn reconcile_dirty(&mut self) -> ChangedDomains {
        let take = |flag: &mut bool, kind: DomainKind| {
            let set = std::mem::take(flag);
            if set {
                domain_reconcile_counter(kind).increment();
            }
            set
        };
        let changed = ChangedDomains {
            windows: take(&mut self.dirty.windows, DomainKind::Windows),
            workspaces: take(&mut self.dirty.workspaces, DomainKind::Workspaces),
            panels: take(&mut self.dirty.panels, DomainKind::Panels),
            system: take(&mut self.dirty.system, DomainKind::System),
            applications: false,
        };
        // No application signal is owned by this reconciliation path; keep
        // the flag consumed if a producer marks it before a future snapshot
        // contract is added.
        self.dirty.applications = false;
        changed
    }

    /// Compatibility name for callers that only consume applied snapshots.
    pub fn reconcile_applied(&mut self) -> ChangedDomains {
        self.reconcile_dirty()
    }

    #[must_use]
    pub fn snapshot(&self) -> &ShellSnapshot {
        &self.snapshot
    }

    /// Cheap shared handle to the current snapshot (refcount bump, no copy).
    /// Writers use `Rc::make_mut`, so a held handle keeps its original value.
    #[must_use]
    pub fn snapshot_rc(&self) -> Rc<ShellSnapshot> {
        Rc::clone(&self.snapshot)
    }

    /// Select the panel output that originated the current taskbar input.
    /// Stale outputs are ignored; callers must resolve global actions separately.
    pub fn select_panel_output(&mut self, output: flamewm_api::OutputId) {
        if self
            .snapshot
            .panels
            .panels
            .iter()
            .any(|panel| panel.output == output)
        {
            self.selected_panel_output = Some(output);
        }
    }

    fn selected_output(&self) -> Option<flamewm_api::OutputId> {
        self.shown_panel_output().cloned()
    }

    /// Keyboard Start fallback when no root pointer output is available.
    #[must_use]
    pub fn focused_panel_output(&self) -> Option<flamewm_api::OutputId> {
        self.snapshot
            .windows
            .iter()
            .find(|window| window.focused)
            .map(|window| window.output.clone())
            .filter(|output| {
                self.snapshot
                    .panels
                    .panels
                    .iter()
                    .any(|panel| &panel.output == output)
            })
    }

    /// Maps a pager cell action (`workspace-{k}` / `workspace.{k}`) to the
    /// activation control for the workspace the live snapshot places in cell
    /// `k`. Cells are column-major and count-dependent, so an empty cell (or a
    /// missing snapshot) is a no-op instead of a nearby workspace.
    #[must_use]
    pub fn control_request_for_action(&self, action: &str) -> Option<ControlRequest> {
        let cell = action
            .strip_prefix("workspace-")
            .or_else(|| action.strip_prefix("workspace."))
            .and_then(|value| value.parse::<usize>().ok())?;
        let snapshot = self.snapshot.workspaces.as_ref()?;
        let index = crate::taskbar::workspaces::cell_index(cell, snapshot.count)?;
        crate::taskbar::workspaces::control_request_for_click(Some(snapshot), index)
    }

    /// Map a stable context-menu row action (`task.menu.<row>` /
    /// `workspace.menu.<row>`) plus the open menu state to a typed control.
    /// Row identity is the row id, never the label. Pinned task entries
    /// resolve `unpin`; running entries resolve `activate`/`close`/`pin`.
    /// Workspace rows are revision-fenced against the
    /// open menu.
    #[must_use]
    pub fn context_menu_request_for_action(
        &self,
        action: &str,
        menu: Option<&ContextMenuState>,
    ) -> Option<ControlRequest> {
        let menu = menu?;
        match &menu.kind {
            ContextMenuKind::Task(id) => {
                if self.snapshot.panels.revision != menu.revision {
                    return None;
                }
                let row = action.strip_prefix("task.menu.")?;
                let entry = self
                    .snapshot
                    .panels
                    .tasks
                    .iter()
                    .find(|entry| &entry.id == id)?;
                if entry.id.is_pinned() {
                    if row != crate::taskbar::context_menu::TASK_ROW_UNPIN {
                        return None;
                    }
                    return Some(ControlRequest::UnpinApp {
                        app: entry.app_id.clone(),
                        expected_revision: self.snapshot.panels.revision,
                    });
                }
                let window = match entry.kind {
                    flamewm_api::panels::TaskEntryKind::PinnedSlot { window } => window,
                    flamewm_api::panels::TaskEntryKind::Window { window } => Some(window),
                }?;
                if !self
                    .snapshot
                    .windows
                    .iter()
                    .any(|snapshot| snapshot.reference == window)
                {
                    return None;
                }
                match row {
                    crate::taskbar::context_menu::TASK_ROW_ACTIVATE => {
                        Some(ControlRequest::ActivateWindow(window))
                    }
                    crate::taskbar::context_menu::TASK_ROW_CLOSE => {
                        Some(ControlRequest::CloseWindow(window))
                    }
                    crate::taskbar::context_menu::TASK_ROW_PIN => Some(ControlRequest::PinApp {
                        app: entry.app_id.clone(),
                        expected_revision: self.snapshot.panels.revision,
                    }),
                    _ => None,
                }
            }
            ContextMenuKind::Taskbar => None,
            ContextMenuKind::Workspace { index } => {
                let row = action.strip_prefix("workspace.menu.")?;
                let snapshot = self.snapshot.workspaces.as_ref()?;
                if snapshot.revision != menu.revision {
                    return None;
                }
                crate::taskbar::context_menu::workspace_action(snapshot, *index, row)
                    .map(super::taskbar::context_menu::ContextAction::into_control_request)
            }
        }
    }

    /// Typed `SystemAction` control for audio rows from a provider snapshot.
    /// The provider owns volumes/mutes; the shell only forwards the id-keyed
    /// action. Rejects out-of-range percents the same way the provider does.
    #[must_use]
    pub fn audio_system_action_for_row(
        &self,
        row_id: &str,
        percent: Option<u8>,
        muted: Option<bool>,
    ) -> Option<ControlRequest> {
        crate::taskbar::status::audio::system_action_for_row(
            &self.snapshot.system,
            row_id,
            percent,
            muted,
        )
    }

    /// Typed `SystemAction` control for network rows from a provider snapshot.
    /// Secrets travel via the `SecretAgent` snapshot only; this never carries
    /// a secret value.
    #[must_use]
    pub fn network_system_action_for_path(
        &self,
        path: &str,
        scan: bool,
        disconnect: bool,
    ) -> Option<ControlRequest> {
        crate::taskbar::status::network::system_action_for_path(
            &self.snapshot.system,
            path,
            scan,
            disconnect,
        )
    }

    /// Reorder request moving the task entry at `from` to final index `to`.
    /// Slot numbers are local to the selected panel output.
    fn task_visual_states(&self) -> Vec<flamewm_shell_core::TaskVisualState> {
        let Some(output) = self.shown_panel_output() else {
            return Vec::new();
        };
        self.task_states_on(output)
    }

    /// Task slots of one output on the active workspace.
    fn task_states_on(
        &self,
        output: &flamewm_api::OutputId,
    ) -> Vec<flamewm_shell_core::TaskVisualState> {
        flamewm_shell_core::task_visual_states_on_output_and_workspace(
            &self.snapshot.panels,
            &self.snapshot.windows,
            &self.snapshot.applications,
            Some(output),
            self.snapshot
                .workspaces
                .as_ref()
                .map(|workspaces| workspaces.active_index),
        )
    }

    #[must_use]
    pub fn task_slot_count(&self) -> usize {
        self.task_visual_states().len()
    }

    /// `from`/`to` are task-slot indices on the shown output; the request carries the
    /// full task-list index of the entry now occupying `to`.
    #[must_use]
    pub fn task_reorder_request(&self, from: usize, to: usize) -> Option<ControlRequest> {
        self.shown_panel_output()?;
        let states = self.task_visual_states();
        let entry_id = states.get(from)?.id.clone();
        let target = &states.get(to)?.id;
        let index = self
            .snapshot
            .panels
            .tasks
            .iter()
            .position(|entry| &entry.id == target)?;
        Some(ControlRequest::ReorderTask {
            entry_id,
            index,
            expected_revision: self.snapshot.panels.revision,
        })
    }

    /// Reorder from a press-captured task identity, rejecting a stale drag
    /// and resolving the destination slot against the current output list.
    #[must_use]
    pub fn task_reorder_request_for_target(
        &self,
        source: &TaskSlotTarget,
        to_slot: usize,
    ) -> Option<ControlRequest> {
        self.current_task_for_target(source)?;
        let states = self.task_states_on(&source.output);
        let destination = &states.get(to_slot)?.id;
        let index = self
            .snapshot
            .panels
            .tasks
            .iter()
            .position(|entry| &entry.id == destination)?;
        Some(ControlRequest::ReorderTask {
            entry_id: source.id.clone(),
            index,
            expected_revision: self.snapshot.panels.revision,
        })
    }

    /// Output of the panel whose task slots are shown; `None` on a single-panel desktop.
    fn shown_panel_output(&self) -> Option<&flamewm_api::OutputId> {
        self.selected_panel_output.as_ref().filter(|selected| {
            self.snapshot
                .panels
                .panels
                .iter()
                .any(|panel| &panel.output == *selected)
        })
    }

    #[must_use]
    pub fn task_click_requests_for_slot(&self, slot: usize) -> Option<Vec<ControlRequest>> {
        let target = self.task_target_for_slot(slot)?;
        self.task_click_requests_for_target(&target)
    }

    /// Capture the task identity at input press time, including its owning
    /// output and exact window generation.
    #[must_use]
    pub fn task_target_for_slot(&self, slot: usize) -> Option<TaskSlotTarget> {
        let output = self.shown_panel_output()?.clone();
        self.task_target_for_slot_on_output(slot, &output)
    }

    /// Capture a task target using the output resolved from the event's
    /// source surface. Pointer routing must not infer an output from ambient
    /// selection or fall back to the first panel.
    #[must_use]
    pub fn task_target_for_slot_on_output(
        &self,
        slot: usize,
        output: &flamewm_api::OutputId,
    ) -> Option<TaskSlotTarget> {
        if !self
            .snapshot
            .panels
            .panels
            .iter()
            .any(|panel| &panel.output == output)
        {
            return None;
        }
        let states = self.task_states_on(output);
        let state = states.get(slot)?;
        Some(TaskSlotTarget {
            id: state.id.clone(),
            app_id: state.app_id.clone(),
            window: state.window,
            output: output.clone(),
        })
    }

    /// Resolve a press-captured target only while the same task identity and
    /// window generation still occupy that output. A refreshed slot cannot
    /// redirect a delayed release to another application.
    #[must_use]
    pub fn task_click_requests_for_target(
        &self,
        target: &TaskSlotTarget,
    ) -> Option<Vec<ControlRequest>> {
        self.current_task_for_target(target)?;
        let entry = self
            .snapshot
            .panels
            .tasks
            .iter()
            .find(|entry| entry.id == target.id)?;
        // A pinned launcher whose live window belongs to another workspace
        // must not activate that hidden window when clicked here.
        if target.window.is_none() {
            return Some(vec![ControlRequest::LaunchApplication {
                app: target.app_id.clone(),
                options: ApplicationLaunchOptions::default(),
            }]);
        }
        Some(
            match flamewm_shell_core::task_click_action(entry, &self.snapshot.windows) {
                flamewm_shell_core::TaskClickAction::Launch(app) => {
                    vec![ControlRequest::LaunchApplication {
                        app,
                        options: ApplicationLaunchOptions::default(),
                    }]
                }
                // Restore maps the frame and focuses it, and bumps the window generation: a second
                // ActivateWindow carrying the pre-restore reference would fail with StaleRevision.
                flamewm_shell_core::TaskClickAction::RestoreAndActivate(window) => {
                    flamewm_debug::emit(
                        flamewm_debug::DebugEventId("restore.trace.shell.send"),
                        std::time::Duration::ZERO,
                        || {
                            let ms = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map_or(0, |d| d.as_millis());
                            format!("t_ms={ms} window={window:?} request=RestoreWindow")
                        },
                    );
                    vec![ControlRequest::RestoreWindow(window)]
                }
                flamewm_shell_core::TaskClickAction::Minimize(window) => {
                    vec![ControlRequest::MinimizeWindow(window)]
                }
                flamewm_shell_core::TaskClickAction::Activate(window) => {
                    vec![ControlRequest::ActivateWindow(window)]
                }
            },
        )
    }

    fn current_task_for_target(
        &self,
        target: &TaskSlotTarget,
    ) -> Option<&flamewm_api::panels::TaskEntry> {
        if !self
            .snapshot
            .panels
            .panels
            .iter()
            .any(|panel| panel.output == target.output)
        {
            return None;
        }
        let states = self.task_states_on(&target.output);
        let state = states.iter().find(|state| state.id == target.id)?;
        if state.app_id != target.app_id || state.window != target.window {
            return None;
        }
        self.snapshot
            .panels
            .tasks
            .iter()
            .find(|entry| entry.id == target.id)
    }

    /// Open a task context menu for a press-captured identity, rejecting a
    /// stale release instead of opening a menu for the task now in that slot.
    pub fn open_task_context_for_target(
        &mut self,
        target: &TaskSlotTarget,
        pointer: Option<flamewm_api::Point>,
    ) {
        self.open_task_context_for_target_scoped(target, pointer, None);
    }

    pub fn open_task_context_for_target_scoped(
        &mut self,
        target: &TaskSlotTarget,
        pointer: Option<flamewm_api::Point>,
        interaction: Option<InteractionContext>,
    ) {
        if self.current_task_for_target(target).is_none() {
            return;
        }
        self.push_context_menu(
            ContextMenuKind::Task(target.id.clone()),
            self.snapshot.panels.revision,
            pointer,
            interaction,
        );
    }

    /// Pointer-anchored variant: `pointer` is the root-coordinate click
    /// point (local x/y + source surface global rect).
    pub fn open_task_context_at_slot(&mut self, slot: usize, pointer: Option<flamewm_api::Point>) {
        let states = self.task_visual_states();
        if let Some(state) = states.get(slot) {
            let kind = ContextMenuKind::Task(state.id.clone());
            self.push_context_menu(kind, self.snapshot.panels.revision, pointer, None);
        }
    }

    fn push_context_menu(
        &mut self,
        kind: ContextMenuKind,
        revision: u64,
        anchor: Option<flamewm_api::Point>,
        interaction: Option<InteractionContext>,
    ) {
        self.controls
            .push_back(ShellControl::OpenContextMenu(ContextMenuState {
                kind,
                revision,
                output: interaction
                    .as_ref()
                    .map(|context| context.output.clone())
                    .or_else(|| self.selected_output()),
                interaction,
                anchor,
            }));
    }

    /// Pointer-anchored variant: `pointer` is the root-coordinate click
    /// point (local x/y + source surface global rect). `cell` is a pager cell
    /// id; an empty cell opens nothing.
    pub fn open_workspace_context_at_cell(
        &mut self,
        cell: usize,
        pointer: Option<flamewm_api::Point>,
    ) {
        self.push_workspace_context(cell, pointer, None);
    }

    fn push_workspace_context(
        &mut self,
        cell: usize,
        pointer: Option<flamewm_api::Point>,
        interaction: Option<InteractionContext>,
    ) {
        let Some(snapshot) = self.snapshot.workspaces.as_ref() else {
            return;
        };
        let Some(index) = crate::taskbar::workspaces::cell_index(cell, snapshot.count) else {
            return;
        };
        self.push_context_menu(
            ContextMenuKind::Workspace { index },
            snapshot.revision,
            pointer,
            interaction,
        );
    }

    /// Taskbar-background context menu, anchored at the root-coordinate click.
    pub fn open_taskbar_context_at(&mut self, pointer: Option<flamewm_api::Point>) {
        self.push_context_menu(
            ContextMenuKind::Taskbar,
            self.snapshot.panels.revision,
            pointer,
            None,
        );
    }

    pub fn open_workspace_context_at_cell_scoped(
        &mut self,
        cell: usize,
        pointer: Option<flamewm_api::Point>,
        interaction: InteractionContext,
    ) {
        self.push_workspace_context(cell, pointer, Some(interaction));
    }

    pub fn open_taskbar_context_at_scoped(
        &mut self,
        pointer: Option<flamewm_api::Point>,
        interaction: InteractionContext,
    ) {
        self.push_context_menu(
            ContextMenuKind::Taskbar,
            self.snapshot.panels.revision,
            pointer,
            Some(interaction),
        );
    }

    pub fn dispatch(&mut self, action: &str) {
        self.dispatch_with_output(action, None);
    }

    /// Dispatch an input action with its already-resolved source output.
    /// The supplied target is captured into the control before another event
    /// can change panel selection. `None` is reserved for global actions,
    /// whose output is resolved by the shell's keyboard policy.
    pub fn dispatch_with_output(&mut self, action: &str, output: Option<flamewm_api::OutputId>) {
        self.dispatch_with_interaction(action, output, None);
    }

    pub fn dispatch_with_context(&mut self, action: &str, context: InteractionContext) {
        self.dispatch_with_interaction(action, Some(context.output.clone()), Some(context));
    }

    fn dispatch_with_interaction(
        &mut self,
        action: &str,
        output: Option<flamewm_api::OutputId>,
        interaction: Option<InteractionContext>,
    ) {
        let control =
            match action {
                "start.toggle" => Some(ShellControl::ToggleStart {
                    output: output.clone(),
                    interaction: interaction.clone(),
                }),
                "start.close" => Some(ShellControl::CloseStart),
                value if value.starts_with("start.category.") => start::category_for_action(value)
                    .map(|category| ShellControl::SelectStartCategory {
                        category,
                        output: output.clone(),
                        interaction: interaction.clone(),
                    }),
                "media.open" | "media.toggle" | "volume.open" | "network.open" | "battery.open"
                | "clock.open" | "weather.open" => Some(ShellControl::OpenPopover {
                    name: action.split('.').next().unwrap_or(action).to_owned(),
                    output,
                    interaction,
                }),
                "popover.close" => Some(ShellControl::ClosePopovers),
                "session.lock" => Some(ShellControl::SessionAction(SessionAction::Lock)),
                "session.logout" => Some(ShellControl::SessionAction(SessionAction::Logout)),
                "session.suspend" => Some(ShellControl::SessionAction(SessionAction::Suspend)),
                "session.reboot" => Some(ShellControl::SessionAction(SessionAction::Reboot)),
                "session.shutdown" => Some(ShellControl::SessionAction(SessionAction::Shutdown)),
                value if value.starts_with("start.") => {
                    Some(ShellControl::Launch(value[6..].to_owned()))
                }
                _ => None,
            };
        if let Some(control) = control {
            self.controls.push_back(control);
        }
    }

    pub fn drain_controls(&mut self) -> impl Iterator<Item = ShellControl> + '_ {
        self.controls.drain(..)
    }

    /// True when a signal- or dispatch-queued control awaits the drain.
    #[must_use]
    pub fn has_pending_controls(&self) -> bool {
        !self.controls.is_empty()
    }

    /// Takes the pending Alt+Tab switcher snapshot, if one arrived since the last tick.
    pub fn take_switcher_frame(&mut self) -> Option<crate::switcher::SwitcherFrame> {
        self.switcher.take()
    }
}

/// Resolve root pointer coordinates to a connected output. Half-open bounds
/// handle adjacent monitors without assigning their shared edge twice.
#[must_use]
pub fn output_at_root_point(
    outputs: &[flamewm_api::display::OutputSnapshot],
    point: flamewm_api::Point,
) -> Option<flamewm_api::OutputId> {
    outputs
        .iter()
        .find(|output| {
            let rect = output.geometry;
            output.connected
                && point.x >= rect.x
                && point.y >= rect.y
                && i64::from(point.x) < i64::from(rect.x) + i64::from(rect.width)
                && i64::from(point.y) < i64::from(rect.y) + i64::from(rect.height)
        })
        .map(|output| output.id.clone())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Transient {
    StartGroup,
    Media,
    ContextMenu,
}

/// Resolved panel placement inputs: snapshot rects plus derived work
/// area and Start layout. Stored in `ShellSurfaces`; recomputed by
/// `sync_panel_context` on panel/display snapshot change (pure/state,
/// main.rs wiring reserved for J07).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelPlacementContext {
    pub output: flamewm_api::display::OutputSnapshot,
    pub panel: flamewm_api::panels::PanelSnapshot,
    pub work_area: flamewm_api::Rect,
    pub start_layout: flamewm_shell_core::StartSurfaceLayout,
    pub display_generation: u64,
    pub panels_revision: u64,
}

impl PanelPlacementContext {
    #[must_use]
    pub fn new(
        output: flamewm_api::display::OutputSnapshot,
        panel: flamewm_api::panels::PanelSnapshot,
        display_generation: u64,
        panels_revision: u64,
    ) -> Self {
        let work_area = work_area_for_panel(output.geometry, panel.geometry, panel.edge);
        let start_layout = start_surface_layout(
            panel.output.clone(),
            output.geometry,
            panel.geometry,
            panel.edge,
            ShellMetrics::default(),
        );
        Self {
            output,
            panel,
            work_area,
            start_layout,
            display_generation,
            panels_revision,
        }
    }
}

/// Resolve a `PanelPlacementContext` from a snapshot without touching
/// live surfaces. Returns `None` when the panel or its connected output
/// is missing, or when the snapshot edge differs from `expected_edge`
/// (mixed-edge snapshots are rejected; the stored context is kept).
#[must_use]
pub fn panel_context_for_snapshot(
    snapshot: &ShellSnapshot,
    expected_edge: flamewm_api::PanelEdge,
) -> Option<PanelPlacementContext> {
    let panel = snapshot.panels.panels.first()?;
    if panel.edge != expected_edge {
        return None;
    }
    panel_context_for_panel(snapshot, panel)
}

fn panel_context_for_panel(
    snapshot: &ShellSnapshot,
    panel: &flamewm_api::panels::PanelSnapshot,
) -> Option<PanelPlacementContext> {
    let output = snapshot
        .displays
        .outputs
        .iter()
        .find(|output| output.id == panel.output && output.connected)
        .cloned()?;
    Some(PanelPlacementContext::new(
        output,
        panel.clone(),
        snapshot.displays.generation,
        snapshot.panels.revision,
    ))
}

/// Source-of-truth match by output ID, including coordinates, mode, scale and
/// panel edge. The native surface may need a move even if both snapshot
/// generation counters are unchanged.
fn panel_context_is_current(placement: &PanelPlacementContext, snapshot: &ShellSnapshot) -> bool {
    snapshot
        .panels
        .panels
        .iter()
        .any(|panel| panel.output == placement.output.id && panel == &placement.panel)
        && snapshot.displays.outputs.iter().any(|output| {
            output.connected && output.id == placement.output.id && output == &placement.output
        })
}

/// A popup may stay open across topology revisions when its own output and
/// panel placement are unchanged. Sequence identity is preserved separately.
fn popup_context_unchanged(current: &PanelPlacementContext, next: &PanelPlacementContext) -> bool {
    current.output.id == next.output.id
        && current.output.geometry == next.output.geometry
        && current.output.shell_scale_percent == next.output.shell_scale_percent
        && current.panel == next.panel
}

/// A12-1: native context-menu rect clamped inside the PANEL-AWARE work area,
/// not the raw output geometry. A menu opened near a bottom panel must stop at
/// the panel's outer top instead of sliding under the panel strip (the output
/// rect clamp let it overlap the taskbar). `PanelPlacementContext.work_area`
/// already has the panel strip subtracted, so it is the only correct bound.
#[must_use]
pub fn context_menu_rect_for_placement(
    placement: &PanelPlacementContext,
    pointer: Option<flamewm_api::Point>,
    size: flamewm_api::Size,
) -> flamewm_api::Rect {
    context_menu_rect(
        placement.panel.output.clone(),
        placement.work_area,
        pointer,
        size,
    )
}

/// One placement context per connected panel whose edge matches
/// `expected_edge`, each tied to its own output. Single-output snapshots
/// yield the same single context as [`panel_context_for_snapshot`].
#[must_use]
pub fn panel_contexts_for_snapshot(
    snapshot: &ShellSnapshot,
    expected_edge: flamewm_api::PanelEdge,
) -> Vec<PanelPlacementContext> {
    snapshot
        .panels
        .panels
        .iter()
        .filter(|panel| panel.edge == expected_edge)
        .filter_map(|panel| panel_context_for_panel(snapshot, panel))
        .collect()
}

/// Pure hotplug seam: panel surfaces (keyed by output) whose output is no
/// longer a connected panel output in `snapshot`. Only those are removed.
#[must_use]
pub fn panel_surface_outputs_to_remove(
    existing: &[flamewm_api::OutputId],
    snapshot: &ShellSnapshot,
) -> Vec<flamewm_api::OutputId> {
    existing
        .iter()
        .filter(|id| {
            !snapshot
                .displays
                .outputs
                .iter()
                .any(|output| output.connected && output.id == **id)
        })
        .cloned()
        .collect()
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PanelReconcilePlan {
    update: Vec<flamewm_api::OutputId>,
    create: Vec<flamewm_api::OutputId>,
    remove: Vec<flamewm_api::OutputId>,
}

fn panel_reconciliation_plan(
    existing: &[(flamewm_api::OutputId, PanelPlacementContext)],
    desired: &[PanelPlacementContext],
) -> PanelReconcilePlan {
    let mut plan = PanelReconcilePlan::default();
    for context in desired {
        match existing.iter().find(|(id, _)| id == &context.output.id) {
            Some((_, current)) if current == context => (),
            Some((id, _)) => plan.update.push(id.clone()),
            None => plan.create.push(context.output.id.clone()),
        }
    }
    plan.remove.extend(
        existing
            .iter()
            .filter(|&(id, _)| !desired.iter().any(|context| context.output.id == *id))
            .map(|(id, _)| id.clone()),
    );
    plan
}

fn cleanup_staged_panels(runtime: &mut SurfaceRuntime, staged: &mut Vec<PanelSurfaceRecord>) {
    for record in staged.drain(..) {
        let _ = runtime.destroy(record.panel);
    }
}

/// Largest floating margin the settings accept, in logical px (`taskbarFloatingMargin`).
const FLOATING_MARGIN_MAX_LOGICAL: i32 = 32;

/// F10: pure edge/geometry consistency predicate shared by
/// `check_edge_consistent` and unit tests. A floating bar sits `max_inset` device px
/// inside its output edge, so the edge-axis check accepts `0..=max_inset` of inset.
#[must_use]
pub fn edge_geometry_consistent(
    edge: flamewm_api::PanelEdge,
    panel: flamewm_api::Rect,
    output: flamewm_api::Rect,
    max_inset: i32,
) -> bool {
    match edge {
        flamewm_api::PanelEdge::Bottom => {
            (0..=max_inset).contains(&(output.bottom() - panel.bottom()))
        }
        flamewm_api::PanelEdge::Top => (0..=max_inset).contains(&(panel.y - output.y)),
        flamewm_api::PanelEdge::Left => (0..=max_inset).contains(&(panel.x - output.x)),
        flamewm_api::PanelEdge::Right => {
            (0..=max_inset).contains(&(output.right() - panel.right()))
        }
    }
}

/// F10: pure resync resolver. Prefer the stored edge (rejects mixed-edge
/// snapshots); on an edge switch fall back to the snapshot's own edge so
/// a genuine Top -> Bottom move still resyncs (geometry-driven).
#[must_use]
pub fn synced_placement_for_snapshot(
    snapshot: &ShellSnapshot,
    stored_edge: flamewm_api::PanelEdge,
) -> Option<PanelPlacementContext> {
    if let Some(placement) = panel_context_for_snapshot(snapshot, stored_edge) {
        return Some(placement);
    }
    let first = snapshot.panels.panels.first()?;
    if first.edge == stored_edge {
        return None;
    }
    panel_context_for_snapshot(snapshot, first.edge)
}

/// One native panel surface for one connected panel output. Each record
/// owns its handle and the placement context resolved from the snapshot.
pub struct PanelSurfaceRecord {
    pub panel: SurfaceHandle,
    pub placement: PanelPlacementContext,
    /// Backdrop installed on this output's panel, never the primary panel's globally.
    backdrop: Option<PanelBackdrop>,
}

/// J07 parent role: panel + Start + task menu + media only. The parent no
/// longer creates or owns audio/network/calendar native surfaces; the
/// quick-control helper owns those surfaces and the parent reaches them
/// through [`crate::quick_controls::QuickControlSupervisor`].
///
/// `panels` holds one record per connected panel output. Index 0 is the
/// primary record (the output of the first snapshot panel); every
/// single-output code path reads only the primary record.
pub struct ShellSurfaces {
    pub start: SurfaceHandle,
    pub task_menu: SurfaceHandle,
    pub media: SurfaceHandle,
    /// Top-center Launcher popup; `launcher_output` is `Some` while it is open.
    pub launcher: SurfaceHandle,
    launcher_output: Option<flamewm_api::OutputId>,
    panels: Vec<PanelSurfaceRecord>,
    selected_output: Option<flamewm_api::OutputId>,
    transient_output: Option<flamewm_api::OutputId>,
    transient: Option<Transient>,
    /// A Start slide, fade, or Magic Lamp is configured, so Start toggles close through `hide`.
    start_motion: bool,
    /// Slide armed for task menu, media, and status popups on their next open; `None` is immediate.
    popup_slide: Option<SlideConfig>,
    /// Display and panel generations under which every panel's native rect was last verified
    /// equal to its synced geometry. Cleared by `sync_panel_context`.
    native_verified: std::cell::Cell<Option<(u64, u64)>>,
}

/// Pure drift probe over native panel rects. `probe` returns `None` when a rect cannot be read,
/// which counts as drift. A clean result is cached under `key`, so idle ticks skip the probe.
fn native_geometry_drifted<P: Copy>(
    verified: &std::cell::Cell<Option<(u64, u64)>>,
    key: (u64, u64),
    panels: impl IntoIterator<Item = (P, flamewm_api::Rect)>,
    mut probe: impl FnMut(P) -> Option<flamewm_api::Rect>,
) -> bool {
    if verified.get() == Some(key) {
        return false;
    }
    let drifted = panels
        .into_iter()
        .any(|(panel, target)| probe(panel) != Some(target));
    verified.set((!drifted).then_some(key));
    drifted
}

/// What a panel's installed backdrop was made for; an equal request installs nothing.
#[derive(Debug, Clone, PartialEq)]
struct PanelBackdrop {
    rect: (i32, i32, u32, u32),
    opacity: u8,
    blur: u8,
    source: Option<DesktopSource>,
}

/// Whether a dismissed transient slides out through `hide` instead of unmapping at once.
/// Start follows the Start motion; the task menu and media follow the popup slide.
#[must_use]
pub fn dismiss_animates(is_start: bool, start_motion: bool, popup_slide: bool) -> bool {
    if is_start {
        start_motion
    } else {
        popup_slide
    }
}

/// Whether a Start toggle-close animates out through `hide` (a configured motion) instead of
/// the immediate `close_surface` used by Escape, outside clicks, and every other dismiss.
#[must_use]
pub fn start_toggle_animates(start_motion: bool) -> bool {
    start_motion
}

impl ShellSurfaces {
    /// Publishes the dock frame on every panel surface: floating reserves the bar's edge and
    /// rounds its corners; classic passes `None` so the surface returns to legacy behaviour.
    pub fn apply_dock_frame(
        &self,
        runtime: &mut SurfaceRuntime,
        frame: TaskbarFrame,
    ) -> Result<(), String> {
        for record in &self.panels {
            let placement = &record.placement;
            let mut panel = OutputPanel::new(
                placement.output.id.clone(),
                placement.output.geometry,
                placement.panel.edge,
                placement.output.shell_scale_percent,
            );
            panel.set_logical_size(placement.panel.logical_size);
            panel.frame = frame;
            let edge = frame.floating.then_some(match placement.panel.edge {
                PanelEdge::Bottom => DockEdge::Bottom,
                PanelEdge::Top => DockEdge::Top,
                PanelEdge::Left => DockEdge::Left,
                PanelEdge::Right => DockEdge::Right,
            });
            let radius = if frame.floating {
                u32::try_from(panel.corner_radius()).unwrap_or(0)
            } else {
                0
            };
            // A binary XShape mask alone makes jagged corners: the painted taskbar
            // must also use the same physical radius through its scaled UI scene.
            let scale_percent = placement.output.shell_scale_percent;
            let logical_radius = radius as f32 / panel_ui_scale(scale_percent);
            runtime
                .with_document(record.panel, |document| {
                    document.corner_radius("taskbar", logical_radius)
                })
                .map_err(|error| format!("panel corner paint: {error:?}"))?;
            runtime
                .set_dock_frame(record.panel, edge, radius)
                .map_err(|error| format!("panel dock frame: {error:?}"))?;
        }
        Ok(())
    }
}

fn create_panel_surface(
    runtime: &mut SurfaceRuntime,
    placement: &PanelPlacementContext,
) -> Result<SurfaceHandle, String> {
    let panel_snapshot = &placement.panel;
    let panel = create_surface(
        runtime,
        PANEL_ARTIFACT,
        SurfaceConfig {
            width: checked_size(panel_snapshot.geometry.width)?,
            height: checked_size(panel_snapshot.geometry.height)?,
            title: "FlameWM panel".to_owned(),
            role: SurfaceRole::Dock,
            input: SurfaceInputMode::Interactive,
            initially_visible: panel_snapshot.visible,
            x: panel_snapshot.geometry.x,
            y: panel_snapshot.geometry.y,
        },
    )?;
    // The window is `logical * scale` device pixels tall while the bar is
    // laid out in CSS pixels; without the document scale the bar stops at
    // 44 device pixels and leaves an undrawn band at the bottom edge.
    let scale = runtime
        .apply_output_scale(panel, panel_ui_scale(placement.output.shell_scale_percent))
        .map(|_| ())
        .map_err(|error| format!("panel ui scale: {error:?}"));
    let mut cleanup_error = None;
    finish_scaled_surface(panel, scale, |panel| {
        cleanup_error = runtime
            .destroy(panel)
            .err()
            .map(|error| format!("panel cleanup after scale failure: {error:?}"));
    })
    .map_err(|error| match cleanup_error {
        Some(cleanup) => format!("{error}; {cleanup}"),
        None => error,
    })
}

fn finish_scaled_surface<T, E>(
    surface: T,
    scale: Result<(), E>,
    destroy: impl FnOnce(T),
) -> Result<T, E> {
    match scale {
        Ok(()) => Ok(surface),
        Err(error) => {
            destroy(surface);
            Err(error)
        }
    }
}

impl ShellSurfaces {
    fn primary(&self) -> &PanelSurfaceRecord {
        // Invariant: `create` and `sync_panel_context` never leave `panels`
        // empty, and the primary record is kept at index 0.
        &self.panels[0]
    }

    fn selected(&self) -> &PanelSurfaceRecord {
        self.selected_output
            .as_ref()
            .and_then(|output| {
                self.panels
                    .iter()
                    .find(|record| &record.placement.output.id == output)
            })
            .unwrap_or_else(|| self.primary())
    }

    fn for_output(&self, output: &flamewm_api::OutputId) -> Option<&PanelSurfaceRecord> {
        self.panels
            .iter()
            .find(|record| &record.placement.output.id == output)
    }

    /// Set the output that owns the next popup transaction.
    pub fn select_panel_output(&mut self, output: &flamewm_api::OutputId) -> bool {
        if self
            .panels
            .iter()
            .any(|record| &record.placement.output.id == output)
        {
            self.selected_output = Some(output.clone());
            true
        } else {
            false
        }
    }

    /// Resolve panel input to its output. Events from the one shared transient
    /// keep the opener's output, so popup actions stay on the monitor that opened it.
    pub fn input_output(&self, surface: SurfaceHandle) -> Option<flamewm_api::OutputId> {
        self.panels
            .iter()
            .find(|record| record.panel == surface)
            .map(|record| record.placement.output.id.clone())
            .or_else(|| {
                (surface == self.launcher)
                    .then(|| self.launcher_output.clone())
                    .flatten()
            })
            .or_else(|| {
                let owns_popup = match self.transient {
                    Some(Transient::StartGroup) => surface == self.start,
                    Some(Transient::ContextMenu) => surface == self.task_menu,
                    Some(Transient::Media) => surface == self.media,
                    None => false,
                };
                owns_popup.then(|| self.transient_output.clone()).flatten()
            })
    }

    /// Selected panel surface for popup anchors and monitor-local presentation.
    #[must_use]
    pub fn selected_panel(&self) -> SurfaceHandle {
        self.selected().panel
    }

    /// Resolve the native panel surface for an output without changing the
    /// current input selection.
    #[must_use]
    pub fn panel_for_output(&self, output: &flamewm_api::OutputId) -> Option<SurfaceHandle> {
        self.panels
            .iter()
            .find(|record| &record.placement.output.id == output)
            .map(|record| record.panel)
    }

    /// Snapshot geometry of the selected panel, used only before native layout exists.
    #[must_use]
    pub fn selected_panel_rect(&self) -> flamewm_api::Rect {
        self.selected().placement.panel.geometry
    }

    /// Selected panel placement used by output-local popup transactions.
    #[must_use]
    pub fn selected_placement(&self) -> &PanelPlacementContext {
        &self.selected().placement
    }

    /// Native handle of the primary panel surface.
    #[must_use]
    pub fn panel(&self) -> SurfaceHandle {
        self.primary().panel
    }

    /// Every native panel surface handle, primary first.
    #[must_use]
    pub fn panel_handles(&self) -> Vec<SurfaceHandle> {
        self.panels.iter().map(|record| record.panel).collect()
    }

    /// Resolve the output that originated input on a shell surface.
    pub fn select_input_output(&self, surface: SurfaceHandle) -> Option<flamewm_api::OutputId> {
        self.input_output(surface)
    }

    /// Installs each panel's desktop-layer backdrop, or none when it is opaque. Separate mode
    /// installs the unblurred layer: the tint and blur live on each element capsule, so the gaps
    /// must not be blurred. An unchanged panel, source and material installs nothing.
    pub fn apply_panel_backdrop(
        &mut self,
        runtime: &mut SurfaceRuntime,
        source: Option<&DesktopSource>,
        opacity_percent: u8,
        blur_percent: u8,
        separate: bool,
    ) -> Result<(), String> {
        let install = separate || opacity_percent < 100;
        let (opacity, blur) = if separate {
            (100, 0)
        } else {
            (opacity_percent, blur_percent)
        };
        for record in &mut self.panels {
            let panel = record.panel;
            let device = runtime.surface_device_rect(panel).map_err(ui_error)?;
            let rect = (
                device.x as i32,
                device.y as i32,
                device.width as u32,
                device.height as u32,
            );
            let wanted = PanelBackdrop {
                rect,
                opacity,
                blur,
                source: source.filter(|_| install).cloned(),
            };
            if record.backdrop.as_ref() == Some(&wanted) {
                continue;
            }
            let words = wanted
                .source
                .as_ref()
                .filter(|_| rect.2 > 0 && rect.3 > 0)
                .and_then(|_| runtime.desktop_backdrop(rect, opacity, blur));
            crate::surface_backdrop::backdrop_trace(format_args!(
                "panel={panel:?} apply rect={rect:?} opacity={opacity} blur={blur} install={}",
                words.is_some(),
            ));
            match words {
                Some(words) => runtime
                    .set_surface_backdrop(panel, Some((&words, rect.2, rect.3)))
                    .map_err(ui_error)?,
                None => runtime
                    .set_surface_backdrop(panel, None)
                    .map_err(ui_error)?,
            }
            record.backdrop = Some(wanted);
        }
        Ok(())
    }

    /// Every panel record as (native handle, output id), primary first.
    #[must_use]
    pub fn panel_outputs(&self) -> Vec<(SurfaceHandle, flamewm_api::OutputId)> {
        self.panels
            .iter()
            .map(|record| (record.panel, record.placement.output.id.clone()))
            .collect()
    }

    pub fn create(runtime: &mut SurfaceRuntime, snapshot: &ShellSnapshot) -> Result<Self, String> {
        let first = snapshot.panels.panels.first().cloned().ok_or_else(|| {
            let outputs = snapshot
                .displays
                .outputs
                .iter()
                .map(|output| {
                    format!(
                        "{} connected={} mode={} geometry={:?}",
                        output.id.as_str(),
                        output.connected,
                        output.current_mode.0,
                        output.geometry,
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("GetPanels returned no panel (displays: [{outputs}])")
        })?;
        let contexts = panel_contexts_for_snapshot(snapshot, first.edge);
        let primary_placement = contexts
            .iter()
            .find(|context| context.panel.output == first.output)
            .cloned()
            .ok_or_else(|| "GetDisplays returned no connected panel output".to_owned())?;
        let metrics = ShellMetrics::default();
        let primary_panel = create_panel_surface(runtime, &primary_placement)?;
        let output = primary_placement.output.clone();
        let panel_snapshot = primary_placement.panel.clone();
        let start = create_surface(
            runtime,
            START_ARTIFACT,
            popup_config(
                "FlameWM start",
                SurfaceRole::PopupMenu,
                primary_placement.start_layout.popover,
            ),
        )?;
        let task_menu = create_surface(
            runtime,
            TASK_MENU_ARTIFACT,
            popup_config(
                "FlameWM task menu",
                SurfaceRole::DropdownMenu,
                flamewm_api::Rect::new(output.geometry.x, output.geometry.y, 217, 180),
            ),
        )?;
        let media = create_surface(
            runtime,
            MEDIA_ARTIFACT,
            status_popup_config(
                crate::quick_controls::host::MEDIA_POPUP_TITLE,
                flamewm_shell_core::MEDIA_POPOVER_SIZE,
                panel_anchor(&panel_snapshot, &output, metrics, 1),
            ),
        )?;
        let launcher = create_surface(
            runtime,
            LAUNCHER_ARTIFACT,
            popup_config(
                "FlameWM launcher",
                SurfaceRole::PopupMenu,
                flamewm_api::Rect::new(output.geometry.x, output.geometry.y, 600, 360),
            ),
        )?;
        let mut panels = vec![PanelSurfaceRecord {
            panel: primary_panel,
            placement: primary_placement.clone(),
            backdrop: None,
        }];
        for context in contexts {
            if context.output.id == primary_placement.output.id {
                continue;
            }
            let panel = create_panel_surface(runtime, &context)?;
            panels.push(PanelSurfaceRecord {
                panel,
                placement: context,
                backdrop: None,
            });
        }
        Ok(Self {
            start,
            task_menu,
            media,
            launcher,
            launcher_output: None,
            panels,
            selected_output: None,
            transient_output: None,
            transient: None,
            start_motion: false,
            popup_slide: None,
            native_verified: std::cell::Cell::new(None),
        })
    }

    /// Proven close branch: single-turn close per surface (ungrab-once
    /// when grabbed + unmap + Expose-present, same turn, no sleeps, no
    /// compositor), then clear the open marker. Never touches panel or
    /// mapped popup geometry.
    /// Dismiss of every transient: a configured slide slides out through `hide`; otherwise
    /// each surface unmaps at once. Reopen paths use it too, so a closing overlay is reversed
    /// by the following `show`.
    pub fn close_transients(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        for surface in [self.start, self.task_menu, self.media] {
            let animated = dismiss_animates(
                surface == self.start,
                self.start_motion,
                self.popup_slide.is_some(),
            );
            if animated {
                runtime.hide(surface).map_err(ui_error)?;
            } else {
                runtime.close_surface(surface).map_err(ui_error)?;
            }
        }
        self.transient = None;
        self.transient_output = None;
        Ok(())
    }

    /// Sets whether Start toggles animate out (a Start slide, fade, or lamp is configured).
    pub fn set_start_motion(&mut self, configured: bool) {
        self.start_motion = configured;
    }

    /// Sets the slide for task menu, media, and status popups on their next open. `None`
    /// keeps the immediate path. Start keeps its own slide from `set_start_motion`.
    pub fn set_popup_slide(&mut self, slide: Option<SlideConfig>) {
        self.popup_slide = slide;
    }

    pub fn toggle_start(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        if self.transient == Some(Transient::StartGroup) {
            if start_toggle_animates(self.start_motion) {
                // Animated close: `hide` keeps the grab/focus bookkeeping and plays the motion.
                // The task menu and media are not open here (`transient` names only Start), so
                // unmapping them would only repeat a present on unrelated surfaces.
                runtime.hide(self.start).map_err(ui_error)?;
                self.transient = None;
                self.transient_output = None;
                return Ok(());
            }
            return self.close_transients(runtime);
        }
        // A closing Start overlay is reversed by `show`, so it must not be cut short here.
        if start_toggle_animates(self.start_motion) {
            for surface in [self.task_menu, self.media] {
                runtime.close_surface(surface).map_err(ui_error)?;
            }
        } else {
            self.close_transients(runtime)?;
        }
        self.open_start_group_measured(runtime)
    }

    /// Single measured unified Start show via the shared popup
    /// transaction (`runtime::start_surface`). Never mapped at 0,0:
    /// missing layout/anchor refuses via `PopupError`.
    fn open_start_group_measured(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        let output = self.selected().placement.output.id.clone();
        self.open_start_group_on(runtime, &output)
    }

    fn open_start_group_on(
        &mut self,
        runtime: &mut SurfaceRuntime,
        output: &flamewm_api::OutputId,
    ) -> Result<(), String> {
        let panel = self
            .for_output(output)
            .ok_or_else(|| format!("start output {output} is unavailable"))?;
        runtime
            .apply_output_scale(
                self.start,
                panel_ui_scale(panel.placement.output.shell_scale_percent),
            )
            .map_err(ui_error)?;
        start_surface::open(
            runtime,
            panel.panel,
            self.start,
            panel.placement.work_area,
            panel.placement.panel.edge,
        )?;
        self.transient = Some(Transient::StartGroup);
        self.transient_output = Some(output.clone());
        Ok(())
    }

    /// outside release, Escape, or launch closes the unified surface, and
    /// the grab is released exactly once through `close_transients`.
    /// Already-open category switches resize transactionally: the surface
    /// is re-measured and re-prepared before re-map, so no 0,0 or
    /// half-resized frame is ever visible.
    pub fn open_start_group(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        if self.transient == Some(Transient::StartGroup) {
            return self.resize_start_group_measured(runtime);
        }
        self.close_transients(runtime)?;
        self.open_start_group_measured(runtime)
    }

    pub fn open_start_group_for_output(
        &mut self,
        runtime: &mut SurfaceRuntime,
        output: &flamewm_api::OutputId,
    ) -> Result<(), String> {
        if self.transient == Some(Transient::StartGroup)
            && self.transient_output.as_ref() == Some(output)
        {
            return self.resize_start_group_for_output(runtime, output);
        }
        if self.transient.is_some() {
            self.close_transients(runtime)?;
        }
        self.open_start_group_on(runtime, output)
    }

    fn resize_start_group_for_output(
        &mut self,
        runtime: &mut SurfaceRuntime,
        output: &flamewm_api::OutputId,
    ) -> Result<(), String> {
        let panel = self
            .for_output(output)
            .ok_or_else(|| format!("start output {output} is unavailable"))?;
        runtime
            .apply_output_scale(
                self.start,
                panel_ui_scale(panel.placement.output.shell_scale_percent),
            )
            .map_err(ui_error)?;
        let rect = start_surface::refit(
            runtime,
            panel.panel,
            self.start,
            panel.placement.work_area,
            panel.placement.panel.edge,
            POPUP_GAP,
        )
        .map_err(|error| error.to_string())?;
        self.map_raise(runtime, self.start, rect)
    }

    /// Transactional resize of the already-open unified Start surface
    /// via the shared `start_surface::refit` (anchor -> measure -> fit);
    /// prepared before re-map without dropping the group grab.
    fn resize_start_group_measured(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        let output = self.selected().placement.output.id.clone();
        self.resize_start_group_for_output(runtime, &output)
    }

    #[must_use]
    pub fn is_start_group_open(&self) -> bool {
        self.transient == Some(Transient::StartGroup)
    }

    #[must_use]
    pub fn is_media_open(&self) -> bool {
        self.transient == Some(Transient::Media)
    }

    pub fn open_status(&mut self, runtime: &mut SurfaceRuntime, name: &str) -> Result<(), String> {
        // J07: media stays in-process; audio/network/clock delegate to the
        // helper via the supervisor (caller path in main.rs). Direct
        // in-process opens of helper-owned popups are refused here so the
        // parent never regrows native ownership.
        if matches!(name, "volume" | "audio" | "network" | "clock") {
            return Err(format!("quick-control {name} owned by helper process"));
        }
        // Live path: stable panel node ids, edge from the panel edge, Align
        // End; no fixed popup_rect. Single measured show via the shared
        // `status_surface` transaction (see open_status_anchored_id).
        self.open_status_anchored_id(runtime, name, status_surface::spec().source_node_id)
    }

    pub fn open_status_for_output(
        &mut self,
        runtime: &mut SurfaceRuntime,
        name: &str,
        output: &flamewm_api::OutputId,
    ) -> Result<(), String> {
        if name != "media" {
            return Err(format!("unsupported shell popup {name}"));
        }
        let (panel_handle, scale_percent, work_area, edge) = self
            .for_output(output)
            .map(|panel| {
                (
                    panel.panel,
                    panel.placement.output.shell_scale_percent,
                    panel.placement.work_area,
                    panel.placement.panel.edge,
                )
            })
            .ok_or_else(|| format!("popup output {output} is unavailable"))?;
        self.close_transients(runtime)?;
        runtime
            .set_slide(self.media, self.popup_slide)
            .map_err(ui_error)?;
        runtime
            .apply_output_scale(self.media, panel_ui_scale(scale_percent))
            .map_err(ui_error)?;
        status_surface::open(runtime, panel_handle, self.media, work_area, edge)?;
        self.transient = Some(Transient::Media);
        self.transient_output = Some(output.clone());
        Ok(())
    }

    /// Anchored open from a stable panel node id (`tray-media`,
    /// `tray-volume`, `tray-network`, `clock-button`).
    /// Single measured show via the shared `status_surface` transaction.
    /// Refuses `PendingLayout`/`PendingAnchor`; never maps at 0,0.
    pub fn open_status_anchored_id(
        &mut self,
        runtime: &mut SurfaceRuntime,
        name: &str,
        source_id: &str,
    ) -> Result<(), String> {
        let (surface, transient) = match name {
            "media" => (self.media, Transient::Media),
            _ => return Err(format!("unsupported shell popup {name}")),
        };
        // Contract: this entry resolves the source id from the role spec;
        // the id path keeps accepting the legacy caller-supplied id only
        // when it matches the role's canonical source node.
        let spec = status_surface::spec();
        if source_id != spec.source_node_id {
            return Err(format!("unsupported shell popup source {source_id}"));
        }
        self.close_transients(runtime)?;
        runtime
            .set_slide(surface, self.popup_slide)
            .map_err(ui_error)?;
        runtime
            .apply_output_scale(
                surface,
                panel_ui_scale(self.selected().placement.output.shell_scale_percent),
            )
            .map_err(ui_error)?;
        status_surface::open(
            runtime,
            self.selected().panel,
            surface,
            self.selected().placement.work_area,
            self.selected().placement.panel.edge,
        )?;
        self.transient = Some(transient);
        self.transient_output = Some(self.selected().placement.output.id.clone());
        Ok(())
    }

    pub fn open_context_menu(
        &mut self,
        runtime: &mut SurfaceRuntime,
        state: &ContextMenuState,
        snapshot: &ShellSnapshot,
        backdrop: (u8, u8),
    ) -> Result<(), String> {
        let output = state
            .output
            .as_ref()
            .ok_or_else(|| "context menu has no source output".to_owned())?;
        if self.for_output(output).is_none() {
            return Err(format!("context menu output {output} is unavailable"));
        }
        self.select_panel_output(output);
        self.close_transients(runtime)?;
        // Shrink the native surface to its visible content rows so no
        // spare (black) area remains for the shape mask to cover.
        let rows = crate::taskbar::context_menu::visible_row_count_for_kind(
            &snapshot.panels,
            &snapshot.windows,
            snapshot.workspaces.as_ref(),
            &state.kind,
        );
        let panel = self
            .for_output(output)
            .ok_or_else(|| format!("context menu output {output} is unavailable"))?;
        let percent = panel.placement.output.shell_scale_percent;
        let logical = context_menu_size(rows);
        let size = flamewm_api::Size::new(
            flamewm_platform::panel::logical_to_physical(logical.width.max(0) as u16, percent),
            flamewm_platform::panel::logical_to_physical(logical.height.max(0) as u16, percent),
        );
        runtime
            .apply_output_scale(self.task_menu, panel_ui_scale(percent))
            .map_err(ui_error)?;
        // A12-1: clamp inside the panel-aware work area so a menu opened near
        // the panel stops above the panel strip instead of under it.
        let rect = context_menu_rect_for_placement(&panel.placement, state.anchor, size);
        runtime
            .set_popup_backdrop(self.task_menu, Some(backdrop))
            .map_err(ui_error)?;
        self.show_grabbed(runtime, self.task_menu, rect, Transient::ContextMenu)
    }

    /// Retained-device anchor for a quick-control source id, with the
    /// snapshot-geometry fallback used only pre-map. Mirrors the
    /// in-process contract without regrowing native ownership.
    pub fn quick_anchor(
        &self,
        runtime: &SurfaceRuntime,
        fallback: &flamewm_api::Rect,
        source_id: &str,
    ) -> flamewm_api::Rect {
        crate::runtime::retained_source_rect(runtime, self.selected().panel, source_id)
            .unwrap_or(*fallback)
    }

    pub fn quick_anchor_for_output(
        &self,
        runtime: &SurfaceRuntime,
        output: &flamewm_api::OutputId,
        fallback: &flamewm_api::Rect,
        source_id: &str,
    ) -> Option<flamewm_api::Rect> {
        let panel = self.for_output(output)?;
        Some(
            crate::runtime::retained_source_rect(runtime, panel.panel, source_id)
                .unwrap_or(*fallback),
        )
    }

    pub fn placement_for_output(
        &self,
        output: &flamewm_api::OutputId,
    ) -> Option<&PanelPlacementContext> {
        self.for_output(output).map(|panel| &panel.placement)
    }

    /// Recompute the panel placement records from a fresh snapshot. Updates
    /// every retained output's record in place, creates a native panel
    /// surface for a newly connected panel output, and destroys records
    /// whose output disconnected. The primary record stays at index 0 and
    /// tracks the snapshot's first panel. Returns `Ok(false)` when no panel
    /// resolves (state untouched). On an edge switch (stored Top, snapshot
    /// Bottom) the stored edge misses, so the snapshot's own edge is used.
    pub fn sync_panel_context(
        &mut self,
        runtime: &mut SurfaceRuntime,
        snapshot: &ShellSnapshot,
    ) -> Result<bool, String> {
        self.native_verified.set(None);
        // Each output's panel has its own authoritative edge and geometry.
        // Filtering against the previous primary edge silently discarded
        // panels after an output swap (or a mixed-edge arrangement).
        let contexts: Vec<_> = snapshot
            .panels
            .panels
            .iter()
            .filter_map(|panel| panel_context_for_panel(snapshot, panel))
            .collect();
        let Some(first) = contexts.first() else {
            return Ok(false);
        };
        let primary_output = first.output.id.clone();
        let existing: Vec<_> = self
            .panels
            .iter()
            .map(|record| (record.placement.output.id.clone(), record.placement.clone()))
            .collect();
        let plan = panel_reconciliation_plan(&existing, &contexts);

        // Stage all new panel surfaces hidden. A failure leaves the live
        // records and their placements untouched; staged handles are retired.
        let mut staged = Vec::new();
        for id in &plan.create {
            let Some(context) = contexts.iter().find(|context| &context.output.id == id) else {
                continue;
            };
            let mut hidden = context.clone();
            hidden.panel.visible = false;
            let panel = match create_panel_surface(runtime, &hidden) {
                Ok(panel) => panel,
                Err(error) => {
                    cleanup_staged_panels(runtime, &mut staged);
                    return Err(error);
                }
            };
            staged.push(PanelSurfaceRecord {
                panel,
                placement: context.clone(),
                backdrop: None,
            });
            if context.panel.visible {
                if let Err(error) = runtime.show(panel) {
                    cleanup_staged_panels(runtime, &mut staged);
                    return Err(ui_error(error));
                }
            }
        }

        // Retire stale native surfaces before publishing new placement
        // records. If one destroy fails, discard staged additions and leave
        // every still-live record available for the next topology retry.
        for id in &plan.remove {
            if let Some(index) = self
                .panels
                .iter()
                .position(|record| &record.placement.output.id == id)
            {
                let panel = self.panels[index].panel;
                if let Err(error) = runtime.destroy(panel) {
                    cleanup_staged_panels(runtime, &mut staged);
                    return Err(ui_error(error));
                }
                self.panels.remove(index);
            }
        }

        for id in &plan.update {
            let Some(context) = contexts.iter().find(|context| &context.output.id == id) else {
                continue;
            };
            if let Some(record) = self
                .panels
                .iter_mut()
                .find(|record| &record.placement.output.id == id)
            {
                record.placement = context.clone();
            }
        }
        self.panels.extend(staged);
        let retained: Vec<_> = contexts
            .iter()
            .map(|context| context.output.id.clone())
            .collect();
        if self
            .selected_output
            .as_ref()
            .is_some_and(|id| !retained.contains(id))
        {
            self.selected_output = None;
        }
        if self
            .transient_output
            .as_ref()
            .is_some_and(|id| !retained.contains(id))
        {
            self.transient_output = None;
        }
        if let Some(index) = self
            .panels
            .iter()
            .position(|record| record.placement.output.id == primary_output)
        {
            let record = self.panels.remove(index);
            self.panels.insert(0, record);
        }
        Ok(true)
    }

    /// Parent-side anchor inputs for a helper open: retained anchor +
    /// work area + panel edge. The parent computes, the helper places.
    #[must_use]
    pub fn work_area(&self) -> flamewm_api::Rect {
        self.selected().placement.work_area
    }

    /// F10: current context generation read by every Start/Quick open
    /// path in main.rs before projection. Proves the open uses the
    /// post-sync context, never a stale cached rect.
    #[must_use]
    pub fn placement_generation(&self) -> (u64, u64) {
        (
            self.selected().placement.display_generation,
            self.selected().placement.panels_revision,
        )
    }

    /// Whether an open popup anchored to `output` remains valid under the
    /// incoming snapshot, ignoring global revision counters and other outputs.
    #[must_use]
    pub fn popup_context_unchanged(
        &self,
        output: &flamewm_api::OutputId,
        snapshot: &ShellSnapshot,
    ) -> bool {
        let Some(current) = self
            .panels
            .iter()
            .find(|record| &record.placement.output.id == output)
            .map(|record| &record.placement)
        else {
            return false;
        };
        let Some(next) = snapshot
            .panels
            .panels
            .iter()
            .find(|panel| &panel.output == output)
            .and_then(|panel| panel_context_for_panel(snapshot, panel))
        else {
            return false;
        };
        popup_context_unchanged(current, &next)
    }

    /// F10: last-synced panel snapshot geometry.
    #[must_use]
    pub fn panel_geometry(&self) -> flamewm_api::Rect {
        self.primary().placement.panel.geometry
    }

    /// Detect changes in every output by stable ID, not the primary panel's
    /// position alone. `RandR` can change CRTC coordinates without a reliable
    /// panels revision/generation bump. This gate runs before every popup.
    #[must_use]
    pub fn panel_topology_drifted(&self, snapshot: &ShellSnapshot) -> bool {
        if self.panels.len() != snapshot.panels.panels.len() {
            return true;
        }
        if snapshot.panels.panels.first().map(|panel| &panel.output)
            != Some(&self.primary().placement.output.id)
        {
            return true;
        }
        self.panels
            .iter()
            .any(|record| !panel_context_is_current(&record.placement, snapshot))
    }

    /// F10: true when any panel output's shell scale differs from the scale
    /// last synced into its panel document. A scale change keeps the logical
    /// panel geometry, so geometry/generation alone cannot detect it.
    #[must_use]
    pub fn output_scale_drifted(&self, snapshot: &ShellSnapshot) -> bool {
        self.panels.iter().any(|record| {
            let id = &record.placement.output.id;
            snapshot
                .panels
                .panels
                .iter()
                .any(|panel| panel.output == *id)
                && snapshot
                    .displays
                    .outputs
                    .iter()
                    .find(|output| output.id == *id && output.connected)
                    .is_some_and(|output| {
                        output.shell_scale_percent != record.placement.output.shell_scale_percent
                    })
        })
    }

    /// F10: edge/geometry consistency gate for every panel record.
    /// Refusal strings are returned to callers, which refuse popup open on `Err`.
    pub fn check_edge_consistent(&self) -> Result<(), String> {
        for record in &self.panels {
            let panel = record.placement.panel.geometry;
            let output = record.placement.output.geometry;
            let edge = record.placement.panel.edge;
            let max_inset = (FLOATING_MARGIN_MAX_LOGICAL
                * i32::from(record.placement.output.shell_scale_percent)
                + 99)
                / 100;
            let consistent = edge_geometry_consistent(edge, panel, output, max_inset);
            debug_assert!(
                consistent,
                "panel edge {edge:?} inconsistent: panel={panel:?} output={output:?}"
            );
            if !consistent {
                return Err(format!(
                    "panel edge {edge:?} inconsistent: panel={panel:?} output={output:?}"
                ));
            }
        }
        Ok(())
    }

    /// Reapplies each panel document UI scale from its synced output. The
    /// scale is otherwise set only at surface creation, so a live scale
    /// change left the retained bar laid out at the old scale. Setting a
    /// changed scale bumps the document revision, forcing a layout recompute
    /// on the next redraw.
    pub fn sync_panel_ui_scale(&self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        for record in &self.panels {
            runtime
                .apply_output_scale(
                    record.panel,
                    panel_ui_scale(record.placement.output.shell_scale_percent),
                )
                .map(|_| ())
                .map_err(|error| format!("panel ui scale: {error:?}"))?;
        }
        Ok(())
    }

    /// The retained placement record can be current while the X11 surface is
    /// stale (for example if a previous `ConfigureWindow` failed). Compare the
    /// actual native rectangles for *every* output, not just snapshot revisions.
    /// `key` is the (display generation, panels revision) under which the
    /// result is cached: a clean result is not re-probed until it changes, and
    /// a drifted result is re-probed every tick until a re-sync repairs it.
    // shortcut: an external move of a panel surface is not observed here; only
    // RandR (display generation) and our own re-sync invalidate the cache.
    // revisit when ConfigureNotify for panel surfaces is routed into the shell.
    #[must_use]
    pub fn panel_native_geometry_drifted(&self, runtime: &SurfaceRuntime, key: (u64, u64)) -> bool {
        native_geometry_drifted(
            &self.native_verified,
            key,
            self.panels
                .iter()
                .map(|record| (record.panel, record.placement.panel.geometry)),
            |panel| {
                runtime.surface_device_rect(panel).ok().map(|rect| {
                    flamewm_api::Rect::new(
                        rect.x as i32,
                        rect.y as i32,
                        rect.width as i32,
                        rect.height as i32,
                    )
                })
            },
        )
    }

    /// F10: `move_resize` every panel native surface to its synced snapshot
    /// geometry. Returns `true` when at least one `move_resize` was issued.
    pub fn move_panel_to_synced(&self, runtime: &mut SurfaceRuntime) -> Result<bool, String> {
        let mut moved = false;
        for record in &self.panels {
            let target = record.placement.panel.geometry;
            let current = runtime
                .surface_device_rect(record.panel)
                .map(|rect| {
                    flamewm_api::Rect::new(
                        rect.x as i32,
                        rect.y as i32,
                        rect.width as i32,
                        rect.height as i32,
                    )
                })
                .unwrap_or(target);
            if current == target {
                continue;
            }
            let width = checked_size(target.width)?;
            let height = checked_size(target.height)?;
            runtime
                .move_resize(record.panel, target.x, target.y, width, height)
                .map_err(ui_error)?;
            moved = true;
        }
        Ok(moved)
    }

    #[must_use]
    pub fn placement(&self) -> &PanelPlacementContext {
        &self.selected().placement
    }

    #[must_use]
    pub fn panel_edge(&self) -> flamewm_api::PanelEdge {
        self.selected().placement.panel.edge
    }

    /// Resolve a surface-local pointer position (`ActionEvent` x/y from the
    /// panel surface) to root coordinates via the retained panel device
    /// rect. Falls back to the snapshot geometry only when no retained
    /// layout exists (pre-map).
    #[must_use]
    pub fn root_pointer(&self, runtime: &SurfaceRuntime, x: f32, y: f32) -> flamewm_api::Point {
        let origin = runtime
            .surface_device_rect(self.selected().panel)
            .map(|rect| (rect.x as i32, rect.y as i32))
            .unwrap_or((
                self.selected().placement.panel.geometry.x,
                self.selected().placement.panel.geometry.y,
            ));
        flamewm_api::Point::new(origin.0 + x as i32, origin.1 + y as i32)
    }

    pub fn root_pointer_for_output(
        &self,
        runtime: &SurfaceRuntime,
        output: &flamewm_api::OutputId,
        x: f32,
        y: f32,
    ) -> Option<flamewm_api::Point> {
        let panel = self.for_output(output)?;
        let origin = runtime
            .surface_device_rect(panel.panel)
            .map(|rect| (rect.x as i32, rect.y as i32))
            .unwrap_or((
                panel.placement.panel.geometry.x,
                panel.placement.panel.geometry.y,
            ));
        Some(flamewm_api::Point::new(
            origin.0 + x as i32,
            origin.1 + y as i32,
        ))
    }

    fn show_grabbed(
        &mut self,
        runtime: &mut SurfaceRuntime,
        surface: SurfaceHandle,
        rect: flamewm_api::Rect,
        transient: Transient,
    ) -> Result<(), String> {
        self.map_raise_grab(runtime, surface, rect)?;
        self.transient = Some(transient);
        self.transient_output = Some(self.selected().placement.output.id.clone());
        Ok(())
    }

    fn prepare_traced(
        &self,
        runtime: &mut SurfaceRuntime,
        surface: SurfaceHandle,
        rect: flamewm_api::Rect,
        trace: &GeometryTrace,
    ) -> Result<(), String> {
        if rect.width <= 0 || rect.height <= 0 {
            return Err(PopupRefusal::PendingLayout.to_string());
        }
        runtime
            .move_resize_traced(
                surface,
                rect.x,
                rect.y,
                checked_size(rect.width)?,
                checked_size(rect.height)?,
                Some(*trace),
            )
            .map_err(ui_error)
    }

    /// Present step for the grabbed tail: canonical `present_mapped` path
    /// (show = `geometry/move_resize` origin-tracked + map/raise/present).
    /// No separate raise or redraw chains: `show` owns the single ordered
    /// present cycle, then the single group grab.
    fn map_raise_grab(
        &mut self,
        runtime: &mut SurfaceRuntime,
        surface: SurfaceHandle,
        rect: flamewm_api::Rect,
    ) -> Result<(), String> {
        self.prepare_traced(runtime, surface, rect, &GeometryTrace::begin("shell-popup"))?;
        runtime.show(surface).map_err(ui_error)?;
        runtime
            .grab_pointer(surface)
            .map_err(|_| PopupRefusal::PointerGrabRefused.to_string())
    }

    /// Present step without grab (Start root / remeasure tail): canonical
    /// present via `show` only.
    fn map_raise(
        &self,
        runtime: &mut SurfaceRuntime,
        surface: SurfaceHandle,
        rect: flamewm_api::Rect,
    ) -> Result<(), String> {
        self.prepare_traced(runtime, surface, rect, &GeometryTrace::begin("shell-popup"))?;
        runtime.show(surface).map_err(ui_error)
    }
}

/// Retained-device node rect by id (root-space device rect, no
/// recompute). Shared by the in-process media path and the parent-side
/// quick-control anchor computation.
#[must_use]
pub fn retained_source_rect(
    runtime: &SurfaceRuntime,
    panel: SurfaceHandle,
    source_id: &str,
) -> Option<flamewm_api::Rect> {
    retained_source_by_id(runtime, panel, source_id)
}

fn create_surface(
    runtime: &mut SurfaceRuntime,
    bytes: &'static [u8],
    config: SurfaceConfig,
) -> Result<SurfaceHandle, String> {
    let document = decode_document(bytes)?;
    runtime
        .create_surface(UiTemplate::new(document), config)
        .map_err(ui_error)
}

fn popup_config(title: &str, role: SurfaceRole, rect: flamewm_api::Rect) -> SurfaceConfig {
    SurfaceConfig {
        width: checked_size(rect.width).unwrap_or(1),
        height: checked_size(rect.height).unwrap_or(1),
        title: title.to_owned(),
        role,
        input: SurfaceInputMode::Interactive,
        initially_visible: false,
        x: rect.x,
        y: rect.y,
    }
}

fn status_popup_config(
    title: &str,
    size: flamewm_api::Size,
    anchor: flamewm_api::Rect,
) -> SurfaceConfig {
    let rect = anchor_popover_rect(anchor, size);
    popup_config(title, SurfaceRole::PopupMenu, rect)
}

fn checked_size(value: i32) -> Result<u32, String> {
    u32::try_from(value).map_err(|_| format!("invalid surface size {value}"))
}

/// Document UI scale for the panel surface. Must match the physical thickness
/// rule in `flamewm_platform::panel` (supported scales only; others fall back
/// to 100%) so the CSS-sized bar fills the whole panel window.
pub(crate) fn panel_ui_scale(scale_percent: u16) -> f32 {
    let percent = if (100..=200).contains(&scale_percent) && scale_percent % 5 == 0 {
        scale_percent
    } else {
        100
    };
    f32::from(percent) / 100.0
}

/// Retained-device anchor: live panel node rect (`node_device_rect_by_id`,
/// root-space device rect, no recompute) when retained layout exists.
fn retained_source_by_id(
    runtime: &SurfaceRuntime,
    panel: SurfaceHandle,
    source_id: &str,
) -> Option<flamewm_api::Rect> {
    runtime
        .node_device_rect_by_id(panel, source_id)
        .map(|rect| to_api_rect(rect.x, rect.y, rect.width, rect.height))
        .filter(|rect| rect.width > 0 && rect.height > 0)
}

/// Render-space device rect (f32 root space) -> API integer rect.
/// Inferred render rect type; this crate names no render-core type.
fn to_api_rect(x: f32, y: f32, width: f32, height: f32) -> flamewm_api::Rect {
    flamewm_api::Rect::new(
        x as i32,
        y as i32,
        width.max(1.0) as i32,
        height.max(1.0) as i32,
    )
}

fn ui_error(error: UiBackendError) -> String {
    format!("{error:?}")
}

/// Shell spans (static only): shell.windows.reconcile, shell.workspaces.reconcile,
/// shell.tasks.project, shell.pager.project, shell.popup.request,
/// `shell.quick_control.open_request`, `shell.quick_control.restart`,
/// `shell.signal_to_visual.windows`, `shell.signal_to_visual.workspaces`,
/// `shell.signal_to_visual.system`.
#[must_use]
pub fn shell_span(label: &'static str) -> flamewm_profiler::ProfilePoint {
    debug_assert!(
        matches!(
            label,
            "shell.windows.reconcile"
                | "shell.workspaces.reconcile"
                | "shell.tasks.project"
                | "shell.pager.project"
                | "shell.popup.request"
                | "shell.quick_control.open_request"
                | "shell.quick_control.restart"
                | "shell.quick.project"
                | "shell.quick.measure"
                | "shell.quick.place"
                | "shell.quick.prepare/commit_geometry"
                | "shell.quick.present"
                | "shell.quick.pointer_grab"
                | "shell.quick.close/ungrab/unmap"
                | "shell.quick.open.total"
                | "shell.signal_to_visual.windows"
                | "shell.signal_to_visual.workspaces"
                | "shell.signal_to_visual.system"
        ),
        "shell span label must be a static J07 span, got {label}"
    );
    flamewm_profiler::ProfilePoint::new(label)
}

/// J07 reconcile counters, one per domain. `OnceLock` statics keep labels
/// stable across turns; the profiler registry dedups by label.
/// J07 reconcile domain tag for per-domain counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainKind {
    Windows,
    Workspaces,
    Panels,
    System,
    Applications,
}

/// Declares a per-call-site `OnceLock` counter and yields `&'static CounterPoint`.
macro_rules! static_counter {
    ($label:literal) => {{
        static C: std::sync::OnceLock<flamewm_profiler::CounterPoint> = std::sync::OnceLock::new();
        C.get_or_init(|| flamewm_profiler::CounterPoint::new($label))
    }};
}

pub fn domain_reconcile_counter(kind: DomainKind) -> &'static flamewm_profiler::CounterPoint {
    match kind {
        DomainKind::Windows => static_counter!("shell.domain.windows.reconcile"),
        DomainKind::Workspaces => static_counter!("shell.domain.workspaces.reconcile"),
        DomainKind::Panels => static_counter!("shell.domain.panels.reconcile"),
        DomainKind::System => static_counter!("shell.domain.system.reconcile"),
        DomainKind::Applications => static_counter!("shell.domain.applications.reconcile"),
    }
}

/// J07 projection scope tag: partial panel vs start vs popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionKind {
    Partial,
    Start,
    Popup,
}

pub fn projection_counter(kind: ProjectionKind) -> &'static flamewm_profiler::CounterPoint {
    match kind {
        ProjectionKind::Partial => static_counter!("shell.projection.panel.partial"),
        ProjectionKind::Start => static_counter!("shell.projection.panel.start"),
        ProjectionKind::Popup => static_counter!("shell.projection.panel.popup"),
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod start_toggle_tests {
    use super::start_toggle_animates;

    #[test]
    fn configured_motion_animates_the_start_toggle_close() {
        assert!(start_toggle_animates(true));
    }

    #[test]
    fn no_motion_keeps_the_immediate_toggle_close() {
        assert!(!start_toggle_animates(false));
    }
}

#[cfg(test)]
mod popup_motion_tests {
    use super::dismiss_animates;

    #[test]
    fn each_transient_dismisses_through_its_own_motion() {
        assert!(
            dismiss_animates(true, true, false),
            "Start slide animates Start"
        );
        assert!(
            !dismiss_animates(true, false, true),
            "popup slide must not change Start"
        );
        assert!(
            dismiss_animates(false, false, true),
            "popup slide animates menu and media"
        );
        assert!(
            !dismiss_animates(false, true, false),
            "Start motion must not animate popups"
        );
        assert!(
            !dismiss_animates(false, false, false),
            "no slide keeps the immediate unmap"
        );
    }
}

#[cfg(test)]
mod panel_scale_transaction_tests {
    use super::finish_scaled_surface;
    use std::cell::Cell;

    #[test]
    fn scale_failure_destroys_the_just_created_surface() {
        let destroyed = Cell::new(false);
        let result = finish_scaled_surface(17, Err("scale refused"), |handle| {
            assert_eq!(handle, 17);
            destroyed.set(true);
        });
        assert_eq!(result, Err("scale refused"));
        assert!(
            destroyed.get(),
            "failed scale must not leak the newly created handle"
        );
    }

    #[test]
    fn successful_scale_keeps_the_surface_owned_by_the_caller() {
        let destroyed = Cell::new(false);
        let result = finish_scaled_surface(17, Ok::<(), &str>(()), |_| destroyed.set(true));
        assert_eq!(result, Ok(17));
        assert!(!destroyed.get());
    }
}

#[cfg(test)]
mod native_drift_probe_tests {
    use super::native_geometry_drifted;
    use flamewm_api::Rect;
    use std::cell::{Cell, RefCell};

    const TARGET_A: Rect = Rect::new(0, 0, 1920, 40);
    const TARGET_B: Rect = Rect::new(1920, 0, 1280, 40);

    fn panels() -> [(u32, Rect); 2] {
        [(1, TARGET_A), (2, TARGET_B)]
    }

    #[test]
    fn idle_ticks_probe_native_rects_once_per_generation() {
        let verified = Cell::new(None);
        let probes = Cell::new(0_u32);
        let mut drifted_ticks = 0;
        for _ in 0..1_000 {
            let drifted = native_geometry_drifted(&verified, (7, 3), panels(), |panel| {
                probes.set(probes.get() + 1);
                Some(if panel == 1 { TARGET_A } else { TARGET_B })
            });
            drifted_ticks += usize::from(drifted);
        }
        assert_eq!(drifted_ticks, 0);
        assert_eq!(
            probes.get(),
            2,
            "one probe per panel across 1000 idle ticks"
        );
    }

    #[test]
    fn generation_change_reprobes_once_then_caches_again() {
        let verified = Cell::new(None);
        let probes = Cell::new(0_u32);
        let probe = |_panel: u32| {
            probes.set(probes.get() + 1);
            Some(TARGET_A)
        };
        assert!(!native_geometry_drifted(
            &verified,
            (7, 3),
            [(1, TARGET_A)],
            probe
        ));
        assert!(!native_geometry_drifted(
            &verified,
            (8, 3),
            [(1, TARGET_A)],
            probe
        ));
        assert!(!native_geometry_drifted(
            &verified,
            (8, 3),
            [(1, TARGET_A)],
            probe
        ));
        assert_eq!(probes.get(), 2);
    }

    #[test]
    fn drift_is_reported_and_retried_until_repaired() {
        let verified = Cell::new(None);
        let native = RefCell::new(vec![(1_u32, TARGET_A), (2, Rect::new(0, 0, 10, 10))]);
        let probes = Cell::new(0_u32);
        let probe = |panel: u32| {
            probes.set(probes.get() + 1);
            native
                .borrow()
                .iter()
                .find(|(id, _)| *id == panel)
                .map(|(_, r)| *r)
        };
        assert!(native_geometry_drifted(&verified, (1, 1), panels(), probe));
        assert!(native_geometry_drifted(&verified, (1, 1), panels(), probe));
        native.borrow_mut()[1].1 = TARGET_B;
        assert!(!native_geometry_drifted(&verified, (1, 1), panels(), probe));
        assert!(!native_geometry_drifted(&verified, (1, 1), panels(), probe));
        // Two drifted ticks and one repairing tick each probe both panels; the last tick is cached.
        assert_eq!(probes.get(), 2 + 2 + 2);
    }

    #[test]
    fn unreadable_rect_counts_as_drift() {
        let verified = Cell::new(None);
        assert!(native_geometry_drifted(
            &verified,
            (1, 1),
            [(1_u32, TARGET_A)],
            |_| None
        ));
        assert_eq!(verified.get(), None);
    }
}

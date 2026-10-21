use std::collections::VecDeque;

pub mod start_surface;
pub mod status_surface;
use crate::popup_controller::POPUP_GAP;
use crate::start::{self, StartCategory};
use crate::surface_backdrop::{blurred_argb, capture_step, CaptureKey, CaptureStep};
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
use flamewm_image_core::RgbaImage;
use flamewm_platform::panel::{OutputPanel, TaskbarFrame};
use flamewm_shell_core::popup::{
    anchor_popover_rect, context_menu_rect, context_menu_size, panel_anchor, work_area_for_panel,
    PopupRefusal,
};
use flamewm_shell_core::start_surface_layout;
use flamewm_ui_core::style::ShellMetrics;
use flamewm_ui_x11::{
    decode_document, DockEdge, GeometryTrace, SlideConfig, SurfaceConfig, SurfaceHandle,
    SurfaceInputMode, SurfaceRole, SurfaceRuntime, UiBackendError, UiDocumentAccess, UiTemplate,
};

const PANEL_ARTIFACT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-panel.rwr"));
const START_ARTIFACT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-start.rwr"));
const TASK_MENU_ARTIFACT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-task-menu.rwr"));
const MEDIA_ARTIFACT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-media.rwr"));
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
    /// Root-coordinate pointer anchor (ActionEvent local x/y resolved
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
    snapshot: ShellSnapshot,
    selected_panel_output: Option<flamewm_api::OutputId>,
    controls: VecDeque<ShellControl>,
    dirty: ShellDirty,
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
            snapshot,
            selected_panel_output: None,
            controls: VecDeque::new(),
            dirty: ShellDirty::default(),
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
                    self.snapshot.windows = windows.clone();
                    self.dirty.windows = true;
                }
            }
            ControlSignal::WorkspacesSnapshotChanged { snapshot } => {
                let _s = crate::runtime::shell_span("shell.signal_to_visual.workspaces").start();
                if self.snapshot.workspaces.as_ref() != Some(snapshot) {
                    self.snapshot.workspaces = Some(snapshot.clone());
                    self.dirty.workspaces = true;
                }
            }
            ControlSignal::PanelsSnapshotChanged { snapshot } => {
                if *snapshot != self.snapshot.panels {
                    self.snapshot.panels = snapshot.clone();
                    self.dirty.panels = true;
                }
            }
            ControlSignal::DisplaysSnapshotChanged { snapshot } => {
                // Output shell scale lives only in the display snapshot. Refreshing it here lets
                // resync_panel_geometry see the scale drift; panels are marked dirty so the
                // frame loop runs the panel projection path this turn.
                if *snapshot != self.snapshot.displays {
                    self.snapshot.displays = snapshot.clone();
                    self.dirty.panels = true;
                }
            }
            ControlSignal::SystemSnapshotChanged { snapshot } => {
                let _s = crate::runtime::shell_span("shell.signal_to_visual.system").start();
                if snapshot != &self.snapshot.system {
                    self.snapshot.system = snapshot.clone();
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
                    .map(|intent| intent.into_control_request())
            }
        }
    }

    /// Typed SystemAction control for audio rows from a provider snapshot.
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

    /// Typed SystemAction control for network rows from a provider snapshot.
    /// Secrets travel via the SecretAgent snapshot only; this never carries
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
        let states = flamewm_shell_core::task_visual_states_on_output_and_workspace(
            &self.snapshot.panels,
            &self.snapshot.windows,
            &self.snapshot.applications,
            Some(&source.output),
            self.snapshot
                .workspaces
                .as_ref()
                .map(|workspaces| workspaces.active_index),
        );
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
        let states = flamewm_shell_core::task_visual_states_on_output_and_workspace(
            &self.snapshot.panels,
            &self.snapshot.windows,
            &self.snapshot.applications,
            Some(output),
            self.snapshot
                .workspaces
                .as_ref()
                .map(|workspaces| workspaces.active_index),
        );
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
        let states = flamewm_shell_core::task_visual_states_on_output_and_workspace(
            &self.snapshot.panels,
            &self.snapshot.windows,
            &self.snapshot.applications,
            Some(&target.output),
            self.snapshot
                .workspaces
                .as_ref()
                .map(|workspaces| workspaces.active_index),
        );
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
        let Some(snapshot) = self.snapshot.workspaces.as_ref() else {
            return;
        };
        let Some(index) = crate::taskbar::workspaces::cell_index(cell, snapshot.count) else {
            return;
        };
        let kind = ContextMenuKind::Workspace { index };
        self.push_context_menu(kind, snapshot.revision, pointer, None);
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
            Some(interaction),
        );
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
    plan.remove.extend(existing.iter().filter_map(|(id, _)| {
        (!desired.iter().any(|context| context.output.id == *id)).then(|| id.clone())
    }));
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
    /// Backdrop state belongs to this output, never to the primary panel globally.
    snapshot: Option<RgbaImage>,
    capture: Option<PanelCapture>,
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
    panels: Vec<PanelSurfaceRecord>,
    selected_output: Option<flamewm_api::OutputId>,
    transient_output: Option<flamewm_api::OutputId>,
    transient: Option<Transient>,
    /// A Start slide, fade, or Magic Lamp is configured, so Start toggles close through `hide`.
    start_motion: bool,
    /// Slide armed for task menu, media, and status popups on their next open; `None` is immediate.
    popup_slide: Option<SlideConfig>,
}

/// One panel capture attempt: the key it was taken for, attempts so far, and whether it is final.
#[derive(Debug, Clone)]
struct PanelCapture {
    key: CaptureKey,
    attempts: u8,
    settled: bool,
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
            let edge = frame.floating.then(|| match placement.panel.edge {
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
            let logical_radius =
                radius as f32 / panel_ui_scale(placement.output.shell_scale_percent);
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

    /// Pre-map hook kept for the startup sequence. The snapshot is no longer read from the root
    /// here: a root read before the desktop paints its wallpaper yields the wrong pixels, so the
    /// snapshot is derived by `refresh_panel_backdrop` from the wallpaper itself.
    pub fn capture_panel_backdrop(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        runtime.watch_root_changes();
        Ok(())
    }

    /// Re-opens settled-but-blank captures on every output, not only the primary.
    pub fn reopen_panel_capture(&mut self) {
        for record in &mut self.panels {
            if let Some(capture) = record
                .capture
                .as_mut()
                .filter(|capture| capture.settled && record.snapshot.is_none())
            {
                capture.attempts = 0;
                capture.settled = false;
            }
        }
    }

    /// Resolve the output that originated input on a shell surface.
    pub fn select_input_output(&self, surface: SurfaceHandle) -> Option<flamewm_api::OutputId> {
        self.input_output(surface)
    }

    #[must_use]
    pub fn panel_capture_pending(&self) -> bool {
        self.panels.iter().any(|record| {
            record
                .capture
                .as_ref()
                .is_none_or(|capture| !capture.settled)
        })
    }

    /// One wallpaper/capture state per native panel. The key is derived from that panel's
    /// device rect, so monitor changes cannot reuse another output's wallpaper pixels.
    pub fn refresh_panel_backdrops(
        &mut self,
        runtime: &mut SurfaceRuntime,
        wallpaper_name: &str,
        fit_name: &str,
        mut wallpaper: impl FnMut((i32, i32, u32, u32)) -> Option<RgbaImage>,
    ) -> Result<(), String> {
        runtime.watch_root_changes();
        for record in &mut self.panels {
            let panel = record.panel;
            let rect = runtime.surface_device_rect(panel).map_err(ui_error)?;
            let key = CaptureKey {
                wallpaper: wallpaper_name.to_owned(),
                fit: fit_name.to_owned(),
                rect: (
                    rect.x as i32,
                    rect.y as i32,
                    rect.width as u32,
                    rect.height as u32,
                ),
            };
            let attempts_so_far = match &record.capture {
                Some(capture) if capture.key == key && capture.settled => continue,
                Some(capture) if capture.key == key => capture.attempts,
                _ => 0,
            };
            let attempts = attempts_so_far.saturating_add(1);
            let (step, raw) = match wallpaper(key.rect) {
                Some(image) => (CaptureStep::Use, image),
                None => {
                    let capture = runtime
                        .capture_root_under_unmapped(panel)
                        .map_err(ui_error)?;
                    let ready = flamewm_ui_x11::backdrop::capture_is_ready(&capture.pixels);
                    let image = RgbaImage {
                        width: capture.width,
                        height: capture.height,
                        pixels: capture.pixels,
                    };
                    (capture_step(ready, attempts), image)
                }
            };
            crate::surface_backdrop::backdrop_trace(format_args!(
                "panel={panel:?} snapshot rect={:?} size={}x{} step={step:?} attempts={attempts}",
                key.rect, raw.width, raw.height,
            ));
            record.snapshot = (step == CaptureStep::Use).then_some(raw);
            record.capture = Some(PanelCapture {
                key,
                attempts,
                settled: step != CaptureStep::Retry,
            });
        }
        Ok(())
    }

    /// Each panel gets its own backdrop, or no backdrop if opaque/unavailable/resized.
    pub fn apply_panel_backdrop(
        &self,
        runtime: &mut SurfaceRuntime,
        opacity_percent: u8,
        blur_percent: u8,
    ) -> Result<(), String> {
        for record in &self.panels {
            let panel = record.panel;
            let rect = runtime.surface_device_rect(panel).map_err(ui_error)?;
            let fitting = record.snapshot.as_ref().filter(|snapshot| {
                opacity_percent < 100
                    && snapshot.width as f32 == rect.width
                    && snapshot.height as f32 == rect.height
            });
            crate::surface_backdrop::backdrop_trace(format_args!(
                "panel={panel:?} apply rect={}x{}@{},{} opacity={opacity_percent} blur={blur_percent} snapshot={:?} install={}",
                rect.width,
                rect.height,
                rect.x,
                rect.y,
                record.snapshot.as_ref().map(|s| (s.width, s.height)),
                fitting.is_some(),
            ));
            match fitting {
                Some(snapshot) => {
                    let words = blurred_argb(snapshot, opacity_percent, blur_percent);
                    runtime
                        .set_surface_backdrop(
                            panel,
                            Some((&words, snapshot.width, snapshot.height)),
                        )
                        .map_err(ui_error)?;
                }
                None => runtime
                    .set_surface_backdrop(panel, None)
                    .map_err(ui_error)?,
            }
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
        let mut panels = vec![PanelSurfaceRecord {
            panel: primary_panel,
            placement: primary_placement.clone(),
            snapshot: None,
            capture: None,
        }];
        for context in contexts {
            if context.output.id == primary_placement.output.id {
                continue;
            }
            let panel = create_panel_surface(runtime, &context)?;
            panels.push(PanelSurfaceRecord {
                panel,
                placement: context,
                snapshot: None,
                capture: None,
            });
        }
        Ok(Self {
            start,
            task_menu,
            media,
            panels,
            selected_output: None,
            transient_output: None,
            transient: None,
            start_motion: false,
            popup_slide: None,
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
                for surface in [self.task_menu, self.media] {
                    runtime.close_surface(surface).map_err(ui_error)?;
                }
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
        let work_area = self.selected().placement.work_area;
        let edge = self.selected().placement.panel.edge;
        runtime
            .apply_output_scale(
                self.start,
                panel_ui_scale(self.selected().placement.output.shell_scale_percent),
            )
            .map_err(ui_error)?;
        start_surface::open(runtime, self.selected().panel, self.start, work_area, edge)?;
        self.transient = Some(Transient::StartGroup);
        self.transient_output = Some(self.selected().placement.output.id.clone());
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
        let work_area = self.selected().placement.work_area;
        let edge = self.selected().placement.panel.edge;
        runtime
            .apply_output_scale(
                self.start,
                panel_ui_scale(self.selected().placement.output.shell_scale_percent),
            )
            .map_err(ui_error)?;
        let rect = start_surface::refit(
            runtime,
            self.selected().panel,
            self.start,
            work_area,
            edge,
            POPUP_GAP,
        )
        .map_err(|error| error.to_string())?;
        self.map_raise(runtime, self.start, rect)
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
        wallpaper_region: impl FnOnce((i32, i32, u32, u32)) -> Option<Vec<u8>>,
    ) -> Result<(), String> {
        let output = state
            .output
            .as_ref()
            .ok_or_else(|| "context menu has no source output".to_owned())?;
        if self.for_output(output).is_none() {
            return Err(format!("context menu output {} is unavailable", output));
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
            .ok_or_else(|| format!("context menu output {} is unavailable", output))?;
        let percent = panel.placement.output.shell_scale_percent;
        let logical = context_menu_size(rows);
        let size = flamewm_api::Size::new(
            flamewm_platform::panel::logical_to_physical(logical.width.max(0) as u16, percent),
            flamewm_platform::panel::logical_to_physical(logical.height.max(0) as u16, percent),
        );
        runtime
            .apply_output_scale(self.task_menu, panel_ui_scale(percent))
            .map_err(ui_error)?;
        let rect = context_menu_rect(
            panel.placement.panel.output.clone(),
            panel.placement.output.geometry,
            state.anchor,
            size,
        );
        let wallpaper_rect = (
            rect.x,
            rect.y,
            checked_size(rect.width)?,
            checked_size(rect.height)?,
        );
        runtime
            .set_popup_wallpaper_region(
                self.task_menu,
                wallpaper_rect,
                wallpaper_region(wallpaper_rect),
            )
            .map_err(ui_error)?;
        runtime
            .set_popup_backdrop(self.task_menu, Some(backdrop))
            .map_err(ui_error)?;
        let opened = self.show_grabbed(runtime, self.task_menu, rect, Transient::ContextMenu);
        if opened.is_err() {
            let _ = runtime.set_popup_wallpaper_region(self.task_menu, wallpaper_rect, None);
        }
        opened
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
                snapshot: None,
                capture: None,
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
    /// position alone. RandR can change CRTC coordinates without a reliable
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
    /// stale (for example if a previous ConfigureWindow failed). Compare the
    /// actual native rectangles for *every* output, not just snapshot revisions.
    #[must_use]
    pub fn panel_native_geometry_drifted(&self, runtime: &SurfaceRuntime) -> bool {
        self.panels.iter().any(|record| {
            let target = record.placement.panel.geometry;
            runtime
                .surface_device_rect(record.panel)
                .map_or(true, |rect| {
                    rect.x as i32 != target.x
                        || rect.y as i32 != target.y
                        || rect.width as i32 != target.width
                        || rect.height as i32 != target.height
                })
        })
    }

    /// F10: move_resize every panel native surface to its synced snapshot
    /// geometry. Returns `true` when at least one move_resize was issued.
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

    /// Resolve a surface-local pointer position (ActionEvent x/y from the
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

    /// Present step for the grabbed tail: canonical present_mapped path
    /// (show = geometry/move_resize origin-tracked + map/raise/present).
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
    bytes: &[u8],
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
fn panel_ui_scale(scale_percent: u16) -> f32 {
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
/// shell.quick_control.open_request, shell.quick_control.restart,
/// shell.signal_to_visual.windows, shell.signal_to_visual.workspaces,
/// shell.signal_to_visual.system.
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
mod tests {
    use super::*;
    use flamewm_api::{PanelEdge, Rect};

    fn test_output_rect() -> flamewm_api::Rect {
        flamewm_api::Rect::new(0, 0, 1920, 1080)
    }

    #[test]
    fn floating_bottom_bar_inset_is_consistent_and_start_popover_stays_above_it() {
        let output = Rect::new(0, 0, 1350, 641);
        // Floating bottom bar: 8 px margin below it, so bottom != output.bottom().
        let floating = Rect::new(8, 589, 1334, 44);
        assert!(edge_geometry_consistent(
            PanelEdge::Bottom,
            floating,
            output,
            FLOATING_MARGIN_MAX_LOGICAL
        ));
        // A stale Top edge against floating Bottom geometry is still refused.
        assert!(!edge_geometry_consistent(
            PanelEdge::Top,
            floating,
            output,
            FLOATING_MARGIN_MAX_LOGICAL
        ));
        // A bar that does not sit on the edge is refused.
        assert!(!edge_geometry_consistent(
            PanelEdge::Bottom,
            Rect::new(8, 500, 1334, 44),
            output,
            FLOATING_MARGIN_MAX_LOGICAL
        ));

        // Start popover anchored to the floating bar's Start button lands above the bar,
        // fully inside the work area.
        let work = work_area_for_panel(output, floating, PanelEdge::Bottom);
        let anchor = Rect::new(16, 597, 32, 28);
        let placed = crate::popup_controller::fit_start(
            anchor,
            flamewm_api::Size::new(550, 349),
            PanelEdge::Bottom,
            work,
            8,
        )
        .expect("floating start placement");
        assert!(placed.bottom() <= anchor.y - 8);
        assert!(placed.x >= work.x && placed.right() <= work.right());
        assert!(placed.y >= work.y);
    }

    #[test]
    fn panel_ui_scale_matches_platform_physical_thickness() {
        // 44 logical px * scale must equal the document's laid-out bar height.
        for percent in (100_u16..=200).step_by(5) {
            let scale = panel_ui_scale(percent);
            let thickness = (44.0 * scale).round() as i32;
            assert_eq!(thickness, ((44 * u32::from(percent) + 50) / 100) as i32);
        }
        assert_eq!(panel_ui_scale(111), 1.0);
    }

    fn test_output_id() -> flamewm_api::OutputId {
        flamewm_api::OutputId::new("HDMI-1")
    }

    #[test]
    fn workspace_right_click_rect_is_near_pointer_not_origin() {
        let size = context_menu_size(3);
        let pointer = Some(flamewm_api::Point::new(400, 900));
        let rect = context_menu_rect(test_output_id(), test_output_rect(), pointer, size);
        assert_eq!((rect.x, rect.y), (400, 900));
        assert_eq!((rect.width, rect.height), (size.width, size.height));
    }

    #[test]
    fn context_menu_is_clamped_inside_output() {
        let size = context_menu_size(3);
        let pointer = Some(flamewm_api::Point::new(1900, 1060));
        let rect = context_menu_rect(test_output_id(), test_output_rect(), pointer, size);
        assert_eq!(rect.width, size.width);
        assert_eq!(rect.height, size.height);
        assert!(rect.right() <= 1920);
        assert!(rect.bottom() <= 1080);
        assert!(rect.x >= 0 && rect.y >= 0);
    }

    #[test]
    fn context_menu_size_shrinks_to_visible_rows() {
        use flamewm_ui_core::context_menu::{MENU_PADDING, ROW_HEIGHT};
        let height = |rows: i32| 2 * MENU_PADDING + rows * ROW_HEIGHT;
        assert_eq!(context_menu_size(1).height, height(1));
        assert_eq!(context_menu_size(2).height, height(2));
        assert_eq!(context_menu_size(3).height, height(3));
        assert!(context_menu_size(1).height < 180);
        assert!(context_menu_size(3).height < 180);
    }

    #[test]
    fn absent_pointer_falls_back_to_output_origin() {
        let size = context_menu_size(2);
        let rect = context_menu_rect(test_output_id(), test_output_rect(), None, size);
        assert_eq!((rect.x, rect.y), (0, 0));
    }

    fn test_window(reference: flamewm_api::WindowRef) -> flamewm_api::window::WindowSnapshot {
        flamewm_api::window::WindowSnapshot {
            reference,
            title: "term".to_owned(),
            app_id: flamewm_api::DesktopAppId::new("term"),
            outer_geometry: flamewm_api::Rect::new(0, 0, 100, 100),
            restore_geometry: flamewm_api::Rect::new(0, 0, 100, 100),
            state: flamewm_api::window::WindowState::Normal,
            sticky: false,
            focused: true,
            workspace: flamewm_api::WorkspaceRef::new(0, 1),
            output: test_output_id(),
            state_generation: 1,
        }
    }

    // REGRESSION: the weather tray click must reach the popup opener. Without the
    // "weather.open" arm, dispatch yields no control and the popup never opens.
    #[test]
    fn weather_tray_click_dispatches_open_popover() {
        let mut runtime = ShellRuntime::default();
        runtime.dispatch("weather.open");
        let controls: Vec<_> = runtime.drain_controls().collect();
        assert!(
            matches!(controls.as_slice(), [ShellControl::OpenPopover { name, output: None, .. }] if name == "weather"),
            "{controls:?}"
        );
    }

    #[test]
    fn window_snapshot_signal_applies_without_fetch() {
        let mut runtime = ShellRuntime::default();
        let windows = vec![test_window(flamewm_api::WindowRef::new(1, 1))];
        runtime.mark_signal(&ControlSignal::WindowsSnapshotChanged {
            revision: 7,
            windows: windows.clone(),
        });
        assert_eq!(runtime.snapshot().windows, windows);
        let changed = runtime.reconcile_applied();
        assert!(changed.windows);
        assert!(!runtime.dirty().any());
    }

    #[test]
    fn legacy_window_revision_signal_sets_no_flag() {
        let mut runtime = ShellRuntime::default();
        runtime.mark_signal(&ControlSignal::WindowsChanged { revision: 9 });
        runtime.mark_signal(&ControlSignal::WorkspacesChanged { revision: 3 });
        runtime.mark_signal(&ControlSignal::PanelsChanged { revision: 4 });
        runtime.mark_signal(&ControlSignal::SystemChanged { revision: 6 });
        assert!(!runtime.dirty().any());
    }

    #[test]
    fn start_menu_toggle_signal_queues_one_toggle_start_control() {
        let mut runtime = ShellRuntime::default();
        runtime.mark_signal(&ControlSignal::StartMenuToggleRequested);
        let controls: Vec<_> = runtime.drain_controls().collect();
        assert_eq!(
            controls,
            vec![ShellControl::ToggleStart {
                output: None,
                interaction: None
            }]
        );
        assert!(!runtime.dirty().any());
    }

    #[test]
    fn start_menu_signal_and_click_dispatch_queue_identical_controls() {
        let mut by_signal = ShellRuntime::default();
        by_signal.mark_signal(&ControlSignal::StartMenuToggleRequested);
        assert!(by_signal.has_pending_controls());
        let mut by_click = ShellRuntime::default();
        by_click.dispatch("start.toggle");
        assert!(by_click.has_pending_controls());
        let signal_controls: Vec<_> = by_signal.drain_controls().collect();
        let click_controls: Vec<_> = by_click.drain_controls().collect();
        assert_eq!(signal_controls, click_controls);
        assert!(!by_signal.has_pending_controls());
    }

    #[test]
    fn workspace_snapshot_signal_updates_model_without_fetch() {
        let mut runtime = ShellRuntime::default();
        let snapshot = flamewm_api::workspace::WorkspaceSnapshot {
            revision: 5,
            count: 2,
            active_index: 1,
            last_index: Some(0),
            names: vec!["one".to_owned(), "two".to_owned()],
        };
        runtime.mark_signal(&ControlSignal::WorkspacesSnapshotChanged {
            snapshot: snapshot.clone(),
        });
        assert_eq!(runtime.snapshot().workspaces, Some(snapshot));
        let changed = runtime.reconcile_applied();
        assert!(changed.workspaces);
        assert!(!runtime.dirty().any());
    }

    #[test]
    fn panel_snapshot_signal_updates_task_model_without_fetch() {
        let mut runtime = ShellRuntime::default();
        let window = flamewm_api::WindowRef::new(2, 1);
        let snapshot = flamewm_api::panels::PanelsSnapshot {
            revision: 6,
            panels: Vec::new(),
            tasks: vec![flamewm_api::panels::TaskEntry {
                id: flamewm_api::TaskEntryId::window(window),
                app_id: flamewm_api::DesktopAppId::new("term"),
                kind: flamewm_api::panels::TaskEntryKind::Window { window },
                order_index: 0,
            }],
            pinned_apps: Vec::new(),
        };
        runtime.mark_signal(&ControlSignal::PanelsSnapshotChanged {
            snapshot: snapshot.clone(),
        });
        assert_eq!(runtime.snapshot().panels, snapshot);
        let changed = runtime.reconcile_applied();
        assert!(changed.panels);
        assert!(!runtime.dirty().any());
    }

    #[test]
    fn system_snapshot_signal_updates_model_without_fetch() {
        let mut runtime = ShellRuntime::default();
        let mut snapshot = runtime.snapshot().system.clone();
        snapshot.revision = 9;
        runtime.mark_signal(&ControlSignal::SystemSnapshotChanged {
            snapshot: snapshot.clone(),
        });
        assert_eq!(runtime.snapshot().system, snapshot);
        let changed = runtime.reconcile_dirty();
        assert!(changed.system);
        assert!(!runtime.dirty().any());
    }

    #[test]
    fn identical_snapshot_signal_sets_no_flag() {
        let mut runtime = ShellRuntime::default();
        let windows = runtime.snapshot().windows.clone();
        runtime.mark_signal(&ControlSignal::WindowsSnapshotChanged {
            revision: 1,
            windows,
        });
        assert!(!runtime.dirty().any());
    }

    #[test]
    fn red_t08_steady_state_has_no_blocking_get_system_reconciliation() {
        // RED(T08): current steady state still enters reconcile_dirty with a
        // ControlClient and calls GetSystem from the shell tick.
        let source = include_str!("runtime.rs")
            .split_once("\n#[cfg(test)]\nmod tests")
            .map(|(source, _)| source)
            .expect("runtime tests must have a production section");
        assert!(
            !source.contains("pub fn reconcile_dirty(&mut self, client: &ControlClient)"),
            "T08 RED: shell runtime must not expose blocking ControlClient reconciliation"
        );
        assert!(
            !source.contains("client.call(&ControlRequest::GetSystem)"),
            "T08 RED: steady-state shell updates must not fetch GetSystem"
        );
    }

    fn panel_output(id: &str, geometry: flamewm_api::Rect) -> flamewm_api::display::OutputSnapshot {
        flamewm_api::display::OutputSnapshot {
            id: flamewm_api::OutputId::new(id),
            connector: id.to_owned(),
            edid_identity: String::new(),
            connected: true,
            primary: true,
            geometry,
            current_mode: flamewm_api::ModeId(1),
            modes: Vec::new(),
            shell_scale_percent: 100,
        }
    }

    #[test]
    fn root_pointer_selects_negative_origin_output_with_half_open_bounds() {
        let outputs = vec![
            panel_output("A", flamewm_api::Rect::new(0, 0, 1920, 1080)),
            panel_output("B", flamewm_api::Rect::new(-1280, 100, 1280, 720)),
        ];
        assert_eq!(
            output_at_root_point(&outputs, flamewm_api::Point::new(-640, 500)),
            Some(flamewm_api::OutputId::new("B")),
        );
        assert_eq!(
            output_at_root_point(&outputs, flamewm_api::Point::new(0, 500)),
            Some(flamewm_api::OutputId::new("A")),
        );
        assert_eq!(
            output_at_root_point(&outputs, flamewm_api::Point::new(-1281, 500)),
            None
        );
    }

    fn panel_entry(
        output: &str,
        edge: flamewm_api::PanelEdge,
        geometry: flamewm_api::Rect,
    ) -> flamewm_api::panels::PanelSnapshot {
        flamewm_api::panels::PanelSnapshot {
            output: flamewm_api::OutputId::new(output),
            edge,
            logical_size: 44,
            visible: true,
            geometry,
        }
    }

    fn bottom_panel_snapshot(generation: u64, revision: u64) -> ShellSnapshot {
        placement_snapshot(
            flamewm_api::Rect::new(0, 0, 1920, 1080),
            flamewm_api::Rect::new(0, 1036, 1920, 44),
            PanelEdge::Bottom,
            generation,
            revision,
        )
    }

    fn placement_snapshot(
        output_geo: flamewm_api::Rect,
        panel_geo: flamewm_api::Rect,
        edge: flamewm_api::PanelEdge,
        generation: u64,
        revision: u64,
    ) -> ShellSnapshot {
        let mut snapshot = ShellSnapshot::default();
        snapshot.displays.generation = generation;
        snapshot.displays.outputs = vec![panel_output("HDMI-1", output_geo)];
        snapshot.panels.revision = revision;
        snapshot.panels.panels = vec![panel_entry("HDMI-1", edge, panel_geo)];
        snapshot
    }

    #[test]
    fn panel_context_top_to_bottom_recompute() {
        let top = placement_snapshot(
            flamewm_api::Rect::new(0, 0, 1920, 1080),
            flamewm_api::Rect::new(0, 0, 1920, 44),
            PanelEdge::Top,
            1,
            1,
        );
        let ctx = panel_context_for_snapshot(&top, PanelEdge::Top).expect("top context");
        assert_eq!(ctx.work_area, flamewm_api::Rect::new(0, 44, 1920, 1036));
        assert_eq!((ctx.display_generation, ctx.panels_revision), (1, 1));
        // Mixed edge rejected: Bottom snapshot against Top expectation.
        let bottom = bottom_panel_snapshot(2, 2);
        assert!(panel_context_for_snapshot(&bottom, PanelEdge::Top).is_none());
        let ctx2 = panel_context_for_snapshot(&bottom, PanelEdge::Bottom).expect("bottom context");
        assert_eq!(ctx2.work_area, flamewm_api::Rect::new(0, 0, 1920, 1036));
    }

    #[test]
    fn panel_context_bottom_excludes_panel_and_starts_above() {
        let snapshot = bottom_panel_snapshot(3, 5);
        let ctx = panel_context_for_snapshot(&snapshot, PanelEdge::Bottom).expect("context");
        assert_eq!(ctx.work_area.bottom(), 1036);
        assert!(ctx.work_area.height < 1080);
        // Start anchor (static layout) sits at the panel; the layout
        // popover must lie inside the work area, not overlap the panel.
        assert_eq!(ctx.start_layout.anchor.bottom(), 1080);
        assert!(ctx.start_layout.popover.bottom() <= ctx.work_area.bottom());
    }

    #[test]
    fn panel_context_quick_anchor_above_and_negative_origins() {
        use flamewm_shell_core::popup::fitted_start_placement;
        let output = flamewm_api::Rect::new(-1920, 0, 1920, 1080);
        let snapshot = placement_snapshot(
            output,
            flamewm_api::Rect::new(-1920, 1036, 1920, 44),
            PanelEdge::Bottom,
            4,
            6,
        );
        let ctx = panel_context_for_snapshot(&snapshot, PanelEdge::Bottom).expect("context");
        assert_eq!(ctx.work_area, flamewm_api::Rect::new(-1920, 0, 1920, 1036));
        // Quick anchor above the panel: end-aligned popover stays inside
        // the work area and above the panel top.
        let anchor = flamewm_api::Rect::new(-100, 1036, 30, 44);
        let placed = fitted_start_placement(
            anchor,
            flamewm_api::Size::new(310, 200),
            PanelEdge::Bottom,
            ctx.work_area,
            8,
        )
        .expect("placed");
        assert!(placed.rect.bottom() <= anchor.y - 8);
        assert!(placed.rect.x >= ctx.work_area.x);
    }

    #[test]
    fn panel_context_hidpi_panel_rect_used_as_is() {
        let snapshot = placement_snapshot(
            flamewm_api::Rect::new(0, 0, 3840, 2160),
            flamewm_api::Rect::new(0, 2072, 3840, 88),
            PanelEdge::Bottom,
            7,
            8,
        );
        let ctx = panel_context_for_snapshot(&snapshot, PanelEdge::Bottom).expect("context");
        assert_eq!(ctx.work_area, flamewm_api::Rect::new(0, 0, 3840, 2072));
    }

    #[test]
    fn f10_top_to_bottom_resync_uses_bottom() {
        use flamewm_shell_core::popup::work_area_for_panel;
        let output = flamewm_api::Rect::new(0, 0, 1920, 1080);
        let top = placement_snapshot(
            output,
            flamewm_api::Rect::new(0, 0, 1920, 44),
            PanelEdge::Top,
            1,
            1,
        );
        let initial = panel_context_for_snapshot(&top, PanelEdge::Top).expect("top context");
        assert_eq!(initial.panel.edge, PanelEdge::Top);
        let bottom = bottom_panel_snapshot(2, 2);
        // Next Start/Quick resolve through the synced resolver: stored Top
        // misses the gated path, the geometry-driven fallback returns the
        // Bottom context with Bottom generation/revision/work area.
        let synced =
            synced_placement_for_snapshot(&bottom, initial.panel.edge).expect("resync to bottom");
        assert_eq!(synced.panel.edge, PanelEdge::Bottom);
        assert_eq!((synced.display_generation, synced.panels_revision), (2, 2));
        assert_eq!(
            synced.work_area,
            work_area_for_panel(output, synced.panel.geometry, PanelEdge::Bottom)
        );
        assert_eq!(synced.work_area, flamewm_api::Rect::new(0, 0, 1920, 1036));
        // Gate the open paths would check: stale Top edge against the new
        // Bottom geometry is inconsistent; synced Bottom is consistent.
        assert!(!edge_geometry_consistent(
            PanelEdge::Top,
            synced.panel.geometry,
            synced.output.geometry,
            FLOATING_MARGIN_MAX_LOGICAL
        ));
        assert!(edge_geometry_consistent(
            PanelEdge::Bottom,
            synced.panel.geometry,
            synced.output.geometry,
            0
        ));
    }

    #[test]
    fn panel_context_missing_output_returns_none() {
        let mut snapshot = bottom_panel_snapshot(1, 1);
        snapshot.displays.outputs.clear();
        assert!(panel_context_for_snapshot(&snapshot, PanelEdge::Bottom).is_none());
        let mut empty = ShellSnapshot::default();
        empty.displays.outputs = vec![panel_output("HDMI-1", test_output_rect())];
        assert!(panel_context_for_snapshot(&empty, PanelEdge::Bottom).is_none());
    }

    /// Two panels (A shown first, then B) with tasks t1 and t2 on the windows below.
    fn task_slot_snapshot(outputs: &[&str]) -> ShellSnapshot {
        use flamewm_api::panels::{PanelSnapshot, TaskEntry, TaskEntryKind};
        use flamewm_api::window::WindowState;
        use flamewm_api::{
            DesktopAppId, OutputId, PanelEdge, TaskEntryId, WindowRef, WorkspaceRef,
        };
        let window_on = |id: u64, output: &str| WindowSnapshot {
            reference: WindowRef::new(id, 1),
            title: String::new(),
            app_id: DesktopAppId::new(if output == "B" { "konsole" } else { "dolphin" }),
            outer_geometry: flamewm_api::Rect::new(0, 0, 10, 10),
            restore_geometry: flamewm_api::Rect::new(0, 0, 10, 10),
            state: WindowState::Normal,
            sticky: false,
            focused: false,
            workspace: WorkspaceRef {
                index: 0,
                revision: 1,
            },
            output: OutputId::new(output),
            state_generation: 1,
        };
        let task = |id: u64, output: &str| {
            let window = WindowRef::new(id, 1);
            TaskEntry {
                id: TaskEntryId::window(window),
                app_id: DesktopAppId::new(if output == "B" { "konsole" } else { "dolphin" }),
                kind: TaskEntryKind::Window { window },
                order_index: id as usize,
            }
        };
        let panel = |output: &str| PanelSnapshot {
            output: OutputId::new(output),
            edge: PanelEdge::Bottom,
            logical_size: 40,
            visible: true,
            geometry: if output == "B" {
                flamewm_api::Rect::new(-1280, 776, 1280, 44)
            } else {
                flamewm_api::Rect::new(0, 1036, 1920, 44)
            },
        };
        let mut snapshot = ShellSnapshot::default();
        let ids: Vec<u64> = (1..=outputs.len() as u64).collect();
        snapshot.panels = PanelsSnapshot {
            revision: 7,
            panels: vec![panel("A"), panel("B")],
            tasks: ids
                .iter()
                .zip(outputs)
                .map(|(&id, output)| task(id, output))
                .collect(),
            pinned_apps: Vec::new(),
        };
        snapshot.displays = DisplaySnapshot {
            generation: 1,
            outputs: vec![
                panel_output("A", flamewm_api::Rect::new(0, 0, 1920, 1080)),
                panel_output("B", flamewm_api::Rect::new(-1280, 100, 1280, 720)),
            ],
            pending: None,
        };
        snapshot.windows = ids
            .iter()
            .zip(outputs)
            .map(|(&id, output)| window_on(id, output))
            .collect();
        snapshot
    }

    fn win_ref(id: u64) -> flamewm_api::WindowRef {
        flamewm_api::WindowRef::new(id, 1)
    }

    fn activated(id: u64) -> Option<Vec<ControlRequest>> {
        Some(vec![ControlRequest::ActivateWindow(win_ref(id))])
    }

    #[test]
    fn task_slot_click_maps_to_the_task_on_the_shown_panel_output() {
        // Window 1 on A: slot 0 of the shown panel A is window 1; B holds window 2 only.
        let mut runtime = ShellRuntime::new(task_slot_snapshot(&["A", "B"]));
        runtime.select_panel_output(flamewm_api::OutputId::new("A"));
        assert_eq!(runtime.task_click_requests_for_slot(0), activated(1));
        assert_eq!(runtime.task_click_requests_for_slot(1), None);
        assert_eq!(
            runtime.task_target_for_slot(0).unwrap().app_id.as_str(),
            "dolphin"
        );

        // Window 1 moves to B: panel A now shows nothing, so slot 0 no longer maps to window 1.
        runtime = ShellRuntime::new(task_slot_snapshot(&["B", "B"]));
        runtime.select_panel_output(flamewm_api::OutputId::new("A"));
        assert_eq!(runtime.task_click_requests_for_slot(0), None);
        // Panel B shown first: slot 0 is window 1, slot 1 is window 2.
        let mut snapshot = task_slot_snapshot(&["B", "B"]);
        snapshot.panels.panels.reverse();
        runtime = ShellRuntime::new(snapshot);
        runtime.select_panel_output(flamewm_api::OutputId::new("B"));
        assert_eq!(runtime.task_click_requests_for_slot(0), activated(1));
        assert_eq!(runtime.task_click_requests_for_slot(1), activated(2));
    }

    #[test]
    fn task_slot_actions_follow_the_output_that_received_the_input() {
        let mut runtime = ShellRuntime::new(task_slot_snapshot(&["A", "B", "B"]));
        let output = flamewm_api::OutputId::new("B");
        runtime.select_panel_output(output.clone());
        assert_eq!(runtime.task_click_requests_for_slot(0), activated(2));
        assert_eq!(runtime.task_click_requests_for_slot(1), activated(3));
        let first = runtime.task_target_for_slot(0).unwrap();
        let second = runtime.task_target_for_slot(1).unwrap();
        assert_eq!(first.app_id.as_str(), "konsole");
        assert_eq!(second.app_id.as_str(), "konsole");
        assert_eq!(first.window, Some(win_ref(2)));
        assert_eq!(second.window, Some(win_ref(3)));
        assert_eq!(runtime.task_click_requests_for_target(&first), activated(2));
        runtime.open_task_context_for_target(&second, None);
        assert!(matches!(
            runtime.controls.back(),
            Some(ShellControl::OpenContextMenu(state))
                if state.kind == ContextMenuKind::Task(second.id.clone())
                    && state.output.as_ref() == Some(&output)
        ));

        let _: Vec<_> = runtime.drain_controls().collect();
        runtime.open_task_context_at_slot(0, None);
        match runtime.controls.front() {
            Some(ShellControl::OpenContextMenu(state)) => {
                assert_eq!(
                    state.kind,
                    ContextMenuKind::Task(flamewm_api::TaskEntryId::window(win_ref(2)))
                );
                assert_eq!(state.output.as_ref(), Some(&output));
            }
            other => panic!("expected second-output task context, got {other:?}"),
        }
    }

    #[test]
    fn running_task_context_menu_pins_and_pinned_menu_unpins() {
        let mut runtime = ShellRuntime::new(task_slot_snapshot(&["A"]));
        runtime.select_panel_output(flamewm_api::OutputId::new("A"));
        let target = runtime.task_target_for_slot(0).unwrap();
        let menu = ContextMenuState {
            kind: ContextMenuKind::Task(target.id.clone()),
            revision: 7,
            output: Some(target.output.clone()),
            interaction: None,
            anchor: None,
        };
        assert_eq!(
            runtime.context_menu_request_for_action("task.menu.pin", Some(&menu)),
            Some(ControlRequest::PinApp {
                app: target.app_id.clone(),
                expected_revision: 7,
            })
        );

        let mut pinned = task_slot_snapshot(&["A"]);
        pinned.panels.tasks[0].id = flamewm_api::TaskEntryId::pinned(&target.app_id);
        pinned.panels.tasks[0].kind = flamewm_api::panels::TaskEntryKind::PinnedSlot {
            window: Some(win_ref(1)),
        };
        pinned.panels.pinned_apps = vec![target.app_id.clone()];
        let mut runtime = ShellRuntime::new(pinned);
        runtime.select_panel_output(flamewm_api::OutputId::new("A"));
        let target = runtime.task_target_for_slot(0).unwrap();
        let menu = ContextMenuState {
            kind: ContextMenuKind::Task(target.id.clone()),
            revision: 7,
            output: Some(target.output.clone()),
            interaction: None,
            anchor: None,
        };
        assert_eq!(
            runtime.context_menu_request_for_action("task.menu.unpin", Some(&menu)),
            Some(ControlRequest::UnpinApp {
                app: target.app_id,
                expected_revision: 7,
            })
        );
    }

    #[test]
    fn unbound_task_actions_do_not_fall_back_to_the_first_panel() {
        let runtime = ShellRuntime::new(task_slot_snapshot(&["A", "B"]));
        assert_eq!(runtime.task_target_for_slot(0), None);
        assert_eq!(runtime.task_click_requests_for_slot(0), None);
        assert_eq!(runtime.task_slot_count(), 0);
        assert_eq!(runtime.task_reorder_request(0, 1), None);
        assert_eq!(
            runtime
                .task_target_for_slot_on_output(0, &flamewm_api::OutputId::new("B"))
                .and_then(|target| target.window),
            Some(win_ref(2)),
            "a source-resolved output still identifies its own task slot"
        );
    }

    #[test]
    fn stale_task_slot_release_cannot_retarget_after_projection_refresh() {
        let mut initial = task_slot_snapshot(&["A", "B"]);
        let app = flamewm_api::DesktopAppId::new("konsole");
        initial.panels.tasks[1].id = flamewm_api::TaskEntryId::pinned(&app);
        initial.panels.tasks[1].kind = flamewm_api::panels::TaskEntryKind::PinnedSlot {
            window: Some(win_ref(2)),
        };
        let mut runtime = ShellRuntime::new(initial);
        runtime.select_panel_output(flamewm_api::OutputId::new("B"));
        let pressed = runtime.task_target_for_slot(0).unwrap();
        assert_eq!(pressed.window, Some(win_ref(2)));

        let mut same_task = task_slot_snapshot(&["A", "B"]);
        same_task.panels.tasks[1].id = flamewm_api::TaskEntryId::pinned(&app);
        same_task.panels.tasks[1].kind = flamewm_api::panels::TaskEntryKind::PinnedSlot {
            window: Some(win_ref(2)),
        };
        same_task.windows[1].focused = true;
        runtime = ShellRuntime::new(same_task);
        runtime.select_panel_output(flamewm_api::OutputId::new("B"));
        assert_eq!(
            runtime.task_click_requests_for_target(&pressed),
            Some(vec![ControlRequest::MinimizeWindow(win_ref(2))])
        );

        let mut refreshed = task_slot_snapshot(&["A", "B"]);
        refreshed.panels.tasks[1].id = flamewm_api::TaskEntryId::pinned(&app);
        refreshed.panels.tasks[1].kind = flamewm_api::panels::TaskEntryKind::PinnedSlot {
            window: Some(win_ref(3)),
        };
        refreshed.panels.tasks[1].order_index = 3;
        refreshed.windows[1].reference = win_ref(3);
        refreshed.windows[1].state_generation = 1;
        runtime = ShellRuntime::new(refreshed);
        runtime.select_panel_output(flamewm_api::OutputId::new("B"));
        assert_eq!(runtime.task_click_requests_for_target(&pressed), None);
        assert_eq!(runtime.task_click_requests_for_slot(0), activated(3));
        let _: Vec<_> = runtime.drain_controls().collect();
        runtime.open_task_context_for_target(&pressed, None);
        assert!(
            runtime.controls.is_empty(),
            "stale secondary release must not open another task's menu"
        );
    }

    #[test]
    fn task_reorder_uses_captured_identity_and_current_output_slots() {
        let mut runtime = ShellRuntime::new(task_slot_snapshot(&["A", "B", "B"]));
        runtime.select_panel_output(flamewm_api::OutputId::new("B"));
        let source = runtime.task_target_for_slot(0).unwrap();
        assert_eq!(
            runtime.task_reorder_request_for_target(&source, 1),
            Some(ControlRequest::ReorderTask {
                entry_id: flamewm_api::TaskEntryId::window(win_ref(2)),
                index: 2,
                expected_revision: 7,
            })
        );

        let mut changed = task_slot_snapshot(&["A", "B", "B"]);
        changed.panels.tasks[1].kind = flamewm_api::panels::TaskEntryKind::PinnedSlot {
            window: Some(win_ref(3)),
        };
        changed.panels.tasks[1].id = source.id.clone();
        changed.panels.tasks[1].app_id = source.app_id.clone();
        changed.windows[1].reference = win_ref(3);
        runtime = ShellRuntime::new(changed);
        runtime.select_panel_output(source.output.clone());
        assert_eq!(runtime.task_reorder_request_for_target(&source, 1), None);
    }

    #[test]
    fn popup_openers_capture_explicit_output_across_selection_changes() {
        let mut runtime = ShellRuntime::new(task_slot_snapshot(&["A", "B"]));
        let previous_selection = flamewm_api::OutputId::new("A");
        let output = flamewm_api::OutputId::new("B");
        runtime.select_panel_output(previous_selection.clone());
        for action in [
            "start.toggle",
            "volume.open",
            "network.open",
            "battery.open",
            "clock.open",
            "weather.open",
            "media.open",
        ] {
            runtime.dispatch_with_output(action, Some(output.clone()));
            runtime.select_panel_output(previous_selection.clone());
        }
        let controls: Vec<_> = runtime.drain_controls().collect();
        assert!(matches!(
            controls.first(),
            Some(ShellControl::ToggleStart { output: Some(target), .. }) if target == &output
        ));
        assert_eq!(controls.len(), 7);
        for control in &controls[1..] {
            assert!(matches!(
                control,
                ShellControl::OpenPopover { output: Some(target), .. } if target == &output
            ));
        }
    }

    #[test]
    fn global_start_dispatch_does_not_capture_ambient_panel_selection() {
        let mut runtime = ShellRuntime::new(task_slot_snapshot(&["A", "B"]));
        runtime.select_panel_output(flamewm_api::OutputId::new("A"));
        runtime.dispatch("start.toggle");
        assert!(matches!(
            runtime.drain_controls().next(),
            Some(ShellControl::ToggleStart { output: None, .. })
        ));
    }

    #[test]
    fn switching_workspaces_changes_task_click_targets_without_a_panel_update() {
        let mut snapshot = task_slot_snapshot(&["A", "A"]);
        snapshot.windows[1].workspace.index = 1;
        snapshot.workspaces = Some(flamewm_api::workspace::WorkspaceSnapshot {
            revision: 1,
            count: 2,
            active_index: 0,
            last_index: None,
            names: Vec::new(),
        });
        let mut runtime = ShellRuntime::new(snapshot);
        runtime.select_panel_output(flamewm_api::OutputId::new("A"));
        assert_eq!(runtime.task_click_requests_for_slot(0), activated(1));
        assert_eq!(runtime.task_click_requests_for_slot(1), None);
        let next = flamewm_api::workspace::WorkspaceSnapshot {
            revision: 2,
            count: 2,
            active_index: 1,
            last_index: Some(0),
            names: Vec::new(),
        };
        runtime.mark_signal(&ControlSignal::WorkspacesSnapshotChanged { snapshot: next });
        let changed = runtime.reconcile_applied();
        assert!(changed.workspaces);
        assert!(
            !changed.windows,
            "no window snapshot update should be necessary"
        );
        assert_eq!(runtime.task_click_requests_for_slot(0), activated(2));
        assert_eq!(runtime.task_click_requests_for_slot(1), None);
    }

    fn workspace_snapshot(
        count: usize,
        active_index: usize,
    ) -> flamewm_api::workspace::WorkspaceSnapshot {
        flamewm_api::workspace::WorkspaceSnapshot {
            revision: 1,
            count,
            active_index,
            last_index: None,
            names: Vec::new(),
        }
    }

    // REGRESSION: a pager action names a topology CELL, not a page slot. With
    // six workspaces (`1 2 3 / 4 5 6`, column-major cells) cell 2 is workspace
    // 4, so the click must activate index 3 instead of the second page slot.
    // MUTATION: keep mapping `workspace-N` to page slot `N - 1`.
    #[test]
    fn pager_cell_actions_activate_the_workspace_in_that_cell() {
        let mut snapshot = ShellSnapshot::default();
        snapshot.workspaces = Some(workspace_snapshot(6, 0));
        let runtime = ShellRuntime::new(snapshot);
        assert_eq!(
            runtime.control_request_for_action("workspace.2"),
            Some(ControlRequest::ActivateWorkspace {
                index: 3,
                expected_revision: 1,
            })
        );
        assert_eq!(
            runtime.control_request_for_action("workspace-4"),
            Some(ControlRequest::ActivateWorkspace {
                index: 4,
                expected_revision: 1,
            })
        );
        assert_eq!(
            runtime.control_request_for_action("workspace.6"),
            Some(ControlRequest::ActivateWorkspace {
                index: 5,
                expected_revision: 1,
            })
        );
        // Cells 7..=18 hold no workspace when only six exist, and a missing
        // snapshot has no topology to resolve against.
        assert_eq!(runtime.control_request_for_action("workspace-18"), None);
        assert_eq!(runtime.control_request_for_action("workspace.0"), None);
        assert_eq!(runtime.control_request_for_action("task.slot.1"), None);
        let runtime = ShellRuntime::new(ShellSnapshot::default());
        assert_eq!(runtime.control_request_for_action("workspace.1"), None);
    }

    #[test]
    fn pager_keeps_global_workspace_identity_while_input_origin_selects_local_tasks() {
        let mut snapshot = task_slot_snapshot(&["A", "B"]);
        snapshot.workspaces = Some(workspace_snapshot(6, 0));
        let mut runtime = ShellRuntime::new(snapshot);
        let workspace_request = Some(ControlRequest::ActivateWorkspace {
            index: 3,
            expected_revision: 1,
        });

        runtime.select_panel_output(flamewm_api::OutputId::new("A"));
        assert_eq!(
            runtime.control_request_for_action("workspace.2"),
            workspace_request
        );
        assert_eq!(runtime.task_click_requests_for_slot(0), activated(1));

        runtime.select_panel_output(flamewm_api::OutputId::new("B"));
        assert_eq!(
            runtime.control_request_for_action("workspace.2"),
            workspace_request
        );
        assert_eq!(runtime.task_click_requests_for_slot(0), activated(2));
    }

    #[test]
    fn global_workspace_snapshot_refreshes_task_views_on_each_output() {
        let mut snapshot = task_slot_snapshot(&["A", "B"]);
        snapshot.windows[1].workspace.index = 1;
        snapshot.workspaces = Some(workspace_snapshot(2, 0));
        let mut runtime = ShellRuntime::new(snapshot);

        runtime.select_panel_output(flamewm_api::OutputId::new("A"));
        assert_eq!(runtime.task_click_requests_for_slot(0), activated(1));
        runtime.select_panel_output(flamewm_api::OutputId::new("B"));
        assert_eq!(runtime.task_click_requests_for_slot(0), None);

        runtime.mark_signal(&ControlSignal::WorkspacesSnapshotChanged {
            snapshot: flamewm_api::workspace::WorkspaceSnapshot {
                revision: 2,
                count: 2,
                active_index: 1,
                last_index: Some(0),
                names: Vec::new(),
            },
        });
        let changed = runtime.reconcile_dirty();
        assert!(changed.workspaces);
        assert!(!changed.windows);

        runtime.select_panel_output(flamewm_api::OutputId::new("A"));
        assert_eq!(runtime.task_click_requests_for_slot(0), None);
        runtime.select_panel_output(flamewm_api::OutputId::new("B"));
        assert_eq!(runtime.task_click_requests_for_slot(0), activated(2));
    }

    // REGRESSION: the workspace context menu follows the clicked cell, and an
    // empty cell must not open the menu of a neighbouring workspace.
    #[test]
    fn workspace_context_menu_opens_for_the_cell_workspace_only() {
        let mut snapshot = ShellSnapshot::default();
        snapshot.workspaces = Some(workspace_snapshot(6, 0));
        let mut runtime = ShellRuntime::new(snapshot);
        runtime.open_workspace_context_at_cell(6, None);
        match runtime.controls.front() {
            Some(ShellControl::OpenContextMenu(state)) => {
                assert_eq!(state.kind, ContextMenuKind::Workspace { index: 5 });
            }
            other => panic!("expected workspace context menu, got {other:?}"),
        }
        runtime.controls.clear();
        runtime.open_workspace_context_at_cell(18, None);
        assert!(runtime.controls.is_empty(), "empty cell opens nothing");
    }

    #[test]
    fn task_slot_context_menu_follows_the_shown_panel_output() {
        let mut runtime = ShellRuntime::new(task_slot_snapshot(&["B", "B"]));
        runtime.open_task_context_at_slot(0, None);
        assert!(runtime.controls.is_empty(), "panel A shows no tasks");

        let mut snapshot = task_slot_snapshot(&["A", "B"]);
        snapshot.panels.panels.reverse();
        runtime = ShellRuntime::new(snapshot);
        runtime.select_panel_output(flamewm_api::OutputId::new("B"));
        runtime.open_task_context_at_slot(0, None);
        // Panel B shown first: slot 0 is window 2 (task 2) only after window 1 leaves B.
        let expected = flamewm_api::TaskEntryId::window(win_ref(2));
        match runtime.controls.front() {
            Some(ShellControl::OpenContextMenu(state)) => {
                assert_eq!(state.kind, ContextMenuKind::Task(expected));
            }
            other => panic!("expected task context menu, got {other:?}"),
        }
    }

    #[test]
    fn task_slot_reorder_skips_hidden_entries_with_full_list_index() {
        // Tasks 1, 2, 4 on B (shown first), task 3 on A (hidden). Slot 1 is task 2, slot 2 is task 4.
        let mut snapshot = task_slot_snapshot(&["B", "B", "A", "B"]);
        snapshot.panels.panels.reverse();
        let mut runtime = ShellRuntime::new(snapshot);
        runtime.select_panel_output(flamewm_api::OutputId::new("B"));
        assert_eq!(runtime.task_slot_count(), 3);
        assert_eq!(
            runtime.task_reorder_request(1, 2),
            Some(ControlRequest::ReorderTask {
                entry_id: flamewm_api::TaskEntryId::window(win_ref(2)),
                index: 3,
                expected_revision: 7,
            })
        );
    }

    #[test]
    fn task_slot_reorder_uses_shown_output_slots_and_full_list_index() {
        // Panel B shown first with both windows on B: slots equal the full task list.
        let mut snapshot = task_slot_snapshot(&["B", "B"]);
        snapshot.panels.panels.reverse();
        let mut runtime = ShellRuntime::new(snapshot);
        runtime.select_panel_output(flamewm_api::OutputId::new("B"));
        assert_eq!(runtime.task_slot_count(), 2);
        assert_eq!(
            runtime.task_reorder_request(0, 1),
            Some(ControlRequest::ReorderTask {
                entry_id: flamewm_api::TaskEntryId::window(win_ref(1)),
                index: 1,
                expected_revision: 7,
            })
        );

        // Window 1 on A: panel B shows only window 2, so its single slot cannot reorder.
        let mut snapshot = task_slot_snapshot(&["A", "B"]);
        snapshot.panels.panels.reverse();
        let mut runtime = ShellRuntime::new(snapshot);
        runtime.select_panel_output(flamewm_api::OutputId::new("B"));
        assert_eq!(runtime.task_slot_count(), 1);
        assert_eq!(runtime.task_reorder_request(0, 1), None);
    }

    fn two_output_snapshot() -> ShellSnapshot {
        let mut snapshot = ShellSnapshot::default();
        snapshot.displays.generation = 3;
        snapshot.displays.outputs = vec![
            panel_output("HDMI-1", flamewm_api::Rect::new(0, 0, 1920, 1080)),
            panel_output("HDMI-2", flamewm_api::Rect::new(1920, 0, 1280, 1024)),
        ];
        snapshot.panels.revision = 5;
        snapshot.panels.panels = vec![
            panel_entry(
                "HDMI-1",
                flamewm_api::PanelEdge::Bottom,
                flamewm_api::Rect::new(0, 1036, 1920, 44),
            ),
            panel_entry(
                "HDMI-2",
                flamewm_api::PanelEdge::Bottom,
                flamewm_api::Rect::new(1920, 980, 1280, 44),
            ),
        ];
        snapshot
    }

    #[test]
    fn two_outputs_two_panels_yield_one_context_per_output() {
        let snapshot = two_output_snapshot();
        let contexts = panel_contexts_for_snapshot(&snapshot, PanelEdge::Bottom);
        assert_eq!(contexts.len(), 2);
        assert_eq!(contexts[0].output.id, flamewm_api::OutputId::new("HDMI-1"));
        assert_eq!(
            contexts[0].output.geometry,
            flamewm_api::Rect::new(0, 0, 1920, 1080)
        );
        assert_eq!(
            contexts[0].work_area,
            flamewm_api::Rect::new(0, 0, 1920, 1036)
        );
        assert_eq!(contexts[1].output.id, flamewm_api::OutputId::new("HDMI-2"));
        assert_eq!(
            contexts[1].output.geometry,
            flamewm_api::Rect::new(1920, 0, 1280, 1024)
        );
        assert_eq!(
            contexts[1].panel.geometry,
            flamewm_api::Rect::new(1920, 980, 1280, 44)
        );
        assert_eq!(
            contexts[1].work_area,
            flamewm_api::Rect::new(1920, 0, 1280, 980)
        );
        assert!(panel_contexts_for_snapshot(&snapshot, PanelEdge::Top).is_empty());
    }

    #[test]
    fn secondary_monitor_swap_with_unchanged_generation_is_detected() {
        let previous = two_output_snapshot();
        let contexts = panel_contexts_for_snapshot(&previous, PanelEdge::Bottom);
        assert!(contexts
            .iter()
            .all(|ctx| panel_context_is_current(ctx, &previous)));
        let mut moved = previous.clone();
        // KDE or xrandr may deliver CRTC geometry before changing the
        // generation/revision: secondary placement must not stay cached.
        moved.displays.outputs[1].geometry.x = 0;
        moved.displays.outputs[0].geometry.x = 1280;
        assert!(!panel_context_is_current(&contexts[1], &moved));
        assert!(!panel_context_is_current(&contexts[0], &moved));
    }

    #[test]
    fn popup_context_survives_unrelated_output_and_global_revision_changes() {
        let previous = two_output_snapshot();
        let old = panel_contexts_for_snapshot(&previous, PanelEdge::Bottom);
        let mut next = previous.clone();
        next.displays.generation += 1;
        next.panels.revision += 1;
        next.displays.outputs[1].geometry.x += 80;
        next.panels.panels[1].geometry.x += 80;
        let new = panel_contexts_for_snapshot(&next, PanelEdge::Bottom);
        assert!(
            popup_context_unchanged(&old[0], &new[0]),
            "unchanged A keeps its popup"
        );
        assert!(
            !popup_context_unchanged(&old[1], &new[1]),
            "changed B invalidates its popup"
        );
    }

    #[test]
    fn mixed_panel_edges_resolve_independently() {
        let mut snapshot = two_output_snapshot();
        snapshot.panels.panels[1].edge = PanelEdge::Top;
        snapshot.panels.panels[1].geometry.y = 0;
        let contexts: Vec<_> = snapshot
            .panels
            .panels
            .iter()
            .filter_map(|panel| panel_context_for_panel(&snapshot, panel))
            .collect();
        assert_eq!(contexts.len(), 2);
        assert_eq!(contexts[0].panel.edge, PanelEdge::Bottom);
        assert_eq!(contexts[1].panel.edge, PanelEdge::Top);
    }

    #[test]
    fn single_output_contexts_match_legacy_single_context() {
        let snapshot = bottom_panel_snapshot(1, 1);
        let contexts = panel_contexts_for_snapshot(&snapshot, PanelEdge::Bottom);
        let legacy = panel_context_for_snapshot(&snapshot, PanelEdge::Bottom);
        assert_eq!(contexts.len(), 1);
        assert_eq!(Some(contexts[0].clone()), legacy);
    }

    #[test]
    fn hotplug_removal_of_one_output_removes_only_its_panel_surface() {
        let mut snapshot = two_output_snapshot();
        snapshot.displays.outputs[1].connected = false;
        let existing = vec![
            flamewm_api::OutputId::new("HDMI-1"),
            flamewm_api::OutputId::new("HDMI-2"),
        ];
        let stale = panel_surface_outputs_to_remove(&existing, &snapshot);
        assert_eq!(stale, vec![flamewm_api::OutputId::new("HDMI-2")]);
        let kept = panel_surface_outputs_to_remove(&existing[..1], &snapshot);
        assert!(kept.is_empty());
    }

    #[test]
    fn panel_reconciliation_plans_updates_creates_and_removals_before_mutation() {
        let snapshot = two_output_snapshot();
        let current = panel_contexts_for_snapshot(&snapshot, PanelEdge::Bottom);
        let existing = current
            .iter()
            .map(|context| (context.output.id.clone(), context.clone()))
            .collect::<Vec<_>>();

        let mut updated_a = current[0].clone();
        updated_a.panels_revision += 1;
        let mut added_c = current[1].clone();
        added_c.output.id = flamewm_api::OutputId::new("C");
        added_c.panel.output = flamewm_api::OutputId::new("C");
        let desired = vec![updated_a, current[1].clone(), added_c];
        assert_eq!(
            panel_reconciliation_plan(&existing, &desired),
            PanelReconcilePlan {
                update: vec![flamewm_api::OutputId::new("HDMI-1")],
                create: vec![flamewm_api::OutputId::new("C")],
                remove: Vec::new(),
            },
            "unchanged B keeps its record while A updates and C is staged"
        );

        let only_a = vec![current[0].clone()];
        assert_eq!(
            panel_reconciliation_plan(&existing, &only_a).remove,
            vec![flamewm_api::OutputId::new("HDMI-2")],
            "disconnect retires only its output record"
        );
    }
}

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

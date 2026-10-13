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
    decode_document, DockEdge, GeometryTrace, SurfaceConfig, SurfaceHandle, SurfaceInputMode,
    SurfaceRole, SurfaceRuntime, UiBackendError, UiTemplate,
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
pub enum ShellControl {
    ToggleStart,
    CloseStart,
    SelectStartCategory(StartCategory),
    Launch(String),
    ActivateWindow(flamewm_api::WindowRef),
    MinimizeWindow(flamewm_api::WindowRef),
    RestoreWindow(flamewm_api::WindowRef),
    SystemAction {
        action: SystemAction,
        expected_revision: u64,
    },
    SessionAction(SessionAction),
    OpenPopover(String),
    ClosePopovers,
    OpenContextMenu(ContextMenuState),
    CloseContextMenu,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextMenuKind {
    Task(flamewm_api::TaskEntryId),
    Workspace { index: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextMenuState {
    pub kind: ContextMenuKind,
    pub revision: u64,
    /// Root-coordinate pointer anchor (ActionEvent local x/y resolved
    /// against the source surface global rect). `None` only when pointer
    /// data was absent; that is the sole output-origin fallback case.
    pub anchor: Option<flamewm_api::Point>,
}

#[derive(Debug, Clone)]
pub struct ShellRuntime {
    snapshot: ShellSnapshot,
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
                self.controls.push_back(ShellControl::ToggleStart);
            }
            _ => {}
        }
    }

    /// Reconcile already-applied snapshot domains into a change set without
    /// any fetch. Every steady-state domain is supplied by a snapshot signal;
    /// this method only consumes dirty flags and never owns a control client.
    pub fn reconcile_dirty(&mut self) -> ChangedDomains {
        let mut changed = ChangedDomains::default();
        if self.dirty.windows {
            changed.windows = true;
            self.dirty.windows = false;
            domain_reconcile_counter(DomainKind::Windows).increment();
        }
        if self.dirty.workspaces {
            changed.workspaces = true;
            self.dirty.workspaces = false;
            domain_reconcile_counter(DomainKind::Workspaces).increment();
        }
        if self.dirty.panels {
            changed.panels = true;
            self.dirty.panels = false;
            domain_reconcile_counter(DomainKind::Panels).increment();
        }
        if self.dirty.system {
            changed.system = true;
            self.dirty.system = false;
            domain_reconcile_counter(DomainKind::System).increment();
        }
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

    pub fn replace_snapshot(&mut self, snapshot: ShellSnapshot) {
        self.snapshot = snapshot;
    }

    #[must_use]
    pub fn control_request_for_action(&self, action: &str) -> Option<ControlRequest> {
        let slot = action
            .strip_prefix("workspace-")
            .or_else(|| action.strip_prefix("workspace."))
            .and_then(|value| value.parse::<usize>().ok())?
            .checked_sub(1)?;
        let workspaces = crate::taskbar::workspaces::project(self.snapshot.workspaces.as_ref());
        let page = crate::taskbar::workspaces::visible_page(&workspaces);
        let workspace = page.get(slot)?;
        crate::taskbar::workspaces::control_request_for_click(
            self.snapshot.workspaces.as_ref(),
            workspace.index,
        )
    }

    /// Map a stable context-menu row action (`task.menu.<row>` /
    /// `workspace.menu.<row>`) plus the open menu state to a typed control.
    /// Row identity is the row id, never the label. Pinned task entries
    /// resolve `unpin` only; running non-pinned entries resolve
    /// `activate`/`close`. Workspace rows are revision-fenced against the
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
                    _ => None,
                }
            }
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
    #[must_use]
    pub fn task_reorder_request(&self, from: usize, to: usize) -> Option<ControlRequest> {
        let entry = self.snapshot.panels.tasks.get(from)?;
        (to < self.snapshot.panels.tasks.len()).then(|| ControlRequest::ReorderTask {
            entry_id: entry.id.clone(),
            index: to,
            expected_revision: self.snapshot.panels.revision,
        })
    }

    #[must_use]
    pub fn task_click_requests_for_slot(&self, slot: usize) -> Option<Vec<ControlRequest>> {
        let task_visual_states = flamewm_shell_core::task_visual_states(
            &self.snapshot.panels,
            &self.snapshot.windows,
            &self.snapshot.applications,
        );
        let state = task_visual_states.get(slot)?;
        let entry = self
            .snapshot
            .panels
            .tasks
            .iter()
            .find(|entry| entry.id == state.id)?;
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

    pub fn open_task_context_for_slot(&mut self, slot: usize) {
        self.open_task_context_at_slot(slot, None);
    }

    /// Pointer-anchored variant: `pointer` is the root-coordinate click
    /// point (local x/y + source surface global rect).
    pub fn open_task_context_at_slot(&mut self, slot: usize, pointer: Option<flamewm_api::Point>) {
        let states = flamewm_shell_core::task_visual_states(
            &self.snapshot.panels,
            &self.snapshot.windows,
            &self.snapshot.applications,
        );
        if let Some(state) = states.get(slot) {
            self.controls
                .push_back(ShellControl::OpenContextMenu(ContextMenuState {
                    kind: ContextMenuKind::Task(state.id.clone()),
                    revision: self.snapshot.panels.revision,
                    anchor: pointer,
                }));
        }
    }

    pub fn open_workspace_context_for_slot(&mut self, slot: usize) {
        self.open_workspace_context_at_slot(slot, None);
    }

    /// Pointer-anchored variant: `pointer` is the root-coordinate click
    /// point (local x/y + source surface global rect).
    pub fn open_workspace_context_at_slot(
        &mut self,
        slot: usize,
        pointer: Option<flamewm_api::Point>,
    ) {
        let views = crate::taskbar::workspaces::project(self.snapshot.workspaces.as_ref());
        if let Some(workspace) = crate::taskbar::workspaces::visible_page(&views).get(slot) {
            if let Some(snapshot) = self.snapshot.workspaces.as_ref() {
                self.controls
                    .push_back(ShellControl::OpenContextMenu(ContextMenuState {
                        kind: ContextMenuKind::Workspace {
                            index: workspace.index,
                        },
                        revision: snapshot.revision,
                        anchor: pointer,
                    }));
            }
        }
    }

    pub fn dispatch(&mut self, action: &str) {
        let control = match action {
            "start.toggle" => Some(ShellControl::ToggleStart),
            "start.close" => Some(ShellControl::CloseStart),
            value if value.starts_with("start.category.") => {
                start::category_for_action(value).map(ShellControl::SelectStartCategory)
            }
            "media.open" | "media.toggle" | "volume.open" | "network.open" | "clock.open" => Some(
                ShellControl::OpenPopover(action.split('.').next().unwrap_or(action).to_owned()),
            ),
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
    let panel = snapshot.panels.panels.first().cloned()?;
    if panel.edge != expected_edge {
        return None;
    }
    let output = snapshot
        .displays
        .outputs
        .iter()
        .find(|output| output.id == panel.output && output.connected)
        .cloned()?;
    Some(PanelPlacementContext::new(
        output,
        panel,
        snapshot.displays.generation,
        snapshot.panels.revision,
    ))
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
        .filter_map(|panel| {
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
        })
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
    transient: Option<Transient>,
    /// A Start slide, fade, or Magic Lamp is configured, so Start toggles close through `hide`.
    start_motion: bool,
    /// Root pixels under the primary panel, captured before its first map (see `surface_backdrop`).
    panel_snapshot: Option<RgbaImage>,
    /// Key and outcome of the last panel capture attempt (see `refresh_panel_backdrop`).
    panel_capture: Option<PanelCapture>,
}

/// One panel capture attempt: the key it was taken for, attempts so far, and whether it is final.
#[derive(Debug, Clone)]
struct PanelCapture {
    key: CaptureKey,
    attempts: u8,
    settled: bool,
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
    runtime
        .apply_output_scale(panel, panel_ui_scale(placement.output.shell_scale_percent))
        .map_err(|error| format!("panel ui scale: {error:?}"))?;
    Ok(panel)
}

impl ShellSurfaces {
    fn primary(&self) -> &PanelSurfaceRecord {
        // Invariant: `create` and `sync_panel_context` never leave `panels`
        // empty, and the primary record is kept at index 0.
        &self.panels[0]
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

    /// Re-opens a settled-but-blank panel capture (after our own panel Map), so the next refresh
    /// captures again. A non-uniform capture that already installed is left alone.
    pub fn reopen_panel_capture(&mut self) {
        if let Some(capture) = self
            .panel_capture
            .as_mut()
            .filter(|capture| capture.settled && self.panel_snapshot.is_none())
        {
            capture.attempts = 0;
            capture.settled = false;
        }
    }

    /// Whether the panel snapshot is still unsettled (never captured, or a blank root awaiting retry).
    #[must_use]
    pub fn panel_capture_pending(&self) -> bool {
        self.panel_capture
            .as_ref()
            .is_none_or(|capture| !capture.settled)
    }

    /// Recomputes the panel snapshot when its key changed (wallpaper, fit, or bar rect). `wallpaper`
    /// supplies the fitted wallpaper pixels under the bar; those are deterministic and used as-is.
    /// Only when it yields nothing (wallpaper unavailable) does the root get read under the unmapped
    /// bar, and a blank root is retried on the next call, up to `MAX_CAPTURE_ATTEMPTS`. Each call is
    /// driven by a settings or panel event, so no timer is involved.
    pub fn refresh_panel_backdrop(
        &mut self,
        runtime: &mut SurfaceRuntime,
        key: CaptureKey,
        wallpaper: impl FnOnce() -> Option<RgbaImage>,
    ) -> Result<(), String> {
        runtime.watch_root_changes();
        let attempts_so_far = match &self.panel_capture {
            Some(capture) if capture.key == key && capture.settled => return Ok(()),
            Some(capture) if capture.key == key => capture.attempts,
            _ => 0,
        };
        let panel = self.panel();
        let attempts = attempts_so_far.saturating_add(1);
        let (step, raw) = match wallpaper() {
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
            "snapshot rect={:?} size={}x{} step={step:?} attempts={attempts}",
            key.rect, raw.width, raw.height
        ));
        self.panel_snapshot = (step == CaptureStep::Use).then_some(raw);
        self.panel_capture = Some(PanelCapture {
            key,
            attempts,
            settled: step != CaptureStep::Retry,
        });
        Ok(())
    }

    /// Installs the blurred snapshot under the primary panel. Opaque bars, missing snapshots and
    /// a bar resized since capture get no backdrop (solid colour, no stale pixels).
    pub fn apply_panel_backdrop(
        &self,
        runtime: &mut SurfaceRuntime,
        opacity_percent: u8,
        blur_percent: u8,
    ) -> Result<(), String> {
        let panel = self.panel();
        let rect = runtime.surface_device_rect(panel).map_err(ui_error)?;
        let fitting = self.panel_snapshot.as_ref().filter(|snapshot| {
            opacity_percent < 100
                && snapshot.width as f32 == rect.width
                && snapshot.height as f32 == rect.height
        });
        crate::surface_backdrop::backdrop_trace(format_args!(
            "apply rect={}x{}@{},{} opacity={opacity_percent} blur={blur_percent} snapshot={:?} install={}",
            rect.width, rect.height, rect.x, rect.y,
            self.panel_snapshot.as_ref().map(|s| (s.width, s.height)),
            fitting.is_some()
        ));
        match fitting {
            Some(snapshot) => {
                let words = blurred_argb(snapshot, opacity_percent, blur_percent);
                runtime
                    .set_surface_backdrop(panel, Some((&words, snapshot.width, snapshot.height)))
                    .map_err(ui_error)
            }
            None => runtime.set_surface_backdrop(panel, None).map_err(ui_error),
        }
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
        let first = snapshot
            .panels
            .panels
            .first()
            .cloned()
            .ok_or_else(|| "GetPanels returned no panel".to_owned())?;
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
                "FlameWM media",
                flamewm_shell_core::MEDIA_POPOVER_SIZE,
                panel_anchor(&panel_snapshot, &output, metrics, 1),
            ),
        )?;
        let mut panels = vec![PanelSurfaceRecord {
            panel: primary_panel,
            placement: primary_placement.clone(),
        }];
        for context in contexts {
            if context.output.id == primary_placement.output.id {
                continue;
            }
            let panel = create_panel_surface(runtime, &context)?;
            panels.push(PanelSurfaceRecord {
                panel,
                placement: context,
            });
        }
        Ok(Self {
            start,
            task_menu,
            media,
            panels,
            transient: None,
            start_motion: false,
            panel_snapshot: None,
            panel_capture: None,
        })
    }

    /// Proven close branch: single-turn close per surface (ungrab-once
    /// when grabbed + unmap + Expose-present, same turn, no sleeps, no
    /// compositor), then clear the open marker. Never touches panel or
    /// mapped popup geometry.
    pub fn close_transients(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        for surface in [self.start, self.task_menu, self.media] {
            runtime.close_surface(surface).map_err(ui_error)?;
        }
        self.transient = None;
        Ok(())
    }

    /// Sets whether Start toggles animate out (a Start slide, fade, or lamp is configured).
    pub fn set_start_motion(&mut self, configured: bool) {
        self.start_motion = configured;
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
        let work_area = self.primary().placement.work_area;
        let edge = self.primary().placement.panel.edge;
        start_surface::open(runtime, self.primary().panel, self.start, work_area, edge)?;
        self.transient = Some(Transient::StartGroup);
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

    /// Transactional resize of the already-open unified Start surface
    /// via the shared `start_surface::refit` (anchor -> measure -> fit);
    /// prepared before re-map without dropping the group grab.
    fn resize_start_group_measured(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        let work_area = self.primary().placement.work_area;
        let edge = self.primary().placement.panel.edge;
        let rect = start_surface::refit(
            runtime,
            self.primary().panel,
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

    #[must_use]
    pub fn is_network_open(&self) -> bool {
        false
    }

    /// Closes the unified Start surface with a single ungrab,
    /// tolerating any legacy stray state through `close_transients`.
    pub fn close_start_group(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        if self.transient == Some(Transient::StartGroup) {
            return self.close_transients(runtime);
        }
        Ok(())
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
        status_surface::open(
            runtime,
            self.primary().panel,
            surface,
            self.primary().placement.work_area,
            self.primary().placement.panel.edge,
        )?;
        self.transient = Some(transient);
        Ok(())
    }

    pub fn open_context_menu(
        &mut self,
        runtime: &mut SurfaceRuntime,
        state: &ContextMenuState,
        snapshot: &ShellSnapshot,
    ) -> Result<(), String> {
        self.close_transients(runtime)?;
        // Shrink the native surface to its visible content rows so no
        // spare (black) area remains for the shape mask to cover.
        let rows = crate::taskbar::context_menu::visible_row_count_for_kind(
            &snapshot.panels,
            &snapshot.windows,
            snapshot.workspaces.as_ref(),
            &state.kind,
        );
        let size = context_menu_size(rows);
        let rect = context_menu_rect(
            self.primary().placement.panel.output.clone(),
            self.primary().placement.output.geometry,
            state.anchor,
            size,
        );
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
        crate::runtime::retained_source_rect(runtime, self.primary().panel, source_id)
            .unwrap_or(*fallback)
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
        let stored_edge = self.primary().placement.panel.edge;
        let Some(first) = snapshot.panels.panels.first() else {
            return Ok(false);
        };
        let primary_output = first.output.clone();
        let mut contexts = panel_contexts_for_snapshot(snapshot, stored_edge);
        if contexts.is_empty() {
            if first.edge == stored_edge {
                return Ok(false);
            }
            contexts = panel_contexts_for_snapshot(snapshot, first.edge);
        }
        if contexts.is_empty() {
            return Ok(false);
        }
        let retained: Vec<flamewm_api::OutputId> = contexts
            .iter()
            .map(|context| context.output.id.clone())
            .collect();
        let existing: Vec<flamewm_api::OutputId> = self
            .panels
            .iter()
            .map(|record| record.placement.output.id.clone())
            .collect();
        let mut stale = panel_surface_outputs_to_remove(&existing, snapshot);
        for id in &existing {
            if !retained.contains(id) && !stale.contains(id) {
                stale.push(id.clone());
            }
        }
        for context in contexts {
            match self
                .panels
                .iter()
                .position(|record| record.placement.output.id == context.output.id)
            {
                Some(index) => self.panels[index].placement = context,
                None => {
                    let panel = create_panel_surface(runtime, &context)?;
                    if context.panel.visible {
                        runtime.show(panel).map_err(ui_error)?;
                    }
                    self.panels.push(PanelSurfaceRecord {
                        panel,
                        placement: context,
                    });
                }
            }
        }
        for id in stale {
            if let Some(index) = self
                .panels
                .iter()
                .position(|record| record.placement.output.id == id)
            {
                let record = self.panels.remove(index);
                runtime.destroy(record.panel).map_err(ui_error)?;
            }
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
        self.primary().placement.work_area
    }

    /// F10: current context generation read by every Start/Quick open
    /// path in main.rs before projection. Proves the open uses the
    /// post-sync context, never a stale cached rect.
    #[must_use]
    pub fn placement_generation(&self) -> (u64, u64) {
        (
            self.primary().placement.display_generation,
            self.primary().placement.panels_revision,
        )
    }

    /// F10: last-synced panel snapshot geometry.
    #[must_use]
    pub fn panel_geometry(&self) -> flamewm_api::Rect {
        self.primary().placement.panel.geometry
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
                    .map(|output| {
                        output.shell_scale_percent != record.placement.output.shell_scale_percent
                    })
                    .unwrap_or(false)
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
        &self.primary().placement
    }

    #[must_use]
    pub fn panel_edge(&self) -> flamewm_api::PanelEdge {
        self.primary().placement.panel.edge
    }

    /// Resolve a surface-local pointer position (ActionEvent x/y from the
    /// panel surface) to root coordinates via the retained panel device
    /// rect. Falls back to the snapshot geometry only when no retained
    /// layout exists (pre-map).
    #[must_use]
    pub fn root_pointer(&self, runtime: &SurfaceRuntime, x: f32, y: f32) -> flamewm_api::Point {
        let origin = runtime
            .surface_device_rect(self.primary().panel)
            .map(|rect| (rect.x as i32, rect.y as i32))
            .unwrap_or((
                self.primary().placement.panel.geometry.x,
                self.primary().placement.panel.geometry.y,
            ));
        flamewm_api::Point::new(origin.0 + x as i32, origin.1 + y as i32)
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
        self.map_raise_grab_traced(runtime, surface, rect, &GeometryTrace::begin("shell-popup"))
    }

    fn map_raise_grab_traced(
        &mut self,
        runtime: &mut SurfaceRuntime,
        surface: SurfaceHandle,
        rect: flamewm_api::Rect,
        trace: &GeometryTrace,
    ) -> Result<(), String> {
        self.prepare_traced(runtime, surface, rect, trace)?;
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
        self.map_raise_traced(runtime, surface, rect, &GeometryTrace::begin("shell-popup"))
    }

    fn map_raise_traced(
        &self,
        runtime: &mut SurfaceRuntime,
        surface: SurfaceHandle,
        rect: flamewm_api::Rect,
        trace: &GeometryTrace,
    ) -> Result<(), String> {
        self.prepare_traced(runtime, surface, rect, trace)?;
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
    const SUPPORTED: [u16; 5] = [100, 125, 150, 175, 200];
    let percent = if SUPPORTED.contains(&scale_percent) {
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

pub fn domain_reconcile_counter(kind: DomainKind) -> &'static flamewm_profiler::CounterPoint {
    use std::sync::OnceLock;
    match kind {
        DomainKind::Windows => {
            static C: OnceLock<flamewm_profiler::CounterPoint> = OnceLock::new();
            C.get_or_init(|| flamewm_profiler::CounterPoint::new("shell.domain.windows.reconcile"))
        }
        DomainKind::Workspaces => {
            static C: OnceLock<flamewm_profiler::CounterPoint> = OnceLock::new();
            C.get_or_init(|| {
                flamewm_profiler::CounterPoint::new("shell.domain.workspaces.reconcile")
            })
        }
        DomainKind::Panels => {
            static C: OnceLock<flamewm_profiler::CounterPoint> = OnceLock::new();
            C.get_or_init(|| flamewm_profiler::CounterPoint::new("shell.domain.panels.reconcile"))
        }
        DomainKind::System => {
            static C: OnceLock<flamewm_profiler::CounterPoint> = OnceLock::new();
            C.get_or_init(|| flamewm_profiler::CounterPoint::new("shell.domain.system.reconcile"))
        }
        DomainKind::Applications => {
            static C: OnceLock<flamewm_profiler::CounterPoint> = OnceLock::new();
            C.get_or_init(|| {
                flamewm_profiler::CounterPoint::new("shell.domain.applications.reconcile")
            })
        }
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
    use std::sync::OnceLock;
    match kind {
        ProjectionKind::Partial => {
            static C: OnceLock<flamewm_profiler::CounterPoint> = OnceLock::new();
            C.get_or_init(|| flamewm_profiler::CounterPoint::new("shell.projection.panel.partial"))
        }
        ProjectionKind::Start => {
            static C: OnceLock<flamewm_profiler::CounterPoint> = OnceLock::new();
            C.get_or_init(|| flamewm_profiler::CounterPoint::new("shell.projection.panel.start"))
        }
        ProjectionKind::Popup => {
            static C: OnceLock<flamewm_profiler::CounterPoint> = OnceLock::new();
            C.get_or_init(|| flamewm_profiler::CounterPoint::new("shell.projection.panel.popup"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_output_rect() -> flamewm_api::Rect {
        flamewm_api::Rect::new(0, 0, 1920, 1080)
    }

    #[test]
    fn floating_bottom_bar_inset_is_consistent_and_start_popover_stays_above_it() {
        use flamewm_api::Rect;
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
        for percent in [100u16, 125, 150, 175, 200] {
            let scale = panel_ui_scale(percent);
            let thickness = (44.0 * scale).round() as i32;
            assert_eq!(thickness, ((44 * u32::from(percent) + 50) / 100) as i32);
        }
        assert_eq!(panel_ui_scale(110), 1.0);
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
        assert_eq!(context_menu_size(1).height, 41);
        assert_eq!(context_menu_size(2).height, 72);
        assert_eq!(context_menu_size(3).height, 103);
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
        assert_eq!(controls, vec![ShellControl::ToggleStart]);
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
        use flamewm_api::PanelEdge;
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
        let bottom = placement_snapshot(
            flamewm_api::Rect::new(0, 0, 1920, 1080),
            flamewm_api::Rect::new(0, 1036, 1920, 44),
            PanelEdge::Bottom,
            2,
            2,
        );
        assert!(panel_context_for_snapshot(&bottom, PanelEdge::Top).is_none());
        let ctx2 = panel_context_for_snapshot(&bottom, PanelEdge::Bottom).expect("bottom context");
        assert_eq!(ctx2.work_area, flamewm_api::Rect::new(0, 0, 1920, 1036));
    }

    #[test]
    fn panel_context_bottom_excludes_panel_and_starts_above() {
        use flamewm_api::PanelEdge;
        let snapshot = placement_snapshot(
            flamewm_api::Rect::new(0, 0, 1920, 1080),
            flamewm_api::Rect::new(0, 1036, 1920, 44),
            PanelEdge::Bottom,
            3,
            5,
        );
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
        use flamewm_api::PanelEdge;
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
        use flamewm_api::PanelEdge;
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
        use flamewm_api::PanelEdge;
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
        let bottom = placement_snapshot(
            output,
            flamewm_api::Rect::new(0, 1036, 1920, 44),
            PanelEdge::Bottom,
            2,
            2,
        );
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
        use flamewm_api::PanelEdge;
        let mut snapshot = placement_snapshot(
            flamewm_api::Rect::new(0, 0, 1920, 1080),
            flamewm_api::Rect::new(0, 1036, 1920, 44),
            PanelEdge::Bottom,
            1,
            1,
        );
        snapshot.displays.outputs.clear();
        assert!(panel_context_for_snapshot(&snapshot, PanelEdge::Bottom).is_none());
        let mut empty = ShellSnapshot::default();
        empty.displays.outputs = vec![panel_output(
            "HDMI-1",
            flamewm_api::Rect::new(0, 0, 1920, 1080),
        )];
        assert!(panel_context_for_snapshot(&empty, PanelEdge::Bottom).is_none());
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
        use flamewm_api::PanelEdge;
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
    fn single_output_contexts_match_legacy_single_context() {
        use flamewm_api::PanelEdge;
        let snapshot = placement_snapshot(
            flamewm_api::Rect::new(0, 0, 1920, 1080),
            flamewm_api::Rect::new(0, 1036, 1920, 44),
            PanelEdge::Bottom,
            1,
            1,
        );
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

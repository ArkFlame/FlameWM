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

mod actions;
mod context_menus;
mod dispatch;
mod lifecycle;
mod panels;
mod start_status;
mod state;
mod tasks;

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

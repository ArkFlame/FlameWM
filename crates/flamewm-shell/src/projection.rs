use std::fmt::Write as _;

use chrono::{DateTime, Datelike, Local};
use flamewm_api::applications::DesktopApplication;
use flamewm_api::session::SessionCapabilities;
use flamewm_api::settings::SettingsSnapshot;
use flamewm_integrations_linux::icons::{IconResolver, IconSize, Rgb8Raster};
use flamewm_platform::settings::ClockFormat;
use flamewm_ui_x11::{RuntimeImage, UiColor, UiDocumentAccess};

use crate::start::{StartCategory, PRESENTATION_CATEGORIES};
use crate::taskbar::workspaces::{
    cell_left, pager_cell, pager_columns, pager_width, project as project_workspaces,
    WorkspaceView, PAGER_CELLS, PAGER_COLUMNS, PAGER_HEIGHT,
};
use crate::ShellSnapshot;
use flamewm_shell_core::start_menu::{bucket_for_application, StartBucket};
use flamewm_shell_core::{task_visual_states_on_output_and_workspace, StartModel};

#[cfg(test)]
macro_rules! forward {
    ($($method:ident($($arg:ident: $ty:ty),*)),* $(,)?) => {
        $(fn $method(&mut self, $($arg: $ty),*) -> Result<(), String> {
            UiDocumentAccess::$method(self.document, $($arg),*)
        })*
    };
}

mod audio;
mod calendar;
mod network;
mod weather;

pub use audio::{project_audio, project_audio_full};
pub use calendar::project_calendar;
pub use network::{project_network, project_network_full, project_network_secret};
pub use weather::{project_weather_popup, project_weather_tray};

const TASK_SLOT_COUNT: usize = 8;
/// Bar height in CSS px used when the panel snapshot carries no logical size.
/// Matches the `.taskbar` height in ui/shell/flamewm.css.
const TASKBAR_DEFAULT_HEIGHT: f32 = 44.0;
/// Logical edge of a taskbar task icon, before the output's shell scale.
const TASK_ICON_LOGICAL_PX: f32 = 25.0;
/// `.task-slot` width in CSS px (`ui/shell/flamewm.css`).
const TASK_SLOT_WIDTH: f32 = 42.0;
/// `.tray-button` width in CSS px.
const TRAY_BUTTON_WIDTH: f32 = 30.0;
/// Height of one taskbar element row in CSS px (`.task-slots`, `.tray-button`).
const TASKBAR_ELEMENT_HEIGHT: f32 = 44.0;

/// Taskbar task icon raster edge in device pixels. The icon node is laid out
/// in logical px and scaled by the panel's UI scale, so requesting only 25 px
/// upscales it blurrily on HiDPI outputs; requesting the scaled edge matches
/// how desktop entries request icons.
pub fn task_icon_px(output: Option<&flamewm_api::display::OutputSnapshot>) -> u32 {
    let scale =
        crate::runtime::panel_ui_scale(output.map_or(100, |output| output.shell_scale_percent));
    (TASK_ICON_LOGICAL_PX * scale).round() as u32
}

/// Maps the panel's logical (settings) height to the CSS height of the bar.
/// A zero logical size falls back to the stylesheet default so 100% output
/// stays identical to the 44px layout.
/// Accent used for the focused task underline (ARGB), fed by the settings projection so the
/// underline follows `accent`/`accentOpacity` instead of a fixed colour.
static TASK_ACCENT_ARGB: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0xFF7F_878E);

/// Records the accent colour the focused task underline uses from the next task projection on.
pub fn set_task_accent(color: UiColor) {
    let packed = (u32::from(color.a) << 24)
        | (u32::from(color.r) << 16)
        | (u32::from(color.g) << 8)
        | u32::from(color.b);
    TASK_ACCENT_ARGB.store(packed, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn task_accent() -> UiColor {
    let packed = TASK_ACCENT_ARGB.load(std::sync::atomic::Ordering::Relaxed);
    UiColor {
        a: (packed >> 24) as u8,
        r: (packed >> 16) as u8,
        g: (packed >> 8) as u8,
        b: packed as u8,
    }
}

/// Surface fill each separate-mode element capsule paints, fed by the settings projection.
/// Default matches `ProductSettings::default()`: surfaceColor `#191b1d` at 50% opacity.
static TASKBAR_SURFACE_ARGB: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0x8019_1b1d);

/// Records the surface fill used by the separate-taskbar capsules.
pub fn set_taskbar_surface(color: UiColor) {
    let packed = (u32::from(color.a) << 24)
        | (u32::from(color.r) << 16)
        | (u32::from(color.g) << 8)
        | u32::from(color.b);
    TASKBAR_SURFACE_ARGB.store(packed, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn taskbar_surface() -> UiColor {
    let packed = TASKBAR_SURFACE_ARGB.load(std::sync::atomic::Ordering::Relaxed);
    UiColor {
        a: (packed >> 24) as u8,
        r: (packed >> 16) as u8,
        g: (packed >> 8) as u8,
        b: packed as u8,
    }
}

/// Same 8px rounding the taskbar elements use on hover (`ui/shell/flamewm.css`), so a
/// separate-mode capsule and its hover highlight share one shape.
const ELEMENT_RADIUS: f32 = 8.0;
const TRANSPARENT_FILL: UiColor = UiColor {
    r: 0,
    g: 0,
    b: 0,
    a: 0,
};

/// Separate mode: every element paints its own rounded capsule and the bar root paints none,
/// so the raw wallpaper shows between them. The blur is per node, never over the whole bar.
/// Unified mode clears the capsules and the root keeps the single surface fill.
pub(crate) fn project_element_backgrounds(
    document: &mut impl UiDocumentAccess,
    items: TaskbarItems,
    rendered_slots: usize,
    tray_visible: bool,
    clock_visible: bool,
) -> Result<(), String> {
    RENDERED_SLOTS.store(rendered_slots, std::sync::atomic::Ordering::Relaxed);
    TRAY_VISIBLE.store(tray_visible, std::sync::atomic::Ordering::Relaxed);
    let fill = if items.separate {
        taskbar_surface()
    } else {
        TRANSPARENT_FILL
    };
    let blur = items.separate.then_some(items.blur_percent);
    // The capsules own the background; start/clock buttons keep their CSS hover on top.
    document.background("start-capsule", fill)?;
    document.corner_radius("start-capsule", ELEMENT_RADIUS)?;
    document.node_backdrop("start-capsule", blur)?;
    // A capsule with no icons inside is not painted at all.
    let capsule = |shown: bool| if shown { fill } else { TRANSPARENT_FILL };
    let capsule_blur = |shown: bool| (items.separate && shown).then_some(items.blur_percent);
    document.background("tray-group", capsule(tray_visible))?;
    document.corner_radius("tray-group", ELEMENT_RADIUS)?;
    document.node_backdrop("tray-group", capsule_blur(tray_visible))?;
    document.background("clock-capsule", capsule(clock_visible))?;
    document.corner_radius("clock-capsule", ELEMENT_RADIUS)?;
    document.node_backdrop("clock-capsule", capsule_blur(clock_visible))?;
    // No capsule behind the workspace selector: its cells carry the selector's own fills.
    document.background("workspace-pager", TRANSPARENT_FILL)?;
    document.corner_radius("workspace-pager", 0.0)?;
    document.node_backdrop("workspace-pager", None)?;
    let slots = if items.separate && rendered_slots > 0 {
        taskbar_surface()
    } else {
        TRANSPARENT_FILL
    };
    document.background("task-slots", slots)?;
    document.corner_radius("task-slots", ELEMENT_RADIUS)?;
    document.node_backdrop(
        "task-slots",
        (items.separate && rendered_slots > 0).then_some(items.blur_percent),
    )?;
    Ok(())
}

/// Whether the grouped tray renders any icon; the settings path reuses the last state value.
pub(crate) fn tray_any_visible(
    system: &flamewm_api::system::SystemSnapshot,
    items: TaskbarItems,
) -> bool {
    let status = crate::taskbar::status::project(system).views;
    let media = status.media.visible
        && matches!(
            system.media.playback,
            flamewm_api::system::PlaybackState::Playing
                | flamewm_api::system::PlaybackState::Paused
        );
    media
        || (status.audio.visible && items.volume)
        || (status.network.visible && items.network)
        || crate::taskbar::status::battery::tray_visible(
            &system.power,
            items.battery,
            items.battery_only_when_relevant,
        )
        // Weather is projected by its own provider; the setting is the only signal here.
        || items.weather
}

/// Window buttons the last element projection rendered; a settings change reuses it so the
/// task-slots capsule is not cleared when only the surface colour/opacity changed.
static RENDERED_SLOTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static TRAY_VISIBLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Re-applies the element capsules from a settings snapshot without a panel state pass.
pub fn reapply_element_backgrounds(
    document: &mut impl UiDocumentAccess,
    items: TaskbarItems,
) -> Result<(), String> {
    project_element_backgrounds(
        document,
        items,
        RENDERED_SLOTS.load(std::sync::atomic::Ordering::Relaxed),
        TRAY_VISIBLE.load(std::sync::atomic::Ordering::Relaxed),
        items.clock,
    )
}

/// Active pager cell fill: the live accent at the stylesheet's `--accent-soft` alpha.
pub(crate) fn pager_active_fill() -> UiColor {
    let accent = task_accent();
    UiColor {
        a: ((u16::from(flamewm_platform::chrome::ACCENT_SOFT_ALPHA) * u16::from(accent.a)) / 255)
            as u8,
        ..accent
    }
}

/// Inactive pager cell fill (matches the `.workspace-button` background).
const PAGER_IDLE_FILL: UiColor = UiColor {
    r: 36,
    g: 39,
    b: 42,
    a: 255,
};

/// Projected workspaces paired with the pager cell each one occupies, using the
/// count-dependent two-row topology (`cell = 2 * column + row + 1`).
fn pager_cells(
    snapshot: Option<&flamewm_api::workspace::WorkspaceSnapshot>,
) -> Vec<(usize, WorkspaceView)> {
    let count = snapshot.map_or(0, |snapshot| snapshot.count);
    project_workspaces(snapshot)
        .into_iter()
        .filter_map(|view| pager_cell(view.index, count).map(|cell| (cell, view)))
        .collect()
}

/// Pager columns, box and cell visibility for `cells`; no labels or fills.
///
/// `workspace-column-j` is an absolutely positioned child of the pager, so
/// hidden columns consume no space and the visible ones keep the screen
/// positions of the box they replaced: column `j` (1-based) starts at
/// `cell_left(j - 1)` inside the pager content box.
fn project_pager_layout(
    document: &mut impl UiDocumentAccess,
    snapshot: Option<&flamewm_api::workspace::WorkspaceSnapshot>,
    cells: &[(usize, WorkspaceView)],
) -> Result<(), String> {
    let columns = pager_columns(snapshot.map_or(0, |snapshot| snapshot.count));
    document.size("workspace-pager", pager_width(columns), PAGER_HEIGHT)?;
    for column in 0..PAGER_COLUMNS {
        let column_id = format!("workspace-column-{}", column + 1);
        if column < columns {
            document.position(&column_id, cell_left(column), 0.0)?;
            document.visible(&column_id, true)?;
        } else {
            document.visible(&column_id, false)?;
        }
    }
    for cell in 1..=PAGER_CELLS {
        document.visible(
            &format!("workspace-{cell}"),
            cells.iter().any(|(occupied, _)| *occupied == cell),
        )?;
    }
    Ok(())
}

/// Pager columns, box and cell visibility for the live snapshot; no labels or
/// fills (the `PanelsChanged` geometry pass uses this alone).
pub(crate) fn project_pager_visibility(
    document: &mut impl UiDocumentAccess,
    snapshot: Option<&flamewm_api::workspace::WorkspaceSnapshot>,
) -> Result<(), String> {
    let cells = pager_cells(snapshot);
    project_pager_layout(document, snapshot, &cells)
}

/// Pager pass: columns, box, cell visibility, labels and fills.
///
/// Single owner of the pager document projection so the sync panel pass and
/// the async pager pass cannot show different topologies; both call here.
pub(crate) fn project_pager(
    document: &mut impl UiDocumentAccess,
    snapshot: Option<&flamewm_api::workspace::WorkspaceSnapshot>,
) -> Result<(), String> {
    let cells = pager_cells(snapshot);
    project_pager_layout(document, snapshot, &cells)?;
    for (cell, workspace) in cells {
        let cell_id = format!("workspace-{cell}");
        document.text(&cell_id, workspace.label.clone())?;
        let fill = if workspace.active {
            pager_active_fill()
        } else {
            PAGER_IDLE_FILL
        };
        document.background(&cell_id, fill)?;
    }
    Ok(())
}

pub fn taskbar_css_height(logical_size: u16) -> f32 {
    if logical_size == 0 {
        TASKBAR_DEFAULT_HEIGHT
    } else {
        f32::from(logical_size)
    }
}

/// Panel width in CSS px. `geometry.width` is device pixels; the document
/// lays out in CSS px at `scale_percent`, so dividing by the scale keeps the
/// bar exactly one window wide. Unsupported scales fall back to 100%, as the
/// panel surface does when it applies its UI scale.
pub fn taskbar_css_width(device_width: i32, scale_percent: u16) -> f32 {
    let percent = if (100..=200).contains(&scale_percent) && scale_percent % 5 == 0 {
        scale_percent
    } else {
        100
    };
    device_width.max(0) as f32 * 100.0 / f32::from(percent)
}

/// Sizes the `taskbar` node to the panel's logical width and height so the
/// painted bar fills the panel window instead of leaving an undrawn band.
/// A device-pixel width here pushed the pager, tray and clock past the
/// layout viewport at 125% scale.
pub(crate) fn project_taskbar_height(
    document: &mut impl UiDocumentAccess,
    view: PanelView<'_>,
) -> Result<(), String> {
    let Some(panel) = view.panel else {
        return Ok(());
    };
    let scale_percent = view
        .output
        .map(|output| output.shell_scale_percent)
        .unwrap_or(100);
    document.size(
        "taskbar",
        taskbar_css_width(panel.geometry.width, scale_percent),
        taskbar_css_height(panel.logical_size),
    )
}

/// Borrowed single-panel view. `output` and `panel` are the record's own
/// entries (or the first panel for full-snapshot callers); everything else
/// is read from the shared snapshot without cloning it.
#[derive(Clone, Copy)]
pub struct PanelView<'a> {
    pub snapshot: &'a ShellSnapshot,
    pub output: Option<&'a flamewm_api::display::OutputSnapshot>,
    pub panel: Option<&'a flamewm_api::panels::PanelSnapshot>,
    pub items: TaskbarItems,
    pub task_progress: &'a [flamewm_api::panels::TaskProgressEntry],
}

/// Taskbar item settings. Each flag is the setting alone; the
/// projection ANDs it with the item's availability. Defaults match
/// `ProductSettings::default()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskbarItems {
    pub weather: bool,
    pub volume: bool,
    pub network: bool,
    pub battery: bool,
    pub battery_only_when_relevant: bool,
    pub clock: bool,
    pub workspaces: bool,
    /// `taskbarActivities`: the running-window area renders at all.
    pub activities: bool,
    /// `taskbarSeparate`: one rounded background per element instead of a unified bar.
    pub separate: bool,
    /// `surfaceBlurPercent`: blur applied behind each separate-mode capsule.
    pub blur_percent: u8,
}

impl Default for TaskbarItems {
    fn default() -> Self {
        Self {
            weather: true,
            volume: true,
            network: true,
            battery: true,
            battery_only_when_relevant: false,
            clock: true,
            workspaces: true,
            activities: true,
            separate: true,
            blur_percent: 80,
        }
    }
}

impl<'a> PanelView<'a> {
    /// First panel of the snapshot, with its connected output.
    #[must_use]
    pub fn first(snapshot: &'a ShellSnapshot) -> Self {
        let panel = snapshot.panels.panels.first();
        let output = panel.and_then(|panel| {
            snapshot
                .displays
                .outputs
                .iter()
                .find(|output| output.id == panel.output && output.connected)
        });
        Self {
            snapshot,
            output,
            panel,
            items: TaskbarItems::default(),
            task_progress: &[],
        }
    }

    /// Same view with the taskbar item settings applied.
    #[must_use]
    pub fn with_items(self, items: TaskbarItems) -> Self {
        Self { items, ..self }
    }

    /// Attach provider-owned task progress for this projection turn.
    #[must_use]
    pub fn with_task_progress(
        self,
        task_progress: &'a [flamewm_api::panels::TaskProgressEntry],
    ) -> Self {
        Self {
            task_progress,
            ..self
        }
    }
}

/// Calls `project` once per panel record, in order, with that record's
/// borrowed view of the live snapshot. A record whose connected output or
/// panel is absent from the snapshot is skipped; sync removes its surface.
/// The first error stops the walk.
pub fn for_each_panel_record<K, E>(
    snapshot: &ShellSnapshot,
    records: impl IntoIterator<Item = (K, flamewm_api::OutputId)>,
    mut project: impl FnMut(K, PanelView<'_>) -> Result<(), E>,
) -> Result<(), E> {
    for (key, output_id) in records {
        let Some(output) = snapshot
            .displays
            .outputs
            .iter()
            .find(|output| output.id == output_id && output.connected)
        else {
            continue;
        };
        let Some(panel) = snapshot
            .panels
            .panels
            .iter()
            .find(|panel| panel.output == output_id)
        else {
            continue;
        };
        project(
            key,
            PanelView {
                snapshot,
                output: Some(output),
                panel: Some(panel),
                items: TaskbarItems::default(),
                task_progress: &[],
            },
        )?;
    }
    Ok(())
}

pub fn project(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
    icon_resolver: &mut IconResolver,
) -> Result<(), String> {
    project_panel(document, PanelView::first(snapshot), icon_resolver)
}

pub fn project_panel(
    document: &mut impl UiDocumentAccess,
    view: PanelView<'_>,
    icon_resolver: &mut IconResolver,
) -> Result<(), String> {
    let snapshot = view.snapshot;
    project_taskbar_height(document, view)?;
    let states = task_visual_states_on_output_and_workspace(
        &snapshot.panels,
        &snapshot.windows,
        &snapshot.applications,
        view.panel.map(|panel| &panel.output),
        snapshot
            .workspaces
            .as_ref()
            .map(|workspaces| workspaces.active_index),
    );
    let (mut slot_id, mut icon_id, mut label_id, mut line_id) =
        (String::new(), String::new(), String::new(), String::new());
    for index in 0..TASK_SLOT_COUNT {
        let n = index + 1;
        slot_id.clear();
        write!(slot_id, "task-slot-{n}").expect("write to String is infallible");
        icon_id.clear();
        write!(icon_id, "task-slot-{n}-icon").expect("write to String is infallible");
        label_id.clear();
        write!(label_id, "task-slot-{n}-label").expect("write to String is infallible");
        line_id.clear();
        write!(line_id, "task-slot-{n}-line").expect("write to String is infallible");
        project_task_progress(
            document,
            index,
            states.get(index).map(|state| &state.id),
            view.task_progress,
        )?;
        if let Some(state) = states.get(index) {
            let label = snapshot
                .applications
                .iter()
                .find(|application| application.id == state.app_id)
                .map(|application| application.name.as_str())
                .filter(|name| !name.is_empty())
                .unwrap_or(state.app_id.as_str());
            let px = task_icon_px(view.output);
            let icon = resolve_icon(icon_resolver, &state.icon_name, IconSize::new(px, px))?;
            document.image_rgb8(&icon_id, icon)?;
            document.text(&label_id, label.to_owned())?;
            document.visible(&line_id, state.running)?;
            document.background_clear(&slot_id)?;
            if state.focused {
                document.background(&line_id, task_accent())?;
            } else {
                document.background(
                    &line_id,
                    UiColor {
                        r: 135,
                        g: 145,
                        b: 155,
                        a: 255,
                    },
                )?;
            }
        } else {
            document.text(&label_id, String::new())?;
            document.visible(&line_id, false)?;
            document.background_clear(&slot_id)?;
        }
        document.visible(
            &slot_id,
            view.items.activities && states.get(index).is_some(),
        )?;
    }
    project_task_slots_extent(document, task_slots_rendered(states.len(), view.items))?;
    document.visible("task-slot-insert", view.items.activities)?;

    project_pager(document, snapshot.workspaces.as_ref())?;
    document.visible("workspace-pager", view.items.workspaces)?;

    project_clock(document)?;
    project_status_tray(document, &snapshot.system, view.items)?;
    project_element_backgrounds(
        document,
        view.items,
        task_slots_rendered(states.len(), view.items),
        tray_any_visible(&snapshot.system, view.items),
        view.items.clock,
    )
}

/// Window buttons `#task-slots` renders: `taskbarActivities` off hides the whole area.
pub(crate) fn task_slots_rendered(states: usize, items: TaskbarItems) -> usize {
    if items.activities {
        states.min(TASK_SLOT_COUNT)
    } else {
        0
    }
}

/// `#task-slots` covers exactly the window buttons that render, so a separate-mode capsule hugs
/// them and the bar reflows; zero buttons leave no area behind for an empty capsule.
pub(crate) fn project_task_slots_extent(
    document: &mut impl UiDocumentAccess,
    rendered: usize,
) -> Result<(), String> {
    document.size(
        "task-slots",
        rendered as f32 * TASK_SLOT_WIDTH,
        TASKBAR_ELEMENT_HEIGHT,
    )
}

/// Collapses a hidden tray button so the grouped tray capsule encloses only the icons that render.
pub(crate) fn project_tray_button(
    document: &mut impl UiDocumentAccess,
    id: &str,
    shown: bool,
) -> Result<(), String> {
    document.size(
        id,
        if shown { TRAY_BUTTON_WIDTH } else { 0.0 },
        TASKBAR_ELEMENT_HEIGHT,
    )
}

/// Projects a determinate progress fill for one task slot, clearing stale fills when
/// identity disappears or the provider no longer reports progress.
pub(crate) fn project_task_progress(
    document: &mut impl UiDocumentAccess,
    index: usize,
    task_id: Option<&flamewm_api::TaskEntryId>,
    progress: &[flamewm_api::panels::TaskProgressEntry],
) -> Result<(), String> {
    let fill_id = format!("task-slot-{}-progress", index + 1);
    let percent = task_id
        .and_then(|id| progress.iter().find(|entry| &entry.task_id == id))
        .map(|entry| entry.progress.percent());
    document.visible(&fill_id, percent.is_some())?;
    document.size(
        &fill_id,
        percent.map_or(0.0, |percent| 28.0 * f32::from(percent) / 100.0),
        2.0,
    )?;
    Ok(())
}

/// Projects only the status tray and clock button. The async partial update
/// path shares this routine without touching task or pager nodes.
pub(crate) fn project_status_tray(
    document: &mut impl UiDocumentAccess,
    system: &flamewm_api::system::SystemSnapshot,
    items: TaskbarItems,
) -> Result<(), String> {
    let status = crate::taskbar::status::project(system).views;
    let media = status.media.visible
        && matches!(
            system.media.playback,
            flamewm_api::system::PlaybackState::Playing
                | flamewm_api::system::PlaybackState::Paused
        );
    document.visible("tray-media", media)?;
    project_tray_button(document, "tray-media", media)?;
    document.visible(
        "tray-play-icon",
        matches!(
            status.media.play_pause_icon,
            Some(flamewm_ui_core::IconRole::MediaPlay)
        ),
    )?;
    document.visible(
        "tray-pause-icon",
        matches!(
            status.media.play_pause_icon,
            Some(flamewm_ui_core::IconRole::MediaPause)
        ),
    )?;
    let volume = status.audio.visible && items.volume;
    document.visible("tray-volume", volume)?;
    project_tray_button(document, "tray-volume", volume)?;
    document.visible("tray-volume-icon", !status.audio.muted)?;
    document.visible("tray-volume-muted-icon", status.audio.muted)?;
    let network = status.network.visible && items.network;
    document.visible("tray-network", network)?;
    project_tray_button(document, "tray-network", network)?;
    project_battery_tray(document, &system.power, items)?;
    document.visible("clock-button", items.clock)?;
    Ok(())
}

/// Tray battery: visibility and glyph come from the pure `battery` rules. Exactly
/// one glyph node is shown; the charging variant only while Charging.
pub fn project_battery_tray(
    document: &mut impl UiDocumentAccess,
    power: &flamewm_api::system::PowerSnapshot,
    items: TaskbarItems,
) -> Result<(), String> {
    use crate::taskbar::status::battery::{glyph_for_percent, tray_visible};
    use flamewm_api::system::PowerState;
    let charging = power.state == PowerState::Charging;
    let glyph = glyph_for_percent(power.percent);
    let tray = tray_visible(power, items.battery, items.battery_only_when_relevant);
    document.visible("tray-battery", tray)?;
    project_tray_button(document, "tray-battery", tray)?;
    project_glyph_nodes(document, "tray-battery-", glyph, charging)
}

/// Shows only `glyph` under `prefix`, using the charging variant while charging.
fn project_glyph_nodes(
    document: &mut impl UiDocumentAccess,
    prefix: &str,
    glyph: crate::taskbar::status::battery::BatteryGlyph,
    charging: bool,
) -> Result<(), String> {
    use crate::taskbar::status::battery::BATTERY_GLYPHS;
    for candidate in BATTERY_GLYPHS {
        let shown = candidate == glyph;
        document.visible(
            &format!("{prefix}{}", candidate.id_suffix()),
            shown && !charging,
        )?;
        document.visible(
            &format!("{prefix}{}-charging", candidate.id_suffix()),
            shown && charging,
        )?;
    }
    Ok(())
}

/// Battery menu body: percent, state and CPU text, progress fill, and the
/// charging glyph variant. Uses the full 0..=100 range (menu is not hidden).
pub fn project_battery_menu(
    document: &mut impl UiDocumentAccess,
    power: &flamewm_api::system::PowerSnapshot,
    track_width: f32,
) -> Result<(), String> {
    use crate::taskbar::status::battery::{
        cpu_text, glyph_for_percent, progress_width, rate_text, state_text,
    };
    use flamewm_api::system::PowerState;
    let charging = power.state == PowerState::Charging;
    let glyph = glyph_for_percent(power.percent);
    document.text("battery-percent", format!("{}%", power.percent.min(100)))?;
    document.text("battery-state", state_text(power.state).to_owned())?;
    document.text("battery-rate", rate_text(power.power_rate_mw))?;
    document.text("battery-cpu", cpu_text(power.cpu_percent))?;
    document.size(
        "battery-progress-fill",
        progress_width(power.percent, track_width),
        BATTERY_PROGRESS_HEIGHT,
    )?;
    project_glyph_nodes(document, "battery-icon-", glyph, charging)
}

/// Height of the battery menu progress fill (matches ui/shell/battery.html).
pub const BATTERY_PROGRESS_HEIGHT: f32 = 8.0;

/// The single Start result owner: every projection and click path reads this.
/// A non-empty query searches ALL categories (the category rail is hidden);
/// an empty query shows the selected presentation category.
#[must_use]
pub fn start_visible_results<'a>(
    model: &'a StartModel,
    category: StartCategory,
    query: &str,
) -> Vec<&'a flamewm_api::applications::DesktopApplication> {
    if !query.trim().is_empty() {
        return model.query_view(query);
    }
    let base = model.results_view();
    match category {
        StartCategory::All => base,
        StartCategory::Power => Vec::new(),
        _ => base
            .into_iter()
            .filter(|application| application_matches_category(application, category))
            .collect(),
    }
}

pub fn project_start(
    document: &mut impl UiDocumentAccess,
    model: &StartModel,
    category: StartCategory,
    query: &str,
    session: &SessionCapabilities,
) -> Result<(), String> {
    // Search mode hides the whole category rail (incl. Power) and searches all.
    let searching = !query.trim().is_empty();
    document.visible("start-categories", !searching)?;
    // A search ignores the category, so one query serves every rail check and the count.
    let results = start_visible_results(model, category, query);
    for (name, value) in PRESENTATION_CATEGORIES {
        let visible = match value {
            StartCategory::Power => session_available(session),
            _ if value == category => true,
            _ if searching => !results.is_empty(),
            _ => !start_visible_results(model, value, query).is_empty(),
        };
        document.visible(&format!("start-category-{name}"), visible)?;
    }
    let count = results.len();
    document.text(
        "start-search-label",
        if query.trim().is_empty() {
            "Search applications".to_owned()
        } else {
            format!("{} ({})", query, count)
        },
    )?;
    Ok(())
}

/// Start error line. Markup must provide this node (hidden by default).
pub const START_STATUS_NODE: &str = "start-status-label";

/// Project the Start error line: shown with `message`, hidden when `None`.
pub fn project_start_error(
    document: &mut impl UiDocumentAccess,
    message: Option<&str>,
) -> Result<(), String> {
    match message {
        Some(message) => {
            document.text(START_STATUS_NODE, message.to_owned())?;
            document.visible(START_STATUS_NODE, true)
        }
        None => document.visible(START_STATUS_NODE, false),
    }
}

#[must_use]
pub fn start_application_for_slot(
    model: &StartModel,
    category: StartCategory,
    query: &str,
    slot: usize,
) -> Option<flamewm_api::DesktopAppId> {
    start_visible_results(model, category, query)
        .get(slot)
        .map(|application| application.id.clone())
}

/// User-visible name of the application in `slot`, for Start error text.
#[must_use]
pub fn start_application_name_for_slot(
    model: &StartModel,
    category: StartCategory,
    query: &str,
    slot: usize,
) -> Option<String> {
    start_visible_results(model, category, query)
        .get(slot)
        .map(|application| application.name.clone())
}

pub fn project_context_menu(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
    settings: &SettingsSnapshot,
    state: Option<&crate::ContextMenuState>,
) -> Result<(), String> {
    // Shared menu surface (radius + fill) from the Surfaces settings, same owner as desktop menus.
    let surface = flamewm_ui_core::context_menu::menu_surface(settings);
    // The shell task-menu popup document holds only the `task-menu` root; desktop roots live in the desktop document.
    document.background(
        "task-menu",
        crate::settings_projection::surface_background(settings),
    )?;
    document.corner_radius("task-menu", surface.radius as f32)?;
    for id in [
        "context-task-activate",
        "context-task-close",
        "context-task-pin",
        "context-task-unpin",
        "context-workspace-activate",
        "context-workspace-insert-after",
        "context-workspace-remove",
        "context-taskbar-settings",
    ] {
        document.visible(id, false)?;
    }
    let Some(state) = state else {
        return Ok(());
    };
    match &state.kind {
        crate::ContextMenuKind::Task(id) => {
            for entry in crate::taskbar::context_menu::task_menu_for_entry(
                &snapshot.panels,
                &snapshot.windows,
                id,
            ) {
                if let flamewm_ui_core::MenuEntry::Action { id, .. } = entry {
                    document.visible(&format!("context-task-{id}"), true)?;
                }
            }
        }
        crate::ContextMenuKind::Workspace { index } => {
            if let Some(workspaces) = snapshot.workspaces.as_ref() {
                for (entry, _) in crate::taskbar::context_menu::workspace_menu(workspaces, *index) {
                    if let flamewm_ui_core::MenuEntry::Action { id, .. } = entry {
                        document.visible(&format!("context-workspace-{id}"), true)?;
                    }
                }
            }
        }
        crate::ContextMenuKind::Taskbar => {
            for row in crate::taskbar::context_menu::taskbar_menu() {
                document.visible(&format!("context-{}", row.id), true)?;
            }
        }
    }
    Ok(())
}

/// Char budget for the media popup title. The title box is 276px wide with
/// 10px padding per side (256px usable) at 13px / weight 600; ~7.8px per glyph
/// gives ~32 glyphs. The render compiler has no text-overflow, so the ellipsis
/// is applied here. Counted with `chars()`, so multibyte text never splits.
const MEDIA_TITLE_MAX_CHARS: usize = 32;

/// Returns `title` unchanged when it fits `MEDIA_TITLE_MAX_CHARS`; otherwise
/// keeps the leading chars, trims trailing whitespace and appends `…` so the
/// total stays within the budget.
#[must_use]
fn fit_media_title(title: &str) -> String {
    if title.chars().count() <= MEDIA_TITLE_MAX_CHARS {
        return title.to_owned();
    }
    let kept: String = title.chars().take(MEDIA_TITLE_MAX_CHARS - 1).collect();
    format!("{}…", kept.trim_end())
}

pub fn project_media(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
) -> Result<(), String> {
    let view = crate::taskbar::status::media::project(&snapshot.system);
    document.text("media-title", fit_media_title(&snapshot.system.media.title))?;
    document.visible(
        "media-play-icon",
        matches!(
            view.play_pause_icon,
            Some(flamewm_ui_core::IconRole::MediaPlay)
        ),
    )?;
    document.visible(
        "media-pause-icon",
        matches!(
            view.play_pause_icon,
            Some(flamewm_ui_core::IconRole::MediaPause)
        ),
    )?;
    let (can_previous, can_play_pause, can_next) =
        flamewm_shell_core::StatusPopovers::from_snapshot(&snapshot.system)
            .media
            .map_or((false, false, false), |media| {
                (media.can_previous, media.can_play_pause, media.can_next)
            });
    for (enabled, disabled, active) in [
        ("media-previous", "media-previous-disabled", can_previous),
        (
            "media-playpause",
            "media-playpause-disabled",
            can_play_pause,
        ),
        ("media-next", "media-next-disabled", can_next),
    ] {
        document.visible(enabled, active)?;
        document.visible(disabled, !active)?;
    }
    Ok(())
}

/// Taskbar clock presentation from `taskbarClockFormat`, `taskbarShowSeconds`
/// and `taskbarShowDate`. Defaults match `ProductSettings::default()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockDisplay {
    pub format: ClockFormat,
    pub show_seconds: bool,
    pub show_date: bool,
}

impl Default for ClockDisplay {
    fn default() -> Self {
        Self {
            format: ClockFormat::TwentyFour,
            show_seconds: false,
            show_date: true,
        }
    }
}

/// Time text for the taskbar clock: `HH:MM` or `HH:MM:SS` in 24h, `H:MM AM/PM`
/// or `H:MM:SS AM/PM` in 12h.
#[must_use]
pub fn clock_time_text(display: ClockDisplay, now: DateTime<Local>) -> String {
    let pattern = match (display.format, display.show_seconds) {
        (ClockFormat::TwentyFour, false) => "%H:%M",
        (ClockFormat::TwentyFour, true) => "%H:%M:%S",
        (ClockFormat::Twelve, false) => "%-I:%M %p",
        (ClockFormat::Twelve, true) => "%-I:%M:%S %p",
    };
    now.format(pattern).to_string()
}

/// Panel rebuild paths carry no settings; the settings projection re-applies the
/// configured clock on the next settings pass and on every clock tick.
pub fn project_clock(document: &mut impl UiDocumentAccess) -> Result<(), String> {
    project_clock_at(document, Local::now(), ClockDisplay::default())
}

pub fn project_clock_at(
    document: &mut impl UiDocumentAccess,
    now: DateTime<Local>,
    display: ClockDisplay,
) -> Result<(), String> {
    document.text("clock-time", clock_time_text(display, now))?;
    document.text("clock-date", now.format("%-d/%-m/%y").to_string())?;
    document.visible("clock-date", display.show_date)?;
    Ok(())
}

#[must_use]
pub fn local_date() -> (i32, u8, u8) {
    let now = Local::now();
    (now.year(), now.month() as u8, now.day() as u8)
}

fn month_name(month: u8) -> &'static str {
    [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ]
    .get(month.saturating_sub(1) as usize)
    .copied()
    .unwrap_or("Unknown")
}

#[must_use]
pub(crate) fn session_available(session: &SessionCapabilities) -> bool {
    session.lock || session.logout || session.suspend || session.reboot || session.shutdown
}

fn application_matches_category(application: &DesktopApplication, category: StartCategory) -> bool {
    let bucket = match category {
        StartCategory::All => return true,
        StartCategory::Power => return false,
        StartCategory::Development => StartBucket::Development,
        StartCategory::Games => StartBucket::Games,
        StartCategory::Graphics => StartBucket::Graphics,
        StartCategory::Internet => StartBucket::Internet,
        StartCategory::Multimedia => StartBucket::Multimedia,
        StartCategory::System => StartBucket::System,
        StartCategory::Utilities => StartBucket::Utilities,
    };
    bucket_for_application(application) == bucket
}

fn resolve_icon(
    resolver: &mut IconResolver,
    icon_name: &str,
    size: IconSize,
) -> Result<RuntimeImage, String> {
    let raster = resolver.prepare_name(icon_name, size).or_else(|error| {
        resolver.prepare_name("", size).map_err(|fallback| {
            format!("icon '{icon_name}' failed: {error}; generic fallback failed: {fallback}")
        })
    })?;
    Ok(runtime_image(raster))
}

fn runtime_image(raster: Rgb8Raster) -> RuntimeImage {
    RuntimeImage {
        source: raster.source,
        width: raster.width,
        height: raster.height,
        pixels: raster.pixels,
    }
}

#[cfg(test)]
mod panel_template_tests {
    use super::*;

    use flamewm_api::system::{PowerSnapshot, PowerState, ServiceAvailability};
    use flamewm_api::workspace::WorkspaceSnapshot;

    use crate::taskbar::workspaces::cell_index;

    const PANEL_ARTIFACT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-panel.rwr"));

    fn decode_panel() -> flamewm_ui_x11::UiDocument {
        flamewm_ui_x11::decode_document(PANEL_ARTIFACT).expect("panel artifact must decode")
    }

    fn power_snapshot(
        available: ServiceAvailability,
        present: bool,
        percent: u8,
        state: PowerState,
    ) -> PowerSnapshot {
        PowerSnapshot {
            available,
            present,
            percent,
            state,
            cpu_percent: None,
            power_rate_mw: None,
        }
    }

    fn glyph_node_ids(prefix: &str) -> Vec<String> {
        use crate::taskbar::status::battery::BATTERY_GLYPHS;
        BATTERY_GLYPHS
            .iter()
            .flat_map(|glyph| {
                [
                    format!("{prefix}{}", glyph.id_suffix()),
                    format!("{prefix}{}-charging", glyph.id_suffix()),
                ]
            })
            .collect()
    }

    fn box_width(document: &flamewm_ui_x11::UiDocument, id: &str, viewport: (f32, f32)) -> f32 {
        document
            .layout_box_by_id(id, viewport)
            .map_or(0.0, |rect| rect.2)
    }

    fn lit_ids<'a>(
        document: &flamewm_ui_x11::UiDocument,
        ids: &'a [String],
        viewport: (f32, f32),
    ) -> Vec<&'a str> {
        ids.iter()
            .map(String::as_str)
            .filter(|id| box_width(document, id, viewport) > 0.0)
            .collect()
    }

    // CONTRACT-REGRESSION: the clock and workspace pager text targets and the
    // tray visibility targets written by the panel projection must exist in the
    // compiled panel template. A refused write aborts the scoped panel pass
    // after the Start button, which is the "only Start renders" failure.
    // TRIGGER: panel.html id/child edits or project_clock / project_workspaces changes.
    // OBSERVABLE: project_clock_at and each pager/tray target succeed on the real artifact.
    // MUTATION: drop the text child of #clock-time; project_clock_at must be refused.
    #[test]
    fn every_projected_panel_clock_and_pager_target_is_writable() {
        let mut document = decode_panel();
        project_clock_at(&mut document, Local::now(), ClockDisplay::default())
            .unwrap_or_else(|error| panic!("panel clock refused: {error}"));
        for slot in 1..=PAGER_CELLS {
            let id = format!("workspace-{slot}");
            document
                .text(&id, String::new())
                .unwrap_or_else(|error| panic!("panel pager text '{id}' refused: {error}"));
            document
                .visible(&id, true)
                .unwrap_or_else(|error| panic!("panel pager visible '{id}' refused: {error}"));
        }
        for id in ["tray-media", "tray-volume", "tray-network"] {
            document
                .visible(id, true)
                .unwrap_or_else(|error| panic!("panel tray '{id}' refused: {error}"));
        }
    }

    // REGRESSION: the clock, workspace pager and tray must lay out with a
    // non-zero rect inside the panel viewport. The live panel window is
    // 1350x90 device px at 125%, i.e. a 1080x72 CSS viewport. A device-pixel
    // taskbar width (1350) pushed these nodes past the right edge, so they
    // were laid out but never painted.
    // OBSERVABLE: every listed node has width/height > 0 and lies inside 1080x72.
    // MUTATION: pass geometry.width unscaled to taskbar_css_width (or to size)
    // and the rightmost nodes fall outside and this test fails.
    #[test]
    fn panel_clock_pager_tray_lay_out_inside_scaled_window() {
        let mut document = decode_panel();
        // Live panel: 1350x90 device px window, 125% output scale, logical 72.
        let output = flamewm_api::OutputId::new("HDMI-1");
        let mut snapshot = crate::ShellSnapshot::default();
        snapshot
            .displays
            .outputs
            .push(flamewm_api::display::OutputSnapshot {
                id: output.clone(),
                connector: "HDMI-1".to_owned(),
                edid_identity: String::new(),
                connected: true,
                primary: true,
                geometry: flamewm_api::Rect::new(0, 0, 1350, 641),
                current_mode: flamewm_api::ModeId(1),
                modes: Vec::new(),
                shell_scale_percent: 125,
            });
        snapshot
            .panels
            .panels
            .push(flamewm_api::panels::PanelSnapshot {
                output,
                edge: flamewm_api::PanelEdge::Bottom,
                logical_size: 72,
                visible: true,
                geometry: flamewm_api::Rect::new(0, 551, 1350, 90),
            });
        project_taskbar_height(&mut document, PanelView::first(&snapshot))
            .expect("taskbar size projects from the panel snapshot");
        project_clock_at(&mut document, Local::now(), ClockDisplay::default())
            .expect("clock projects");
        for slot in 1..=PAGER_CELLS {
            let id = format!("workspace-{slot}");
            document
                .text(&id, slot.to_string())
                .expect("pager text projects");
            document.visible(&id, true).expect("pager visible projects");
        }
        for id in [
            "tray-media",
            "tray-play-icon",
            "tray-volume",
            "tray-volume-icon",
            "tray-network",
        ] {
            document.visible(id, true).expect("tray visible projects");
        }
        let mut checked = Vec::new();
        for id in [
            "clock-time",
            "clock-date",
            "clock-button",
            "workspace-1",
            "workspace-2",
            "workspace-3",
            "workspace-4",
            "workspace-pager",
            "tray-media",
            "tray-volume",
            "tray-network",
            "tray-wifi",
        ] {
            let (x, y, width, height) = document
                .layout_box_by_id(id, (1080.0, 72.0))
                .unwrap_or_else(|| panic!("panel node '{id}' missing from artifact"));
            assert!(
                width > 0.0 && height > 0.0,
                "panel node '{id}' is zero-size: ({x}, {y}, {width}, {height})"
            );
            assert!(
                x >= 0.0 && y >= 0.0 && x + width <= 1080.5 && y + height <= 72.5,
                "panel node '{id}' lies outside the 1080x72 viewport: ({x}, {y}, {width}, {height})"
            );
            checked.push(id);
        }
        assert_eq!(checked.len(), 12);
    }

    fn pager_snapshot(count: usize, active_index: usize) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            revision: 1,
            count,
            active_index,
            last_index: None,
            names: Vec::new(),
        }
    }

    // REGRESSION: the pager must draw the same two-row topology the WM
    // navigates (`1 2 / 3 4`, `1 2 3 / 4 5 6`), not a fixed 2x2 page of four
    // slots, up to nine columns at the 18 workspace maximum. At four
    // workspaces every cell keeps the device rect it had before the pager
    // became count-dependent, and only then does the box grow.
    // TRIGGER: panel.html pager markup, ui/shell/flamewm.css pager rules or
    // project_pager changes.
    // OBSERVABLE: `workspace-column-j` / `workspace-k` device rects on the
    // compiled panel artifact.
    // MUTATION: project cell ids by page slot, or drop `position:absolute` from
    // `.workspace-column` (flow columns take width, so hidden columns would
    // shift every later cell right).
    #[test]
    fn pager_cells_follow_the_two_row_workspace_topology() {
        let viewport = (1080.0, 72.0);
        let mut document = decode_panel();
        document
            .visible("workspace-pager", true)
            .expect("pager node projects");
        let rect = |document: &flamewm_ui_x11::UiDocument, id: &str| {
            document
                .layout_box_by_id(id, viewport)
                .unwrap_or_else(|| panic!("panel node '{id}' missing from artifact"))
        };
        let column_rects = |document: &flamewm_ui_x11::UiDocument| {
            (1..=PAGER_COLUMNS)
                .map(|column| rect(document, &format!("workspace-column-{column}")))
                .collect::<Vec<_>>()
        };
        let hidden = (0.0, 0.0, 0.0, 0.0);

        // Four workspaces: `1 2 / 3 4`. Workspace index 0/1 fill the upper
        // cells of columns 1/2, index 2/3 the lower ones.
        project_pager(&mut document, Some(&pager_snapshot(4, 0))).expect("pager projects");
        let (pager_x, pager_y, pager_width_px, pager_height) = rect(&document, "workspace-pager");
        assert_eq!(
            (pager_width_px, pager_height),
            (51.0, 44.0),
            "four workspaces keep today's two-column pager box"
        );
        for (index, cell, column, row) in [
            (0usize, 1usize, 0usize, 0usize),
            (1, 3, 1, 0),
            (2, 2, 0, 1),
            (3, 4, 1, 1),
        ] {
            assert_eq!(pager_cell(index, 4), Some(cell), "workspace {index} cell");
            assert_eq!(
                rect(&document, &format!("workspace-{cell}")),
                (
                    pager_x + 3.0 + column as f32 * 23.0,
                    pager_y + 4.0 + row as f32 * 17.0,
                    22.0,
                    16.0
                ),
                "workspace {index} device rect"
            );
        }
        for cell in 5..=PAGER_CELLS {
            assert_eq!(
                rect(&document, &format!("workspace-{cell}")),
                hidden,
                "cell {cell}"
            );
        }
        let columns = column_rects(&document);
        assert_eq!(columns[0], (pager_x + 3.0, pager_y + 4.0, 22.0, 33.0));
        assert_eq!(columns[1], (pager_x + 26.0, pager_y + 4.0, 22.0, 33.0));
        for (index, column) in columns.iter().enumerate().skip(2) {
            assert_eq!(*column, hidden, "column {}", index + 1);
        }

        // Six workspaces: `1 2 3 / 4 5 6` needs a third column and a 74px box.
        project_pager(&mut document, Some(&pager_snapshot(6, 5))).expect("pager projects");
        let (pager_x, pager_y, pager_width_px, _) = rect(&document, "workspace-pager");
        assert_eq!(pager_width_px, 74.0);
        for (index, cell, column, row) in [
            (0usize, 1usize, 0usize, 0usize),
            (1, 3, 1, 0),
            (2, 5, 2, 0),
            (3, 2, 0, 1),
            (4, 4, 1, 1),
            (5, 6, 2, 1),
        ] {
            assert_eq!(pager_cell(index, 6), Some(cell), "workspace {index} cell");
            assert_eq!(
                rect(&document, &format!("workspace-{cell}")),
                (
                    pager_x + 3.0 + column as f32 * 23.0,
                    pager_y + 4.0 + row as f32 * 17.0,
                    22.0,
                    16.0
                ),
                "workspace {index} device rect"
            );
        }
        let columns = column_rects(&document);
        assert_eq!(columns[2], (pager_x + 49.0, pager_y + 4.0, 22.0, 33.0));
        for (index, column) in columns.iter().enumerate().skip(3) {
            assert_eq!(*column, hidden, "column {}", index + 1);
        }

        // Eighteen workspaces: nine columns, a 212px box, every cell filled.
        project_pager(&mut document, Some(&pager_snapshot(18, 9))).expect("pager projects");
        let (pager_x, pager_y, pager_width_px, _) = rect(&document, "workspace-pager");
        assert_eq!(pager_width_px, 212.0);
        let columns = column_rects(&document);
        assert_eq!(columns.len(), PAGER_COLUMNS);
        for (index, column) in columns.iter().enumerate() {
            assert_eq!(
                *column,
                (
                    pager_x + 3.0 + index as f32 * 23.0,
                    pager_y + 4.0,
                    22.0,
                    33.0
                ),
                "column {}",
                index + 1
            );
        }
        for cell in 1..=PAGER_CELLS {
            let column = (cell - 1) / 2;
            let row = (cell - 1) % 2;
            assert_eq!(
                rect(&document, &format!("workspace-{cell}")),
                (
                    pager_x + 3.0 + column as f32 * 23.0,
                    pager_y + 4.0 + row as f32 * 17.0,
                    22.0,
                    16.0
                ),
                "cell {cell} device rect"
            );
        }
        // With eighteen workspaces every cell holds exactly one workspace, so
        // the cell -> workspace mapping is a bijection over 1..=18.
        for index in 0..18 {
            let cell = pager_cell(index, 18).expect("workspace cell");
            assert_eq!(cell_index(cell, 18), Some(index), "cell {cell} workspace");
        }
    }

    // REGRESSION: the battery popup shows exactly one glyph node, charging-aware,
    // on the compiled flamewm-battery.rwr artifact for each charge bucket.
    // MUTATION: keep a retired low node or project the wrong bucket in the menu.
    #[test]
    fn battery_menu_shows_exactly_one_glyph_on_real_artifact() {
        const BATTERY_ARTIFACT: &[u8] =
            include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-battery.rwr"));
        let cases = [
            (5, PowerState::Discharging, "battery-icon-caution"),
            (15, PowerState::Discharging, "battery-icon-020"),
            (45, PowerState::Discharging, "battery-icon-050"),
            (100, PowerState::Discharging, "battery-icon-100"),
            (45, PowerState::Charging, "battery-icon-050-charging"),
        ];
        let ids = glyph_node_ids("battery-icon-");
        for (percent, state, expected) in cases {
            let power = power_snapshot(ServiceAvailability::Available, true, percent, state);
            let mut document = flamewm_ui_x11::decode_document(BATTERY_ARTIFACT)
                .expect("battery artifact must decode");
            project_battery_menu(&mut document, &power, 268.0).expect("battery menu projects");
            assert_eq!(
                lit_ids(&document, &ids, (300.0, 132.0)),
                vec![expected],
                "{percent}% {state:?}"
            );
        }
    }

    // REGRESSION: the tray battery button and its glyph must follow the power
    // bucket on the compiled panel through every panel projection path: the
    // sync `project` pass, the full async refresh, and a power-only SystemChanged
    // scope. Visible = laid out with a non-zero box.
    // OBSERVABLE: hidden only when the service is unavailable or no battery is
    // present; charging and full batteries show one glyph (Caution 0-10,
    // otherwise the level rounded up to 10%).
    // MUTATION: drop the battery projection from one path (default markup stays
    // hidden there, so that path's case fails) or skip the glyph bucket.
    #[test]
    fn tray_battery_button_follows_power_bucket_on_real_artifact() {
        let glyphs = glyph_node_ids("tray-battery-");
        let avail = ServiceAvailability::Available;
        let cases: [(PowerSnapshot, Option<&str>); 14] = [
            (
                power_snapshot(avail, true, 87, PowerState::Unknown),
                Some("tray-battery-090"),
            ),
            (
                power_snapshot(avail, true, 45, PowerState::Discharging),
                Some("tray-battery-050"),
            ),
            (
                power_snapshot(avail, true, 45, PowerState::Unknown),
                Some("tray-battery-050"),
            ),
            (
                power_snapshot(avail, true, 15, PowerState::Discharging),
                Some("tray-battery-020"),
            ),
            (
                power_snapshot(avail, true, 5, PowerState::Discharging),
                Some("tray-battery-caution"),
            ),
            (
                power_snapshot(avail, true, 30, PowerState::Charging),
                Some("tray-battery-030-charging"),
            ),
            (
                power_snapshot(avail, true, 79, PowerState::Charging),
                Some("tray-battery-080-charging"),
            ),
            (
                power_snapshot(avail, true, 80, PowerState::Charging),
                Some("tray-battery-080-charging"),
            ),
            (
                power_snapshot(avail, true, 100, PowerState::Charging),
                Some("tray-battery-100-charging"),
            ),
            (
                power_snapshot(
                    ServiceAvailability::Unavailable,
                    true,
                    40,
                    PowerState::Discharging,
                ),
                None,
            ),
            (
                power_snapshot(avail, true, 79, PowerState::Discharging),
                Some("tray-battery-080"),
            ),
            (
                power_snapshot(avail, true, 80, PowerState::Discharging),
                Some("tray-battery-080"),
            ),
            (
                power_snapshot(avail, true, 100, PowerState::Full),
                Some("tray-battery-100"),
            ),
            (
                power_snapshot(avail, false, 45, PowerState::Discharging),
                None,
            ),
        ];
        for (power, expected) in cases {
            let mut snapshot = crate::ShellSnapshot::default();
            snapshot.system.power = power;
            let paths: [(
                &str,
                fn(&mut flamewm_ui_x11::UiDocument, &crate::ShellSnapshot),
            ); 3] = [
                ("sync project", |document, snapshot| {
                    let mut resolver = flamewm_integrations_linux::icons::IconResolver::new(
                        std::path::PathBuf::new(),
                        Vec::new(),
                        Vec::new(),
                        0,
                    );
                    project(document, snapshot, &mut resolver).expect("sync panel projects");
                }),
                ("async full refresh", |document, snapshot| {
                    crate::async_projection::project_panel_async(
                        document,
                        PanelView::first(snapshot),
                        None,
                        &mut Default::default(),
                    )
                    .expect("async panel projects");
                }),
                ("async system scope", |document, snapshot| {
                    crate::async_projection::project_panel_for_changed(
                        document,
                        PanelView::first(snapshot),
                        None,
                        &mut Default::default(),
                        crate::runtime::ChangedDomains {
                            system: true,
                            ..crate::runtime::ChangedDomains::default()
                        },
                    )
                    .expect("system-scope panel projects");
                }),
            ];
            for (path, project_path) in paths {
                let mut document = decode_panel();
                project_path(&mut document, &snapshot);
                let viewport = (1080.0, 72.0);
                let button = box_width(&document, "tray-battery", viewport) > 0.0;
                assert_eq!(
                    button,
                    expected.is_some(),
                    "{path}: button for {:?}",
                    snapshot.system.power
                );
                let lit = lit_ids(&document, &glyphs, viewport);
                match expected {
                    Some(glyph) => assert_eq!(
                        lit,
                        vec![glyph],
                        "{path}: glyph for {:?}",
                        snapshot.system.power
                    ),
                    None => assert!(
                        lit.is_empty(),
                        "{path}: no glyph while hidden for {:?}",
                        snapshot.system.power
                    ),
                }
            }
        }
    }

    // REGRESSION: the tray battery matrix over state x percent x presence. The
    // button shows iff present and state != Unknown; exactly one glyph is
    // lit, and only the Charging case may light a `-charging` node.
    // MUTATION: light the plain glyph while charging, or drop the charging
    // hide rule at 80%.
    #[test]
    fn battery_tray_matrix_shows_one_glyph_and_charging_variant_only_while_charging() {
        let glyph_for = |percent: u8| match percent {
            15 => "020",
            45 => "050",
            79 | 80 => "080",
            100 => "100",
            other => panic!("matrix has no expected glyph for {other}%"),
        };
        let all_ids = glyph_node_ids("tray-battery-");
        for state in [
            PowerState::Charging,
            PowerState::Discharging,
            PowerState::Full,
            PowerState::Empty,
            PowerState::Unknown,
        ] {
            for percent in [15_u8, 45, 79, 80, 100] {
                for present in [true, false] {
                    let power =
                        power_snapshot(ServiceAvailability::Available, present, percent, state);
                    let charging = state == PowerState::Charging;
                    let visible = present;
                    let mut document = decode_panel();
                    project_battery_tray(&mut document, &power, TaskbarItems::default())
                        .expect("tray battery projects");
                    let viewport = (1080.0, 72.0);
                    assert_eq!(
                        box_width(&document, "tray-battery", viewport) > 0.0,
                        visible,
                        "{state:?} {percent}% present={present}"
                    );
                    let lit = lit_ids(&document, &all_ids, viewport);
                    let expected = if visible {
                        let suffix = if charging { "-charging" } else { "" };
                        vec![format!("tray-battery-{}{suffix}", glyph_for(percent))]
                    } else {
                        Vec::new()
                    };
                    assert_eq!(lit, expected, "{state:?} {percent}% present={present}");
                }
            }
        }
    }

    // Full cold-start panel pass against the real artifact: a refused write in
    // any scope (task slots, geometry, pager, tray, clock) aborts the pass
    // before the later nodes are written.
    #[test]
    fn full_panel_projection_succeeds_on_real_artifact() {
        let mut document = decode_panel();
        let snapshot = crate::ShellSnapshot::default();
        crate::async_projection::project_panel_async(
            &mut document,
            PanelView::first(&snapshot),
            None,
            &mut Default::default(),
        )
        .unwrap_or_else(|error| panic!("full panel projection refused: {error}"));
    }
}

#[cfg(test)]
mod panel_record_tests {
    use super::*;
    use flamewm_api::{OutputId, PanelEdge, Rect};

    fn output(id: &str, x: i32, scale: u16) -> flamewm_api::display::OutputSnapshot {
        flamewm_api::display::OutputSnapshot {
            id: OutputId::new(id),
            connector: id.to_owned(),
            edid_identity: String::new(),
            connected: true,
            primary: x == 0,
            geometry: Rect::new(x, 0, 1920, 1080),
            current_mode: flamewm_api::ModeId::default(),
            modes: Vec::new(),
            shell_scale_percent: scale,
        }
    }

    #[test]
    fn task_icon_request_scales_with_output_shell_scale() {
        assert_eq!(task_icon_px(None), 25);
        assert_eq!(task_icon_px(Some(&output("a", 0, 100))), 25);
        assert_eq!(task_icon_px(Some(&output("a", 0, 125))), 31);
        assert_eq!(task_icon_px(Some(&output("a", 0, 150))), 38);
        assert_eq!(task_icon_px(Some(&output("a", 0, 200))), 50);
    }

    fn panel(id: &str, x: i32) -> flamewm_api::panels::PanelSnapshot {
        flamewm_api::panels::PanelSnapshot {
            output: OutputId::new(id),
            edge: PanelEdge::Bottom,
            logical_size: 44,
            visible: true,
            geometry: Rect::new(x, 1036, 1920, 44),
        }
    }

    #[test]
    fn panel_projection_visits_every_record_with_its_own_geometry_and_scale() {
        let mut snapshot = ShellSnapshot::default();
        snapshot.displays.outputs = vec![output("A", 0, 100), output("B", 1920, 150)];
        snapshot.panels.panels = vec![panel("A", 0), panel("B", 1920)];
        let mut seen: Vec<(&str, Rect, u16)> = Vec::new();
        let result = for_each_panel_record(
            &snapshot,
            vec![
                ("primary", OutputId::new("A")),
                ("secondary", OutputId::new("B")),
            ],
            |key: &str, view: PanelView<'_>| -> Result<(), String> {
                seen.push((
                    key,
                    view.panel.expect("panel").geometry,
                    view.output.expect("output").shell_scale_percent,
                ));
                Ok(())
            },
        );
        assert_eq!(result, Ok(()));
        assert_eq!(
            seen,
            vec![
                ("primary", Rect::new(0, 1036, 1920, 44), 100),
                ("secondary", Rect::new(1920, 1036, 1920, 44), 150),
            ]
        );
    }

    #[test]
    fn panel_projection_skips_disconnected_outputs_and_revisits_replugged_output() {
        let mut snapshot = ShellSnapshot::default();
        snapshot.displays.outputs = vec![output("A", 0, 100), output("B", 1920, 150)];
        snapshot.panels.panels = vec![panel("A", 0), panel("B", 1920)];
        let records = vec![
            ("primary", OutputId::new("A")),
            ("secondary", OutputId::new("B")),
        ];
        let mut seen = Vec::new();

        snapshot.displays.outputs[1].connected = false;
        for_each_panel_record(&snapshot, records.iter().cloned(), |key, _| {
            seen.push(key);
            Ok::<_, String>(())
        })
        .expect("partial output snapshot projects");
        assert_eq!(seen, vec!["primary"]);

        snapshot.displays.outputs[1].connected = true;
        snapshot.displays.outputs[1].shell_scale_percent = 175;
        for_each_panel_record(&snapshot, records, |key, view| {
            seen.push(key);
            if key == "secondary" {
                assert_eq!(
                    view.panel.expect("panel").geometry,
                    Rect::new(1920, 1036, 1920, 44)
                );
                assert_eq!(view.output.expect("output").shell_scale_percent, 175);
            }
            Ok::<_, String>(())
        })
        .expect("replugged output projects with fresh scale");
        assert_eq!(seen, vec!["primary", "primary", "secondary"]);
    }
}

#[cfg(test)]
mod clock_display_tests {
    use super::{clock_time_text, ClockDisplay, ClockFormat};
    use chrono::{Local, TimeZone};

    fn at(hour: u32, minute: u32, second: u32) -> chrono::DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 10, 12, hour, minute, second)
            .single()
            .expect("unambiguous local time")
    }

    fn display(format: ClockFormat, show_seconds: bool) -> ClockDisplay {
        ClockDisplay {
            format,
            show_seconds,
            show_date: true,
        }
    }

    #[test]
    fn twenty_four_hour_clock_omits_seconds_by_default() {
        let text = clock_time_text(display(ClockFormat::TwentyFour, false), at(14, 5, 9));
        assert_eq!(text, "14:05");
    }

    #[test]
    fn twenty_four_hour_clock_shows_seconds_when_enabled() {
        let text = clock_time_text(display(ClockFormat::TwentyFour, true), at(14, 5, 9));
        assert_eq!(text, "14:05:09");
    }

    #[test]
    fn twelve_hour_clock_uses_unpadded_hour_and_meridiem() {
        let text = clock_time_text(display(ClockFormat::Twelve, false), at(14, 5, 9));
        assert_eq!(text, "2:05 PM");
    }

    #[test]
    fn twelve_hour_midnight_and_noon_map_to_twelve() {
        assert_eq!(
            clock_time_text(display(ClockFormat::Twelve, false), at(0, 30, 0)),
            "12:30 AM"
        );
        assert_eq!(
            clock_time_text(display(ClockFormat::Twelve, false), at(12, 0, 0)),
            "12:00 PM"
        );
    }

    #[test]
    fn twelve_hour_clock_shows_seconds_when_enabled() {
        let text = clock_time_text(display(ClockFormat::Twelve, true), at(9, 7, 3));
        assert_eq!(text, "9:07:03 AM");
    }

    #[test]
    fn default_display_matches_product_defaults() {
        let defaults = ClockDisplay::default();
        assert_eq!(defaults.format, ClockFormat::TwentyFour);
        assert!(!defaults.show_seconds);
        assert!(defaults.show_date);
    }
}

#[cfg(test)]
mod media_projection_tests {
    use super::*;
    use crate::async_projection::tests::FakeDoc;

    // REGRESSION: the popup has one title row and no artist line, so project_media
    // must not write `media-artist` (the compiled artifact no longer has that node).
    #[test]
    fn media_projects_title_only_without_artist_node() {
        let mut snapshot = crate::ShellSnapshot::default();
        snapshot.system.media.title = "Song Title".to_owned();
        snapshot.system.media.artist = "Some Artist".to_owned();
        let mut document = FakeDoc::default();

        project_media(&mut document, &snapshot).expect("media projection must succeed");

        assert_eq!(
            document.texts.get("media-title").map(String::as_str),
            Some("Song Title")
        );
        assert!(
            !document.texts.contains_key("media-artist"),
            "artist line was removed from media.html"
        );
    }

    fn media_title_text(title: &str) -> String {
        let mut snapshot = crate::ShellSnapshot::default();
        snapshot.system.media.title = title.to_owned();
        let mut document = FakeDoc::default();
        project_media(&mut document, &snapshot).expect("media projection must succeed");
        document
            .texts
            .get("media-title")
            .cloned()
            .expect("media-title must be written")
    }

    #[test]
    fn media_popup_projects_title_and_controls_only_no_artist_node() {
        let mut snapshot = crate::ShellSnapshot::default();
        snapshot.system.media.title = "Song Title".to_owned();
        snapshot.system.media.artist = "Some Artist".to_owned();
        snapshot.system.media.availability = flamewm_api::system::ServiceAvailability::Available;
        snapshot.system.media.playback = flamewm_api::system::PlaybackState::Playing;
        snapshot.system.media.can_previous = true;
        snapshot.system.media.can_next = true;
        let mut document = FakeDoc::default();

        project_media(&mut document, &snapshot).expect("media projection must succeed");

        let mut text_ids: Vec<&str> = document.texts.keys().map(String::as_str).collect();
        text_ids.sort_unstable();
        assert_eq!(
            text_ids,
            ["media-title"],
            "popup text nodes must be title only"
        );
        assert!(!document.texts.contains_key("media-artist"));
        assert_eq!(document.visible.get("media-previous"), Some(&true));
        assert_eq!(document.visible.get("media-next"), Some(&true));
    }

    #[test]
    fn media_title_short_is_unchanged() {
        assert_eq!(media_title_text("Song Title"), "Song Title");
        let exact = "a".repeat(MEDIA_TITLE_MAX_CHARS);
        assert_eq!(media_title_text(&exact), exact);
    }

    #[test]
    fn media_title_long_ascii_is_truncated_with_ellipsis_within_budget() {
        let long = format!("{} tail words that keep going", "Abcdefghij ".repeat(5));
        let fitted = media_title_text(&long);
        assert!(fitted.ends_with('…'), "{fitted:?}");
        assert!(
            fitted.chars().count() <= MEDIA_TITLE_MAX_CHARS,
            "{fitted:?}"
        );
        assert!(fitted.starts_with("Abcdefghij Abcdefghij"), "{fitted:?}");
        assert!(!fitted.contains("  "), "{fitted:?}");
    }

    #[test]
    fn media_title_trims_whitespace_before_ellipsis() {
        // Budget keeps 31 chars: 30 'x' plus one space, which must be trimmed.
        let title = format!("{}     {}", "x".repeat(30), "y".repeat(10));
        let fitted = media_title_text(&title);
        assert_eq!(fitted, format!("{}…", "x".repeat(30)));
    }

    #[test]
    fn media_title_multibyte_and_emoji_do_not_panic_and_end_on_char_boundary() {
        let title = "🎵 Ünïcødé 日本語のタイトル 🎶 ".repeat(4);
        let fitted = media_title_text(&title);
        assert!(fitted.ends_with('…'), "{fitted:?}");
        assert!(
            fitted.chars().count() <= MEDIA_TITLE_MAX_CHARS,
            "{fitted:?}"
        );
        assert!(std::str::from_utf8(fitted.as_bytes()).is_ok());
    }

    // REGRESSION: a disabled control must show only its action-less twin; the
    // enabled button (which carries data-action) must be hidden, and vice versa.
    #[test]
    fn media_controls_swap_to_disabled_twins_by_capability() {
        let mut snapshot = crate::ShellSnapshot::default();
        snapshot.system.media.availability = flamewm_api::system::ServiceAvailability::Available;
        snapshot.system.media.can_next = true;
        snapshot.system.media.can_play = true;
        let mut document = FakeDoc::default();

        project_media(&mut document, &snapshot).expect("media projection must succeed");

        let visible = |id: &str| document.visible.get(id).copied();
        assert_eq!(visible("media-next"), Some(true));
        assert_eq!(visible("media-next-disabled"), Some(false));
        assert_eq!(visible("media-playpause"), Some(true));
        assert_eq!(visible("media-playpause-disabled"), Some(false));
        assert_eq!(visible("media-previous"), Some(false));
        assert_eq!(visible("media-previous-disabled"), Some(true));
    }

    #[test]
    fn media_controls_all_disabled_without_player() {
        let snapshot = crate::ShellSnapshot::default();
        let mut document = FakeDoc::default();

        project_media(&mut document, &snapshot).expect("media projection must succeed");

        for id in ["media-previous", "media-playpause", "media-next"] {
            assert_eq!(document.visible.get(id).copied(), Some(false), "{id}");
        }
        for id in [
            "media-previous-disabled",
            "media-playpause-disabled",
            "media-next-disabled",
        ] {
            assert_eq!(document.visible.get(id).copied(), Some(true), "{id}");
        }
    }
}

#[cfg(test)]
mod start_category_table_tests {
    use super::*;
    use flamewm_api::DesktopAppId;

    fn application(categories: &[&str]) -> DesktopApplication {
        DesktopApplication {
            id: DesktopAppId::new("org.example.app"),
            name: "App".to_owned(),
            generic_name: String::new(),
            comment: String::new(),
            startup_wm_class: String::new(),
            argv: Vec::new(),
            keywords: Vec::new(),
            icon_name: String::new(),
            categories: categories.iter().map(|value| (*value).to_owned()).collect(),
        }
    }

    /// Literal oracle: the presentation bucket each application set must land in.
    /// Unmatched or empty category sets fall to Utilities.
    const ORACLE: &[(&[&str], StartCategory)] = &[
        (&["Accessories"], StartCategory::Utilities),
        (&["utility"], StartCategory::Utilities),
        (&["Education"], StartCategory::Utilities),
        (&["Game"], StartCategory::Games),
        (&["GAMES"], StartCategory::Games),
        (&["Graphics"], StartCategory::Graphics),
        (&["Network"], StartCategory::Internet),
        (&["Internet"], StartCategory::Internet),
        (&["AudioVideo"], StartCategory::Multimedia),
        (&["Audio"], StartCategory::Multimedia),
        (&["Video"], StartCategory::Multimedia),
        (&["Multimedia"], StartCategory::Multimedia),
        (&["Development"], StartCategory::Development),
        (&["Office"], StartCategory::Utilities),
        (&["Science"], StartCategory::Utilities),
        (&["Settings"], StartCategory::System),
        (&["System"], StartCategory::System),
        (&["Unknown"], StartCategory::Utilities),
        (&[""], StartCategory::Utilities),
        (&[], StartCategory::Utilities),
        (&["Unknown", "Games"], StartCategory::Games),
        (&["Office", "Games"], StartCategory::Utilities),
        (&["Unknown", "Development"], StartCategory::Development),
    ];

    #[test]
    fn application_matches_category_is_the_oracle_table_for_every_selectable_category() {
        let rows = [
            (StartCategory::Development, StartCategory::Development),
            (StartCategory::Games, StartCategory::Games),
            (StartCategory::Graphics, StartCategory::Graphics),
            (StartCategory::Internet, StartCategory::Internet),
            (StartCategory::Multimedia, StartCategory::Multimedia),
            (StartCategory::System, StartCategory::System),
            (StartCategory::Utilities, StartCategory::Utilities),
        ];
        for (categories, bucket) in ORACLE {
            let app = application(categories);
            for (selected, expected_bucket) in rows {
                assert_eq!(
                    application_matches_category(&app, selected),
                    expected_bucket == *bucket,
                    "categories {categories:?} vs {selected:?}"
                );
            }
            assert!(application_matches_category(&app, StartCategory::All));
            assert!(!application_matches_category(&app, StartCategory::Power));
        }
    }
}

#[cfg(test)]
mod task_menu_root_tests {
    use super::*;
    use flamewm_ui_core::style::UiLayer;
    use flamewm_ui_x11::{Overflow, RuntimeImage};

    const TASK_MENU_HTML: &str = include_str!("../../../ui/shell/task-menu.html");

    /// Records `(method, id)` for every write and never fails, so the set is exactly what projection asked for.
    #[derive(Default)]
    struct Recorder {
        writes: Vec<(&'static str, String)>,
    }

    impl Recorder {
        fn hit(&mut self, method: &'static str, id: &str) -> Result<(), String> {
            self.writes.push((method, id.to_owned()));
            Ok(())
        }
    }

    impl UiDocumentAccess for Recorder {
        fn text(&mut self, id: &str, _: String) -> Result<(), String> {
            self.hit("text", id)
        }
        fn image_rgb8(&mut self, id: &str, _: RuntimeImage) -> Result<(), String> {
            self.hit("image_rgb8", id)
        }
        fn image_rgba8(&mut self, id: &str, _: RuntimeImage) -> Result<(), String> {
            self.hit("image_rgba8", id)
        }
        fn visible(&mut self, id: &str, _: bool) -> Result<(), String> {
            self.hit("visible", id)
        }
        fn toggle(&mut self, track_id: &str, _: &str, _: bool) -> Result<(), String> {
            self.hit("toggle", track_id)
        }
        fn position(&mut self, id: &str, _: f32, _: f32) -> Result<(), String> {
            self.hit("position", id)
        }
        fn size(&mut self, id: &str, _: f32, _: f32) -> Result<(), String> {
            self.hit("size", id)
        }
        fn background(&mut self, id: &str, _: UiColor) -> Result<(), String> {
            self.hit("background", id)
        }
        fn border(&mut self, id: &str, _: UiColor) -> Result<(), String> {
            self.hit("border", id)
        }
        fn foreground(&mut self, id: &str, _: UiColor) -> Result<(), String> {
            self.hit("foreground", id)
        }
        fn background_clear(&mut self, id: &str) -> Result<(), String> {
            self.hit("background_clear", id)
        }
        fn border_clear(&mut self, id: &str) -> Result<(), String> {
            self.hit("border_clear", id)
        }
        fn foreground_clear(&mut self, id: &str) -> Result<(), String> {
            self.hit("foreground_clear", id)
        }
        fn font_size(&mut self, id: &str, _: f32) -> Result<(), String> {
            self.hit("font_size", id)
        }
        fn font_weight(&mut self, id: &str, _: u16) -> Result<(), String> {
            self.hit("font_weight", id)
        }
        fn overflow(&mut self, id: &str, _: Overflow, _: Overflow) -> Result<(), String> {
            self.hit("overflow", id)
        }
        fn layer(&mut self, id: &str, _: UiLayer) -> Result<(), String> {
            self.hit("layer", id)
        }
    }

    fn project_idle() -> Recorder {
        let mut recorder = Recorder::default();
        let snapshot = ShellSnapshot::default();
        let settings = SettingsSnapshot::new(0);
        project_context_menu(&mut recorder, &snapshot, &settings, None).expect("projection");
        recorder
    }

    #[test]
    fn task_menu_projection_touches_only_existing_roots() {
        let recorder = project_idle();
        assert!(!recorder.writes.is_empty());
        for (method, id) in &recorder.writes {
            assert!(
                id == "task-menu" || TASK_MENU_HTML.contains(&format!("id=\"{id}\"")),
                "{method} writes `{id}`, which the task menu markup does not define"
            );
        }
    }

    #[test]
    fn task_menu_projection_never_writes_a_border() {
        let recorder = project_idle();
        assert!(
            recorder
                .writes
                .iter()
                .all(|(method, _)| !method.starts_with("border")),
            "task menu frame comes from the shared sheet; projection must not paint a border: {:?}",
            recorder.writes
        );
    }
}

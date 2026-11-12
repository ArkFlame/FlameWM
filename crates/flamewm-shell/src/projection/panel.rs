//! Taskbar panel projection: task slots, pager, tray, element backgrounds and battery.

use super::*;

const TASK_SLOT_COUNT: usize = 8;
/// Bar height in CSS px used when the panel snapshot carries no logical size.
/// Matches the `.taskbar` height in ui/shell/flamewm.css.
const TASKBAR_DEFAULT_HEIGHT: f32 = 44.0;
/// Logical edge of a taskbar task icon, before the output's shell scale.
const TASK_ICON_LOGICAL_PX: f32 = 25.0;
/// `.task-slot` width in CSS px (`ui/shell/flamewm.css`).
const TASK_SLOT_WIDTH: f32 = 42.0;
/// Fixed chrome beside the task cells, in CSS px: start capsule (44), workspace pager (51), clock
/// (74) and a tray budget of six buttons. shortcut: fixed budget, not measured; revisit when the
/// `#tray-group` and `#clock` widths are read back from the document.
const TASK_CHROME_RESERVED_PX: f32 = 44.0 + 51.0 + 74.0 + 6.0 * TRAY_BUTTON_WIDTH;
/// Cells that can fit: the eight task slots plus the overflow cell.
const TASK_CELL_CAPACITY_MAX: usize = TASK_SLOT_COUNT + 1;
/// `.underline` track width and heights in CSS px; the projection re-applies them scaled and
/// centered on the slot so the line cannot drift from the icon at another `taskbarHeight`.
const TASK_LINE_WIDTH: f32 = 28.0;
const TASK_LINE_HEIGHT: f32 = 2.0;
const TASK_LINE_FOCUSED_HEIGHT: f32 = 3.0;
/// `.tray-button` width in CSS px.
const TRAY_BUTTON_WIDTH: f32 = 30.0;
/// `.tray-button img` edge in CSS px.
const TRAY_ICON_LOGICAL_PX: f32 = 17.0;
/// `.clock` width in CSS px.
const CLOCK_WIDTH: f32 = 74.0;
/// Clock time and date font sizes in CSS px.
const CLOCK_TIME_FONT: f32 = 14.0;
const CLOCK_DATE_FONT: f32 = 10.0;
/// `.workspace-button` and `.tray-weather-temp` font sizes in CSS px.
const PAGER_CELL_FONT: f32 = 9.0;
const TRAY_WEATHER_FONT: f32 = 9.0;
/// Weather condition glyph edge in CSS px (`.tray-weather img`); smaller than the other tray
/// glyphs so the temperature pill fits below it inside the entry.
const TRAY_WEATHER_GLYPH_PX: f32 = 14.0;
/// Tray glyph nodes (the `img` children of tray buttons) that share `.tray-button img` sizing.
const TRAY_GLYPH_IDS: &[&str] = &[
    "tray-play-icon",
    "tray-pause-icon",
    "tray-volume-icon",
    "tray-volume-muted-icon",
    "tray-wifi",
    "tray-battery-caution",
    "tray-battery-caution-charging",
    "tray-battery-020",
    "tray-battery-020-charging",
    "tray-battery-030",
    "tray-battery-030-charging",
    "tray-battery-040",
    "tray-battery-040-charging",
    "tray-battery-050",
    "tray-battery-050-charging",
    "tray-battery-060",
    "tray-battery-060-charging",
    "tray-battery-070",
    "tray-battery-070-charging",
    "tray-battery-080",
    "tray-battery-080-charging",
    "tray-battery-090",
    "tray-battery-090-charging",
    "tray-battery-100",
    "tray-battery-100-charging",
];
/// Weather condition glyph nodes; sized by `TRAY_WEATHER_GLYPH_PX`, not `TRAY_ICON_LOGICAL_PX`.
const TRAY_WEATHER_GLYPH_IDS: &[&str] = &[
    "tray-weather-clear",
    "tray-weather-few-clouds",
    "tray-weather-fog",
    "tray-weather-none-available",
    "tray-weather-overcast",
    "tray-weather-showers",
    "tray-weather-showers-scattered",
    "tray-weather-snow",
    "tray-weather-storm",
];

/// Taskbar task icon raster edge in device pixels. The icon node is laid out
/// in logical px and scaled by the panel's UI scale, so requesting only 25 px
/// upscales it blurrily on `HiDPI` outputs; requesting the scaled edge matches
/// how desktop entries request icons.
#[must_use]
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

/// Status-line tint for a task whose window missed its ping: the existing `--c-ef4048` fixed tone
/// (`flamewm_skin::shell_tones::FIXED_TONES`), not a new colour. Pinned by a test.
pub(crate) const TASK_NOT_RESPONDING: UiColor = UiColor {
    r: 0xef,
    g: 0x40,
    b: 0x48,
    a: 0xff,
};

/// Status-line tint for a task whose window asked for attention: the existing `--c-f39c12` amber
/// (`flamewm_skin::shell_tones::FIXED_TONES`, already the `swatch-amber` token). Pinned by a test.
pub(crate) const TASK_ATTENTION: UiColor = UiColor {
    r: 0xf3,
    g: 0x9c,
    b: 0x12,
    a: 0xff,
};

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
/// Default matches `ProductSettings::default()`: surfaceColor `#333333` at 65% opacity.
static TASKBAR_SURFACE_ARGB: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0xA533_3333);

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

/// Shared inner contour applied to the separate-panel capsules.
/// The panel settings projection supplies the same token as popup and frame borders.
static TASKBAR_BORDER_ARGB: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0x3fff_ffff);

pub(crate) fn set_taskbar_border(color: UiColor) {
    let packed = (u32::from(color.a) << 24)
        | (u32::from(color.r) << 16)
        | (u32::from(color.g) << 8)
        | u32::from(color.b);
    TASKBAR_BORDER_ARGB.store(packed, std::sync::atomic::Ordering::Relaxed);
}

fn taskbar_border() -> UiColor {
    let packed = TASKBAR_BORDER_ARGB.load(std::sync::atomic::Ordering::Relaxed);
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
    let stroke = taskbar_border();
    let contour = |shown: bool| {
        if items.separate && shown {
            stroke
        } else {
            TRANSPARENT_FILL
        }
    };
    // The capsules own the background; start/clock buttons keep their CSS hover on top.
    document.background("start-capsule", fill)?;
    document.border("start-capsule", contour(true))?;
    document.corner_radius("start-capsule", ELEMENT_RADIUS)?;
    document.node_backdrop("start-capsule", blur)?;
    // A capsule with no icons inside is not painted at all.
    let capsule = |shown: bool| if shown { fill } else { TRANSPARENT_FILL };
    let capsule_blur = |shown: bool| (items.separate && shown).then_some(items.blur_percent);
    document.background("tray-group", capsule(tray_visible))?;
    document.border("tray-group", contour(tray_visible))?;
    document.corner_radius("tray-group", ELEMENT_RADIUS)?;
    document.node_backdrop("tray-group", capsule_blur(tray_visible))?;
    document.background("clock-capsule", capsule(clock_visible))?;
    document.border("clock-capsule", contour(clock_visible))?;
    document.corner_radius("clock-capsule", ELEMENT_RADIUS)?;
    document.node_backdrop("clock-capsule", capsule_blur(clock_visible))?;
    // The pager is a single capsule, just like Start, tasks, tray and clock.
    // Its existing individual workspace cells still own their active/hover fills.
    document.background("workspace-pager", capsule(items.workspaces))?;
    document.border("workspace-pager", contour(items.workspaces))?;
    document.corner_radius("workspace-pager", ELEMENT_RADIUS)?;
    document.node_backdrop("workspace-pager", capsule_blur(items.workspaces))?;
    let slots = if items.separate && rendered_slots > 0 {
        taskbar_surface()
    } else {
        TRANSPARENT_FILL
    };
    document.background("task-slots", slots)?;
    document.border("task-slots", contour(rendered_slots > 0))?;
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
        || crate::tray_host::sni_shown()
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

/// Inactive pager cell fill (matches the `.workspace-button` background): 50% black.
const PAGER_IDLE_FILL: UiColor = UiColor {
    r: 0,
    g: 0,
    b: 0,
    a: 128,
};

/// Idle cell that holds windows: the same idle cell lifted to a lighter tint, so occupancy reads
/// without a new element or class (the projection has no class toggle).
const PAGER_OCCUPIED_FILL: UiColor = UiColor {
    r: 255,
    g: 255,
    b: 255,
    a: 40,
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
    document.size(
        "workspace-pager",
        scaled(pager_width(columns)),
        taskbar_scale::node_height(taskbar_scale::factor()),
    )?;
    for column in 0..PAGER_COLUMNS {
        let column_id = format!("workspace-column-{}", column + 1);
        if column < columns {
            document.position(&column_id, scaled(cell_left(column)), 0.0)?;
            document.size(
                &column_id,
                scaled(PAGER_CELL_WIDTH),
                scaled(2.0 * PAGER_CELL_HEIGHT + PAGER_CELL_GAP),
            )?;
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

/// The pager is decorative when fewer than two workspaces exist: the workspace
/// projection deliberately emits no cells in that case. Hide its entire node
/// (including its separate-taskbar capsule) instead of leaving an empty border.
/// Reused by the synchronous and asynchronous panel paths.
pub(crate) fn workspace_pager_visible(
    items: TaskbarItems,
    snapshot: Option<&flamewm_api::workspace::WorkspaceSnapshot>,
) -> bool {
    items.workspaces && snapshot.is_some_and(|workspaces| workspaces.count > 1)
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
        document.size(
            &cell_id,
            scaled(PAGER_CELL_WIDTH),
            scaled(PAGER_CELL_HEIGHT),
        )?;
        document.font_size(&cell_id, scaled(PAGER_CELL_FONT))?;
        document.text_ref(&cell_id, &workspace.label)?;
        let fill = if workspace.active {
            pager_active_fill()
        } else if workspace.occupied {
            PAGER_OCCUPIED_FILL
        } else {
            PAGER_IDLE_FILL
        };
        document.background(&cell_id, fill)?;
    }
    Ok(())
}

#[must_use]
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
#[must_use]
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
    let logical_size = view.panel.map_or(0, |panel| panel.logical_size);
    taskbar_scale::set_factor(taskbar_scale::scale_for_height(logical_size));
    let Some(panel) = view.panel else {
        return Ok(());
    };
    let scale_percent = view.output.map_or(100, |output| output.shell_scale_percent);
    let (width, height) = taskbar_css_size(panel, scale_percent);
    document.size("taskbar", width, height)?;
    project_taskbar_metrics(document, !panel.edge.is_horizontal())
}

/// True when the taskbar runs down a Left/Right edge: its slots, clock and spacer stack on the
/// vertical axis and their fixed sizes swap axes.
#[must_use]
pub(crate) fn taskbar_is_vertical(view: &PanelView<'_>) -> bool {
    view.panel.is_some_and(|panel| !panel.edge.is_horizontal())
}

/// `#taskbar` border-box size in CSS px for `panel`. A horizontal edge spans the window width
/// with `logical_size` as its height. A vertical edge (Left/Right) spans the window height with
/// `logical_size` as its width, so the bar fills the whole panel window on every edge.
#[must_use]
pub fn taskbar_css_size(
    panel: &flamewm_api::panels::PanelSnapshot,
    scale_percent: u16,
) -> (f32, f32) {
    let thickness = taskbar_css_height(panel.logical_size);
    if panel.edge.is_horizontal() {
        (
            taskbar_css_width(panel.geometry.width, scale_percent),
            thickness,
        )
    } else {
        (
            thickness,
            taskbar_css_width(panel.geometry.height, scale_percent),
        )
    }
}

/// Sizes the fixed-px taskbar nodes and fonts by the taskbar factor, so the contents track the
/// `taskbarHeight` setting along with the bar itself.
fn project_taskbar_metrics(
    document: &mut impl UiDocumentAccess,
    vertical: bool,
) -> Result<(), String> {
    let factor = taskbar_scale::factor();
    let icon = scaled(TASK_ICON_LOGICAL_PX);
    let node = taskbar_scale::node_height(factor);
    // Children of a bordered capsule fill its content box, never its border box.
    let inner = taskbar_scale::inner_node_height(factor);
    let start = taskbar_scale::start_edge(factor);
    // Explicit sizes are border-box: the capsule is a node_height square and its button the inner one.
    document.size("start-capsule", node, node)?;
    document.size("task-start", start, start)?;
    document.size("task-start-icon", icon, icon)?;
    if vertical {
        // Vertical bar: the cross axis (width) is the bar thickness; the main axis stacks.
        document.size("taskbar-spacer", node, 0.0)?;
        document.size("clock-button", inner, scaled(CLOCK_WIDTH))?;
    } else {
        document.size("taskbar-spacer", 0.0, node)?;
        document.size("clock-button", scaled(CLOCK_WIDTH), inner)?;
    }
    document.font_size("clock-time", scaled(CLOCK_TIME_FONT))?;
    document.font_size("clock-date", scaled(CLOCK_DATE_FONT))?;
    document.font_size("tray-weather-temp", scaled(TRAY_WEATHER_FONT))?;
    for slot in 1..=TASK_SLOT_COUNT {
        let slot_size = if vertical {
            (inner, scaled(TASK_SLOT_WIDTH))
        } else {
            (scaled(TASK_SLOT_WIDTH), inner)
        };
        document.size(&format!("task-slot-{slot}"), slot_size.0, slot_size.1)?;
        document.size(&format!("task-slot-{slot}-icon"), icon, icon)?;
    }
    let glyph = scaled(TRAY_ICON_LOGICAL_PX);
    for id in TRAY_GLYPH_IDS {
        document.size(id, glyph, glyph)?;
    }
    let weather_glyph = scaled(TRAY_WEATHER_GLYPH_PX);
    for id in TRAY_WEATHER_GLYPH_IDS {
        document.size(id, weather_glyph, weather_glyph)?;
    }
    Ok(())
}

/// Raster edge for a task icon on `output` at the current taskbar factor. The displayed icon
/// node is sized by the same factor, so the request matches the drawn size.
#[must_use]
pub fn task_icon_device_px(output: Option<&flamewm_api::display::OutputSnapshot>) -> u32 {
    (task_icon_px(output) as f32 * taskbar_scale::factor()).round() as u32
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
    /// Brain button: `assistant.enabled` and `assistant.taskbarIcon` both on.
    pub assistant: bool,
    pub battery: bool,
    pub battery_only_when_relevant: bool,
    pub clock: bool,
    pub workspaces: bool,
    /// `taskbarShowDesktopButton`: the show-desktop strip at the bar end.
    pub desktop_button: bool,
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
            assistant: true,
            battery: true,
            battery_only_when_relevant: false,
            clock: true,
            workspaces: true,
            desktop_button: true,
            activities: true,
            separate: false,
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
    let capacity = task_slot_capacity_for_view(&view);
    let (rendered, overflow) = task_slot_split(states.len(), view.items.activities, capacity);
    let states = &states[..rendered];
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
        project_task_line(
            document,
            &line_id,
            states.get(index).is_some_and(|state| state.focused),
        )?;
        if let Some(state) = states.get(index) {
            let label = task_label(snapshot, state);
            let px = task_icon_device_px(view.output);
            let icon = resolve_icon(icon_resolver, &state.icon_name, IconSize::new(px, px))?;
            document.image_rgb8(&icon_id, icon)?;
            document.text_ref(&label_id, label)?;
            document.visible(&line_id, state.running)?;
            document.background_clear(&slot_id)?;
            if state.unresponsive {
                document.background(&line_id, TASK_NOT_RESPONDING)?;
            } else if state.attention {
                document.background(&line_id, TASK_ATTENTION)?;
            } else if state.focused {
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
    project_task_slots_extent(
        document,
        rendered + usize::from(overflow > 0),
        taskbar_is_vertical(&view),
    )?;
    project_task_overflow(document, overflow)?;
    document.visible("task-slot-insert", view.items.activities)?;

    project_pager(document, snapshot.workspaces.as_ref())?;
    document.visible(
        "workspace-pager",
        workspace_pager_visible(view.items, snapshot.workspaces.as_ref()),
    )?;

    project_clock(document)?;
    project_status_tray(document, &snapshot.system, view.items)?;
    project_element_backgrounds(
        document,
        view.items,
        rendered,
        tray_any_visible(&snapshot.system, view.items),
        view.items.clock,
    )
}

/// Display label of a task slot: the application name, falling back to its app id.
pub(crate) fn task_label<'a>(
    snapshot: &'a ShellSnapshot,
    state: &'a flamewm_shell_core::TaskVisualState,
) -> &'a str {
    snapshot
        .applications
        .iter()
        .find(|application| application.id == state.app_id)
        .map(|application| application.name.as_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(state.app_id.as_str())
}

/// Task cells a bar of `panel` length fits on an output at `scale_percent`. No geometry keeps the cap.
pub(crate) fn task_slot_capacity_for_panel(
    panel: Option<&flamewm_api::panels::PanelSnapshot>,
    scale_percent: u16,
) -> usize {
    let Some(panel) = panel else {
        return TASK_CELL_CAPACITY_MAX;
    };
    task_slot_capacity_for_length(
        panel.geometry.width.max(panel.geometry.height),
        scale_percent,
    )
}

/// Cells for a bar of `length_px` device px; the output's shell scale maps it to CSS px first.
pub(crate) fn task_slot_capacity_for_length(length_px: i32, scale_percent: u16) -> usize {
    let css = length_px as f32 * 100.0 / f32::from(scale_percent.max(1));
    let cells = (css - scaled(TASK_CHROME_RESERVED_PX)) / scaled(TASK_SLOT_WIDTH);
    (cells.max(0.0) as usize).min(TASK_CELL_CAPACITY_MAX)
}

/// Cell capacity for one projected panel view.
pub(crate) fn task_slot_capacity_for_view(view: &PanelView<'_>) -> usize {
    task_slot_capacity_for_panel(
        view.panel,
        view.output.map_or(100, |output| output.shell_scale_percent),
    )
}

/// Splits `total` window tasks into rendered slots and the overflow count. A full bar gives its
/// last cell to the `+N` overflow cell; `taskbarActivities` off hides the whole area.
pub(crate) fn task_slot_split(total: usize, activities: bool, capacity: usize) -> (usize, usize) {
    if !activities {
        return (0, 0);
    }
    let cells = if total <= capacity {
        TASK_SLOT_COUNT
    } else {
        capacity.saturating_sub(1)
    };
    let rendered = total.min(cells.min(TASK_SLOT_COUNT));
    (rendered, total - rendered)
}

/// Shows `#task-overflow` as `+N` when windows exceed the rendered buttons.
pub(crate) fn project_task_overflow(
    document: &mut impl UiDocumentAccess,
    overflow: usize,
) -> Result<(), String> {
    document.visible("task-overflow", overflow > 0)?;
    document.text_ref("task-overflow-label", &format!("+{overflow}"))
}

#[cfg(test)]
mod task_overflow_tests {
    use super::*;

    #[test]
    fn split_keeps_eight_slots_and_counts_the_rest() {
        assert_eq!(
            task_slot_split(TASK_SLOT_COUNT, true, TASK_CELL_CAPACITY_MAX),
            (8, 0)
        );
        assert_eq!(
            task_slot_split(TASK_SLOT_COUNT + 3, true, TASK_CELL_CAPACITY_MAX),
            (8, 3)
        );
        assert_eq!(
            task_slot_split(TASK_SLOT_COUNT + 3, false, TASK_CELL_CAPACITY_MAX),
            (0, 0)
        );
    }

    #[test]
    fn narrow_bar_gives_the_last_cell_to_the_overflow_count() {
        assert_eq!(task_slot_split(5, true, 3), (2, 3));
        assert_eq!(task_slot_split(2, true, 3), (2, 0));
        assert_eq!(task_slot_split(4, true, 0), (0, 4));
    }

    #[test]
    fn capacity_is_bar_length_minus_chrome_in_css_px() {
        let _guard = taskbar_scale::test_lock();
        taskbar_scale::set_factor(1.0);
        let chrome = TASK_CHROME_RESERVED_PX as i32;
        assert_eq!(task_slot_capacity_for_length(chrome + 42 * 3, 100), 3);
        assert_eq!(task_slot_capacity_for_length(chrome + 42 * 3 - 1, 100), 2);
        assert_eq!(
            task_slot_capacity_for_length(4000, 100),
            TASK_CELL_CAPACITY_MAX
        );
        // A 200% shell scale halves the CSS length of the same device bar.
        assert_eq!(task_slot_capacity_for_length(2 * (chrome + 42 * 3), 200), 3);
        assert_eq!(task_slot_capacity_for_length(0, 100), 0);
    }
}

/// `#task-slots` covers exactly the window buttons that render, so a separate-mode capsule hugs
/// them and the bar reflows; zero buttons leave no area behind for an empty capsule.
pub(crate) fn project_task_slots_extent(
    document: &mut impl UiDocumentAccess,
    rendered: usize,
    vertical: bool,
) -> Result<(), String> {
    let factor = taskbar_scale::factor();
    let run = taskbar_scale::task_slots_width(rendered, TASK_SLOT_WIDTH, factor);
    let cross = taskbar_scale::node_height(factor);
    if vertical {
        document.size("task-slots", cross, run)
    } else {
        document.size("task-slots", run, cross)
    }
}

/// Collapses a hidden tray button so the grouped tray capsule encloses only the icons that render.
pub(crate) fn project_tray_button(
    document: &mut impl UiDocumentAccess,
    id: &str,
    shown: bool,
) -> Result<(), String> {
    document.size(
        id,
        scaled(if shown { TRAY_BUTTON_WIDTH } else { 0.0 }),
        taskbar_scale::inner_node_height(taskbar_scale::factor()),
    )
}

/// Places the task status line from the same scaled slot metric that sizes the slot and the
/// icon: it is centered on `TASK_SLOT_WIDTH` and sits on the slot's content-box bottom. The
/// stylesheet keeps its authored 42 px offsets; this projection is the effective geometry for
/// every bar height, so both renderers draw the same line.
pub(crate) fn project_task_line(
    document: &mut impl UiDocumentAccess,
    id: &str,
    focused: bool,
) -> Result<(), String> {
    let width = scaled(TASK_LINE_WIDTH);
    let height = scaled(if focused {
        TASK_LINE_FOCUSED_HEIGHT
    } else {
        TASK_LINE_HEIGHT
    });
    document.position(
        id,
        (scaled(TASK_SLOT_WIDTH) - width) / 2.0,
        taskbar_scale::inner_node_height(taskbar_scale::factor()) - height,
    )?;
    document.size(id, width, height)
}

/// Projects a determinate background fill behind one task slot, clearing stale fills when
/// identity disappears or no provider reports progress. The fill covers the slot's full height
/// and grows left to right with the reported percent; CSS paints it behind the icon and label.
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
    document.position(&fill_id, 0.0, 0.0)?;
    document.size(
        &fill_id,
        percent.map_or(0.0, |percent| {
            scaled(TASK_SLOT_WIDTH) * f32::from(percent) / 100.0
        }),
        taskbar_scale::inner_node_height(taskbar_scale::factor()),
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
    let assistant = items.assistant;
    document.visible("tray-assistant", assistant)?;
    project_tray_button(document, "tray-assistant", assistant)?;
    document.visible("tray-volume", volume)?;
    project_tray_button(document, "tray-volume", volume)?;
    document.visible("tray-volume-icon", !status.audio.muted)?;
    document.visible("tray-volume-muted-icon", status.audio.muted)?;
    let network = status.network.visible && items.network;
    document.visible("tray-network", network)?;
    project_tray_button(document, "tray-network", network)?;
    project_battery_tray(document, &system.power, items)?;
    document.visible("clock-button", items.clock)?;
    document.visible("show-desktop", items.desktop_button)?;
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

/// Battery menu power-profile label: the active profile, or `--` when no service answered.
pub fn project_power_profile_row(
    document: &mut impl UiDocumentAccess,
    profiles: &flamewm_api::system::PowerProfilesSnapshot,
) -> Result<(), String> {
    let label = match (profiles.tile_visible(), profiles.active) {
        (true, Some(flamewm_api::system::PowerProfile::PowerSaver)) => "Power saver",
        (true, Some(flamewm_api::system::PowerProfile::Balanced)) => "Balanced",
        (true, Some(flamewm_api::system::PowerProfile::Performance)) => "Performance",
        _ => "--",
    };
    document.text("battery-profile", label.to_owned())
}

/// Click target of the battery power-profile row: the next offered profile after the active one
/// (wrapping). `None` when the service offers no profiles.
#[must_use]
pub fn power_profile_cycle_action(
    profiles: &flamewm_api::system::PowerProfilesSnapshot,
) -> Option<flamewm_api::system::SystemAction> {
    if !profiles.tile_visible() {
        return None;
    }
    let count = profiles.profiles.len();
    let next = match profiles
        .active
        .and_then(|active| profiles.profiles.iter().position(|p| *p == active))
    {
        Some(index) => profiles.profiles[(index + 1) % count],
        None => profiles.profiles[0],
    };
    Some(flamewm_api::system::SystemAction::SetPowerProfile(next))
}

/// Battery menu brightness label: the panel backlight percent, or `--` when no device answered.
pub fn project_brightness_row(
    document: &mut impl UiDocumentAccess,
    brightness: &flamewm_api::system::BrightnessSnapshot,
) -> Result<(), String> {
    let label = if brightness.availability == flamewm_api::system::ServiceAvailability::Available {
        format!("{}%", brightness.percent.min(100))
    } else {
        "--".to_owned()
    };
    document.text("battery-brightness-value", label)
}

/// Battery menu microphone row: default input level and the mute label. `--` and `Mute`
/// when no default input exists; the row's actions then resolve to no request.
pub fn project_input_row(
    document: &mut impl UiDocumentAccess,
    audio: &flamewm_api::system::AudioSnapshot,
) -> Result<(), String> {
    let row = crate::taskbar::status::audio::default_source_row(audio);
    let value = row
        .as_ref()
        .map_or_else(|| "--".to_owned(), |row| format!("{}%", row.volume_percent));
    let mute = if row.is_some_and(|row| row.muted) {
        "Unmute"
    } else {
        "Mute"
    };
    document.text("battery-input-value", value)?;
    document.text("battery-input-mute", mute.to_owned())
}

/// Click target of a battery brightness step: 10 percent down or up, clamped to `0..=100`.
/// `None` when no device is available or the value is already at the bound.
#[must_use]
pub fn brightness_step_action(
    brightness: &flamewm_api::system::BrightnessSnapshot,
    up: bool,
) -> Option<flamewm_api::system::SystemAction> {
    if brightness.availability != flamewm_api::system::ServiceAvailability::Available {
        return None;
    }
    let current = brightness.percent.min(100);
    let percent = if up {
        (current + 10).min(100)
    } else {
        current.saturating_sub(10)
    };
    (percent != current).then_some(flamewm_api::system::SystemAction::SetBrightness(
        flamewm_api::system::BrightnessAction {
            percent,
            generation: brightness.generation,
        },
    ))
}

#[cfg(test)]
mod record_tests;
#[cfg(test)]
mod template_tests;

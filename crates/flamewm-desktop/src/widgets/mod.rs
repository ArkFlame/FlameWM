use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use chrono::{Datelike, Local, Timelike};
use flamewm_api::settings::SettingsSnapshot;
use flamewm_api::system::{MediaSnapshot, PlaybackState, ServiceAvailability, SystemAction};
use flamewm_api::weather::WeatherFeedSnapshot;
use flamewm_api::{Point, Rect};
use flamewm_desktop_core::sticky::Rgb;
use flamewm_integrations_linux::MprisProvider;
use flamewm_reactor::{WakeDeadline, wall_clock_delay_ms};
use flamewm_ui_core::Color;
use flamewm_ui_core::style::UiLayer;
use flamewm_ui_x11::{
    PointerButton, RuntimeImage, UiActionEvent, UiActionPhase, UiColor, UiDocumentAccess,
};
use flamewm_widgets_core::widget_grid::{self, Edges, GridMetrics, Placement, Span};
use flamewm_widgets_core::widget_text::{
    CpuTimes, MAX_LINE_SPANS, MAX_TEMPLATE_CHARS, MAX_TEMPLATE_LINES, MemoryUsage, Placeholders,
    SpanStyle, WallTime, count_cpus, cpu_percent, display_text, estimate_text_width,
    expand_placeholders, fit_lines, format_date, format_gib, format_time, format_uptime,
    parse_cpu_times, parse_markup, parse_meminfo,
};
use flamewm_widgets_core::widgets::{
    MAX_WIDGETS, OptionControl, TextAlign, Widget, WidgetKind, WidgetStore, move_widget_rect,
    palette_color,
};

use crate::entry::projection;
use crate::entry::{
    DesktopState, MenuKind, MenuState, hide_menus, pointer_button, pointer_on_output, sync_menu,
};

pub(crate) const CONFIG_ROWS: usize = 18;
/// Index of the "Analog" entry in the clock `style` option.
const CLOCK_STYLE_ANALOG: i64 = 1;
const GALLERY_SIZE: (i32, i32) = (560, 590);
const CONFIG_WIDTH: i32 = 420;
const CONFIG_HEADER_HEIGHT: i32 = 62;
const CONFIG_ROW_HEIGHT: i32 = 32;
const DRAG_THRESHOLD_PX: f32 = 4.0;
/// Opacity of the dragged card while a move or resize is live.
const DRAG_OPACITY: f32 = 0.5;
const WIDGET_PADDING: f32 = 16.0;
const CARD_RING_SIZE: u32 = 60;

mod sync;

use sync::{
    sync_analog, sync_calendar, sync_clock, sync_custom, sync_gauge, sync_history, sync_media,
    sync_network, sync_weather,
};

mod pixels;
mod samples;
use pixels::*;
use samples::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TickRate {
    Second,
    Minute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GestureMode {
    Move,
    Resize(Edges),
}

/// Pixel rectangle after dragging only the `edges` of `original` by (`dx`, `dy`). Opposite
/// edges stay put; each side keeps at least one pixel and the result stays inside `bounds`.
fn edge_drag_rect(original: Rect, edges: Edges, dx: i32, dy: i32, bounds: Rect) -> Rect {
    let (mut left, mut top) = (original.x, original.y);
    let (mut right, mut bottom) = (original.right(), original.bottom());
    if edges.left {
        left = (left + dx).min(right - 1);
    }
    if edges.right {
        right = (right + dx).max(left + 1);
    }
    if edges.top {
        top = (top + dy).min(bottom - 1);
    }
    if edges.bottom {
        bottom = (bottom + dy).max(top + 1);
    }
    Rect::new(left, top, right - left, bottom - top).clamp_inside(bounds)
}

#[derive(Debug, Clone, Copy)]
struct Gesture {
    widget: u32,
    mode: GestureMode,
    start_x: f32,
    start_y: f32,
    original: Rect,
    moved: bool,
}

#[derive(Debug, Clone)]
struct TemplateEdit {
    widget: u32,
    buffer: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct RingKey {
    size: u32,
    thickness: u32,
    percent: u8,
    ring: Rgb,
    track: Rgb,
}

/// Inputs of the last history or network graph image written for a slot. An unchanged key
/// skips the per-tick pixel rebuild and the image upload that would redraw the document.
#[derive(Debug, Clone, PartialEq)]
struct GraphKey {
    prefix: &'static str,
    width: u32,
    height: u32,
    series: Vec<(Vec<u8>, Rgb)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct FaceKey {
    size: u32,
    second: u32,
    hands: Rgb,
    accent: Rgb,
}

/// Samples kept for the CPU and RAM graph; one per refresh tick.
const HISTORY_LEN: usize = 60;

/// Sampling cadence for CPU, memory, uptime and network widgets. The CPU and memory graphs
/// shift one point per sample, so this cadence is also the desktop's idle repaint rate.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(4);

/// Every input `sync_widgets` reads. An equal key means the document already shows this state,
/// so the sync skips all document writes.
#[derive(Debug, Clone, PartialEq)]
struct SyncKey {
    widgets: Vec<Widget>,
    settings: SettingsSnapshot,
    weather: WeatherFeedSnapshot,
    media: MediaSnapshot,
    editing: Option<(u32, String)>,
    placeholders: Placeholders,
    time: Option<WallTime>,
    samples: SampleKey,
}

#[derive(Debug, Clone, PartialEq)]
struct SampleKey {
    cpu_percent: Option<u8>,
    cpu_count: usize,
    /// Drawn memory text: percent, used GiB and total GiB, as shown on the widget.
    memory: Option<(Option<u8>, String, String)>,
    uptime_seconds: Option<u64>,
    cpu_history: VecDeque<u8>,
    memory_history: VecDeque<u8>,
    /// Network graph points as drawn (`scale_rates`), so idle traffic compares equal.
    net_rx_history: VecDeque<u8>,
    net_tx_history: VecDeque<u8>,
}

/// Slot-overlay capacity in cells: 32 columns by 18 rows. Must match `build.rs`.
const WIDGET_OVERLAY_COLS: usize = 32;
const WIDGET_OVERLAY_CELLS: usize = WIDGET_OVERLAY_COLS * 18;

pub(crate) struct WidgetState {
    pub(crate) store: WidgetStore,
    path: PathBuf,
    gesture: Option<Gesture>,
    /// True while the slot overlay is shown (from the first drag motion until release).
    slot_overlay: bool,
    editing: Option<TemplateEdit>,
    samples: Samples,
    media: MediaSnapshot,
    pub(crate) mpris: Option<MprisProvider>,
    media_dirty: bool,
    wake: WakeDeadline,
    due: Option<Instant>,
    /// Next sampler refresh; `None` when no widget reads CPU, memory, uptime or network.
    sample_due: Option<Instant>,
    /// Inputs of the last document sync; an equal key skips the sync.
    sync_key: Option<SyncKey>,
    /// True while the desktop window is fully obscured: sampling and document sync are suspended.
    obscured: bool,
    /// Latest drag pointer position, applied once per event flush.
    pending_pointer: Option<(f32, f32)>,
    rings: [Option<RingKey>; MAX_WIDGETS],
    graphs: [Option<GraphKey>; MAX_WIDGETS],
    faces: [Option<FaceKey>; MAX_WIDGETS],
    username: String,
    hostname: String,
}

impl WidgetState {
    pub(crate) fn new(store: WidgetStore, path: PathBuf) -> Self {
        Self {
            store,
            path,
            gesture: None,
            slot_overlay: false,
            editing: None,
            samples: Samples::default(),
            media: MediaSnapshot::default(),
            mpris: None,
            media_dirty: false,
            wake: WakeDeadline::default(),
            due: None,
            sample_due: None,
            sync_key: None,
            obscured: false,
            pending_pointer: None,
            rings: [None; MAX_WIDGETS],
            graphs: std::array::from_fn(|_| None),
            faces: [None; MAX_WIDGETS],
            username: std::env::var("USER")
                .ok()
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "friend".to_owned()),
            hostname: fs::read_to_string("/proc/sys/kernel/hostname")
                .map(|name| name.trim().to_owned())
                .unwrap_or_default(),
        }
    }

    pub(crate) fn attach_wake(&mut self, wake: WakeDeadline) {
        self.wake = wake;
        self.arm(Instant::now());
    }

    fn tick_rate(&self) -> Option<TickRate> {
        let widgets = self.store.widgets();
        if widgets.iter().any(|widget| {
            widget.kind == WidgetKind::Custom
                || (widget.kind == WidgetKind::Clock
                    && (widget.flag("showSeconds") || is_analog(widget)))
        }) {
            Some(TickRate::Second)
        } else if widgets
            .iter()
            .any(|widget| matches!(widget.kind, WidgetKind::Clock | WidgetKind::Calendar))
        {
            Some(TickRate::Minute)
        } else {
            None
        }
    }

    fn arm(&mut self, now: Instant) {
        let sampling = self.store.widgets().iter().any(|widget| {
            matches!(
                widget.kind,
                WidgetKind::Cpu
                    | WidgetKind::Memory
                    | WidgetKind::Ring
                    | WidgetKind::History
                    | WidgetKind::Network
                    | WidgetKind::Custom
            )
        });
        self.sample_due = sampling.then(|| self.sample_due.unwrap_or(now));
        self.due = self.tick_rate().map(|rate| {
            let epoch_ms = (flamewm_api::clock::unix_nanos() / 1_000_000) as u64;
            now + Duration::from_millis(
                wall_clock_delay_ms(epoch_ms, rate == TickRate::Second).max(1),
            )
        });
        self.wake
            .set([self.due, self.sample_due].into_iter().flatten().min());
    }

    fn sample(&mut self) {
        let widgets = self.store.widgets();
        let custom = widgets
            .iter()
            .any(|widget| widget.kind == WidgetKind::Custom);
        let network = widgets
            .iter()
            .any(|widget| widget.kind == WidgetKind::Network);
        let cpu = custom
            || widgets.iter().any(|widget| {
                matches!(
                    widget.kind,
                    WidgetKind::Cpu | WidgetKind::History | WidgetKind::Ring
                )
            });
        let memory = custom
            || widgets.iter().any(|widget| {
                matches!(
                    widget.kind,
                    WidgetKind::Memory | WidgetKind::History | WidgetKind::Ring
                )
            });
        self.samples.refresh(cpu, memory, network, custom);
        self.sample_due = Some(Instant::now() + SAMPLE_INTERVAL);
    }

    fn sample_key(&self) -> SampleKey {
        let samples = &self.samples;
        SampleKey {
            cpu_percent: samples.cpu_percent,
            cpu_count: samples.cpu_count,
            memory: samples.memory.map(|memory| {
                (
                    memory.percent(),
                    format_gib(memory.used_kib()),
                    format_gib(memory.total_kib),
                )
            }),
            uptime_seconds: samples.uptime_seconds,
            cpu_history: samples.cpu_history.clone(),
            memory_history: samples.memory_history.clone(),
            net_rx_history: scale_rates(&samples.net_rx_history),
            net_tx_history: scale_rates(&samples.net_tx_history),
        }
    }

    pub(crate) fn persist(&self) {
        if let Err(error) = flamewm_widgets_core::widgets::persist(&self.path, &self.store) {
            super::log_limit::emit(
                "widget state save failed",
                format_args!("desktop: widget state save failed: {error:?}"),
            );
        }
    }

    pub(crate) fn is_editing(&self) -> bool {
        self.editing.is_some()
    }

    fn has_media_widget(&self) -> bool {
        self.store
            .widgets()
            .iter()
            .any(|widget| widget.kind == WidgetKind::Media)
    }

    fn set_media(&mut self, snapshot: MediaSnapshot) {
        if self.media != snapshot {
            self.media = snapshot;
            self.media_dirty |= self.has_media_widget();
        }
    }
}

/// Corner radius shared by every widget card.
const WIDGET_RADIUS: f32 = 18.0;
/// Date line size of clock widgets.
const WIDGET_DATE_SIZE: f32 = 16.0;
/// Edge and corner resize bands of each widget, named by compass direction.
const WIDGET_EDGES: [&str; 8] = ["n", "s", "e", "w", "ne", "nw", "se", "sw"];

/// Values one widget card hands the document: fill, backdrop blur and corner radius.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct WidgetSurface {
    pub(crate) fill: UiColor,
    pub(crate) blur: Option<u8>,
    pub(crate) radius: f32,
}

/// Inheriting cards take the global surface fill and blur; others use their own options.
pub(crate) fn widget_surface(widget: &Widget, settings: &SettingsSnapshot) -> WidgetSurface {
    let radius = WIDGET_RADIUS;
    let blur = projection::surface_blur_percent(settings);
    let blur = (blur > 0).then_some(blur);
    if widget.inherits_surface() {
        let fill = flamewm_ui_core::surface::surface_fill(settings);
        WidgetSurface {
            fill: UiColor {
                r: (fill.rgb >> 16) as u8,
                g: (fill.rgb >> 8) as u8,
                b: fill.rgb as u8,
                a: fill.alpha,
            },
            blur,
            radius,
        }
    } else {
        WidgetSurface {
            fill: color(
                widget.color("background"),
                percent_to_alpha(widget.value("backgroundOpacity")),
            ),
            blur,
            radius,
        }
    }
}

/// Shows the drag ghost over `target`'s cell, accent-tinted when the cell is a valid
/// landing and error-tinted when nothing fits, or hides it.
fn sync_ghost(
    document: &mut impl UiDocumentAccess,
    metrics: GridMetrics,
    accent: Rgb,
    target: Option<(Placement, bool)>,
) -> Result<(), String> {
    match target {
        Some((cell, valid)) => {
            let rect = metrics.rect_of(cell);
            let tint = if valid { accent } else { danger_rgb() };
            document.position("widget-ghost", rect.x as f32, rect.y as f32)?;
            document.size("widget-ghost", rect.width as f32, rect.height as f32)?;
            document.border("widget-ghost", color(tint, 0xff))?;
            document.visible("widget-ghost", true)
        }
        None => document.visible("widget-ghost", false),
    }
}

fn danger_rgb() -> Rgb {
    let value = Color::DANGER.0;
    Rgb::new((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

/// Shows one outline per grid cell across the whole work area. Called once when a drag
/// starts; idle desktops never run it.
fn show_slot_overlay(
    document: &mut impl UiDocumentAccess,
    metrics: GridMetrics,
) -> Result<(), String> {
    for index in 0..WIDGET_OVERLAY_CELLS {
        let id = format!("widget-slot-{index}");
        let col = (index % WIDGET_OVERLAY_COLS) as i32;
        let row = (index / WIDGET_OVERLAY_COLS) as i32;
        if col < metrics.cols && row < metrics.rows {
            let rect = metrics.rect_of(Placement::left(col, row, Span::SINGLE));
            document.position(&id, rect.x as f32, rect.y as f32)?;
            document.size(&id, rect.width as f32, rect.height as f32)?;
            document.visible(&id, true)?;
        } else {
            document.visible(&id, false)?;
        }
    }
    Ok(())
}

/// Cell a live gesture would land on, with whether it is a valid landing. A cell that is
/// free (or holds a same-size swap partner) is valid. When the snapped cell collides and
/// a nearer free cell exists, the ghost moves there and is valid. When nothing fits, the
/// ghost stays on the snapped cell, clamped inside the grid, and is invalid.
fn gesture_ghost(
    state: &DesktopState,
    id: u32,
    mode: GestureMode,
    rect: Rect,
    metrics: GridMetrics,
) -> Option<(Placement, bool)> {
    let widget = state.widgets.store.get(id)?;
    let current = widget.grid?;
    match mode {
        GestureMode::Move => {
            let snapped = metrics.snap(rect.origin(), current.span);
            match state.widgets.store.preview_drop(id, rect.origin(), metrics) {
                Some(cell) => Some((cell, true)),
                None => Some((snapped, false)),
            }
        }
        GestureMode::Resize(edges) => {
            let want = widget_grid::edge_resize(widget.kind, current, edges, rect, metrics);
            match state.widgets.store.preview_placement(id, want, metrics) {
                Some(cell) => Some((cell, true)),
                None => Some((want, false)),
            }
        }
    }
}

fn hide_slot_overlay(document: &mut impl UiDocumentAccess) -> Result<(), String> {
    for index in 0..WIDGET_OVERLAY_CELLS {
        document.visible(&format!("widget-slot-{index}"), false)?;
    }
    Ok(())
}

/// Widget grid for the current work area and UI scale.
pub(crate) fn widget_metrics(state: &DesktopState) -> GridMetrics {
    GridMetrics::for_work_area(widget_bounds(state), i32::from(state.grid.scale_percent))
}

/// Work area the widget grid covers: the primary output (`state.grid` follows
/// `default_output`), never a union of every output.
pub(crate) fn widget_bounds(state: &DesktopState) -> Rect {
    state.grid.work_area
}

#[must_use]
/// Layer for a widget slot: lifted above sibling cards while a drag is live.
fn dragged_layer(released: bool) -> UiLayer {
    if released {
        UiLayer::Content
    } else {
        UiLayer::Floating
    }
}

pub(crate) fn assign_layers(document: &mut impl UiDocumentAccess) -> Result<(), String> {
    for slot in 1..=MAX_WIDGETS {
        document.layer(&format!("widget-{slot}"), UiLayer::Content)?;
    }
    // Drag ghost sits above every widget slot while a gesture is live.
    document.layer("widget-ghost", UiLayer::Overlay)?;
    for index in 0..WIDGET_OVERLAY_CELLS {
        document.layer(&format!("widget-slot-{index}"), UiLayer::Overlay)?;
    }
    document.layer("widget-menu", UiLayer::Popover)?;
    document.layer("widget-gallery", UiLayer::Popover)?;
    document.layer("widget-config", UiLayer::Popover)
}

pub(crate) fn hide_panels(document: &mut impl UiDocumentAccess) -> Result<(), String> {
    for id in ["widget-menu", "widget-gallery", "widget-config"] {
        document.visible(id, false)?;
        projection::clear_menu_backdrop(document, id)?;
    }
    Ok(())
}

fn set_alignment(
    document: &mut impl UiDocumentAccess,
    line: &str,
    align: TextAlign,
) -> Result<(), String> {
    document.visible(&format!("{line}-lead"), align != TextAlign::Left)?;
    document.visible(&format!("{line}-trail"), align != TextAlign::Right)
}

fn widget_placeholders(state: &DesktopState, time: WallTime) -> Placeholders {
    let widgets = &state.widgets;
    Placeholders {
        username: widgets.username.clone(),
        hostname: widgets.hostname.clone(),
        time: format_time(time, true, false),
        date: format_date(time, false, true),
        weekday: time.weekday_name().to_owned(),
        cpu_percent: widgets.samples.cpu_percent,
        memory_percent: widgets.samples.memory.and_then(MemoryUsage::percent),
        uptime: widgets
            .samples
            .uptime_seconds
            .map(format_uptime)
            .unwrap_or_default(),
    }
}

pub(crate) fn sync_widgets(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    // Covered: skip every write and never store the key, so the resume sync recomputes it.
    if state.widgets.obscured {
        return Ok(());
    }
    let time = local_wall_time();
    let placeholders = widget_placeholders(state, time);
    let widgets = state.widgets.store.widgets();
    // Wall time only reaches the document through time-showing widgets; without one, a
    // second-by-second clock must not force a sync.
    let needs_time = widgets.iter().any(|widget| {
        matches!(
            widget.kind,
            WidgetKind::Clock | WidgetKind::Calendar | WidgetKind::Custom
        )
    });
    let mut key_placeholders = placeholders.clone();
    if !needs_time {
        key_placeholders.time.clear();
        key_placeholders.date.clear();
        key_placeholders.weekday.clear();
    }
    let key = SyncKey {
        widgets: widgets.to_vec(),
        settings: state.settings.clone(),
        weather: state.weather.clone(),
        media: state.widgets.media.clone(),
        editing: state
            .widgets
            .editing
            .as_ref()
            .map(|edit| (edit.widget, edit.buffer.clone())),
        placeholders: key_placeholders,
        time: needs_time.then_some(time),
        samples: state.widgets.sample_key(),
    };
    if state.widgets.sync_key.as_ref() == Some(&key) {
        return Ok(());
    }
    for slot in 0..MAX_WIDGETS {
        let id = format!("widget-{}", slot + 1);
        let Some(widget) = state.widgets.store.widgets().get(slot).cloned() else {
            document.visible(&id, false)?;
            state.widgets.rings[slot] = None;
            state.widgets.graphs[slot] = None;
            state.widgets.faces[slot] = None;
            continue;
        };
        document.position(&id, widget.rect.x as f32, widget.rect.y as f32)?;
        document.size(&id, widget.rect.width as f32, widget.rect.height as f32)?;
        let surface = widget_surface(&widget, &state.settings);
        document.background(&id, surface.fill)?;
        document.corner_radius(&id, surface.radius)?;
        document.node_backdrop(&id, surface.blur)?;
        for edge in WIDGET_EDGES {
            document.visible(&format!("{id}-edge-{edge}"), !widget.locked)?;
        }
        let analog = is_analog(&widget);
        document.visible(
            &format!("{id}-clock"),
            widget.kind == WidgetKind::Clock && !analog,
        )?;
        document.visible(&format!("{id}-analog"), analog)?;
        document.visible(
            &format!("{id}-gauge"),
            matches!(
                widget.kind,
                WidgetKind::Cpu | WidgetKind::Memory | WidgetKind::Ring
            ),
        )?;
        document.visible(&format!("{id}-history"), widget.kind == WidgetKind::History)?;
        document.visible(&format!("{id}-network"), widget.kind == WidgetKind::Network)?;
        document.visible(&format!("{id}-media"), widget.kind == WidgetKind::Media)?;
        document.visible(&format!("{id}-weather"), widget.kind == WidgetKind::Weather)?;
        document.visible(
            &format!("{id}-calendar"),
            widget.kind == WidgetKind::Calendar,
        )?;
        let editing = state
            .widgets
            .editing
            .as_ref()
            .filter(|edit| edit.widget == widget.id)
            .map(|edit| edit.buffer.clone());
        document.visible(
            &format!("{id}-custom"),
            widget.kind == WidgetKind::Custom && editing.is_none(),
        )?;
        document.visible(&format!("{id}-editor"), editing.is_some())?;
        let accent = projection_accent(state);
        match widget.kind {
            WidgetKind::Clock if analog => sync_analog(document, state, slot, &widget, time)?,
            WidgetKind::Clock => sync_clock(document, &id, &widget, time, accent)?,
            WidgetKind::Cpu | WidgetKind::Memory | WidgetKind::Ring => {
                sync_gauge(document, state, slot, &widget)?;
            }
            WidgetKind::Media => sync_media(document, &id, &widget, &state.widgets.media, accent)?,
            WidgetKind::Weather => sync_weather(document, &id, &widget, &state.weather, accent)?,
            WidgetKind::Calendar => sync_calendar(document, &id, &widget, time, accent)?,
            WidgetKind::History => sync_history(
                document,
                &mut state.widgets.graphs[slot],
                &state.widgets.samples,
                accent,
                slot,
                &widget,
            )?,
            WidgetKind::Network => sync_network(
                document,
                &mut state.widgets.graphs[slot],
                &state.widgets.samples,
                accent,
                slot,
                &widget,
            )?,
            WidgetKind::Custom => {
                if let Some(buffer) = editing {
                    document.text(
                        &format!("{id}-editor"),
                        projection::caret_text(&buffer, true),
                    )?;
                } else {
                    sync_custom(document, &id, &widget, &placeholders)?;
                }
            }
        }
        document.visible(&id, true)?;
    }
    state.widgets.sync_key = Some(key);
    Ok(())
}

/// What a change of the window's fully-obscured state asks the widget loop to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisibilityEffect {
    Suspend,
    Resume,
}

/// Gate for `set_obscured`: `None` when the state did not change.
fn visibility_effect(was_obscured: bool, obscured: bool) -> Option<VisibilityEffect> {
    match (was_obscured, obscured) {
        (false, true) => Some(VisibilityEffect::Suspend),
        (true, false) => Some(VisibilityEffect::Resume),
        _ => None,
    }
}

/// Applies a `VisibilityNotify` verdict. While obscured, no sampling and no document sync run and
/// the wake deadline is dropped, so the reactor does not spin on a past deadline. On uncover the
/// cached sync key is discarded: writes made while covered bypass the key, so the resume sync
/// must compare the document again. It then samples and syncs immediately.
pub(crate) fn set_obscured(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    obscured: bool,
) -> Result<(), String> {
    match visibility_effect(state.widgets.obscured, obscured) {
        None => Ok(()),
        Some(VisibilityEffect::Suspend) => {
            state.widgets.obscured = true;
            state.widgets.wake.set(None);
            Ok(())
        }
        Some(VisibilityEffect::Resume) => {
            state.widgets.obscured = false;
            state.widgets.sync_key = None;
            state.widgets.sample();
            state.widgets.arm(Instant::now());
            sync_widgets(document, state)
        }
    }
}

pub(crate) fn poll(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    if state.widgets.obscured {
        state.widgets.wake.set(None);
        return Ok(());
    }
    let now = Instant::now();
    let sampling = state.widgets.sample_due.is_some_and(|due| now >= due);
    let ticking = state.widgets.due.is_some_and(|due| now >= due);
    let media = std::mem::take(&mut state.widgets.media_dirty);
    if !sampling && !ticking && !media {
        return Ok(());
    }
    if sampling {
        state.widgets.sample();
    }
    if sampling || ticking {
        state.widgets.arm(now);
    }
    sync_widgets(document, state)
}

pub(crate) fn refresh_after_change(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    state.widgets.sample();
    state.widgets.arm(Instant::now());
    state.widgets.persist();
    sync_widgets(document, state)
}

fn slot_number(action: &str) -> Option<usize> {
    action
        .strip_prefix("widget.")?
        .split('.')
        .next()?
        .parse::<usize>()
        .ok()
        .filter(|slot| (1..=MAX_WIDGETS).contains(slot))
}

fn slot_widget(state: &DesktopState, action: &str) -> Option<Widget> {
    let slot = slot_number(action)?;
    state.widgets.store.widgets().get(slot - 1).cloned()
}

fn selected_widget(state: &DesktopState) -> Option<u32> {
    state
        .menu
        .as_ref()
        .filter(|menu| matches!(menu.kind, MenuKind::Widget | MenuKind::WidgetConfig))
        .and_then(|menu| menu.selected_id.as_deref())
        .and_then(|id| id.parse().ok())
}

fn open_panel(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    kind: MenuKind,
    anchor: Point,
    widget: Option<u32>,
) -> Result<(), String> {
    let (output, anchor) = pointer_on_output(state, anchor.x as f32, anchor.y as f32);
    state.menu = Some(MenuState {
        kind,
        anchor,
        output,
        selected_id: widget.map(|id| id.to_string()),
    });
    state.pending_delete = None;
    sync_menu(document, state)
}

pub(crate) fn open_gallery(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    anchor: Point,
) -> Result<(), String> {
    commit_edit(state);
    open_panel(document, state, MenuKind::WidgetGallery, anchor, None)
}

fn panel_rect(work_area: Rect, size: (i32, i32)) -> Rect {
    let width = size.0.min(work_area.width);
    let height = size.1.min(work_area.height);
    Rect::new(
        work_area.x + (work_area.width - width) / 2,
        work_area.y + (work_area.height - height) / 2,
        width,
        height,
    )
}

pub(crate) fn place_gallery(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    work_area: Rect,
) -> Result<(), String> {
    let placed = panel_rect(work_area, GALLERY_SIZE);
    let count = state.widgets.store.widgets().len();
    document.text(
        "widget-gallery-count",
        if count >= MAX_WIDGETS {
            format!("All {MAX_WIDGETS} widget slots are in use; remove one to add another")
        } else {
            format!("{count} of {MAX_WIDGETS} placed. Right-click a widget to configure it")
        },
    )?;
    let time = local_wall_time();
    document.text("widget-card-clock-time", format_time(time, true, false))?;
    document.text("widget-card-clock-date", format_date(time, true, false))?;
    let accent = projection_accent(state);
    for (kind, percent) in [
        (
            WidgetKind::Cpu,
            state.widgets.samples.cpu_percent.unwrap_or(42),
        ),
        (
            WidgetKind::Memory,
            state
                .widgets
                .samples
                .memory
                .and_then(MemoryUsage::percent)
                .unwrap_or(63),
        ),
    ] {
        let preview = Widget::new(0, kind, Point::new(0, 0));
        document.image_rgba8(
            &format!("widget-card-{}-image", kind.id()),
            RuntimeImage {
                source: format!("widget-card-{}-{percent}", kind.id()),
                width: CARD_RING_SIZE,
                height: CARD_RING_SIZE,
                pixels: ring_pixels(
                    CARD_RING_SIZE,
                    8.0,
                    percent,
                    if kind == WidgetKind::Cpu {
                        accent
                    } else {
                        preview.color_or_accent("ringColor", accent)
                    },
                    preview.color("trackColor"),
                ),
            },
        )?;
    }
    let full = state.widgets.store.is_full();
    for kind in WidgetKind::ALL {
        document.visible(&format!("widget-card-{}-add", kind.id()), !full)?;
    }
    document.position("widget-gallery", placed.x as f32, placed.y as f32)?;
    document.size("widget-gallery", placed.width as f32, placed.height as f32)?;
    projection::project_menu_surface(document, "widget-gallery", &state.settings)?;
    state.menu_rects.push(placed);
    document.visible("widget-gallery", true)
}

fn projection_accent(state: &DesktopState) -> Rgb {
    let accent = crate::entry::accent_color(&state.settings);
    Rgb::new(accent.r, accent.g, accent.b)
}

pub(crate) fn place_config(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    work_area: Rect,
) -> Result<(), String> {
    let Some(widget) = selected_widget(state).and_then(|id| state.widgets.store.get(id).cloned())
    else {
        state.menu = None;
        return Ok(());
    };
    let options = widget.kind.visible_options(widget.stored_values());
    for row in 0..CONFIG_ROWS {
        let row_id = format!("widget-config-row-{row}");
        let Some(spec) = options.get(row) else {
            document.visible(&row_id, false)?;
            continue;
        };
        let value = widget.value(spec.key);
        document.text_ref(&format!("{row_id}-label"), spec.label)?;
        document.text(&format!("{row_id}-value"), spec.label_for(value))?;
        let is_color = matches!(
            spec.control,
            OptionControl::Color | OptionControl::AccentColor
        );
        document.visible(&format!("{row_id}-swatch"), is_color)?;
        if is_color {
            let swatch = if value == flamewm_widgets_core::widgets::ACCENT_COLOR {
                projection_accent(state)
            } else {
                palette_color(value)
            };
            document.background(&format!("{row_id}-swatch"), color(swatch, 255))?;
        }
        document.visible(&row_id, true)?;
    }
    document.text(
        "widget-config-title",
        format!("{} settings", widget.kind.title()),
    )?;
    let rows = options.len().min(CONFIG_ROWS) as i32;
    let size = (
        CONFIG_WIDTH,
        CONFIG_HEADER_HEIGHT + rows * CONFIG_ROW_HEIGHT + 16,
    );
    let beside = if widget.rect.right() + 12 + size.0 <= work_area.right() {
        widget.rect.right() + 12
    } else {
        widget.rect.x - 12 - size.0
    };
    let placed = Rect::new(beside, widget.rect.y, size.0, size.1).clamp_inside(work_area);
    document.position("widget-config", placed.x as f32, placed.y as f32)?;
    document.size("widget-config", placed.width as f32, placed.height as f32)?;
    projection::project_menu_surface(document, "widget-config", &state.settings)?;
    state.menu_rects.push(placed);
    document.visible("widget-config", true)
}

pub(crate) fn widget_menu_rows(
    document: &mut impl UiDocumentAccess,
    state: &DesktopState,
) -> Result<usize, String> {
    let Some(widget) = selected_widget(state).and_then(|id| state.widgets.store.get(id)) else {
        return Ok(0);
    };
    let custom = widget.kind == WidgetKind::Custom;
    document.visible("widget-row-edit", custom)?;
    document.text(
        "widget-row-lock-label",
        if widget.locked {
            "Unlock Position"
        } else {
            "Lock Position"
        }
        .to_owned(),
    )?;
    Ok(if custom { 5 } else { 4 })
}

pub(crate) fn is_widget_action(action: &str) -> bool {
    action.starts_with("widget.")
}

pub(crate) fn handle_action(
    action: &UiActionEvent,
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    let name = action.action.as_str();
    if name.ends_with(".surface") {
        return Ok(());
    }
    let primary_release = action.phase == UiActionPhase::Release
        && action.inside
        && pointer_button(action) == PointerButton::Primary;
    if let Some(rest) = name.strip_prefix("widget.menu.") {
        if primary_release {
            return handle_menu(rest, action, document, state);
        }
        return Ok(());
    }
    if let Some(rest) = name.strip_prefix("widget.gallery.") {
        if primary_release {
            return handle_gallery(rest, document, state);
        }
        return Ok(());
    }
    if let Some(rest) = name.strip_prefix("widget.config.") {
        if primary_release {
            return handle_config(rest, document, state);
        }
        return Ok(());
    }
    let Some(widget) = slot_widget(state, name) else {
        return Ok(());
    };
    if action.phase == UiActionPhase::Release && pointer_button(action) == PointerButton::Secondary
    {
        commit_edit(state);
        state.widgets.gesture = None;
        return open_panel(
            document,
            state,
            MenuKind::Widget,
            Point::new(action.x.round() as i32, action.y.round() as i32),
            Some(widget.id),
        );
    }
    if let Some(control) = name.split_once(".media.").map(|(_, control)| control) {
        if primary_release {
            media_action(control, state);
        }
        return Ok(());
    }
    if pointer_button(action) != PointerButton::Primary {
        return Ok(());
    }
    let mode = match name
        .rsplit_once(".resize-")
        .and_then(|(_, edge)| Edges::from_name(edge))
    {
        Some(edges) => GestureMode::Resize(edges),
        None => GestureMode::Move,
    };
    match action.phase {
        UiActionPhase::Press => {
            state.widgets.pending_pointer = None;
            if state.menu.is_some() {
                state.menu = None;
                hide_menus(document, &mut state.menu_rects)?;
            }
            if state
                .widgets
                .editing
                .as_ref()
                .is_some_and(|edit| edit.widget != widget.id)
            {
                commit_edit(state);
                sync_widgets(document, state)?;
            }
            if widget.locked || state.widgets.is_editing() {
                return Ok(());
            }
            state.widgets.gesture = Some(Gesture {
                widget: widget.id,
                mode,
                start_x: action.x,
                start_y: action.y,
                original: widget.rect,
                moved: false,
            });
        }
        UiActionPhase::Motion => {
            // Coalesced: only the latest pointer position is applied, once per flush.
            if state
                .widgets
                .gesture
                .is_some_and(|g| g.widget == widget.id && g.mode == mode)
            {
                state.widgets.pending_pointer = Some((action.x, action.y));
            }
        }
        UiActionPhase::Release => {
            state.widgets.pending_pointer = None;
            if let Some(gesture) = state
                .widgets
                .gesture
                .filter(|g| g.widget == widget.id && g.mode == mode)
            {
                drag_to(document, state, gesture, (action.x, action.y), true)?;
            }
        }
        UiActionPhase::Hover => {}
    }
    Ok(())
}

/// Applies the latest coalesced drag motion. Called once per event flush from `main.rs`.
pub(crate) fn apply_pending_drag(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    let Some(pointer) = state.widgets.pending_pointer.take() else {
        return Ok(());
    };
    let Some(gesture) = state.widgets.gesture else {
        return Ok(());
    };
    drag_to(document, state, gesture, pointer, false)
}

/// Moves a live gesture to `pointer`. A motion previews only: ghost outline, slot outlines
/// and the fade/lift of the dragged card. Release commits the snapped rect and clears it.
fn drag_to(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    mut gesture: Gesture,
    pointer: (f32, f32),
    release: bool,
) -> Result<(), String> {
    let id = gesture.widget;
    let dx = pointer.0 - gesture.start_x;
    let dy = pointer.1 - gesture.start_y;
    let was_moved = gesture.moved;
    gesture.moved |= dx * dx + dy * dy >= DRAG_THRESHOLD_PX * DRAG_THRESHOLD_PX;
    state.widgets.gesture = (!release).then_some(gesture);
    if !gesture.moved {
        return Ok(());
    }
    let bounds = widget_bounds(state);
    let rect = match gesture.mode {
        GestureMode::Move => move_widget_rect(gesture.original, dx as i32, dy as i32, bounds),
        GestureMode::Resize(edges) => {
            edge_drag_rect(gesture.original, edges, dx as i32, dy as i32, bounds)
        }
    };
    // The widget stays put during the drag; only the preview zone moves. Release snaps the
    // widget to the grid.
    if release {
        state.widgets.store.set_rect(id, rect);
        let metrics = widget_metrics(state);
        match gesture.mode {
            GestureMode::Move => {
                state.widgets.store.drop_on_grid(id, rect.origin(), metrics);
            }
            GestureMode::Resize(edges) => {
                if let Some((kind, current)) = state
                    .widgets
                    .store
                    .get(id)
                    .and_then(|w| w.grid.map(|g| (w.kind, g)))
                {
                    let want = widget_grid::edge_resize(kind, current, edges, rect, metrics);
                    state.widgets.store.place_nearest(id, want, metrics);
                }
            }
        }
        state.widgets.persist();
    }
    state.widgets.store.anchor_to(id, bounds);
    // Slot outlines exist only while a drag or resize is live.
    if release {
        if state.widgets.slot_overlay {
            hide_slot_overlay(document)?;
            state.widgets.slot_overlay = false;
        }
    } else if !state.widgets.slot_overlay {
        show_slot_overlay(document, widget_metrics(state))?;
        state.widgets.slot_overlay = true;
    }
    // Ghost: the cell this gesture would land on, hidden once released.
    let metrics = widget_metrics(state);
    let ghost = if release {
        None
    } else {
        gesture_ghost(state, id, gesture.mode, rect, metrics)
    };
    let accent = projection_accent(state);
    sync_ghost(document, metrics, accent, ghost)?;
    // The dragged card fades and lifts once per gesture and restores on release.
    if let Some(slot) = state
        .widgets
        .store
        .widgets()
        .iter()
        .position(|w| w.id == id)
        && (!was_moved || release)
    {
        let node = format!("widget-{}", slot + 1);
        document.opacity(&node, if release { 1.0 } else { DRAG_OPACITY })?;
        document.layer(&node, dragged_layer(release))?;
    }
    // Slot content is unchanged until release, so motion does not rebuild it.
    if release {
        sync_widgets(document, state)?;
    }
    Ok(())
}

fn handle_menu(
    rest: &str,
    action: &UiActionEvent,
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    let Some(id) = selected_widget(state) else {
        state.menu = None;
        return hide_menus(document, &mut state.menu_rects);
    };
    match rest {
        "configure" => {
            let anchor = state
                .widgets
                .store
                .get(id)
                .map_or(Point::new(0, 0), |widget| widget.rect.origin());
            return open_panel(document, state, MenuKind::WidgetConfig, anchor, Some(id));
        }
        "edit" => {
            if let Some(widget) = state.widgets.store.get(id) {
                state.widgets.editing = Some(TemplateEdit {
                    widget: id,
                    buffer: widget.template.clone(),
                });
            }
        }
        "lock" => {
            state.widgets.store.toggle_lock(id);
        }
        "gallery" => {
            state.menu = None;
            hide_menus(document, &mut state.menu_rects)?;
            return open_gallery(
                document,
                state,
                Point::new(action.x.round() as i32, action.y.round() as i32),
            );
        }
        "remove" => {
            if state
                .widgets
                .editing
                .as_ref()
                .is_some_and(|edit| edit.widget == id)
            {
                state.widgets.editing = None;
            }
            state.widgets.store.remove(id);
        }
        _ => return Ok(()),
    }
    state.menu = None;
    hide_menus(document, &mut state.menu_rects)?;
    refresh_after_change(document, state)
}

fn handle_gallery(
    rest: &str,
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    if rest == "close" {
        state.menu = None;
        return hide_menus(document, &mut state.menu_rects);
    }
    let Some(kind) = rest.strip_prefix("add.").and_then(WidgetKind::from_id) else {
        return Ok(());
    };
    let (bounds, scale) =
        state
            .menu
            .as_ref()
            .map_or((state.grid.work_area, state.grid.scale_percent), |menu| {
                let grid = crate::entry::grid_for_output(state, &menu.output);
                (grid.work_area, grid.scale_percent)
            });
    let metrics = GridMetrics::for_work_area(bounds, i32::from(scale));
    if state.widgets.store.add_on_grid(kind, metrics).is_none() {
        return Ok(());
    }
    refresh_after_change(document, state)?;
    if kind == WidgetKind::Media {
        refresh_media(state);
        sync_widgets(document, state)?;
    }
    sync_menu(document, state)
}

fn handle_config(
    rest: &str,
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    if rest == "close" {
        state.menu = None;
        return hide_menus(document, &mut state.menu_rects);
    }
    let Some((row, direction)) = rest.split_once('.') else {
        return Ok(());
    };
    let direction = match direction {
        "up" => 1,
        "down" => -1,
        _ => return Ok(()),
    };
    let Some(row) = row.parse::<usize>().ok() else {
        return Ok(());
    };
    let Some(id) = selected_widget(state) else {
        return Ok(());
    };
    let Some(spec) = state.widgets.store.get(id).and_then(|widget| {
        widget
            .kind
            .visible_options(widget.stored_values())
            .get(row)
            .copied()
    }) else {
        return Ok(());
    };
    let Some(widget) = state.widgets.store.get_mut(id) else {
        return Ok(());
    };
    widget.step_value(spec.key, direction);
    let slot = state
        .widgets
        .store
        .widgets()
        .iter()
        .position(|widget| widget.id == id);
    if let Some(slot) = slot {
        state.widgets.rings[slot] = None;
    }
    refresh_after_change(document, state)?;
    sync_menu(document, state)
}

pub(crate) fn commit_edit(state: &mut DesktopState) {
    let Some(edit) = state.widgets.editing.take() else {
        return;
    };
    if let Some(widget) = state.widgets.store.get_mut(edit.widget) {
        widget.template = edit.buffer;
        state.widgets.persist();
    }
}

pub(crate) fn cancel_transient(state: &mut DesktopState) {
    commit_edit(state);
    state.widgets.gesture = None;
}

pub(crate) fn handle_text_input(
    action: &UiActionEvent,
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    let Some(text) = action.text.as_deref() else {
        return Ok(());
    };
    let Some(edit) = state.widgets.editing.as_mut() else {
        return Ok(());
    };
    if text.is_empty() {
        commit_edit(state);
        return refresh_after_change(document, state);
    }
    if text == "\u{8}" || text == "\u{7f}" {
        edit.buffer.pop();
    } else {
        for ch in text
            .chars()
            .map(|ch| if ch == '\r' { '\n' } else { ch })
            .filter(|ch| *ch == '\n' || !ch.is_control())
        {
            if edit.buffer.chars().count() >= MAX_TEMPLATE_CHARS {
                break;
            }
            edit.buffer.push(ch);
        }
    }
    sync_widgets(document, state)
}

fn media_action(control: &str, state: &mut DesktopState) {
    let snapshot = state.widgets.media.clone();
    if snapshot.active_bus_name.is_empty() {
        return;
    }
    let bus_name = snapshot.active_bus_name.clone();
    let action = match control {
        "prev" => SystemAction::Previous { bus_name },
        "next" => SystemAction::Next { bus_name },
        "play" => SystemAction::PlayPause { bus_name },
        _ => return,
    };
    let Some(provider) = state.widgets.mpris.as_mut() else {
        return;
    };
    if let Err(error) = provider.perform_action(action, &snapshot) {
        super::log_limit::emit(
            "media widget action failed",
            format_args!("desktop: media widget action failed: {}", error.message),
        );
        return;
    }
    if let Err(error) = provider.on_ready() {
        super::log_limit::emit(
            "media widget flush failed",
            format_args!("desktop: media widget flush failed: {}", error.message),
        );
    }
}

pub(crate) fn refresh_media(state: &mut DesktopState) {
    let Some(provider) = state.widgets.mpris.as_mut() else {
        return;
    };
    if let Err(error) = provider.on_ready() {
        super::log_limit::emit(
            "media widget update failed",
            format_args!("desktop: media widget update failed: {}", error.message),
        );
    }
    provider.reconcile();
    let snapshot = provider.snapshot();
    state.widgets.set_media(snapshot);
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod visibility_gate_tests {
    use super::{VisibilityEffect, visibility_effect};

    #[test]
    fn gate_suspends_only_on_obscure_and_resumes_only_on_uncover() {
        assert_eq!(
            visibility_effect(false, true),
            Some(VisibilityEffect::Suspend)
        );
        assert_eq!(
            visibility_effect(true, false),
            Some(VisibilityEffect::Resume)
        );
        assert_eq!(visibility_effect(false, false), None);
        assert_eq!(visibility_effect(true, true), None);
    }
}

use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::{Datelike, Local, Timelike};
use flamewm_api::system::{MediaSnapshot, PlaybackState, ServiceAvailability, SystemAction};
use flamewm_api::settings::SettingsSnapshot;
use flamewm_api::weather::WeatherFeedSnapshot;
use flamewm_api::{Point, Rect};
use flamewm_desktop_core::sticky::Rgb;
use flamewm_desktop_core::widget_text::{
    CpuTimes, MAX_LINE_SPANS, MAX_TEMPLATE_CHARS, MAX_TEMPLATE_LINES, MemoryUsage, Placeholders,
    SpanStyle, WallTime, count_cpus, cpu_percent, display_text, estimate_text_width,
    expand_placeholders, fit_lines, format_date, format_gib, format_time, format_uptime,
    parse_cpu_times, parse_markup, parse_meminfo,
};
use flamewm_desktop_core::widget_grid::{self, Edges, GridMetrics, Placement, Span};
use flamewm_desktop_core::widgets::{
    MAX_WIDGETS, OptionControl, TextAlign, Widget, WidgetKind, WidgetStore, move_widget_rect,
    palette_color,
};
use flamewm_integrations_linux::MprisProvider;
use flamewm_reactor::{WakeDeadline, wall_clock_delay_ms};
use flamewm_ui_core::Color;
use flamewm_ui_core::style::UiLayer;
use flamewm_ui_x11::{
    PointerButton, RuntimeImage, UiActionEvent, UiActionPhase, UiColor, UiDocumentAccess,
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

/// Sampling cadence for CPU, memory, uptime and network widgets.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(2);

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
    memory: Option<MemoryUsage>,
    uptime_seconds: Option<u64>,
    cpu_history: VecDeque<u8>,
    memory_history: VecDeque<u8>,
    net_rx_history: VecDeque<u64>,
    net_tx_history: VecDeque<u64>,
}

#[derive(Debug, Default)]
struct Samples {
    cpu_previous: Option<CpuTimes>,
    cpu_percent: Option<u8>,
    cpu_count: usize,
    cpu_history: VecDeque<u8>,
    memory: Option<MemoryUsage>,
    memory_history: VecDeque<u8>,
    uptime_seconds: Option<u64>,
    net_previous: Option<(u64, u64, Instant)>,
    net_rx_history: VecDeque<u64>,
    net_tx_history: VecDeque<u64>,
}

/// Sums non-loopback received and transmitted bytes from `/proc/net/dev`.
fn parse_net_dev(text: &str) -> Option<(u64, u64)> {
    let mut totals = None;
    for line in text.lines().skip(2) {
        let Some((name, fields)) = line.split_once(':') else {
            continue;
        };
        if name.trim() == "lo" {
            continue;
        }
        let fields: Vec<u64> = fields
            .split_whitespace()
            .filter_map(|field| field.parse().ok())
            .collect();
        if let (Some(rx), Some(tx)) = (fields.first(), fields.get(8)) {
            let (sum_rx, sum_tx) = totals.unwrap_or((0u64, 0u64));
            totals = Some((sum_rx + rx, sum_tx + tx));
        }
    }
    totals
}

fn push_rate(history: &mut VecDeque<u64>, value: u64) {
    if history.len() == HISTORY_LEN {
        history.pop_front();
    }
    history.push_back(value);
}

/// Scales rates to percentages of the largest sample in view, so the graph autoscales.
fn scale_rates(history: &VecDeque<u64>) -> VecDeque<u8> {
    let peak = history.iter().copied().max().unwrap_or(0).max(1);
    history
        .iter()
        .map(|rate| (rate * 100 / peak) as u8)
        .collect()
}

fn push_history(history: &mut VecDeque<u8>, value: Option<u8>) {
    if let Some(value) = value {
        if history.len() == HISTORY_LEN {
            history.pop_front();
        }
        history.push_back(value);
    }
}

impl Samples {
    /// Pushes download and upload byte rates since the previous refresh.
    fn refresh_network(&mut self, now: Instant) {
        let Some(totals) = fs::read_to_string("/proc/net/dev")
            .ok()
            .and_then(|text| parse_net_dev(&text))
        else {
            return;
        };
        if let Some((rx, tx, at)) = self.net_previous {
            let seconds = now.duration_since(at).as_secs_f64().max(0.001);
            let rate = |now: u64, before: u64| {
                (now.saturating_sub(before) as f64 / seconds).round() as u64
            };
            push_rate(&mut self.net_rx_history, rate(totals.0, rx));
            push_rate(&mut self.net_tx_history, rate(totals.1, tx));
        }
        self.net_previous = Some((totals.0, totals.1, now));
    }

    fn refresh(&mut self, cpu: bool, memory: bool, network: bool, uptime: bool) {
        if cpu && let Ok(stat) = fs::read_to_string("/proc/stat") {
            let current = parse_cpu_times(&stat);
            if let (Some(previous), Some(current)) = (self.cpu_previous, current) {
                self.cpu_percent = cpu_percent(previous, current).or(self.cpu_percent);
            }
            self.cpu_previous = current.or(self.cpu_previous);
            self.cpu_count = count_cpus(&stat);
            push_history(&mut self.cpu_history, self.cpu_percent);
        }
        if memory {
            self.memory = fs::read_to_string("/proc/meminfo")
                .ok()
                .and_then(|text| parse_meminfo(&text));
            push_history(
                &mut self.memory_history,
                self.memory.and_then(MemoryUsage::percent),
            );
        }
        if network {
            self.refresh_network(Instant::now());
        }
        if uptime {
            self.uptime_seconds = fs::read_to_string("/proc/uptime").ok().and_then(|text| {
                text.split_whitespace()
                    .next()?
                    .parse::<f64>()
                    .ok()
                    .map(|seconds| seconds as u64)
            });
        }
    }
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

    /// Re-derives the tick deadline after the store was replaced wholesale.
    pub(crate) fn rearm_after_store_replace(&mut self) {
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
            let epoch_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_millis() as u64);
            now + Duration::from_millis(
                wall_clock_delay_ms(epoch_ms, rate == TickRate::Second).max(1),
            )
        });
        self.wake.set([self.due, self.sample_due].into_iter().flatten().min());
    }

    fn sample(&mut self) {
        let widgets = self.store.widgets();
        let custom = widgets
            .iter()
            .any(|widget| widget.kind == WidgetKind::Custom);
        let network = widgets.iter().any(|widget| widget.kind == WidgetKind::Network);
        let cpu = custom
            || widgets.iter().any(|widget| {
                matches!(widget.kind, WidgetKind::Cpu | WidgetKind::History | WidgetKind::Ring)
            });
        let memory = custom
            || widgets.iter().any(|widget| {
                matches!(widget.kind, WidgetKind::Memory | WidgetKind::History | WidgetKind::Ring)
            });
        self.samples.refresh(cpu, memory, network, custom);
        self.sample_due = Some(Instant::now() + SAMPLE_INTERVAL);
    }

    fn sample_key(&self) -> SampleKey {
        let samples = &self.samples;
        SampleKey {
            cpu_percent: samples.cpu_percent,
            cpu_count: samples.cpu_count,
            memory: samples.memory,
            uptime_seconds: samples.uptime_seconds,
            cpu_history: samples.cpu_history.clone(),
            memory_history: samples.memory_history.clone(),
            net_rx_history: samples.net_rx_history.clone(),
            net_tx_history: samples.net_tx_history.clone(),
        }
    }

    pub(crate) fn persist(&self) {
        if let Err(error) = flamewm_desktop_core::widgets::persist(&self.path, &self.store) {
            eprintln!("desktop: widget state save failed: {error:?}");
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

fn local_wall_time() -> WallTime {
    let now = Local::now();
    WallTime {
        year: now.year(),
        month: now.month() as u8,
        day: now.day() as u8,
        weekday: now.weekday().num_days_from_sunday() as u8,
        hour: now.hour() as u8,
        minute: now.minute() as u8,
        second: now.second().min(59) as u8,
    }
}

fn is_analog(widget: &Widget) -> bool {
    widget.kind == WidgetKind::Clock && widget.value("style") == CLOCK_STYLE_ANALOG
}

/// Hand positions as fractions of a full turn, clockwise from 12 o'clock.
#[derive(Debug, Clone, Copy, PartialEq)]
struct HandTurns {
    hour: f32,
    minute: f32,
    second: f32,
}

fn hand_turns(time: WallTime) -> HandTurns {
    let hour = f32::from(time.hour % 12);
    let minute = f32::from(time.minute);
    let second = f32::from(time.second);
    HandTurns {
        hour: (hour + minute / 60.0 + second / 3600.0) / 12.0,
        minute: (minute + second / 60.0) / 60.0,
        second: second / 60.0,
    }
}

/// Distance-based anti-aliased coverage of a capsule from `from` to `to`.
fn segment_coverage(px: f32, py: f32, from: (f32, f32), to: (f32, f32), half: f32) -> f32 {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let len_sq = (dx * dx + dy * dy).max(f32::EPSILON);
    let t = (((px - from.0) * dx + (py - from.1) * dy) / len_sq).clamp(0.0, 1.0);
    let (ex, ey) = (from.0 + dx * t - px, from.1 + dy * t - py);
    (half + 0.5 - (ex * ex + ey * ey).sqrt()).clamp(0.0, 1.0)
}

/// Composites `color` over a non-premultiplied RGBA pixel.
fn over(pixel: &mut [f32; 4], color: Rgb, coverage: f32) {
    if coverage <= 0.0 {
        return;
    }
    let below = pixel[3] * (1.0 - coverage);
    let alpha = coverage + below;
    for (channel, value) in pixel[..3]
        .iter_mut()
        .zip([color.red, color.green, color.blue])
    {
        *channel = (f32::from(value) * coverage + *channel * below) / alpha;
    }
    pixel[3] = alpha;
}

/// RGBA8 analog face: 60 ticks (5-minute ticks heavier), hour/minute hands,
/// accent-colored second hand and hub. Numerals are HTML nodes, not pixels.
pub(crate) fn analog_face_pixels(size: u32, time: WallTime, hands: Rgb, accent: Rgb) -> Vec<u8> {
    let size = size.max(1);
    let side = size as f32;
    let center = side / 2.0;
    let radius = center - 1.0;
    let point = |turn: f32, r: f32| {
        let angle = turn * std::f32::consts::TAU;
        (center + r * angle.sin(), center - r * angle.cos())
    };
    let turns = hand_turns(time);
    let hour_half = (side * 0.012).max(0.6);
    let minute_half = (side * 0.005).max(0.5);
    let hand_specs = [
        (hands, turns.hour, -0.12, 0.5, side * 0.028),
        (hands, turns.minute, -0.12, 0.78, side * 0.018),
        (accent, turns.second, -0.25, 0.9, (side * 0.006).max(0.6)),
    ];
    let mut pixels = vec![[0.0f32; 4]; (size * size) as usize];
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let pixel = &mut pixels[(y * size + x) as usize];
            if (px - center).hypot(py - center) >= radius * 0.82 - 1.0 {
                for minute in 0..60 {
                    let turn = minute as f32 / 60.0;
                    let (inner, half) = if minute % 5 == 0 {
                        (radius * 0.84, hour_half)
                    } else {
                        (radius * 0.95, minute_half)
                    };
                    let coverage =
                        segment_coverage(px, py, point(turn, inner), point(turn, radius), half);
                    over(pixel, hands, coverage);
                }
            }
            for (color, turn, from, to, half) in hand_specs {
                let coverage = segment_coverage(
                    px,
                    py,
                    point(turn, radius * from),
                    point(turn, radius * to),
                    half,
                );
                over(pixel, color, coverage);
            }
            let hub = segment_coverage(px, py, (center, center), (center, center), side * 0.035);
            over(pixel, hands, hub);
        }
    }
    pixels
        .iter()
        .flat_map(|pixel| {
            [
                pixel[0].round() as u8,
                pixel[1].round() as u8,
                pixel[2].round() as u8,
                (pixel[3] * 255.0).round() as u8,
            ]
        })
        .collect()
}

fn color(rgb: Rgb, alpha: u8) -> UiColor {
    UiColor {
        r: rgb.red,
        g: rgb.green,
        b: rgb.blue,
        a: alpha,
    }
}

fn percent_to_alpha(percent: i64) -> u8 {
    ((percent.clamp(0, 100) * 255 + 50) / 100) as u8
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
pub(crate) fn ring_pixels(
    size: u32,
    thickness: f32,
    percent: u8,
    ring: Rgb,
    track: Rgb,
) -> Vec<u8> {
    let size = size.max(1);
    let mut pixels = vec![0u8; (size * size * 4) as usize];
    let center = size as f32 / 2.0;
    let half = (thickness / 2.0).clamp(0.5, center);
    let radius = center - half - 0.5;
    let fraction = f32::from(percent.min(100)) / 100.0;
    let cap = |angle: f32| {
        let theta = angle * std::f32::consts::TAU;
        (center + radius * theta.sin(), center - radius * theta.cos())
    };
    let (start_x, start_y) = cap(0.0);
    let (end_x, end_y) = cap(fraction);
    for y in 0..size {
        for x in 0..size {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let dx = px - center;
            let dy = py - center;
            let distance = (dx * dx + dy * dy).sqrt();
            let coverage = (half + 0.5 - (distance - radius).abs()).clamp(0.0, 1.0);
            let near = |cx: f32, cy: f32| {
                let ex = px - cx;
                let ey = py - cy;
                (half + 0.5 - (ex * ex + ey * ey).sqrt()).clamp(0.0, 1.0)
            };
            let mut angle = dx.atan2(-dy) / std::f32::consts::TAU;
            if angle < 0.0 {
                angle += 1.0;
            }
            let on_arc = fraction > 0.0 && angle <= fraction;
            let cap_coverage = if fraction > 0.0 {
                near(start_x, start_y).max(near(end_x, end_y))
            } else {
                0.0
            };
            let arc_coverage = if on_arc { coverage } else { 0.0 }.max(cap_coverage);
            let track_coverage = coverage * (1.0 - arc_coverage);
            let alpha = arc_coverage + track_coverage * 0.55;
            if alpha <= 0.0 {
                continue;
            }
            let mix = |a: u8, b: u8| {
                let total = f32::from(a) * arc_coverage + f32::from(b) * track_coverage * 0.55;
                (total / alpha).round().clamp(0.0, 255.0) as u8
            };
            let index = ((y * size + x) * 4) as usize;
            pixels[index] = mix(ring.red, track.red);
            pixels[index + 1] = mix(ring.green, track.green);
            pixels[index + 2] = mix(ring.blue, track.blue);
            pixels[index + 3] = (alpha.min(1.0) * 255.0).round() as u8;
        }
    }
    pixels
}

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
    let time = local_wall_time();
    let placeholders = widget_placeholders(state, time);
    let widgets = state.widgets.store.widgets();
    // Wall time only reaches the document through time-showing widgets; without one, a
    // second-by-second clock must not force a sync.
    let needs_time = widgets.iter().any(|widget| {
        matches!(widget.kind, WidgetKind::Clock | WidgetKind::Calendar | WidgetKind::Custom)
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
        document.visible(
            &format!("{id}-history"),
            widget.kind == WidgetKind::History,
        )?;
        document.visible(
            &format!("{id}-network"),
            widget.kind == WidgetKind::Network,
        )?;
        document.visible(&format!("{id}-media"), widget.kind == WidgetKind::Media)?;
        document.visible(
            &format!("{id}-weather"),
            widget.kind == WidgetKind::Weather,
        )?;
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

fn sync_analog(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    slot: usize,
    widget: &Widget,
    time: WallTime,
) -> Result<(), String> {
    let id = format!("widget-{}", slot + 1);
    let side = (widget.rect.width.min(widget.rect.height) as f32 - WIDGET_PADDING * 2.0)
        .max(48.0)
        .floor();
    let size = side as u32;
    // Absolute children are placed from the widget's content box, so the padding is subtracted.
    let left = ((widget.rect.width as f32 - side) / 2.0 - WIDGET_PADDING).floor();
    let top = ((widget.rect.height as f32 - side) / 2.0 - WIDGET_PADDING).floor();
    document.position(&format!("{id}-analog"), left, top)?;
    document.size(&format!("{id}-analog"), side, side)?;
    let hands = widget.color("textColor");
    let accent = widget.color_or_accent("accentColor", projection_accent(state));
    let key = FaceKey {
        size,
        second: u32::from(time.hour) * 3600 + u32::from(time.minute) * 60 + u32::from(time.second),
        hands,
        accent,
    };
    if state.widgets.faces[slot] != Some(key) {
        for number in 1..=12 {
            let numeral = format!("{id}-analog-n{number}");
            document.foreground(&numeral, color(hands, 255))?;
            document.font_size(&numeral, (side * 0.11).clamp(9.0, 22.0))?;
        }
        document.image_rgba8(
            &format!("{id}-analog-image"),
            RuntimeImage {
                source: format!("{id}-analog-face"),
                width: size,
                height: size,
                pixels: analog_face_pixels(size, time, hands, accent),
            },
        )?;
        state.widgets.faces[slot] = Some(key);
    }
    Ok(())
}

fn sync_clock(
    document: &mut impl UiDocumentAccess,
    id: &str,
    widget: &Widget,
    time: WallTime,
    accent: Rgb,
) -> Result<(), String> {
    // One-cell cards keep only the time, fitted to the card width.
    let compact = widget.rect.width < COMPACT_CARD_WIDTH;
    let show_time = widget.flag("showTime");
    let show_date = widget.flag("showDate") && !compact;
    let side_by_side = widget.flag("sideBySide") && !compact;
    let time_text = format_time(time, widget.flag("use24Hour"), widget.flag("showSeconds"));
    let time_size = fitted_font_size(
        widget.value("timeSize") as f32,
        time_text.chars().count(),
        widget.rect.width as f32,
    );
    let date_text = format_date(time, widget.flag("showWeekday"), widget.flag("showYear"));
    let time_color = color(widget.color("textColor"), 255);
    let date_color = color(widget.color_or_accent("accentColor", accent), 255);
    let weight = if widget.flag("bold") { 700 } else { 400 };
    let align = widget.align();
    document.visible(&format!("{id}-clock-stack"), !side_by_side)?;
    document.visible(&format!("{id}-clock-row"), side_by_side)?;
    for (time_id, date_id) in [
        (format!("{id}-clock-time"), format!("{id}-clock-date")),
        (
            format!("{id}-clock-row-time"),
            format!("{id}-clock-row-date"),
        ),
    ] {
        document.text(&time_id, time_text.clone())?;
        document.text(&date_id, date_text.clone())?;
        document.font_size(&time_id, time_size)?;
        document.font_size(&date_id, WIDGET_DATE_SIZE)?;
        document.font_weight(&time_id, weight)?;
        document.foreground(&time_id, time_color)?;
        document.foreground(&date_id, date_color)?;
        document.visible(&time_id, show_time)?;
        document.visible(&date_id, show_date)?;
    }
    document.visible(&format!("{id}-clock-time-line"), show_time)?;
    document.visible(&format!("{id}-clock-date-line"), show_date)?;
    for line in [
        format!("{id}-clock-time-line"),
        format!("{id}-clock-date-line"),
        format!("{id}-clock-row"),
    ] {
        set_alignment(document, &line, align)?;
    }
    Ok(())
}

fn sync_gauge(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    slot: usize,
    widget: &Widget,
) -> Result<(), String> {
    let id = format!("widget-{}", slot + 1);
    let samples = &state.widgets.samples;
    // A ring picks its metric from the widget option; the CPU and RAM gauges fix theirs.
    let metric = match widget.kind {
        WidgetKind::Ring if widget.value("metric") == 0 => WidgetKind::Cpu,
        WidgetKind::Ring => WidgetKind::Memory,
        kind => kind,
    };
    let (title, percent, detail) = match metric {
        WidgetKind::Cpu => (
            "CPU",
            samples.cpu_percent,
            match samples.cpu_count {
                0 => String::new(),
                1 => "1 core".to_owned(),
                count => format!("{count} cores"),
            },
        ),
        _ => (
            "Memory",
            samples.memory.and_then(MemoryUsage::percent),
            samples.memory.map_or_else(String::new, |memory| {
                format!(
                    "{} / {} GiB",
                    format_gib(memory.used_kib()),
                    format_gib(memory.total_kib)
                )
            }),
        ),
    };
    let percent_text = percent.map_or_else(|| "--%".to_owned(), |value| format!("{value}%"));
    // One-cell gauges stack the title above the ring; side-by-side needs room for both.
    let side_by_side = widget.flag("sideBySide") && widget.rect.width >= COMPACT_CARD_WIDTH;
    let show_title = widget.flag("showTitle");
    let show_percent = widget.flag("showPercent");
    let show_details = widget.flag("showDetails");
    let text_color = color(widget.color("textColor"), 255);
    let caption_height = if !side_by_side && show_title {
        22.0
    } else {
        0.0
    };
    let inner_width = widget.rect.width as f32 - WIDGET_PADDING * 2.0;
    let inner_height = widget.rect.height as f32 - WIDGET_PADDING * 2.0 - caption_height;
    let ring_size = if side_by_side {
        inner_height.min(inner_width * 0.45)
    } else {
        inner_height.min(inner_width)
    }
    .max(24.0)
    .floor();
    document.size(&format!("{id}-gauge-ring"), ring_size, ring_size)?;
    document.visible(&format!("{id}-gauge-copy"), side_by_side)?;
    document.visible(&format!("{id}-gauge-caption"), !side_by_side && show_title)?;
    document.visible(&format!("{id}-gauge-center"), !side_by_side && show_percent)?;
    document.visible(&format!("{id}-gauge-title"), show_title)?;
    document.visible(&format!("{id}-gauge-value"), show_percent)?;
    document.visible(
        &format!("{id}-gauge-detail"),
        show_details && !detail.is_empty(),
    )?;
    document.text(&format!("{id}-gauge-title"), title.to_owned())?;
    document.text(&format!("{id}-gauge-caption"), title.to_owned())?;
    document.text(&format!("{id}-gauge-value"), percent_text.clone())?;
    document.text(&format!("{id}-gauge-percent"), percent_text)?;
    document.text(&format!("{id}-gauge-detail"), detail)?;
    document.font_size(
        &format!("{id}-gauge-percent"),
        (ring_size * 0.22).clamp(10.0, 40.0),
    )?;
    for part in ["title", "caption", "value", "detail", "percent"] {
        document.foreground(&format!("{id}-gauge-{part}"), text_color)?;
    }
    let key = RingKey {
        size: ring_size as u32,
        thickness: widget.value("thickness") as u32,
        percent: percent.unwrap_or(0),
        ring: widget.color_or_accent("ringColor", projection_accent(state)),
        track: widget.color("trackColor"),
    };
    if state.widgets.rings[slot] != Some(key) {
        document.image_rgba8(
            &format!("{id}-gauge-image"),
            RuntimeImage {
                source: format!("{id}-ring-{}-{}", key.size, key.percent),
                width: key.size,
                height: key.size,
                pixels: ring_pixels(
                    key.size,
                    key.thickness as f32,
                    key.percent,
                    key.ring,
                    key.track,
                ),
            },
        )?;
        state.widgets.rings[slot] = Some(key);
    }
    Ok(())
}

/// Draws one line per series into a straight RGBA8 buffer. Values are percentages
/// (0..=100) spread across the full width, one per tick.
pub(crate) fn history_pixels(width: u32, height: u32, series: &[(&VecDeque<u8>, Rgb)]) -> Vec<u8> {
    let (width, height) = (width.max(2), height.max(2));
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    let max_x = (width - 1) as f32;
    let max_y = (height - 1) as f32;
    for (values, color) in series {
        let count = values.len();
        if count == 0 {
            continue;
        }
        let point = |index: usize, value: u8| {
            let x = if count == 1 {
                max_x
            } else {
                index as f32 * max_x / (HISTORY_LEN - 1) as f32
            };
            let y = max_y - f32::from(value.min(100)) / 100.0 * max_y;
            (x, y)
        };
        let points: Vec<(f32, f32)> = values
            .iter()
            .enumerate()
            .map(|(index, value)| point(index + HISTORY_LEN - count, *value))
            .collect();
        for pair in points.windows(2) {
            let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
            let steps = (x1 - x0).abs().max((y1 - y0).abs()).ceil().max(1.0) as u32;
            for step in 0..=steps {
                let t = step as f32 / steps as f32;
                let x = (x0 + (x1 - x0) * t).round() as u32;
                let y = (y0 + (y1 - y0) * t).round() as u32;
                let index = ((y.min(height - 1) * width + x.min(width - 1)) * 4) as usize;
                pixels[index] = color.red;
                pixels[index + 1] = color.green;
                pixels[index + 2] = color.blue;
                pixels[index + 3] = 255;
            }
        }
    }
    pixels
}

fn sync_history(
    document: &mut impl UiDocumentAccess,
    graph: &mut Option<GraphKey>,
    samples: &Samples,
    accent: Rgb,
    slot: usize,
    widget: &Widget,
) -> Result<(), String> {
    let mut series: Vec<(&VecDeque<u8>, Rgb)> = Vec::with_capacity(2);
    if widget.flag("showMemory") {
        series.push((&samples.memory_history, widget.color("memoryColor")));
    }
    if widget.flag("showCpu") {
        series.push((&samples.cpu_history, widget.color_or_accent("cpuColor", accent)));
    }
    sync_graph(document, graph, slot, widget, "history", &series)
}

fn sync_network(
    document: &mut impl UiDocumentAccess,
    graph: &mut Option<GraphKey>,
    samples: &Samples,
    accent: Rgb,
    slot: usize,
    widget: &Widget,
) -> Result<(), String> {
    let rx = scale_rates(&samples.net_rx_history);
    let tx = scale_rates(&samples.net_tx_history);
    let mut series: Vec<(&VecDeque<u8>, Rgb)> = Vec::with_capacity(2);
    if widget.flag("showTx") {
        series.push((&tx, widget.color("txColor")));
    }
    if widget.flag("showRx") {
        series.push((&rx, widget.color_or_accent("rxColor", accent)));
    }
    sync_graph(document, graph, slot, widget, "network", &series)
}

/// Shared sparkline path: sizes `widget-N-<prefix>-image` to the card and draws `series`.
/// The image is rebuilt only when its size, colours or samples differ from `graph`.
fn sync_graph(
    document: &mut impl UiDocumentAccess,
    graph: &mut Option<GraphKey>,
    slot: usize,
    widget: &Widget,
    prefix: &'static str,
    series: &[(&VecDeque<u8>, Rgb)],
) -> Result<(), String> {
    let id = format!("widget-{}-{prefix}-image", slot + 1);
    let width = (widget.rect.width as f32 - WIDGET_PADDING * 2.0).max(2.0).floor() as u32;
    let height = (widget.rect.height as f32 - WIDGET_PADDING * 2.0).max(2.0).floor() as u32;
    document.size(&id, width as f32, height as f32)?;
    let key = GraphKey {
        prefix,
        width,
        height,
        series: series
            .iter()
            .map(|(values, color)| (values.iter().copied().collect(), *color))
            .collect(),
    };
    if graph.as_ref() == Some(&key) {
        return Ok(());
    }
    document.image_rgba8(
        &id,
        RuntimeImage {
            source: format!("{id}-{width}x{height}"),
            width,
            height,
            pixels: history_pixels(width, height, series),
        },
    )?;
    *graph = Some(key);
    Ok(())
}

const MONTH_NAMES: [&str; 12] = [
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
];
const WEEKDAY_INITIALS: [&str; 7] = ["S", "M", "T", "W", "T", "F", "S"];
/// Shown in place of data that is not available; keeps single-text-node targets valid.
const EMPTY_TEXT: &str = "\u{200b}";

/// Forecast from the shell's shared feed, the same payload and options the tray reads. Without
/// a published feed the widget shows its "No data" state; unset or off states use the tray's text.
fn sync_weather(
    document: &mut impl UiDocumentAccess,
    id: &str,
    widget: &Widget,
    weather: &flamewm_api::weather::WeatherFeedSnapshot,
    accent: Rgb,
) -> Result<(), String> {
    let text = color(widget.color("textColor"), 255);
    let accent = color(widget.color_or_accent("accentColor", accent), 255);
    let view = flamewm_desktop_core::weather_feed::weather_widget_view(weather);
    // City name is one line at 15px semibold: ellipsise it to the card's inner width.
    // The width estimate reads semibold ~8% narrow, so keep a 10% margin.
    let city_width = (widget.rect.width as f32 - WIDGET_PADDING * 2.0) * 0.9;
    document.text(
        &format!("{id}-weather-city"),
        ellipsize(&view.location, 15.0, true, city_width),
    )?;
    document.text(&format!("{id}-weather-temp"), view.temperature.clone())?;
    document.text(&format!("{id}-weather-condition"), view.condition_text.clone())?;
    document.text(&format!("{id}-weather-range"), view.range.clone())?;
    document.foreground(&format!("{id}-weather-city"), text)?;
    document.foreground(&format!("{id}-weather-temp"), text)?;
    document.foreground(&format!("{id}-weather-condition"), text)?;
    document.foreground(&format!("{id}-weather-range"), text)?;
    document.text(
        &format!("{id}-weather-attr"),
        view.attribution.unwrap_or(EMPTY_TEXT).to_owned(),
    )?;
    document.visible(&format!("{id}-weather-attr"), view.attribution.is_some())?;
    // Cards under 300 px wide (one 2x2 cell) have no room beside the city name.
    document.visible(&format!("{id}-weather-right"), widget.rect.width >= 300)?;
    document.visible(&format!("{id}-weather-hours"), widget.flag("showHourly"))?;
    for slot in 0..flamewm_desktop_core::weather_feed::WEATHER_STRIP_DAYS {
        let hour = slot + 1;
        let (day, high) = match view.days.get(slot) {
            Some(day) => (day.weekday, day.high.clone()),
            None => (EMPTY_TEXT, EMPTY_TEXT.to_owned()),
        };
        document.text(&format!("{id}-weather-hour-{hour}-time"), day.to_owned())?;
        document.text(&format!("{id}-weather-hour-{hour}-temp"), high)?;
        document.foreground(&format!("{id}-weather-hour-{hour}-time"), accent)?;
        document.foreground(&format!("{id}-weather-hour-{hour}-temp"), text)?;
    }
    Ok(())
}

fn sync_calendar(
    document: &mut impl UiDocumentAccess,
    id: &str,
    widget: &Widget,
    time: WallTime,
    accent: Rgb,
) -> Result<(), String> {
    let text = color(widget.color("textColor"), 255);
    let clear = color(widget.color("background"), 0);
    let marker = color(widget.color_or_accent("accentColor", accent), 255);
    let ink = color(flamewm_desktop_core::sticky::Rgb::new(16, 16, 16), 255);
    let month = usize::from(time.month.clamp(1, 12)) - 1;
    document.visible(&format!("{id}-cal-month"), widget.flag("showMonth"))?;
    document.text(
        &format!("{id}-cal-month"),
        MONTH_NAMES[month].to_uppercase(),
    )?;
    document.foreground(&format!("{id}-cal-month"), text)?;
    for (column, initial) in WEEKDAY_INITIALS.iter().enumerate() {
        let cell = format!("{id}-cal-wd-{column}");
        document.text(&cell, (*initial).to_owned())?;
        document.foreground(&cell, text)?;
    }
    let weeks = flamewm_desktop_core::widgets::calendar_weeks(time.year, u32::from(time.month));
    for (row, days) in weeks.iter().enumerate() {
        for (column, day) in days.iter().enumerate() {
            let cell = format!("{id}-cal-{row}-{column}");
            let today = *day != 0 && *day == u32::from(time.day);
            let label = if *day == 0 {
                EMPTY_TEXT.to_owned()
            } else {
                day.to_string()
            };
            document.text(&cell, label)?;
            document.foreground(&cell, if today { ink } else { text })?;
            document.background(&cell, if today { marker } else { clear })?;
        }
    }
    Ok(())
}

fn sync_media(
    document: &mut impl UiDocumentAccess,
    id: &str,
    widget: &Widget,
    media: &MediaSnapshot,
    accent: Rgb,
) -> Result<(), String> {
    let available =
        media.availability == ServiceAvailability::Available && !media.active_bus_name.is_empty();
    let title = if available && !media.title.is_empty() {
        media.title.clone()
    } else if available {
        "Unknown track".to_owned()
    } else {
        "Nothing playing".to_owned()
    };
    let player = if available && !media.identity.is_empty() {
        media.identity.to_uppercase()
    } else {
        "MEDIA".to_owned()
    };
    document.text(&format!("{id}-media-player"), player)?;
    document.text(&format!("{id}-media-title"), title)?;
    document.text(&format!("{id}-media-artist"), media.artist.clone())?;
    document.visible(&format!("{id}-media-player"), widget.flag("showPlayer"))?;
    document.visible(
        &format!("{id}-media-artist"),
        widget.flag("showArtist") && available && !media.artist.is_empty(),
    )?;
    document.visible(
        &format!("{id}-media-controls"),
        widget.flag("showControls") && available,
    )?;
    let playing = media.playback == PlaybackState::Playing;
    document.visible(&format!("{id}-media-play-icon"), !playing)?;
    document.visible(&format!("{id}-media-pause-icon"), playing)?;
    document.visible(&format!("{id}-media-prev"), media.can_previous)?;
    document.visible(&format!("{id}-media-next"), media.can_next)?;
    let text = color(widget.color("textColor"), 255);
    document.foreground(&format!("{id}-media-title"), text)?;
    document.foreground(&format!("{id}-media-artist"), text)?;
    document.foreground(
        &format!("{id}-media-player"),
        color(widget.color_or_accent("accentColor", accent), 255),
    )
}

/// Cards narrower than this (one grid cell) use their compact layout.
const COMPACT_CARD_WIDTH: i32 = 150;

/// Font size for `chars` glyphs (about 0.62 em each) across a card's inner width,
/// never above `configured` and never below 9 px.
fn fitted_font_size(configured: f32, chars: usize, card_width: f32) -> f32 {
    let inner = card_width - WIDGET_PADDING * 2.0;
    let fit = inner / (chars.max(1) as f32 * 0.62);
    configured.min(fit).max(9.0).floor()
}

/// Shortens `text` with a trailing ellipsis until it fits `max_width`.
fn ellipsize(text: &str, size: f32, bold: bool, max_width: f32) -> String {
    if estimate_text_width(text, size, bold) <= max_width {
        return text.to_owned();
    }
    let mut kept: Vec<char> = text.chars().collect();
    while kept.pop().is_some() {
        let candidate: String = kept.iter().collect::<String>() + "…";
        if estimate_text_width(&candidate, size, bold) <= max_width {
            return candidate;
        }
    }
    "…".to_owned()
}

fn sync_custom(
    document: &mut impl UiDocumentAccess,
    id: &str,
    widget: &Widget,
    placeholders: &Placeholders,
) -> Result<(), String> {
    let base_color = widget.color("textColor");
    let base = SpanStyle {
        color: Some(base_color),
        background: None,
        bold: widget.flag("bold"),
    };
    let lines = fit_lines(parse_markup(&widget.template, base));
    let inner_width = widget.rect.width as f32 - WIDGET_PADDING * 2.0;
    let configured = widget.value("textSize") as f32;
    // Shrink the text when the widest line would overflow the card, down to 9 px.
    let widest = lines
        .iter()
        .map(|spans| {
            spans
                .iter()
                .map(|span| {
                    let text = expand_placeholders(&span.text, placeholders);
                    estimate_text_width(&text, configured, span.style.bold)
                })
                .sum::<f32>()
        })
        .fold(0.0_f32, f32::max);
    let size = if widest > inner_width {
        (configured * inner_width / widest).floor().clamp(9.0, configured)
    } else {
        configured
    };
    let align = widget.align();
    for line_index in 0..MAX_TEMPLATE_LINES {
        let line_id = format!("{id}-line-{}", line_index + 1);
        let line = lines.get(line_index);
        document.visible(&line_id, line.is_some())?;
        set_alignment(document, &line_id, align)?;
        for span_index in 0..MAX_LINE_SPANS {
            let span_id = format!("{line_id}-span-{}", span_index + 1);
            let Some(span) = line.and_then(|spans| spans.get(span_index)) else {
                document.visible(&span_id, false)?;
                continue;
            };
            let text = ellipsize(
                &expand_placeholders(&span.text, placeholders),
                size,
                span.style.bold,
                inner_width,
            );
            document.size(
                &span_id,
                estimate_text_width(&text, size, span.style.bold),
                (size * 1.3).ceil(),
            )?;
            document.text(
                &span_id,
                if text.is_empty() {
                    "\u{200b}".to_owned()
                } else {
                    display_text(&text)
                },
            )?;
            document.font_size(&span_id, size)?;
            document.font_weight(&span_id, if span.style.bold { 700 } else { 400 })?;
            document.foreground(&span_id, color(span.style.color.unwrap_or(base_color), 255))?;
            match span.style.background {
                Some(background) => document.background(&span_id, color(background, 255))?,
                None => document.background_clear(&span_id)?,
            }
            document.visible(&span_id, true)?;
        }
    }
    Ok(())
}

pub(crate) fn poll(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
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
        document.text(&format!("{row_id}-label"), spec.label.to_owned())?;
        document.text(&format!("{row_id}-value"), spec.label_for(value))?;
        let is_color = matches!(spec.control, OptionControl::Color | OptionControl::AccentColor);
        document.visible(&format!("{row_id}-swatch"), is_color)?;
        if is_color {
            let swatch = if value == flamewm_desktop_core::widgets::ACCENT_COLOR {
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
    let mode = match name.rsplit_once(".resize-").and_then(|(_, edge)| Edges::from_name(edge)) {
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
            if state.widgets.gesture.is_some_and(|g| g.widget == widget.id && g.mode == mode) {
                state.widgets.pending_pointer = Some((action.x, action.y));
            }
        }
        UiActionPhase::Release => {
            state.widgets.pending_pointer = None;
            if let Some(gesture) =
                state.widgets.gesture.filter(|g| g.widget == widget.id && g.mode == mode)
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
                if let Some((kind, current)) =
                    state.widgets.store.get(id).and_then(|w| w.grid.map(|g| (w.kind, g)))
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
    if let Some(slot) = state.widgets.store.widgets().iter().position(|w| w.id == id)
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
    let (bounds, scale) = state.menu.as_ref().map_or(
        (state.grid.work_area, state.grid.scale_percent),
        |menu| {
            let grid = crate::entry::grid_for_output(state, &menu.output);
            (grid.work_area, grid.scale_percent)
        },
    );
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
    let Some(spec) = state
        .widgets
        .store
        .get(id)
        .and_then(|widget| widget.kind.visible_options(widget.stored_values()).get(row).copied())
    else {
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
        eprintln!("desktop: media widget action failed: {}", error.message);
        return;
    }
    if let Err(error) = provider.on_ready() {
        eprintln!("desktop: media widget flush failed: {}", error.message);
    }
}

pub(crate) fn refresh_media(state: &mut DesktopState) {
    let Some(provider) = state.widgets.mpris.as_mut() else {
        return;
    };
    if let Err(error) = provider.on_ready() {
        eprintln!("desktop: media widget update failed: {}", error.message);
    }
    provider.reconcile();
    let snapshot = provider.snapshot();
    state.widgets.set_media(snapshot);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampler_widgets_arm_the_sample_timer_without_a_clock_tick() {
        let mut store = WidgetStore::default();
        store
            .add(WidgetKind::Cpu, Rect { x: 0, y: 0, width: 1920, height: 1080 })
            .expect("cpu widget fits");
        let mut state = WidgetState::new(store, PathBuf::new());
        let now = Instant::now();
        state.arm(now);
        assert_eq!(state.due, None, "a CPU widget must not force a per-second tick");
        assert_eq!(state.sample_due, Some(now), "a CPU widget samples on the sample timer");
    }

    fn alpha_at(pixels: &[u8], size: u32, x: u32, y: u32) -> u8 {
        pixels[((y * size + x) * 4 + 3) as usize]
    }

    fn red_at(pixels: &[u8], size: u32, x: u32, y: u32) -> u8 {
        pixels[((y * size + x) * 4) as usize]
    }

    #[test]
    fn ring_is_transparent_inside_and_outside_the_band() {
        let size = 64;
        let pixels = ring_pixels(size, 8.0, 50, Rgb::new(255, 0, 0), Rgb::new(0, 0, 255));
        assert_eq!(pixels.len(), (size * size * 4) as usize);
        assert_eq!(alpha_at(&pixels, size, 32, 32), 0);
        assert_eq!(alpha_at(&pixels, size, 0, 0), 0);
        assert!(alpha_at(&pixels, size, 32, 3) > 200);
    }

    #[test]
    fn ring_fills_clockwise_from_the_top() {
        let size = 64;
        let pixels = ring_pixels(size, 8.0, 25, Rgb::new(255, 0, 0), Rgb::new(0, 0, 255));
        assert!(
            red_at(&pixels, size, 50, 12) > 200,
            "first quarter is the ring color"
        );
        assert!(
            red_at(&pixels, size, 12, 50) < 60,
            "last quarter is the track color"
        );
        let empty = ring_pixels(size, 8.0, 0, Rgb::new(255, 0, 0), Rgb::new(0, 0, 255));
        assert!(red_at(&empty, size, 50, 12) < 60);
    }

    #[test]
    fn slot_numbers_are_bounded() {
        assert_eq!(slot_number("widget.1.move"), Some(1));
        assert_eq!(slot_number("widget.8.media.play"), Some(8));
        assert_eq!(slot_number("widget.9.move"), None);
        assert_eq!(slot_number("widget.menu.lock"), None);
    }

    #[test]
    fn panels_center_inside_the_work_area() {
        assert_eq!(
            panel_rect(Rect::new(0, 40, 1000, 600), (560, 520)),
            Rect::new(220, 80, 560, 520)
        );
        assert_eq!(
            panel_rect(Rect::new(0, 0, 400, 300), (560, 520)),
            Rect::new(0, 0, 400, 300)
        );
    }

    fn wall_time(hour: u8, minute: u8, second: u8) -> WallTime {
        WallTime {
            year: 2026,
            month: 10,
            day: 28,
            weekday: 3,
            hour,
            minute,
            second,
        }
    }

    #[test]
    fn analog_hands_sweep_clockwise_from_twelve() {
        let at_three = hand_turns(wall_time(3, 0, 0));
        assert_eq!(at_three.hour, 0.25);
        assert_eq!(at_three.minute, 0.0);
        assert_eq!(at_three.second, 0.0);
        // 15:30:30 uses the 12-hour dial and carries minutes and seconds into the hour hand.
        let turns = hand_turns(wall_time(15, 30, 30));
        assert!((turns.hour - (3.0 + 0.5 + 30.0 / 3600.0) / 12.0).abs() < 1e-6);
        assert!((turns.minute - 30.5 / 60.0).abs() < 1e-6);
        assert!((turns.second - 0.5).abs() < 1e-6);
    }

    #[test]
    fn segment_coverage_is_full_on_the_axis_and_zero_far_away() {
        let from = (0.0, 0.0);
        let to = (10.0, 0.0);
        assert_eq!(segment_coverage(5.0, 0.0, from, to, 1.0), 1.0);
        assert_eq!(segment_coverage(5.0, 9.0, from, to, 1.0), 0.0);
        assert_eq!(segment_coverage(20.0, 0.0, from, to, 1.0), 0.0);
    }

    #[test]
    fn analog_minute_hand_points_at_the_minute_mark() {
        // 12:15:00: the minute hand points at 3, the hour hand is still near 12.
        let size = 96;
        let hands = Rgb::new(10, 200, 30);
        let accent = Rgb::new(250, 0, 0);
        let pixels = analog_face_pixels(size, wall_time(12, 15, 0), hands, accent);
        assert_eq!(pixels.len(), (size * size * 4) as usize);
        let center = size / 2;
        // Between the hub and the hand tips (hour hand ends at 0.5R, minute at 0.78R).
        let reach = (size as f32 * 0.3) as u32;
        let index = ((center * size + center + reach) * 4) as usize;
        assert_eq!(pixels[index + 3], 255);
        assert!(pixels[index] < 60 && pixels[index + 1] > 150, "{:?}", &pixels[index..index + 4]);
        // Nothing is drawn beyond the dial edge.
        assert_eq!(pixels[3], 0);
    }

    #[test]
    fn analog_second_hand_uses_the_accent_color() {
        let size = 96;
        let hands = Rgb::new(10, 200, 30);
        let accent = Rgb::new(250, 0, 0);
        // 12:00:15: the second hand points at 3 and the minute hand is at 12.
        let pixels = analog_face_pixels(size, wall_time(12, 0, 15), hands, accent);
        let center = size / 2;
        // Between the hub and the hand tips (hour hand ends at 0.5R, minute at 0.78R).
        let reach = (size as f32 * 0.3) as u32;
        let index = ((center * size + center + reach) * 4) as usize;
        // The hand is under 1px wide, so the sampled row only partly covers it.
        assert!(pixels[index + 3] > 100);
        assert!(pixels[index] > 200 && pixels[index + 1] < 60, "{:?}", &pixels[index..index + 4]);
    }

    #[test]
    fn compact_clock_font_fits_the_time_string_inside_one_cell() {
        // 104 px cell, 16 px padding: "01:45" (5 glyphs) must fit the 72 px inner width.
        let size = fitted_font_size(40.0, 5, 104.0);
        assert!(size * 0.62 * 5.0 <= 72.0 + 0.5);
        assert_eq!(fitted_font_size(20.0, 5, 340.0), 20.0);
        assert_eq!(fitted_font_size(40.0, 8, 40.0), 9.0);
    }

    #[test]
    fn weather_city_name_fits_inside_a_two_cell_card() {
        let inner = (216.0 - WIDGET_PADDING * 2.0) * 0.9;
        let city = ellipsize("Buenos Aires, Argentina Pampeana", 15.0, true, inner);
        assert!(city.ends_with('…'));
        assert!(estimate_text_width(&city, 15.0, true) <= inner);
    }

    #[test]
    fn ellipsize_keeps_text_that_fits_and_trims_the_rest() {
        assert_eq!(ellipsize("ok", 12.0, false, 500.0), "ok");
        let trimmed = ellipsize("a very long custom sentence", 14.0, false, 60.0);
        assert!(trimmed.ends_with('…'));
        assert!(estimate_text_width(&trimmed, 14.0, false) <= 60.0);
    }

    #[test]
    fn opacity_maps_to_alpha() {
        assert_eq!(percent_to_alpha(0), 0);
        assert_eq!(percent_to_alpha(70), 179);
        assert_eq!(percent_to_alpha(140), 255);
    }

    #[test]
    fn edge_drag_moves_only_the_dragged_edges() {
        let bounds = Rect::new(0, 0, 1000, 800);
        let original = Rect::new(100, 100, 200, 100);
        let right = Edges::from_name("e").expect("edge");
        assert_eq!(edge_drag_rect(original, right, 50, 999, bounds), Rect::new(100, 100, 250, 100));
        let left = Edges::from_name("w").expect("edge");
        assert_eq!(edge_drag_rect(original, left, 50, 0, bounds), Rect::new(150, 100, 150, 100));
        // Dragging a left edge past the right edge keeps one pixel instead of inverting.
        assert_eq!(edge_drag_rect(original, left, 900, 0, bounds).width, 1);
        let corner = Edges::from_name("nw").expect("edge");
        assert_eq!(edge_drag_rect(original, corner, -20, -30, bounds), Rect::new(80, 70, 220, 130));
    }

    fn settings_with(values: &[(&str, flamewm_api::settings::SettingValue)]) -> SettingsSnapshot {
        let mut settings = SettingsSnapshot::new(1);
        for (key, value) in values {
            settings.values.insert((*key).to_owned(), value.clone());
        }
        settings
    }

    #[test]
    fn inheriting_card_takes_the_global_surface_fill_blur_and_radius() {
        let widget = Widget::new(1, WidgetKind::Clock, Point::new(0, 0));
        let surface = widget_surface(&widget, &SettingsSnapshot::new(1));
        // Dark glass tint at the regular glass alpha, blurred at the 28 px default (70%).
        assert_eq!(
            surface.fill,
            UiColor { r: 234, g: 244, b: 255, a: 31 }
        );
        assert_eq!(surface.blur, Some(70));
        assert_eq!(surface.radius, 18.0);
    }

    #[test]
    fn card_without_custom_surface_ignores_its_own_colour_and_opacity() {
        let mut widget = Widget::new(1, WidgetKind::Memory, Point::new(0, 0));
        assert!(widget.inherits_surface());
        assert!(widget.set_value("backgroundOpacity", 90));
        let inherited = widget_surface(&widget, &SettingsSnapshot::new(1));
        assert_eq!(inherited.blur, Some(70));
        assert_eq!(inherited.fill.a, 31);
    }

    #[test]
    fn custom_surface_uses_its_own_colour_and_opacity_with_global_blur() {
        let mut widget = Widget::new(1, WidgetKind::Cpu, Point::new(0, 0));
        assert!(widget.set_value("customSurface", 1));
        assert!(widget.set_value("background", 0));
        assert!(widget.set_value("backgroundOpacity", 70));
        let surface = widget_surface(&widget, &settings_with(&[]));
        assert_eq!(surface.fill.a, 179);
        assert_eq!(surface.blur, Some(70));
        assert_eq!(surface.radius, WIDGET_RADIUS);
        // A global blur change follows every card; the custom colour stays put.
        let no_blur = settings_with(&[(
            "surfaceBlurPercent",
            flamewm_api::settings::SettingValue::Percent(0),
        )]);
        let global = widget_surface(&widget, &no_blur);
        assert_eq!(global.blur, None);
        assert_eq!(global.fill, surface.fill);
    }

    #[test]
    fn reduce_transparency_makes_inheriting_cards_opaque_without_blur() {
        let widget = Widget::new(1, WidgetKind::Clock, Point::new(0, 0));
        let reduced = settings_with(&[(
            "reduceTransparency",
            flamewm_api::settings::SettingValue::Boolean(true),
        )]);
        let surface = widget_surface(&widget, &reduced);
        assert_eq!(surface.fill.a, 255);
        assert_eq!(surface.blur, None);
    }

    #[test]
    fn dragged_card_rises_above_content_until_release() {
        assert_eq!(dragged_layer(false), UiLayer::Floating);
        assert!(dragged_layer(false).z_index() > UiLayer::Content.z_index());
        assert!(dragged_layer(false).z_index() < UiLayer::Overlay.z_index());
        assert_eq!(dragged_layer(true), UiLayer::Content);
    }

    #[test]
    fn graph_image_is_rebuilt_only_when_its_inputs_change() {
        use flamewm_ui_x11::test_support::RecordingUiDoc;
        let mut widget = Widget::new(1, WidgetKind::History, Point::new(0, 0));
        widget.rect = Rect::new(0, 0, 300, 160);
        let accent = Rgb { red: 9, green: 9, blue: 9 };
        let mut samples = Samples::default();
        samples.cpu_history = VecDeque::from(vec![10, 20, 30]);
        let mut graph = None;
        let mut doc = RecordingUiDoc::default();
        sync_history(&mut doc, &mut graph, &samples, accent, 0, &widget).expect("first");
        assert!(doc.images.contains_key("widget-1-history-image"));
        // Unchanged samples and size: no image is rebuilt or uploaded.
        doc.images.clear();
        sync_history(&mut doc, &mut graph, &samples, accent, 0, &widget).expect("same");
        assert!(doc.images.is_empty());
        // A new sample scrolls the graph, so the image is uploaded again.
        samples.cpu_history.push_back(40);
        sync_history(&mut doc, &mut graph, &samples, accent, 0, &widget).expect("changed");
        assert!(doc.images.contains_key("widget-1-history-image"));
        // Resizing the card changes the key as well.
        doc.images.clear();
        widget.rect = Rect::new(0, 0, 320, 160);
        sync_history(&mut doc, &mut graph, &samples, accent, 0, &widget).expect("resized");
        assert!(doc.images.contains_key("widget-1-history-image"));
    }

    #[test]
    fn drop_target_ghost_is_an_outline_without_fill() {
        use flamewm_ui_x11::test_support::RecordingUiDoc;
        let metrics = GridMetrics::for_work_area(Rect::new(0, 0, 1920, 1080), 100);
        let accent = Rgb { red: 9, green: 9, blue: 9 };
        let mut doc = RecordingUiDoc::default();
        let cell = Placement::left(1, 1, Span::SINGLE);
        sync_ghost(&mut doc, metrics, accent, Some((cell, true))).expect("ghost");
        assert!(doc.borders.contains_key("widget-ghost"));
        assert!(!doc.backgrounds.contains_key("widget-ghost"));
        assert_eq!(doc.visible.get("widget-ghost"), Some(&true));
    }

    #[test]
    fn history_graph_draws_each_enabled_series_at_its_sample_height() {
        let mut cpu = VecDeque::new();
        cpu.push_back(100u8);
        cpu.push_back(0u8);
        let sky = Rgb { red: 1, green: 2, blue: 3 };
        let pixels = history_pixels(8, 5, &[(&cpu, sky)]);
        assert_eq!(pixels.len(), 8 * 5 * 4);
        // History is right-aligned: the newest samples sit in the right column,
        // 100% on the top row and 0% on the bottom row.
        let top_right = ((7) * 4) as usize;
        assert_eq!(&pixels[top_right..top_right + 4], &[1, 2, 3, 255]);
        let bottom_right = ((4 * 8 + 7) * 4) as usize;
        assert_eq!(&pixels[bottom_right..bottom_right + 4], &[1, 2, 3, 255]);
        // Left-most column is untouched while history is shorter than the graph.
        assert_eq!(&pixels[0..4], &[0, 0, 0, 0]);
        // An empty series and an empty pixel stay transparent.
        let empty = history_pixels(8, 5, &[(&VecDeque::new(), sky)]);
        assert!(empty.iter().all(|byte| *byte == 0));
    }

    #[test]
    fn net_dev_totals_skip_loopback_and_autoscale_rates() {
        let text = "Inter-|   Receive                                                |  Transmit\n\
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n\
    lo:  500 1 0 0 0 0 0 0  500 1 0 0 0 0 0 0\n\
  eth0: 1000 2 0 0 0 0 0 0  300 2 0 0 0 0 0 0\n\
  wlan0: 24 1 0 0 0 0 0 0  76 1 0 0 0 0 0 0\n";
        assert_eq!(parse_net_dev(text), Some((1024, 376)));
        assert_eq!(parse_net_dev("garbage"), None);
        let mut rates = VecDeque::new();
        push_rate(&mut rates, 50);
        push_rate(&mut rates, 200);
        assert_eq!(scale_rates(&rates), VecDeque::from([25u8, 100]));
    }
}

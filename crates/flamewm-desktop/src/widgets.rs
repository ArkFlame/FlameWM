use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::{Datelike, Local, Timelike};
use flamewm_api::system::{MediaSnapshot, PlaybackState, ServiceAvailability, SystemAction};
use flamewm_api::{Point, Rect};
use flamewm_desktop_core::sticky::Rgb;
use flamewm_desktop_core::widget_text::{
    CpuTimes, MAX_LINE_SPANS, MAX_TEMPLATE_CHARS, MAX_TEMPLATE_LINES, MemoryUsage, Placeholders,
    SpanStyle, WallTime, count_cpus, cpu_percent, display_text, estimate_text_width,
    expand_placeholders, fit_lines, format_date, format_gib, format_time, format_uptime,
    parse_cpu_times, parse_markup, parse_meminfo,
};
use flamewm_desktop_core::widgets::{
    MAX_WIDGETS, OptionControl, TextAlign, Widget, WidgetKind, WidgetStore, move_widget_rect,
    palette_color, resize_widget_rect,
};
use flamewm_integrations_linux::MprisProvider;
use flamewm_reactor::{WakeDeadline, wall_clock_delay_ms};
use flamewm_ui_core::style::UiLayer;
use flamewm_ui_x11::{
    PointerButton, RuntimeImage, UiActionEvent, UiActionPhase, UiColor, UiDocumentAccess,
};

use crate::entry::projection;
use crate::entry::{
    DesktopState, MenuKind, MenuState, hide_menus, pointer_button, pointer_on_output, sync_menu,
};

pub(crate) const CONFIG_ROWS: usize = 17;
const GALLERY_SIZE: (i32, i32) = (560, 590);
const CONFIG_WIDTH: i32 = 420;
const CONFIG_HEADER_HEIGHT: i32 = 62;
const CONFIG_ROW_HEIGHT: i32 = 32;
const DRAG_THRESHOLD_PX: f32 = 4.0;
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
    Resize,
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

#[derive(Debug, Default)]
struct Samples {
    cpu_previous: Option<CpuTimes>,
    cpu_percent: Option<u8>,
    cpu_count: usize,
    memory: Option<MemoryUsage>,
    uptime_seconds: Option<u64>,
}

impl Samples {
    fn refresh(&mut self, cpu: bool, memory: bool, uptime: bool) {
        if cpu && let Ok(stat) = fs::read_to_string("/proc/stat") {
            let current = parse_cpu_times(&stat);
            if let (Some(previous), Some(current)) = (self.cpu_previous, current) {
                self.cpu_percent = cpu_percent(previous, current).or(self.cpu_percent);
            }
            self.cpu_previous = current.or(self.cpu_previous);
            self.cpu_count = count_cpus(&stat);
        }
        if memory {
            self.memory = fs::read_to_string("/proc/meminfo")
                .ok()
                .and_then(|text| parse_meminfo(&text));
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

pub(crate) struct WidgetState {
    pub(crate) store: WidgetStore,
    path: PathBuf,
    gesture: Option<Gesture>,
    editing: Option<TemplateEdit>,
    samples: Samples,
    media: MediaSnapshot,
    pub(crate) mpris: Option<MprisProvider>,
    media_dirty: bool,
    wake: WakeDeadline,
    due: Option<Instant>,
    rings: [Option<RingKey>; MAX_WIDGETS],
    username: String,
    hostname: String,
}

impl WidgetState {
    pub(crate) fn new(store: WidgetStore, path: PathBuf) -> Self {
        Self {
            store,
            path,
            gesture: None,
            editing: None,
            samples: Samples::default(),
            media: MediaSnapshot::default(),
            mpris: None,
            media_dirty: false,
            wake: WakeDeadline::default(),
            due: None,
            rings: [None; MAX_WIDGETS],
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
            widget.kind.ticks_every_second()
                || (widget.kind == WidgetKind::Clock && widget.flag("showSeconds"))
        }) {
            Some(TickRate::Second)
        } else if widgets
            .iter()
            .any(|widget| widget.kind == WidgetKind::Clock)
        {
            Some(TickRate::Minute)
        } else {
            None
        }
    }

    fn arm(&mut self, now: Instant) {
        self.due = self.tick_rate().map(|rate| {
            let epoch_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_millis() as u64);
            now + Duration::from_millis(
                wall_clock_delay_ms(epoch_ms, rate == TickRate::Second).max(1),
            )
        });
        self.wake.set(self.due);
    }

    fn sample(&mut self) {
        let widgets = self.store.widgets();
        let custom = widgets
            .iter()
            .any(|widget| widget.kind == WidgetKind::Custom);
        let cpu = custom || widgets.iter().any(|widget| widget.kind == WidgetKind::Cpu);
        let memory = custom
            || widgets
                .iter()
                .any(|widget| widget.kind == WidgetKind::Memory);
        self.samples.refresh(cpu, memory, custom);
    }

    fn persist(&self) {
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

/// Backdrop blur strength for a widget card; `None` (0%) draws no blur.
fn widget_backdrop(widget: &Widget) -> Option<u8> {
    let blur = widget.value("backdropBlur").clamp(0, 100);
    (blur > 0).then_some(blur as u8)
}

pub(crate) fn widget_bounds(state: &DesktopState) -> Rect {
    state
        .output_grids
        .values()
        .map(|grid| grid.work_area)
        .reduce(Rect::union)
        .unwrap_or(state.grid.work_area)
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

pub(crate) fn assign_layers(document: &mut impl UiDocumentAccess) -> Result<(), String> {
    for slot in 1..=MAX_WIDGETS {
        document.layer(&format!("widget-{slot}"), UiLayer::Content)?;
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
    for slot in 0..MAX_WIDGETS {
        let id = format!("widget-{}", slot + 1);
        let Some(widget) = state.widgets.store.widgets().get(slot).cloned() else {
            document.visible(&id, false)?;
            state.widgets.rings[slot] = None;
            continue;
        };
        document.position(&id, widget.rect.x as f32, widget.rect.y as f32)?;
        document.size(&id, widget.rect.width as f32, widget.rect.height as f32)?;
        document.background(
            &id,
            color(
                widget.color("background"),
                percent_to_alpha(widget.value("backgroundOpacity")),
            ),
        )?;
        document.corner_radius(&id, widget.value("radius") as f32)?;
        document.node_backdrop(&id, widget_backdrop(&widget))?;
        document.visible(&format!("{id}-resize"), !widget.locked)?;
        document.visible(&format!("{id}-clock"), widget.kind == WidgetKind::Clock)?;
        document.visible(
            &format!("{id}-gauge"),
            matches!(widget.kind, WidgetKind::Cpu | WidgetKind::Memory),
        )?;
        document.visible(&format!("{id}-media"), widget.kind == WidgetKind::Media)?;
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
        match widget.kind {
            WidgetKind::Clock => sync_clock(document, &id, &widget, time)?,
            WidgetKind::Cpu | WidgetKind::Memory => sync_gauge(document, state, slot, &widget)?,
            WidgetKind::Media => sync_media(document, &id, &widget, &state.widgets.media)?,
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
    Ok(())
}

fn sync_clock(
    document: &mut impl UiDocumentAccess,
    id: &str,
    widget: &Widget,
    time: WallTime,
) -> Result<(), String> {
    let show_time = widget.flag("showTime");
    let show_date = widget.flag("showDate");
    let side_by_side = widget.flag("sideBySide");
    let time_text = format_time(time, widget.flag("use24Hour"), widget.flag("showSeconds"));
    let date_text = format_date(time, widget.flag("showWeekday"), widget.flag("showYear"));
    let time_color = color(widget.color("textColor"), 255);
    let date_color = color(widget.color("accentColor"), 255);
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
        document.font_size(&time_id, widget.value("timeSize") as f32)?;
        document.font_size(&date_id, widget.value("dateSize") as f32)?;
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
    let (title, percent, detail) = match widget.kind {
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
    let side_by_side = widget.flag("sideBySide");
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
        ring: widget.color("ringColor"),
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

fn sync_media(
    document: &mut impl UiDocumentAccess,
    id: &str,
    widget: &Widget,
    media: &MediaSnapshot,
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
        color(widget.color("accentColor"), 255),
    )
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
    let size = widget.value("textSize") as f32;
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
            let text = expand_placeholders(&span.text, placeholders);
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
    let due = state.widgets.due.is_some_and(|due| now >= due);
    if due {
        state.widgets.sample();
        state.widgets.arm(now);
    }
    if !due && !std::mem::take(&mut state.widgets.media_dirty) {
        return Ok(());
    }
    state.widgets.media_dirty = false;
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
                        preview.color("ringColor")
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
    let options = widget.kind.options();
    for row in 0..CONFIG_ROWS {
        let row_id = format!("widget-config-row-{row}");
        let Some(spec) = options.get(row) else {
            document.visible(&row_id, false)?;
            continue;
        };
        let value = widget.value(spec.key);
        document.text(&format!("{row_id}-label"), spec.label.to_owned())?;
        document.text(&format!("{row_id}-value"), spec.label_for(value))?;
        let is_color = spec.control == OptionControl::Color;
        document.visible(&format!("{row_id}-swatch"), is_color)?;
        if is_color {
            document.background(
                &format!("{row_id}-swatch"),
                color(palette_color(value), 255),
            )?;
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
    let mode = if name.ends_with(".resize") {
        GestureMode::Resize
    } else {
        GestureMode::Move
    };
    match action.phase {
        UiActionPhase::Press => {
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
        UiActionPhase::Motion | UiActionPhase::Release => {
            let release = action.phase == UiActionPhase::Release;
            let Some(mut gesture) = state
                .widgets
                .gesture
                .filter(|gesture| gesture.widget == widget.id && gesture.mode == mode)
            else {
                return Ok(());
            };
            let dx = action.x - gesture.start_x;
            let dy = action.y - gesture.start_y;
            gesture.moved |= dx * dx + dy * dy >= DRAG_THRESHOLD_PX * DRAG_THRESHOLD_PX;
            state.widgets.gesture = (!release).then_some(gesture);
            if !gesture.moved {
                return Ok(());
            }
            let bounds = widget_bounds(state);
            let rect = match gesture.mode {
                GestureMode::Move => {
                    move_widget_rect(gesture.original, dx as i32, dy as i32, bounds)
                }
                GestureMode::Resize => {
                    resize_widget_rect(gesture.original, dx as i32, dy as i32, bounds)
                }
            };
            state.widgets.store.set_rect(widget.id, rect);
            state.widgets.store.anchor_to(widget.id, bounds);
            if release {
                state.widgets.persist();
            }
            sync_widgets(document, state)?;
        }
        UiActionPhase::Hover => {}
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
    let bounds = state.menu.as_ref().map_or(state.grid.work_area, |menu| {
        crate::entry::grid_for_output(state, &menu.output).work_area
    });
    if state.widgets.store.add(kind, bounds).is_none() {
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
    let Some(widget) = state.widgets.store.get_mut(id) else {
        return Ok(());
    };
    let Some(spec) = widget.kind.options().get(row).copied() else {
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

    #[test]
    fn opacity_maps_to_alpha() {
        assert_eq!(percent_to_alpha(0), 0);
        assert_eq!(percent_to_alpha(70), 179);
        assert_eq!(percent_to_alpha(140), 255);
    }

    #[test]
    fn backdrop_blur_is_off_by_default_and_follows_the_setting() {
        let mut widget = Widget::new(1, WidgetKind::Memory, Point::new(0, 0));
        assert_eq!(widget_backdrop(&widget), None);
        widget.set_value("backdropBlur", 40);
        assert_eq!(widget_backdrop(&widget), Some(40));
        widget.set_value("backdropBlur", 150);
        assert_eq!(widget_backdrop(&widget), Some(100));
    }
}

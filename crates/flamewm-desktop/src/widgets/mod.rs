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
mod actions;
mod gesture;
mod lifecycle;
mod panels;
mod render;
mod sampling;

pub(crate) use actions::{cancel_transient, handle_action, handle_text_input, is_widget_action};
pub(crate) use gesture::{apply_pending_drag, widget_metrics};
pub(crate) use lifecycle::{is_obscured, poll, refresh_media, set_obscured};
pub(crate) use panels::{hide_panels, open_gallery, place_config, place_gallery, widget_menu_rows};
pub(crate) use render::{assign_layers, sync_widgets};

use actions::*;
use gesture::*;
use lifecycle::*;
use panels::*;
use render::*;

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
    /// Display name and host as drawn into custom templates. The sample values at draw time are
    /// already in `samples` and the time/date/weekday derive from `time`, so the key needs no
    /// formatted placeholder strings: an unchanged sync then formats and allocates nothing.
    username: String,
    hostname: String,
    time: Option<WallTime>,
    samples: SampleKey,
}

#[derive(Debug, Clone)]
struct SampleKey {
    cpu_percent: Option<u8>,
    cpu_count: usize,
    /// Memory inputs as drawn: percent, used KiB and total KiB. Kept numeric so a comparison
    /// allocates nothing; the equality contract still follows the drawn text.
    memory: Option<(Option<u8>, u64, u64)>,
    uptime_seconds: Option<u64>,
    cpu_history: VecDeque<u8>,
    memory_history: VecDeque<u8>,
    /// Network graph points as drawn (`scale_rates`), so idle traffic compares equal.
    net_rx_history: VecDeque<u8>,
    net_tx_history: VecDeque<u8>,
}

impl PartialEq for SampleKey {
    fn eq(&self, other: &Self) -> bool {
        self.cpu_percent == other.cpu_percent
            && self.cpu_count == other.cpu_count
            && memory_text_eq(self.memory, other.memory)
            && self.uptime_seconds == other.uptime_seconds
            && self.cpu_history == other.cpu_history
            && self.memory_history == other.memory_history
            && self.net_rx_history == other.net_rx_history
            && self.net_tx_history == other.net_tx_history
    }
}

/// Equal when both readings render the same percent, used and total text. Raw KiB that rounds
/// to identical text must not repaint.
fn memory_text_eq(
    left: Option<(Option<u8>, u64, u64)>,
    right: Option<(Option<u8>, u64, u64)>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (
            Some((left_percent, left_used, left_total)),
            Some((right_percent, right_used, right_total)),
        ) => {
            (left_percent, left_used, left_total) == (right_percent, right_used, right_total)
                || (left_percent == right_percent
                    && format_gib(left_used) == format_gib(right_used)
                    && format_gib(left_total) == format_gib(right_total))
        }
        _ => false,
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

/// Corner radius shared by every widget card.
const WIDGET_RADIUS: f32 = 18.0;
/// Date line size of clock widgets.
const WIDGET_DATE_SIZE: f32 = 16.0;
/// Edge and corner resize bands of each widget, named by compass direction.
const WIDGET_EDGES: [&str; 8] = ["n", "s", "e", "w", "ne", "nw", "se", "sw"];

#[cfg(test)]
mod tests;

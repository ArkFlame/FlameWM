use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use flamewm_api::{FlameError, FlameResult, Point, Rect};

use crate::atomic_file::io_error;
use crate::persistence::{hex, unhex};
use crate::sticky::Rgb;
use crate::widget_grid::{self, GridMetrics, Placement, Span};

pub const MAX_WIDGETS: usize = 8;
pub const MIN_WIDGET_SIZE: i32 = 64;
pub const STATE_FILE_NAME: &str = "desktop-widgets.state";
pub const CASCADE_STEP: i32 = 28;
/// Per-card keys that override the global surface while `inheritSurface` is off.
pub const SURFACE_OVERRIDE_KEYS: [&str; 3] = ["background", "backgroundOpacity", "backdropBlur"];

pub const DEFAULT_CUSTOM_TEMPLATE: &str = "It's <aqua>%weekday%</aqua>\n<bold>Hope your day is going well, %username%!</bold>\n<gray>%date% · %time%</gray>\n<white><red_background> CPU %cpu% </red_background> <blue_background> RAM %memory% </blue_background>";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaletteColor {
    pub name: &'static str,
    pub rgb: Rgb,
}

pub const PALETTE: [PaletteColor; 16] = [
    PaletteColor {
        name: "White",
        rgb: Rgb::new(0xf4, 0xf6, 0xfb),
    },
    PaletteColor {
        name: "Silver",
        rgb: Rgb::new(0xc3, 0xc8, 0xd4),
    },
    PaletteColor {
        name: "Slate",
        rgb: Rgb::new(0x5a, 0x61, 0x78),
    },
    PaletteColor {
        name: "Night",
        rgb: Rgb::new(0x1e, 0x20, 0x30),
    },
    PaletteColor {
        name: "Black",
        rgb: Rgb::new(0x0b, 0x0c, 0x10),
    },
    PaletteColor {
        name: "Red",
        rgb: Rgb::new(0xff, 0x6b, 0x6b),
    },
    PaletteColor {
        name: "Coral",
        rgb: Rgb::new(0xff, 0x8a, 0x7a),
    },
    PaletteColor {
        name: "Orange",
        rgb: Rgb::new(0xff, 0xa9, 0x4d),
    },
    PaletteColor {
        name: "Yellow",
        rgb: Rgb::new(0xff, 0xd8, 0x5c),
    },
    PaletteColor {
        name: "Green",
        rgb: Rgb::new(0x7c, 0xdb, 0x8a),
    },
    PaletteColor {
        name: "Teal",
        rgb: Rgb::new(0x3d, 0xd6, 0xc0),
    },
    PaletteColor {
        name: "Sky",
        rgb: Rgb::new(0x8c, 0xc8, 0xff),
    },
    PaletteColor {
        name: "Blue",
        rgb: Rgb::new(0x6f, 0x8c, 0xff),
    },
    PaletteColor {
        name: "Indigo",
        rgb: Rgb::new(0x8b, 0x7c, 0xf6),
    },
    PaletteColor {
        name: "Violet",
        rgb: Rgb::new(0xc0, 0x84, 0xfc),
    },
    PaletteColor {
        name: "Pink",
        rgb: Rgb::new(0xff, 0x8f, 0xc7),
    },
];

const WHITE: i64 = 0;
const SLATE: i64 = 2;
const NIGHT: i64 = 3;
const ORANGE: i64 = 7;
const SKY: i64 = 11;
const BLUE: i64 = 12;

#[must_use]
pub fn palette_color(index: i64) -> Rgb {
    let len = PALETTE.len() as i64;
    PALETTE[index.rem_euclid(len) as usize].rgb
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WidgetKind {
    Clock,
    Cpu,
    Memory,
    Media,
    Custom,
    Weather,
    Calendar,
    History,
    Network,
    Ring,
}

impl WidgetKind {
    pub const ALL: [Self; 10] = [
        Self::Clock,
        Self::Cpu,
        Self::Memory,
        Self::Media,
        Self::Custom,
        Self::Weather,
        Self::Calendar,
        Self::History,
        Self::Network,
        Self::Ring,
    ];

    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Clock => "clock",
            Self::Cpu => "cpu",
            Self::Memory => "memory",
            Self::Media => "media",
            Self::Custom => "custom",
            Self::Weather => "weather",
            Self::Calendar => "calendar",
            Self::History => "history",
            Self::Network => "network",
            Self::Ring => "ring",
        }
    }

    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.id() == id)
    }

    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Clock => "Time and date",
            Self::Cpu => "CPU usage",
            Self::Memory => "Memory usage",
            Self::Media => "Media player",
            Self::Custom => "Custom text",
            Self::Weather => "Weather",
            Self::Calendar => "Calendar month",
            Self::History => "CPU and RAM graph",
            Self::Network => "Network traffic",
            Self::Ring => "Display ring",
        }
    }

    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Clock => "Clock with weekday and date, side by side or stacked.",
            Self::Cpu => "Processor load as a circular gauge.",
            Self::Memory => "RAM in use as a circular gauge.",
            Self::Media => "Now playing, with previous, play and next.",
            Self::Custom => "Your own text with %placeholders% and <color> tags.",
            Self::Weather => "City, current temperature and the next six forecast days.",
            Self::Calendar => "Current month with today marked.",
            Self::History => "Rolling CPU and memory history as a sparkline.",
            Self::Network => "Download and upload rates as a sparkline.",
            Self::Ring => "Circular gauge for CPU or RAM, with a percentage.",
        }
    }

    #[must_use]
    pub const fn default_size(self) -> (i32, i32) {
        match self {
            Self::Clock => (340, 150),
            Self::Cpu | Self::Memory | Self::Ring => (220, 110),
            Self::Media => (320, 120),
            Self::Custom => (380, 170),
            Self::Weather => (340, 190),
            Self::Calendar => (200, 230),
            Self::History | Self::Network => (220, 110),
        }
    }

    #[must_use]
    pub const fn ticks_every_second(self) -> bool {
        matches!(
            self,
            Self::Cpu | Self::Memory | Self::Ring | Self::History | Self::Network | Self::Custom
        )
    }

    #[must_use]
    pub fn options(self) -> Vec<OptionSpec> {
        let mut options = match self {
            Self::Clock => vec![
                OptionSpec::choice("style", "Style", &["Digital", "Analog"], 0),
                OptionSpec::toggle("showTime", "Show time", true),
                OptionSpec::toggle("showSeconds", "Show seconds", false),
                OptionSpec::toggle("use24Hour", "24-hour clock", true),
                OptionSpec::toggle("showDate", "Show date", true),
                OptionSpec::toggle("showWeekday", "Show weekday", true),
                OptionSpec::toggle("showYear", "Show year", false),
                OptionSpec::toggle("sideBySide", "Time and date side by side", false),
                OptionSpec::step("timeSize", "Time size", 16, 120, 4, 52, "px"),
                OptionSpec::step("dateSize", "Date size", 8, 48, 2, 16, "px"),
                OptionSpec::toggle("bold", "Bold time", true),
                OptionSpec::color("textColor", "Time color", WHITE),
                OptionSpec::color("accentColor", "Date color", SKY),
                OptionSpec::choice("align", "Alignment", &ALIGNMENTS, 1),
            ],
            Self::Cpu | Self::Memory | Self::Ring => vec![
                OptionSpec::choice("metric", "Metric", &["CPU", "Memory"], 0),
                OptionSpec::toggle("showTitle", "Show title", true),
                OptionSpec::toggle("showPercent", "Show percentage", true),
                OptionSpec::toggle("showDetails", "Show details", true),
                OptionSpec::toggle("sideBySide", "Gauge beside text", true),
                OptionSpec::step("thickness", "Ring thickness", 4, 24, 2, 10, "px"),
                OptionSpec::color(
                    "ringColor",
                    "Ring color",
                    if self == Self::Cpu { SKY } else { ORANGE },
                ),
                OptionSpec::color("trackColor", "Track color", SLATE),
                OptionSpec::color("textColor", "Text color", WHITE),
            ],
            Self::Media => vec![
                OptionSpec::toggle("showArtist", "Show artist", true),
                OptionSpec::toggle("showPlayer", "Show player name", true),
                OptionSpec::toggle("showControls", "Show controls", true),
                OptionSpec::color("textColor", "Text color", WHITE),
                OptionSpec::color("accentColor", "Accent color", SKY),
            ],
            Self::Custom => vec![
                OptionSpec::step("textSize", "Text size", 8, 72, 2, 18, "px"),
                OptionSpec::toggle("bold", "Bold", false),
                OptionSpec::color("textColor", "Default color", WHITE),
                OptionSpec::choice("align", "Alignment", &ALIGNMENTS, 0),
            ],
            Self::Weather => vec![
                OptionSpec::toggle("showHourly", "Show forecast row", true),
                OptionSpec::color("textColor", "Text color", WHITE),
                OptionSpec::color("accentColor", "Accent color", SKY),
            ],
            Self::History => vec![
                OptionSpec::toggle("showCpu", "Show CPU line", true),
                OptionSpec::toggle("showMemory", "Show RAM line", true),
                OptionSpec::color("cpuColor", "CPU line color", SKY),
                OptionSpec::color("memoryColor", "RAM line color", ORANGE),
            ],
            Self::Network => vec![
                OptionSpec::toggle("showRx", "Show download line", true),
                OptionSpec::toggle("showTx", "Show upload line", true),
                OptionSpec::color("rxColor", "Download color", SKY),
                OptionSpec::color("txColor", "Upload color", ORANGE),
            ],
            Self::Calendar => vec![
                OptionSpec::toggle("showMonth", "Show month name", true),
                OptionSpec::color("textColor", "Text color", WHITE),
                OptionSpec::color("accentColor", "Today marker color", WHITE),
            ],
        };
        options.extend([
            OptionSpec::toggle("inheritSurface", "Use global surface", true),
            OptionSpec::color(
                "background",
                "Background",
                if self == Self::Clock { BLUE } else { NIGHT },
            ),
            OptionSpec::step(
                "backgroundOpacity",
                "Background opacity",
                0,
                100,
                10,
                if self == Self::Clock { 0 } else { 70 },
                "%",
            ),
            OptionSpec::step("radius", "Corner radius", 0, 40, 2, 18, "px"),
            OptionSpec::step("backdropBlur", "Backdrop blur", 0, 100, 10, 0, "%"),
        ]);
        options
    }
}

/// Sunday=0..Saturday=6 for a Gregorian date (Sakamoto's method).
#[must_use]
pub fn weekday_sunday_first(year: i32, month: u32, day: u32) -> u32 {
    const OFFSETS: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if month < 3 { year - 1 } else { year };
    let m = month.clamp(1, 12) as usize;
    (y + y / 4 - y / 100 + y / 400 + OFFSETS[m - 1] + day as i32).rem_euclid(7) as u32
}

#[must_use]
pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Six Sunday-first weeks of `month`; `0` marks a cell outside the month.
#[must_use]
pub fn calendar_weeks(year: i32, month: u32) -> [[u32; 7]; 6] {
    let mut weeks = [[0_u32; 7]; 6];
    let first = weekday_sunday_first(year, month, 1) as usize;
    let total = days_in_month(year, month);
    for day in 1..=total {
        let cell = first + day as usize - 1;
        weeks[cell / 7][cell % 7] = day;
    }
    weeks
}

/// Hour labels for the forecast row, starting at `hour` (0..=23), 12-hour style.
#[must_use]
pub fn hour_labels(hour: u32) -> [String; 6] {
    std::array::from_fn(|index| {
        let h = (hour + index as u32) % 24;
        let (display, suffix) = match h {
            0 => (12, "AM"),
            1..=11 => (h, "AM"),
            12 => (12, "PM"),
            _ => (h - 12, "PM"),
        };
        format!("{display} {suffix}")
    })
}

pub const ALIGNMENTS: [&str; 3] = ["Left", "Center", "Right"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

impl TextAlign {
    #[must_use]
    pub const fn from_index(index: i64) -> Self {
        match index {
            1 => Self::Center,
            2 => Self::Right,
            _ => Self::Left,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionControl {
    Toggle,
    Step {
        minimum: i64,
        maximum: i64,
        step: i64,
        unit: &'static str,
    },
    Color,
    Choice(&'static [&'static str]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OptionSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub control: OptionControl,
    pub default: i64,
}

impl OptionSpec {
    const fn toggle(key: &'static str, label: &'static str, default: bool) -> Self {
        Self {
            key,
            label,
            control: OptionControl::Toggle,
            default: default as i64,
        }
    }

    const fn step(
        key: &'static str,
        label: &'static str,
        minimum: i64,
        maximum: i64,
        step: i64,
        default: i64,
        unit: &'static str,
    ) -> Self {
        Self {
            key,
            label,
            control: OptionControl::Step {
                minimum,
                maximum,
                step,
                unit,
            },
            default,
        }
    }

    const fn color(key: &'static str, label: &'static str, default: i64) -> Self {
        Self {
            key,
            label,
            control: OptionControl::Color,
            default,
        }
    }

    const fn choice(
        key: &'static str,
        label: &'static str,
        choices: &'static [&'static str],
        default: i64,
    ) -> Self {
        Self {
            key,
            label,
            control: OptionControl::Choice(choices),
            default,
        }
    }

    #[must_use]
    pub fn normalize(&self, value: i64) -> i64 {
        match self.control {
            OptionControl::Toggle => i64::from(value != 0),
            OptionControl::Step {
                minimum, maximum, ..
            } => value.clamp(minimum, maximum),
            OptionControl::Color => value.rem_euclid(PALETTE.len() as i64),
            OptionControl::Choice(choices) => value.rem_euclid(choices.len().max(1) as i64),
        }
    }

    #[must_use]
    pub fn stepped(&self, value: i64, direction: i64) -> i64 {
        let direction = direction.signum();
        match self.control {
            OptionControl::Toggle => i64::from(value == 0),
            OptionControl::Step { step, .. } => self.normalize(value + direction * step),
            OptionControl::Color | OptionControl::Choice(_) => self.normalize(value + direction),
        }
    }

    #[must_use]
    pub fn label_for(&self, value: i64) -> String {
        match self.control {
            OptionControl::Toggle => if value != 0 { "On" } else { "Off" }.to_owned(),
            OptionControl::Step { unit, .. } => format!("{value} {unit}"),
            OptionControl::Color => PALETTE[self.normalize(value) as usize].name.to_owned(),
            OptionControl::Choice(choices) => choices
                .get(self.normalize(value) as usize)
                .copied()
                .unwrap_or_default()
                .to_owned(),
        }
    }
}

/// Horizontal stick to the nearest screen side: `gap` is the distance from the
/// left edge (`right == false`) or the right edge (`right == true`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdgeAnchor {
    pub right: bool,
    pub gap: i32,
}

impl EdgeAnchor {
    #[must_use]
    pub fn of(rect: Rect, bounds: Rect) -> Self {
        let right = rect.x + rect.width / 2 > bounds.x + bounds.width / 2;
        let gap = if right {
            bounds.right() - rect.right()
        } else {
            rect.x - bounds.x
        };
        Self { right, gap }
    }

    #[must_use]
    fn x_in(self, width: i32, bounds: Rect) -> i32 {
        if self.right {
            bounds.right() - self.gap - width
        } else {
            bounds.x + self.gap
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Widget {
    pub id: u32,
    pub kind: WidgetKind,
    pub rect: Rect,
    pub locked: bool,
    pub template: String,
    /// `None` until resolved against screen bounds (new or legacy-absolute widgets).
    anchor: Option<EdgeAnchor>,
    /// Grid cell and span. `None` for legacy free-position widgets not yet migrated.
    pub grid: Option<Placement>,
    values: BTreeMap<String, i64>,
}

impl Widget {
    #[must_use]
    pub fn new(id: u32, kind: WidgetKind, origin: Point) -> Self {
        let (width, height) = kind.default_size();
        Self {
            id,
            kind,
            rect: Rect::new(origin.x, origin.y, width, height),
            locked: false,
            template: if kind == WidgetKind::Custom {
                DEFAULT_CUSTOM_TEMPLATE.to_owned()
            } else {
                String::new()
            },
            anchor: None,
            grid: None,
            values: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn spec(&self, key: &str) -> Option<OptionSpec> {
        self.kind.options().into_iter().find(|spec| spec.key == key)
    }

    #[must_use]
    pub fn value(&self, key: &str) -> i64 {
        let Some(spec) = self.spec(key) else {
            return 0;
        };
        self.values
            .get(key)
            .map_or(spec.default, |value| spec.normalize(*value))
    }

    #[must_use]
    pub fn flag(&self, key: &str) -> bool {
        self.value(key) != 0
    }

    #[must_use]
    pub fn color(&self, key: &str) -> Rgb {
        palette_color(self.value(key))
    }

    #[must_use]
    pub fn align(&self) -> TextAlign {
        TextAlign::from_index(self.value("align"))
    }

    /// True when the card takes colour, opacity and blur from the global surface settings.
    /// An explicit `inheritSurface` wins; otherwise a card saved with any per-card
    /// background, opacity or blur override keeps those overrides.
    #[must_use]
    pub fn inherits_surface(&self) -> bool {
        match self.values.get("inheritSurface") {
            Some(value) => *value != 0,
            None => !SURFACE_OVERRIDE_KEYS
                .iter()
                .any(|key| self.values.contains_key(*key)),
        }
    }

    pub fn set_value(&mut self, key: &str, value: i64) -> bool {
        let Some(spec) = self.spec(key) else {
            return false;
        };
        let value = spec.normalize(value);
        if key == "inheritSurface" {
            // Stored even at its default, so a card with overrides never reads as inheriting.
            self.values.insert(key.to_owned(), value);
        } else if value == spec.default {
            self.values.remove(key);
        } else {
            self.values.insert(key.to_owned(), value);
        }
        true
    }

    pub fn step_value(&mut self, key: &str, direction: i64) -> bool {
        let Some(spec) = self.spec(key) else {
            return false;
        };
        let next = spec.stepped(self.value(key), direction);
        self.set_value(key, next)
    }

    #[must_use]
    pub fn stored_values(&self) -> &BTreeMap<String, i64> {
        &self.values
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WidgetStore {
    widgets: Vec<Widget>,
}

impl WidgetStore {
    #[must_use]
    pub fn widgets(&self) -> &[Widget] {
        &self.widgets
    }

    #[must_use]
    pub fn is_full(&self) -> bool {
        self.widgets.len() >= MAX_WIDGETS
    }

    #[must_use]
    pub fn get(&self, id: u32) -> Option<&Widget> {
        self.widgets.iter().find(|widget| widget.id == id)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut Widget> {
        self.widgets.iter_mut().find(|widget| widget.id == id)
    }

    #[must_use]
    pub fn count_of(&self, kind: WidgetKind) -> usize {
        self.widgets
            .iter()
            .filter(|widget| widget.kind == kind)
            .count()
    }

    pub fn add(&mut self, kind: WidgetKind, bounds: Rect) -> Option<u32> {
        self.add_on_grid(kind, GridMetrics::for_work_area(bounds, 100))
    }

    /// Adds a widget in the first free cell of `metrics` (row-major, left side).
    /// Returns `None` when the store is full or no cell fits the default span.
    pub fn add_on_grid(&mut self, kind: WidgetKind, metrics: GridMetrics) -> Option<u32> {
        if self.is_full() {
            return None;
        }
        self.layout_grid(metrics);
        let placement = metrics.first_free(widget_grid::default_span(kind), &self.placements())?;
        let id = self
            .widgets
            .iter()
            .map(|widget| widget.id)
            .max()
            .map_or(1, |id| id.saturating_add(1));
        let mut widget = Widget::new(id, kind, Point::new(0, 0));
        widget.grid = Some(placement);
        widget.rect = metrics.rect_of(placement);
        widget.anchor = Some(EdgeAnchor::of(widget.rect, metrics.work));
        self.widgets.push(widget);
        Some(id)
    }

    /// Grid placements of every grid-placed widget.
    #[must_use]
    pub fn placements(&self) -> Vec<Placement> {
        self.widgets.iter().filter_map(|widget| widget.grid).collect()
    }

    /// Moves a grid widget to `placement` if it fits and collides with no other widget.
    pub fn place_on_grid(&mut self, id: u32, placement: Placement, metrics: GridMetrics) -> bool {
        if !placement.fits(metrics) {
            return false;
        }
        let others: Vec<Placement> = self
            .widgets
            .iter()
            .filter(|widget| widget.id != id)
            .filter_map(|widget| widget.grid)
            .collect();
        if others.iter().any(|other| metrics.collides(placement, *other)) {
            return false;
        }
        let Some(widget) = self.get_mut(id) else {
            return false;
        };
        if widget.locked {
            return false;
        }
        widget.grid = Some(placement);
        widget.rect = metrics.rect_of(placement);
        widget.anchor = Some(EdgeAnchor::of(widget.rect, metrics.work));
        true
    }

    /// Fresh-profile store: the grid's default arrangement (`widget_grid::default_layout`).
    #[must_use]
    pub fn with_default_layout(metrics: GridMetrics) -> Self {
        let mut store = Self::default();
        for (kind, slot) in widget_grid::default_layout(metrics) {
            let Some(placement) = slot else {
                continue;
            };
            let mut widget = Widget::new(store.widgets.len() as u32 + 1, kind, Point::new(0, 0));
            widget.grid = Some(placement);
            widget.rect = metrics.rect_of(placement);
            widget.anchor = Some(EdgeAnchor::of(widget.rect, metrics.work));
            store.widgets.push(widget);
        }
        store
    }

    /// Drops a grid widget whose top-left is `origin`: snaps to the nearest cell on the
    /// side of the drop point, or the nearest free cell when that one is taken.
    pub fn drop_on_grid(&mut self, id: u32, origin: Point, metrics: GridMetrics) -> bool {
        match self.preview_drop(id, origin, metrics) {
            Some(slot) => self.place_on_grid(id, slot, metrics),
            None => false,
        }
    }

    /// Cell a drop at `origin` would take (the drag ghost). Equals the snapped cell when it
    /// is free, otherwise the nearest free cell (collision push). `None` when nothing fits.
    #[must_use]
    pub fn preview_drop(&self, id: u32, origin: Point, metrics: GridMetrics) -> Option<Placement> {
        let span = self.get(id).and_then(|widget| widget.grid)?.span;
        let width = metrics.span_px(span).0;
        let right = origin.x + width / 2 > metrics.work.x + metrics.work.width / 2;
        let want = metrics.snap(origin, span, right);
        metrics.nearest_free(want, &self.placements_except(id))
    }

    /// Cell a resize to `span` would take, keeping the current cell when it is free.
    #[must_use]
    pub fn preview_resize(&self, id: u32, span: Span, metrics: GridMetrics) -> Option<Placement> {
        let current = self.get(id).and_then(|widget| widget.grid)?;
        let want = Placement { span, ..current };
        metrics.nearest_free(want, &self.placements_except(id))
    }

    fn placements_except(&self, id: u32) -> Vec<Placement> {
        self.widgets
            .iter()
            .filter(|widget| widget.id != id)
            .filter_map(|widget| widget.grid)
            .collect()
    }

    /// Changes a grid widget's span, keeping its cell when it still fits and is free,
    /// otherwise moving it to the nearest free cell for the new span.
    pub fn resize_on_grid(&mut self, id: u32, span: Span, metrics: GridMetrics) -> bool {
        let Some(current) = self.get(id).and_then(|widget| widget.grid) else {
            return false;
        };
        let want = Placement { span, ..current };
        if self.place_on_grid(id, want, metrics) {
            return true;
        }
        let others: Vec<Placement> = self
            .widgets
            .iter()
            .filter(|widget| widget.id != id)
            .filter_map(|widget| widget.grid)
            .collect();
        match metrics.nearest_free(want, &others) {
            Some(slot) => self.place_on_grid(id, slot, metrics),
            None => false,
        }
    }

    pub fn remove(&mut self, id: u32) -> bool {
        let before = self.widgets.len();
        self.widgets.retain(|widget| widget.id != id);
        before != self.widgets.len()
    }

    pub fn set_rect(&mut self, id: u32, rect: Rect) -> bool {
        if !rect.is_valid() {
            return false;
        }
        let Some(widget) = self.get_mut(id) else {
            return false;
        };
        if widget.locked || widget.rect == rect {
            return false;
        }
        widget.rect = rect;
        true
    }

    pub fn toggle_lock(&mut self, id: u32) -> Option<bool> {
        let widget = self.get_mut(id)?;
        widget.locked = !widget.locked;
        Some(widget.locked)
    }

    /// Re-records the horizontal anchor of a widget after a user move or resize.
    pub fn anchor_to(&mut self, id: u32, bounds: Rect) {
        if let Some(widget) = self.get_mut(id) {
            widget.anchor = Some(EdgeAnchor::of(widget.rect, bounds));
        }
    }

    /// Places every widget from its edge anchor for `bounds`, so a right-side
    /// widget keeps its distance from the right edge when the screen changes.
    /// Unresolved (legacy absolute) anchors pick the nearest side here.
    pub fn clamp_all(&mut self, bounds: Rect) {
        self.layout_grid(GridMetrics::for_work_area(bounds, 100));
    }

    /// Lays every widget out on `metrics`. Grid widgets reflow to their cells (keeping
    /// their side); legacy free-position widgets snap to the nearest free cell. A legacy
    /// widget with no free cell keeps its clamped pixel rect (non-destructive).
    pub fn layout_grid(&mut self, metrics: GridMetrics) {
        let bounds = metrics.work;
        let placed_items: Vec<Placement> = self.placements();
        let mut slots = metrics.reflow(&placed_items).into_iter();
        for widget in &mut self.widgets {
            if widget.grid.is_some() {
                widget.grid = slots.next().flatten();
            }
        }
        let mut placed = self.placements();
        for widget in &mut self.widgets {
            if widget.grid.is_none() {
                let span = widget_grid::default_span(widget.kind);
                if let Some(slot) = metrics.migrate(widget.rect, span, &placed) {
                    placed.push(slot);
                    widget.grid = Some(slot);
                }
            }
        }
        for widget in &mut self.widgets {
            widget.rect = match widget.grid {
                Some(placement) => metrics.rect_of(placement),
                None => {
                    let x = widget
                        .anchor
                        .map_or(widget.rect.x, |anchor| anchor.x_in(widget.rect.width, bounds));
                    Rect::new(x, widget.rect.y, widget.rect.width, widget.rect.height)
                        .clamp_inside(bounds)
                }
            };
            widget.anchor = Some(EdgeAnchor::of(widget.rect, bounds));
        }
    }
}

#[must_use]
pub fn move_widget_rect(original: Rect, dx: i32, dy: i32, bounds: Rect) -> Rect {
    Rect::new(
        original.x.saturating_add(dx),
        original.y.saturating_add(dy),
        original.width,
        original.height,
    )
    .clamp_inside(bounds)
}

#[must_use]
pub fn resize_widget_rect(original: Rect, dx: i32, dy: i32, bounds: Rect) -> Rect {
    let max_width = (bounds.right() - original.x).max(MIN_WIDGET_SIZE);
    let max_height = (bounds.bottom() - original.y).max(MIN_WIDGET_SIZE);
    Rect::new(
        original.x,
        original.y,
        original
            .width
            .saturating_add(dx)
            .clamp(MIN_WIDGET_SIZE, max_width),
        original
            .height
            .saturating_add(dy)
            .clamp(MIN_WIDGET_SIZE, max_height),
    )
}

pub fn state_path() -> FlameResult<PathBuf> {
    let base = match env::var_os("XDG_STATE_HOME") {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => PathBuf::from(
            env::var_os("HOME").ok_or_else(|| FlameError::invalid("HOME is not set"))?,
        )
        .join(".local/state"),
    };
    Ok(base.join(STATE_FILE_NAME))
}

pub fn load(path: &Path) -> FlameResult<WidgetStore> {
    match fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(WidgetStore::default()),
        Err(error) => Err(io_error("read desktop widgets", error)),
    }
}

pub fn persist(path: &Path, store: &WidgetStore) -> FlameResult<()> {
    crate::atomic_file::write_text(path, "desktop widgets", &serialize(store))
}

#[must_use]
pub fn serialize(store: &WidgetStore) -> String {
    let mut output = String::from("v1\n");
    for widget in &store.widgets {
        let values = widget
            .values
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(",");
        let anchor = widget.anchor.map_or_else(String::new, |anchor| {
            format!("{}{}", if anchor.right { 'r' } else { 'l' }, anchor.gap)
        });
        let grid = widget.grid.map_or_else(String::new, |cell| {
            format!(
                "{},{},{},{},{}",
                cell.col,
                cell.row,
                cell.span.w,
                cell.span.h,
                if cell.right { 'r' } else { 'l' }
            )
        });
        output.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            widget.id,
            widget.kind.id(),
            widget.rect.x,
            widget.rect.y,
            widget.rect.width,
            widget.rect.height,
            i32::from(widget.locked),
            hex(widget.template.as_bytes()),
            anchor,
            values,
            grid
        ));
    }
    output
}

pub fn parse(text: &str) -> FlameResult<WidgetStore> {
    let mut lines = text.lines();
    if lines.next() != Some("v1") {
        return Err(FlameError::invalid("invalid desktop widget state header"));
    }
    let mut store = WidgetStore::default();
    for line in lines.filter(|line| !line.is_empty()) {
        if store.is_full() {
            break;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        // 9 fields: legacy record without an edge anchor (resolved on first clamp).
        // 11 fields: current record with a trailing grid cell (absent in older records).
        let (anchor_field, values_field, grid_field) = match fields.len() {
            9 => ("", fields[8], ""),
            10 => (fields[8], fields[9], ""),
            11 => (fields[8], fields[9], fields[10]),
            _ => return Err(FlameError::invalid("invalid desktop widget record")),
        };
        let grid = parse_grid(grid_field)?;
        let anchor = match anchor_field.chars().next() {
            None => None,
            Some(side @ ('l' | 'r')) => Some(EdgeAnchor {
                right: side == 'r',
                gap: number(&anchor_field[1..])?,
            }),
            Some(_) => return Err(FlameError::invalid("invalid desktop widget anchor")),
        };
        let Some(kind) = WidgetKind::from_id(fields[1]) else {
            continue;
        };
        let id = number::<u32>(fields[0])?;
        if store.get(id).is_some() {
            return Err(FlameError::invalid("duplicate desktop widget id"));
        }
        let rect = Rect::new(
            number(fields[2])?,
            number(fields[3])?,
            number::<i32>(fields[4])?.max(MIN_WIDGET_SIZE),
            number::<i32>(fields[5])?.max(MIN_WIDGET_SIZE),
        );
        let locked = match fields[6] {
            "0" => false,
            "1" => true,
            _ => return Err(FlameError::invalid("invalid desktop widget lock flag")),
        };
        let template = String::from_utf8(unhex(
            fields[7],
            "odd desktop widget template",
            "invalid desktop widget template",
        )?)
        .map_err(|_| FlameError::invalid("desktop widget template is not UTF-8"))?;
        let mut widget = Widget::new(id, kind, rect.origin());
        widget.rect = rect;
        widget.locked = locked;
        widget.template = template;
        widget.anchor = anchor;
        widget.grid = grid;
        for pair in values_field.split(',').filter(|pair| !pair.is_empty()) {
            let (key, value) = pair
                .split_once('=')
                .ok_or_else(|| FlameError::invalid("invalid desktop widget option"))?;
            widget.set_value(key, number(value)?);
        }
        store.widgets.push(widget);
    }
    Ok(store)
}

/// Parses `col,row,w,h,side` (side `l` or `r`); an empty field is no grid cell.
fn parse_grid(field: &str) -> FlameResult<Option<Placement>> {
    if field.is_empty() {
        return Ok(None);
    }
    let parts: Vec<&str> = field.split(',').collect();
    if parts.len() != 5 {
        return Err(FlameError::invalid("invalid desktop widget grid cell"));
    }
    let right = match parts[4] {
        "l" => false,
        "r" => true,
        _ => return Err(FlameError::invalid("invalid desktop widget grid side")),
    };
    Ok(Some(Placement {
        col: number(parts[0])?,
        row: number(parts[1])?,
        span: Span {
            w: number(parts[2])?,
            h: number(parts[3])?,
        },
        right,
    }))
}

fn number<T: std::str::FromStr>(value: &str) -> FlameResult<T> {
    value
        .parse()
        .map_err(|_| FlameError::invalid("invalid desktop widget number"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds() -> Rect {
        Rect::new(0, 0, 1920, 1040)
    }

    #[test]
    fn every_kind_has_unique_option_keys_and_valid_defaults() {
        for kind in WidgetKind::ALL {
            let options = kind.options();
            let mut keys: Vec<_> = options.iter().map(|spec| spec.key).collect();
            keys.sort_unstable();
            keys.dedup();
            assert_eq!(keys.len(), options.len(), "{kind:?}");
            for spec in options {
                assert_eq!(spec.normalize(spec.default), spec.default, "{}", spec.key);
            }
            assert_eq!(WidgetKind::from_id(kind.id()), Some(kind));
        }
    }

    #[test]
    fn stepping_follows_each_control() {
        let mut widget = Widget::new(1, WidgetKind::Clock, Point::new(0, 0));
        assert!(!widget.flag("showSeconds"));
        widget.step_value("showSeconds", -1);
        assert!(widget.flag("showSeconds"));
        widget.set_value("timeSize", 118);
        widget.step_value("timeSize", 1);
        assert_eq!(widget.value("timeSize"), 120);
        widget.set_value("align", 2);
        widget.step_value("align", 1);
        assert_eq!(widget.align(), TextAlign::Left);
        widget.set_value("textColor", 0);
        widget.step_value("textColor", -1);
        assert_eq!(widget.value("textColor"), PALETTE.len() as i64 - 1);
        assert!(!widget.step_value("missing", 1));
    }

    #[test]
    fn default_values_are_not_stored() {
        let mut widget = Widget::new(1, WidgetKind::Cpu, Point::new(0, 0));
        widget.step_value("showTitle", 1);
        assert_eq!(widget.stored_values().len(), 1);
        widget.step_value("showTitle", 1);
        assert!(widget.stored_values().is_empty());
    }

    #[test]
    fn store_caps_widgets_and_places_new_ones_in_free_cells() {
        let mut store = WidgetStore::default();
        let first = store.add(WidgetKind::Clock, bounds()).expect("first");
        let second = store.add(WidgetKind::Clock, bounds()).expect("second");
        assert_ne!(first, second);
        let a = store.get(first).expect("a").rect;
        let b = store.get(second).expect("b").rect;
        assert_ne!(a.origin(), b.origin());
        assert!(!a.intersects(b));
        while store.add(WidgetKind::Cpu, bounds()).is_some() {}
        assert_eq!(store.widgets().len(), MAX_WIDGETS);
        assert!(store.remove(first));
        assert_eq!(
            store.add(WidgetKind::Media, bounds()),
            Some(MAX_WIDGETS as u32 + 1)
        );
    }

    #[test]
    fn locked_widgets_do_not_move() {
        let mut store = WidgetStore::default();
        let id = store.add(WidgetKind::Memory, bounds()).expect("id");
        assert_eq!(store.toggle_lock(id), Some(true));
        assert!(!store.set_rect(id, Rect::new(5, 5, 200, 100)));
        assert_eq!(store.toggle_lock(id), Some(false));
        assert!(store.set_rect(id, Rect::new(5, 5, 200, 100)));
    }

    #[test]
    fn move_and_resize_stay_inside_bounds_and_above_the_minimum() {
        let area = Rect::new(0, 0, 500, 400);
        let moved = move_widget_rect(Rect::new(100, 100, 200, 100), 1000, -1000, area);
        assert_eq!(moved, Rect::new(300, 0, 200, 100));
        let shrunk = resize_widget_rect(Rect::new(100, 100, 200, 100), -500, -500, area);
        assert_eq!(shrunk.size(), flamewm_api::Size::new(64, 64));
        let grown = resize_widget_rect(Rect::new(100, 100, 200, 100), 1000, 1000, area);
        assert_eq!(grown, Rect::new(100, 100, 400, 300));
    }

    #[test]
    fn state_round_trips_and_skips_unknown_kinds() {
        let mut store = WidgetStore::default();
        let clock = store.add(WidgetKind::Clock, bounds()).expect("clock");
        let custom = store.add(WidgetKind::Custom, bounds()).expect("custom");
        store
            .get_mut(clock)
            .expect("clock")
            .set_value("sideBySide", 1);
        store.get_mut(custom).expect("custom").template = "Hi\t%username%\n<red>x".into();
        store.toggle_lock(custom);
        let text = serialize(&store);
        assert_eq!(parse(&text).expect("parse"), store);
        let extra = format!("{text}9\tsparkle\t0\t0\t100\t100\t0\t\t\n");
        assert_eq!(parse(&extra).expect("unknown kind").widgets().len(), 2);
        assert!(parse("v9\n").is_err());
        assert!(parse("v1\n1\tclock\t0\t0\t10\t10\t2\t\t\n").is_err());
    }

    #[test]
    fn right_anchored_widget_keeps_its_gap_when_the_screen_shrinks() {
        let mut store = WidgetStore::default();
        let id = store.add(WidgetKind::Cpu, bounds()).expect("id");
        let metrics = GridMetrics::for_work_area(bounds(), 100);
        let right_cell = Placement {
            col: 0,
            row: 0,
            span: Span::SINGLE,
            right: true,
        };
        assert!(store.place_on_grid(id, right_cell, metrics));
        store.clamp_all(Rect::new(0, 0, 1350, 641));
        let widget = store.get(id).expect("widget");
        assert_eq!(widget.rect.right(), 1350 - 16);
        assert_eq!(widget.rect.y, 16);
        assert_eq!(
            widget.anchor,
            Some(EdgeAnchor {
                right: true,
                gap: 16,
            })
        );
    }

    #[test]
    fn left_anchored_widget_keeps_its_x_and_legacy_records_pick_the_nearest_side() {
        let mut store = WidgetStore::default();
        let id = store.add(WidgetKind::Clock, bounds()).expect("id");
        store.get_mut(id).expect("widget").rect = Rect::new(40, 30, 340, 150);
        store.anchor_to(id, bounds());
        store.clamp_all(Rect::new(0, 0, 1350, 641));
        // Grid cells are the layout: a left widget sits in the first left cell.
        assert_eq!(store.get(id).expect("widget").rect.x, 16);

        // Legacy 9-field record with an absolute x on the right half of the old screen.
        let legacy = "v1\n3\tcpu\t1500\t20\t220\t110\t0\t\tbackdropBlur=40\n";
        let mut loaded = parse(legacy).expect("legacy");
        loaded.clamp_all(Rect::new(0, 0, 1350, 641));
        let widget = loaded.get(3).expect("widget");
        assert_eq!(widget.rect.right(), 1350 - 16);
        assert_eq!(widget.value("backdropBlur"), 40);
        assert_eq!(
            serialize(&loaded).lines().nth(1).expect("record").split('\t').nth(8),
            Some("r16")
        );
    }

    #[test]
    fn anchored_widgets_stay_inside_the_screen() {
        let mut store = WidgetStore::default();
        let id = store.add(WidgetKind::Media, bounds()).expect("id");
        store.get_mut(id).expect("widget").rect = Rect::new(1600, 900, 320, 120);
        store.anchor_to(id, bounds());
        store.clamp_all(Rect::new(0, 0, 800, 600));
        let rect = store.get(id).expect("widget").rect;
        assert!(rect.x >= 0 && rect.right() <= 800, "{rect:?}");
        assert!(rect.y >= 0 && rect.bottom() <= 600, "{rect:?}");
        assert!(parse("v1\n1\tclock\t0\t0\t340\t150\t0\t\tz0\t\n").is_err());
    }

    #[test]
    fn clock_defaults_to_centered_alignment() {
        let clock = Widget::new(1, WidgetKind::Clock, Point::new(0, 0));
        assert_eq!(clock.align(), TextAlign::Center);
        let cpu = Widget::new(2, WidgetKind::Cpu, Point::new(0, 0));
        assert_eq!(cpu.align(), TextAlign::Left);
    }

    #[test]
    fn calendar_weeks_place_first_day_and_month_length() {
        // 2026-01-01 is a Thursday; January has 31 days.
        let jan = calendar_weeks(2026, 1);
        assert_eq!(jan[0], [0, 0, 0, 0, 1, 2, 3]);
        assert_eq!(jan[4], [25, 26, 27, 28, 29, 30, 31]);
        assert_eq!(jan[5], [0; 7]);
        // 2028 is a leap year: February has 29 days and starts on a Tuesday.
        let feb = calendar_weeks(2028, 2);
        assert_eq!(feb[0], [0, 0, 1, 2, 3, 4, 5]);
        assert_eq!(days_in_month(2028, 2), 29);
        assert_eq!(days_in_month(2100, 2), 28);
        assert_eq!(days_in_month(2000, 2), 29);
        let total: u32 = calendar_weeks(2026, 10)
            .iter()
            .flatten()
            .filter(|day| **day != 0)
            .count() as u32;
        assert_eq!(total, 31);
    }

    #[test]
    fn hour_labels_wrap_midnight_and_use_twelve_hour_style() {
        assert_eq!(
            hour_labels(9),
            ["9 AM", "10 AM", "11 AM", "12 PM", "1 PM", "2 PM"]
        );
        assert_eq!(hour_labels(22)[2], "12 AM");
        assert_eq!(hour_labels(0)[0], "12 AM");
    }
}

#[cfg(test)]
mod surface_inheritance_tests {
    use super::*;

    fn card(kind: WidgetKind) -> Widget {
        Widget::new(1, kind, Point { x: 0, y: 0 })
    }

    #[test]
    fn new_cards_inherit_the_global_surface() {
        assert!(card(WidgetKind::Clock).inherits_surface());
        assert!(card(WidgetKind::Cpu).inherits_surface());
    }

    #[test]
    fn explicit_toggle_off_keeps_per_card_overrides_and_back_on_restores_inheritance() {
        let mut widget = card(WidgetKind::Clock);
        assert!(widget.set_value("inheritSurface", 0));
        assert!(!widget.inherits_surface());
        assert!(widget.set_value("backdropBlur", 40));
        assert_eq!(widget.value("backdropBlur"), 40);
        assert!(widget.set_value("inheritSurface", 1));
        assert!(widget.inherits_surface());
    }

    #[test]
    fn legacy_card_with_saved_background_override_does_not_inherit() {
        let mut widget = card(WidgetKind::Custom);
        assert!(widget.set_value("background", 5));
        assert!(!widget.inherits_surface());
        assert_eq!(widget.value("background"), 5);
    }

    #[test]
    fn override_key_round_trips_through_persist_format() {
        let mut widget = card(WidgetKind::Media);
        widget.set_value("inheritSurface", 0);
        assert!(widget.stored_values().contains_key("inheritSurface"));
        assert_eq!(widget.stored_values().get("inheritSurface"), Some(&0));
    }

    #[test]
    fn history_kind_exposes_line_toggles_and_fits_one_cell() {
        assert_eq!(WidgetKind::from_id("history"), Some(WidgetKind::History));
        let keys: Vec<_> = WidgetKind::History.options().iter().map(|spec| spec.key).collect();
        assert!(keys.contains(&"showCpu") && keys.contains(&"showMemory"));
        let widget = Widget::new(1, WidgetKind::History, Point::new(0, 0));
        assert!(widget.inherits_surface());
    }

    #[test]
    fn ring_kind_defaults_to_cpu_and_offers_memory() {
        assert_eq!(WidgetKind::from_id("ring"), Some(WidgetKind::Ring));
        let widget = Widget::new(1, WidgetKind::Ring, Point::new(0, 0));
        assert_eq!(widget.value("metric"), 0);
        assert!(widget.spec("metric").is_some());
        assert!(widget.inherits_surface());
    }
}

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use flamewm_api::{FlameError, FlameResult, Point, Rect};

use crate::atomic_file::io_error;
use crate::persistence::{hex, unhex};
use crate::sticky::Rgb;

pub const MAX_WIDGETS: usize = 8;
pub const MIN_WIDGET_SIZE: i32 = 64;
pub const STATE_FILE_NAME: &str = "desktop-widgets.state";
pub const CASCADE_STEP: i32 = 28;

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
}

impl WidgetKind {
    pub const ALL: [Self; 5] = [
        Self::Clock,
        Self::Cpu,
        Self::Memory,
        Self::Media,
        Self::Custom,
    ];

    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Clock => "clock",
            Self::Cpu => "cpu",
            Self::Memory => "memory",
            Self::Media => "media",
            Self::Custom => "custom",
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
        }
    }

    #[must_use]
    pub const fn default_size(self) -> (i32, i32) {
        match self {
            Self::Clock => (340, 150),
            Self::Cpu | Self::Memory => (220, 110),
            Self::Media => (320, 120),
            Self::Custom => (380, 170),
        }
    }

    #[must_use]
    pub const fn ticks_every_second(self) -> bool {
        matches!(self, Self::Cpu | Self::Memory | Self::Custom)
    }

    #[must_use]
    pub fn options(self) -> Vec<OptionSpec> {
        let mut options = match self {
            Self::Clock => vec![
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
            Self::Cpu | Self::Memory => vec![
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
        };
        options.extend([
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
            OptionSpec::step("radius", "Corner radius", 0, 40, 2, 16, "px"),
            OptionSpec::step("backdropBlur", "Backdrop blur", 0, 100, 10, 0, "%"),
        ]);
        options
    }
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

    pub fn set_value(&mut self, key: &str, value: i64) -> bool {
        let Some(spec) = self.spec(key) else {
            return false;
        };
        let value = spec.normalize(value);
        if value == spec.default {
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
        if self.is_full() {
            return None;
        }
        let id = self
            .widgets
            .iter()
            .map(|widget| widget.id)
            .max()
            .map_or(1, |id| id.saturating_add(1));
        let (width, height) = kind.default_size();
        let offset = CASCADE_STEP * self.widgets.len() as i32;
        let origin = Point::new(
            bounds.x + (bounds.width - width) / 2 + offset,
            bounds.y + (bounds.height - height) / 3 + offset,
        );
        let mut widget = Widget::new(id, kind, origin);
        widget.rect = widget.rect.clamp_inside(bounds);
        widget.anchor = Some(EdgeAnchor::of(widget.rect, bounds));
        self.widgets.push(widget);
        Some(id)
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
        for widget in &mut self.widgets {
            let anchor = *widget
                .anchor
                .get_or_insert_with(|| EdgeAnchor::of(widget.rect, bounds));
            let x = anchor.x_in(widget.rect.width, bounds);
            widget.rect = Rect::new(x, widget.rect.y, widget.rect.width, widget.rect.height)
                .clamp_inside(bounds);
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
        output.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            widget.id,
            widget.kind.id(),
            widget.rect.x,
            widget.rect.y,
            widget.rect.width,
            widget.rect.height,
            i32::from(widget.locked),
            hex(widget.template.as_bytes()),
            anchor,
            values
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
        let (anchor_field, values_field) = match fields.len() {
            9 => ("", fields[8]),
            10 => (fields[8], fields[9]),
            _ => return Err(FlameError::invalid("invalid desktop widget record")),
        };
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
    fn store_caps_widgets_and_cascades_new_ones() {
        let mut store = WidgetStore::default();
        let first = store.add(WidgetKind::Clock, bounds()).expect("first");
        let second = store.add(WidgetKind::Clock, bounds()).expect("second");
        assert_ne!(first, second);
        let a = store.get(first).expect("a").rect;
        let b = store.get(second).expect("b").rect;
        assert_eq!((b.x - a.x, b.y - a.y), (CASCADE_STEP, CASCADE_STEP));
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
        let extra = format!("{text}9\tweather\t0\t0\t100\t100\t0\t\t\n");
        assert_eq!(parse(&extra).expect("unknown kind").widgets().len(), 2);
        assert!(parse("v9\n").is_err());
        assert!(parse("v1\n1\tclock\t0\t0\t10\t10\t2\t\t\n").is_err());
    }

    #[test]
    fn right_anchored_widget_keeps_its_gap_when_the_screen_shrinks() {
        let mut store = WidgetStore::default();
        let id = store.add(WidgetKind::Cpu, bounds()).expect("id");
        store.get_mut(id).expect("widget").rect = Rect::new(1600, 50, 220, 110);
        store.anchor_to(id, bounds());
        assert_eq!(
            store.get(id).expect("widget").anchor,
            Some(EdgeAnchor {
                right: true,
                gap: 100,
            })
        );
        store.clamp_all(Rect::new(0, 0, 1350, 641));
        let rect = store.get(id).expect("widget").rect;
        assert_eq!((rect.x, rect.right()), (1030, 1250));
        assert_eq!(rect.y, 50);
    }

    #[test]
    fn left_anchored_widget_keeps_its_x_and_legacy_records_pick_the_nearest_side() {
        let mut store = WidgetStore::default();
        let id = store.add(WidgetKind::Clock, bounds()).expect("id");
        store.get_mut(id).expect("widget").rect = Rect::new(40, 30, 340, 150);
        store.anchor_to(id, bounds());
        store.clamp_all(Rect::new(0, 0, 1350, 641));
        assert_eq!(store.get(id).expect("widget").rect.x, 40);

        // Legacy 9-field record with an absolute x on the right half of the old screen.
        let legacy = "v1\n3\tcpu\t1500\t20\t220\t110\t0\t\tbackdropBlur=40\n";
        let mut loaded = parse(legacy).expect("legacy");
        loaded.clamp_all(Rect::new(0, 0, 1350, 641));
        let widget = loaded.get(3).expect("widget");
        assert_eq!(widget.rect.right(), 1350);
        assert_eq!(widget.value("backdropBlur"), 40);
        assert_eq!(
            serialize(&loaded).lines().nth(1).expect("record").split('\t').nth(8),
            Some("r0")
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
}

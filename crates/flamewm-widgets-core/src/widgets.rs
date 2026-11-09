use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use flamewm_api::{FlameError, FlameResult, Point, Rect};

use crate::widget_grid::{self, GridMetrics, Placement, Span};
use flamewm_desktop_core::atomic_file::io_error;
use flamewm_desktop_core::persistence::{hex, unhex};
use flamewm_desktop_core::sticky::Rgb;

mod kind;
pub use kind::*;
mod store;
pub use store::*;

pub const MAX_WIDGETS: usize = 8;
pub const MIN_WIDGET_SIZE: i32 = 64;
pub const STATE_FILE_NAME: &str = "desktop-widgets.state";
/// Per-card keys that override the global surface while `customSurface` is on.
pub const SURFACE_OVERRIDE_KEYS: [&str; 2] = ["background", "backgroundOpacity"];

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
#[cfg(test)]
const SKY: i64 = 11;
const BLUE: i64 = 12;
/// Colour option value meaning "follow the live Settings accent"; outside the palette range.
pub const ACCENT_COLOR: i64 = -1;

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
    /// Colour that also accepts `ACCENT_COLOR`, which follows the live Settings accent.
    AccentColor,
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
        let control = if default == ACCENT_COLOR {
            OptionControl::AccentColor
        } else {
            OptionControl::Color
        };
        Self {
            key,
            label,
            control,
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
            OptionControl::AccentColor if value == ACCENT_COLOR => value,
            OptionControl::Color | OptionControl::AccentColor => {
                value.rem_euclid(PALETTE.len() as i64)
            }
            OptionControl::Choice(choices) => value.rem_euclid(choices.len().max(1) as i64),
        }
    }

    #[must_use]
    pub fn stepped(&self, value: i64, direction: i64) -> i64 {
        let direction = direction.signum();
        match self.control {
            OptionControl::Toggle => i64::from(value == 0),
            OptionControl::Step { step, .. } => self.normalize(value + direction * step),
            OptionControl::AccentColor => {
                // Palette slots plus one accent slot; stepping wraps through all of them.
                let palette = PALETTE.len() as i64;
                let position = if value == ACCENT_COLOR {
                    palette
                } else {
                    value.rem_euclid(palette)
                };
                match (position + direction).rem_euclid(palette + 1) {
                    slot if slot == palette => ACCENT_COLOR,
                    slot => slot,
                }
            }
            OptionControl::Color | OptionControl::Choice(_) => self.normalize(value + direction),
        }
    }

    #[must_use]
    pub fn label_for(&self, value: i64) -> String {
        match self.control {
            OptionControl::Toggle => if value != 0 { "On" } else { "Off" }.to_owned(),
            OptionControl::Step { unit, .. } => format!("{value} {unit}"),
            OptionControl::AccentColor if value == ACCENT_COLOR => "Accent".to_owned(),
            OptionControl::Color | OptionControl::AccentColor => {
                PALETTE[self.normalize(value) as usize].name.to_owned()
            }
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

    /// Colour option resolved against the live accent: `ACCENT_COLOR` yields `accent`.
    #[must_use]
    pub fn color_or_accent(&self, key: &str, accent: Rgb) -> Rgb {
        if key == "accentColor" && !self.flag("customAccent") {
            return accent;
        }
        match self.value(key) {
            ACCENT_COLOR => accent,
            index => palette_color(index),
        }
    }

    #[must_use]
    pub fn align(&self) -> TextAlign {
        TextAlign::from_index(self.value("align"))
    }

    /// True when the card draws a surface (fill, blur). Off by default: the card then renders
    /// its content directly on the wallpaper with no backdrop work.
    #[must_use]
    pub fn has_surface(&self) -> bool {
        self.flag("surface")
    }

    /// True when the card takes colour and opacity from the global surface settings
    /// (`customSurface` off, the default).
    #[must_use]
    pub fn inherits_surface(&self) -> bool {
        !self.flag("customSurface")
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
    fn move_stays_inside_bounds() {
        let area = Rect::new(0, 0, 500, 400);
        let moved = move_widget_rect(Rect::new(100, 100, 200, 100), 1000, -1000, area);
        assert_eq!(moved, Rect::new(300, 0, 200, 100));
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
        // Grid cells are the layout: the widget sits on its own reflowed cell.
        let small = GridMetrics::for_work_area(Rect::new(0, 0, 1350, 641), 100);
        let cell = store.get(id).expect("widget").grid.expect("cell");
        assert_eq!(store.get(id).expect("widget").rect, small.rect_of(cell));

        // Legacy 9-field record with an absolute x on the right half of the old screen.
        let legacy = "v1\n3\tcpu\t1500\t20\t220\t110\t0\t\tbackdropBlur=40\n";
        let mut loaded = parse(legacy).expect("legacy");
        loaded.clamp_all(Rect::new(0, 0, 1350, 641));
        let widget = loaded.get(3).expect("widget");
        assert_eq!(widget.rect.right(), 1350 - 16);
        assert!(
            !widget.stored_values().contains_key("backdropBlur"),
            "retired key is dropped"
        );
        assert_eq!(
            serialize(&loaded)
                .lines()
                .nth(2)
                .expect("record")
                .split('\t')
                .nth(8),
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
    fn accent_defaults_follow_the_live_accent_and_explicit_palette_picks_do_not() {
        let accent = Rgb {
            red: 0xef,
            green: 0x40,
            blue: 0x48,
        };
        assert_eq!(card(WidgetKind::Clock).value("accentColor"), ACCENT_COLOR);
        assert_eq!(
            card(WidgetKind::Clock).color_or_accent("accentColor", accent),
            accent
        );
        assert_eq!(
            card(WidgetKind::Cpu).color_or_accent("ringColor", accent),
            accent
        );
        assert_eq!(
            card(WidgetKind::Memory).color_or_accent("ringColor", accent),
            palette_color(ORANGE)
        );
        // An explicit pick is ignored until the custom-accent toggle is on.
        let mut picked = card(WidgetKind::Clock);
        assert!(picked.set_value("accentColor", SKY));
        assert_eq!(picked.color_or_accent("accentColor", accent), accent);
        assert!(picked.set_value("customAccent", 1));
        assert_eq!(
            picked.color_or_accent("accentColor", accent),
            palette_color(SKY)
        );
        let mut stepping = card(WidgetKind::Clock);
        assert!(stepping.set_value("accentColor", 0));
        assert!(stepping.step_value("accentColor", -1));
        assert_eq!(stepping.value("accentColor"), ACCENT_COLOR);
        assert!(stepping.step_value("accentColor", 1));
        assert_eq!(stepping.value("accentColor"), 0);
    }

    #[test]
    fn new_cards_inherit_the_global_surface() {
        assert!(card(WidgetKind::Clock).inherits_surface());
        assert!(card(WidgetKind::Cpu).inherits_surface());
    }

    #[test]
    fn new_cards_have_no_surface_by_default() {
        for kind in WidgetKind::ALL {
            let widget = card(kind);
            assert!(!widget.has_surface(), "{kind:?}");
            assert!(
                widget.stored_values().is_empty(),
                "{kind:?} persists nothing"
            );
        }
    }

    #[test]
    fn surface_rows_appear_only_while_surface_is_on() {
        let keys = |widget: &Widget| {
            widget
                .kind
                .visible_options(widget.stored_values())
                .iter()
                .map(|spec| spec.key)
                .collect::<Vec<_>>()
        };
        let mut widget = card(WidgetKind::Custom);
        assert!(keys(&widget).contains(&"surface"));
        assert!(!keys(&widget).contains(&"customSurface"));
        assert!(!keys(&widget).contains(&"background"));
        assert!(widget.set_value("customSurface", 1));
        assert!(
            !keys(&widget).contains(&"background"),
            "still hidden while surface is off"
        );
        assert!(widget.set_value("surface", 1));
        assert!(keys(&widget).contains(&"customSurface"));
        assert!(keys(&widget).contains(&"background"));
        assert!(keys(&widget).contains(&"backgroundOpacity"));
        assert!(widget.set_value("surface", 0));
        assert!(!keys(&widget).contains(&"background"));
    }

    #[test]
    fn explicit_surface_value_persists_and_absent_key_is_off() {
        let mut widget = card(WidgetKind::Clock);
        assert!(widget.set_value("surface", 1));
        assert_eq!(widget.stored_values().get("surface"), Some(&1));
        assert!(widget.set_value("surface", 0));
        assert!(widget.stored_values().is_empty());
    }

    #[test]
    fn custom_surface_keeps_overrides_and_turning_it_off_restores_inheritance() {
        let mut widget = card(WidgetKind::Clock);
        assert!(widget.set_value("customSurface", 1));
        assert!(!widget.inherits_surface());
        assert!(widget.set_value("backgroundOpacity", 40));
        assert_eq!(widget.value("backgroundOpacity"), 40);
        assert!(widget.set_value("customSurface", 0));
        assert!(widget.inherits_surface());
    }

    #[test]
    fn custom_surface_background_override_does_not_inherit() {
        let mut widget = card(WidgetKind::Custom);
        assert!(widget.set_value("background", 5));
        assert!(
            widget.inherits_surface(),
            "colour is ignored while customSurface is off"
        );
        assert!(widget.set_value("customSurface", 1));
        assert!(!widget.inherits_surface());
        assert_eq!(widget.value("background"), 5);
    }

    #[test]
    fn custom_surface_key_round_trips_through_persist_format() {
        let mut widget = card(WidgetKind::Media);
        widget.set_value("customSurface", 1);
        assert!(widget.stored_values().contains_key("customSurface"));
        assert_eq!(widget.stored_values().get("customSurface"), Some(&1));
    }

    #[test]
    fn retired_settings_migrate_to_toggles() {
        // Old inheritSurface=0 means a custom surface; old explicit accent means a custom accent.
        let mut widget = card(WidgetKind::Clock);
        apply_option_pairs(
            &mut widget,
            &[
                ("inheritSurface", 0),
                ("accentColor", SKY),
                ("radius", 30),
                ("backdropBlur", 40),
                ("dateSize", 20),
            ],
        );
        assert!(!widget.inherits_surface());
        assert_eq!(widget.value("customAccent"), 1);
        assert_eq!(widget.value("accentColor"), SKY);
        for key in ["inheritSurface", "radius", "backdropBlur", "dateSize"] {
            assert!(
                !widget.stored_values().contains_key(key),
                "{key} must be dropped"
            );
        }
        // Defaults stay default: an inheriting card with no explicit accent gets no toggles.
        let mut plain = card(WidgetKind::Clock);
        apply_option_pairs(
            &mut plain,
            &[("inheritSurface", 1), ("accentColor", ACCENT_COLOR)],
        );
        assert!(plain.inherits_surface());
        assert_eq!(plain.value("customAccent"), 0);
    }

    #[test]
    fn history_kind_exposes_line_toggles_and_fits_one_cell() {
        assert_eq!(WidgetKind::from_id("history"), Some(WidgetKind::History));
        let keys: Vec<_> = WidgetKind::History
            .options()
            .iter()
            .map(|spec| spec.key)
            .collect();
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

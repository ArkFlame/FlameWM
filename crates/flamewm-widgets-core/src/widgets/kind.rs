use super::*;

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
                OptionSpec::toggle("bold", "Bold time", true),
                OptionSpec::color("textColor", "Time color", WHITE),
                OptionSpec::color("accentColor", "Date color", ACCENT_COLOR),
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
                    if self == Self::Cpu {
                        ACCENT_COLOR
                    } else {
                        ORANGE
                    },
                ),
                OptionSpec::color("trackColor", "Track color", SLATE),
                OptionSpec::color("textColor", "Text color", WHITE),
            ],
            Self::Media => vec![
                OptionSpec::toggle("showArtist", "Show artist", true),
                OptionSpec::toggle("showPlayer", "Show player name", true),
                OptionSpec::toggle("showControls", "Show controls", true),
                OptionSpec::color("textColor", "Text color", WHITE),
                OptionSpec::color("accentColor", "Accent color", ACCENT_COLOR),
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
                OptionSpec::color("accentColor", "Accent color", ACCENT_COLOR),
            ],
            Self::History => vec![
                OptionSpec::toggle("showCpu", "Show CPU line", true),
                OptionSpec::toggle("showMemory", "Show RAM line", true),
                OptionSpec::color("cpuColor", "CPU line color", ACCENT_COLOR),
                OptionSpec::color("memoryColor", "RAM line color", ORANGE),
            ],
            Self::Network => vec![
                OptionSpec::toggle("showRx", "Show download line", true),
                OptionSpec::toggle("showTx", "Show upload line", true),
                OptionSpec::color("rxColor", "Download color", ACCENT_COLOR),
                OptionSpec::color("txColor", "Upload color", ORANGE),
            ],
            Self::Calendar => vec![
                OptionSpec::toggle("showMonth", "Show month name", true),
                OptionSpec::color("textColor", "Text color", WHITE),
                OptionSpec::color("accentColor", "Today marker color", WHITE),
            ],
        };
        // Accent override: the accent row exists only with `customAccent` on, and until then
        // the card follows the live Settings accent.
        if let Some(at) = options.iter().position(|spec| spec.key == "accentColor") {
            options.insert(
                at,
                OptionSpec::toggle("customAccent", "Custom accent", false),
            );
        }
        // Surface: off by default, so the card draws no fill, blur or backdrop. On, the card
        // shows the surface. `customSurface` and its colour/opacity rows exist only with `surface`
        // on; off, the card inherits the global surface.
        options.extend([
            OptionSpec::toggle("surface", "Surface", false),
            OptionSpec::toggle("customSurface", "Custom surface", false),
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
        ]);
        options
    }

    /// Options shown in the widget settings for `widget`'s current values: override rows
    /// (custom accent, custom surface) appear only while their toggle is on, and every
    /// surface row only while `surface` is on.
    #[must_use]
    pub fn visible_options(self, values: &BTreeMap<String, i64>) -> Vec<OptionSpec> {
        let on = |key: &str| values.get(key).is_some_and(|value| *value != 0);
        self.options()
            .into_iter()
            .filter(|spec| match spec.key {
                "accentColor" => on("customAccent"),
                "customSurface" => on("surface"),
                key if SURFACE_OVERRIDE_KEYS.contains(&key) => on("surface") && on("customSurface"),
                _ => true,
            })
            .collect()
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

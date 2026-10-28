use crate::sticky::Rgb;

pub const MAX_TEMPLATE_LINES: usize = 6;
pub const MAX_LINE_SPANS: usize = 6;
pub const MAX_TEMPLATE_CHARS: usize = 600;

const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTHS: [&str; 12] = [
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WallTime {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub weekday: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl WallTime {
    #[must_use]
    pub fn weekday_name(self) -> &'static str {
        WEEKDAYS[usize::from(self.weekday % 7)]
    }

    #[must_use]
    pub fn month_name(self) -> &'static str {
        MONTHS[usize::from(self.month.clamp(1, 12) - 1)]
    }
}

#[must_use]
pub fn format_time(time: WallTime, use_24_hour: bool, show_seconds: bool) -> String {
    let seconds = if show_seconds {
        format!(":{:02}", time.second)
    } else {
        String::new()
    };
    if use_24_hour {
        return format!("{:02}:{:02}{seconds}", time.hour, time.minute);
    }
    let hour = match time.hour % 12 {
        0 => 12,
        hour => hour,
    };
    let meridiem = if time.hour < 12 { "AM" } else { "PM" };
    format!("{hour}:{:02}{seconds} {meridiem}", time.minute)
}

#[must_use]
pub fn format_date(time: WallTime, show_weekday: bool, show_year: bool) -> String {
    let mut date = format!("{} {}", time.day, time.month_name());
    if show_year {
        date.push_str(&format!(" {}", time.year));
    }
    if show_weekday {
        date = format!("{}, {date}", time.weekday_name());
    }
    date
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Placeholders {
    pub username: String,
    pub hostname: String,
    pub time: String,
    pub date: String,
    pub weekday: String,
    pub cpu_percent: Option<u8>,
    pub memory_percent: Option<u8>,
    pub uptime: String,
}

impl Placeholders {
    fn lookup(&self, name: &str) -> Option<String> {
        let percent =
            |value: Option<u8>| value.map_or_else(|| "--%".to_owned(), |v| format!("{v}%"));
        Some(match name {
            "username" | "user" => self.username.clone(),
            "hostname" | "host" => self.hostname.clone(),
            "time" => self.time.clone(),
            "date" => self.date.clone(),
            "weekday" | "day" => self.weekday.clone(),
            "cpu" => percent(self.cpu_percent),
            "memory" | "ram" => percent(self.memory_percent),
            "uptime" => self.uptime.clone(),
            _ => return None,
        })
    }
}

#[must_use]
pub fn expand_placeholders(text: &str, values: &Placeholders) -> String {
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        output.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let value = after
            .find('%')
            .filter(|end| {
                after[..*end]
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
            })
            .and_then(|end| Some((end, values.lookup(&after[..end].to_ascii_lowercase())?)));
        if let Some((end, value)) = value {
            output.push_str(&value);
            rest = &after[end + 1..];
        } else {
            output.push('%');
            rest = after;
        }
    }
    output.push_str(rest);
    output
}

#[must_use]
pub fn format_uptime(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SpanStyle {
    pub color: Option<Rgb>,
    pub background: Option<Rgb>,
    pub bold: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: SpanStyle,
}

const NAMED_COLORS: [(&str, Rgb); 16] = [
    ("black", Rgb::new(0x00, 0x00, 0x00)),
    ("dark_blue", Rgb::new(0x00, 0x00, 0xaa)),
    ("dark_green", Rgb::new(0x00, 0xaa, 0x00)),
    ("dark_aqua", Rgb::new(0x00, 0xaa, 0xaa)),
    ("dark_red", Rgb::new(0xaa, 0x00, 0x00)),
    ("dark_purple", Rgb::new(0xaa, 0x00, 0xaa)),
    ("gold", Rgb::new(0xff, 0xaa, 0x00)),
    ("gray", Rgb::new(0xaa, 0xaa, 0xaa)),
    ("dark_gray", Rgb::new(0x55, 0x55, 0x55)),
    ("blue", Rgb::new(0x55, 0x55, 0xff)),
    ("green", Rgb::new(0x55, 0xff, 0x55)),
    ("aqua", Rgb::new(0x55, 0xff, 0xff)),
    ("red", Rgb::new(0xff, 0x55, 0x55)),
    ("light_purple", Rgb::new(0xff, 0x55, 0xff)),
    ("yellow", Rgb::new(0xff, 0xff, 0x55)),
    ("white", Rgb::new(0xff, 0xff, 0xff)),
];

fn named_color(name: &str) -> Option<Rgb> {
    let name = match name {
        "grey" => "gray",
        "dark_grey" => "dark_gray",
        other => other,
    };
    NAMED_COLORS
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, rgb)| *rgb)
}

fn hex_color(value: &str) -> Option<Rgb> {
    let digits = value.strip_prefix('#')?;
    if digits.len() != 6 || !digits.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |index: usize| u8::from_str_radix(&digits[index..index + 2], 16).ok();
    Some(Rgb::new(channel(0)?, channel(2)?, channel(4)?))
}

fn color_value(value: &str) -> Option<Rgb> {
    named_color(value).or_else(|| hex_color(value))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TagChange {
    Color(Rgb),
    Background(Rgb),
    Bold,
}

fn tag_change(tag: &str) -> Option<TagChange> {
    let tag = tag.trim().to_ascii_lowercase();
    if matches!(tag.as_str(), "b" | "bold") {
        return Some(TagChange::Bold);
    }
    if let Some(name) = tag.strip_suffix("_background") {
        return color_value(name).map(TagChange::Background);
    }
    for prefix in ["bg:", "background:"] {
        if let Some(value) = tag.strip_prefix(prefix) {
            return color_value(value).map(TagChange::Background);
        }
    }
    for prefix in ["color:", "colour:", "c:"] {
        if let Some(value) = tag.strip_prefix(prefix) {
            return color_value(value).map(TagChange::Color);
        }
    }
    color_value(&tag).map(TagChange::Color)
}

fn tag_name(tag: &str) -> String {
    let tag = tag.trim().to_ascii_lowercase();
    match tag.as_str() {
        "b" => "bold".to_owned(),
        _ => tag,
    }
}

fn apply(stack: &[(String, TagChange)], base: SpanStyle) -> SpanStyle {
    stack.iter().fold(base, |mut style, (_, change)| {
        match *change {
            TagChange::Color(rgb) => style.color = Some(rgb),
            TagChange::Background(rgb) => style.background = Some(rgb),
            TagChange::Bold => style.bold = true,
        }
        style
    })
}

#[must_use]
pub fn parse_markup(template: &str, base: SpanStyle) -> Vec<Vec<Span>> {
    let template = template.replace("\\n", "\n");
    let mut lines: Vec<Vec<Span>> = vec![Vec::new()];
    let mut stack: Vec<(String, TagChange)> = Vec::new();
    let mut text = String::new();
    let mut rest = template.as_str();
    let flush = |lines: &mut Vec<Vec<Span>>, text: &mut String, style: SpanStyle| {
        if text.is_empty() {
            return;
        }
        let line = lines.last_mut().expect("at least one line");
        match line.last_mut() {
            Some(last) if last.style == style => last.text.push_str(text),
            _ => line.push(Span {
                text: std::mem::take(text),
                style,
            }),
        }
        text.clear();
    };
    while let Some(ch) = rest.chars().next() {
        if ch == '\n' {
            flush(&mut lines, &mut text, apply(&stack, base));
            lines.push(Vec::new());
            rest = &rest[1..];
            continue;
        }
        if ch == '<'
            && let Some(end) = rest.find('>')
        {
            let inner = &rest[1..end];
            if !inner.contains('<') && !inner.is_empty() {
                if let Some(name) = inner.strip_prefix('/') {
                    let name = tag_name(name);
                    if let Some(position) = stack.iter().rposition(|(open, _)| *open == name) {
                        flush(&mut lines, &mut text, apply(&stack, base));
                        stack.truncate(position);
                        rest = &rest[end + 1..];
                        continue;
                    }
                } else if tag_name(inner) == "reset" {
                    flush(&mut lines, &mut text, apply(&stack, base));
                    stack.clear();
                    rest = &rest[end + 1..];
                    continue;
                } else if let Some(change) = tag_change(inner) {
                    flush(&mut lines, &mut text, apply(&stack, base));
                    stack.push((tag_name(inner), change));
                    rest = &rest[end + 1..];
                    continue;
                }
            }
        }
        text.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    flush(&mut lines, &mut text, apply(&stack, base));
    lines
}

#[must_use]
pub fn fit_lines(mut lines: Vec<Vec<Span>>) -> Vec<Vec<Span>> {
    lines.truncate(MAX_TEMPLATE_LINES);
    for line in &mut lines {
        if line.len() > MAX_LINE_SPANS {
            let overflow: String = line
                .drain(MAX_LINE_SPANS - 1..)
                .map(|span| span.text)
                .collect();
            let style = line
                .last()
                .map_or_else(SpanStyle::default, |span| span.style);
            line.push(Span {
                text: overflow,
                style,
            });
        }
    }
    lines
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CpuTimes {
    pub busy: u64,
    pub total: u64,
}

#[must_use]
pub fn parse_cpu_times(proc_stat: &str) -> Option<CpuTimes> {
    let line = proc_stat.lines().find(|line| line.starts_with("cpu "))?;
    let values: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    if values.len() < 4 {
        return None;
    }
    let total: u64 = values.iter().take(8).sum();
    let idle = values[3] + values.get(4).copied().unwrap_or(0);
    Some(CpuTimes {
        busy: total.saturating_sub(idle),
        total,
    })
}

#[must_use]
pub fn cpu_percent(previous: CpuTimes, current: CpuTimes) -> Option<u8> {
    let total = current.total.checked_sub(previous.total)?;
    if total == 0 {
        return None;
    }
    let busy = current.busy.saturating_sub(previous.busy).min(total);
    Some(((busy * 100 + total / 2) / total) as u8)
}

#[must_use]
pub fn count_cpus(proc_stat: &str) -> usize {
    proc_stat
        .lines()
        .filter(|line| {
            line.strip_prefix("cpu")
                .is_some_and(|rest| rest.starts_with(|ch: char| ch.is_ascii_digit()))
        })
        .count()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemoryUsage {
    pub total_kib: u64,
    pub available_kib: u64,
}

impl MemoryUsage {
    #[must_use]
    pub fn used_kib(self) -> u64 {
        self.total_kib.saturating_sub(self.available_kib)
    }

    #[must_use]
    pub fn percent(self) -> Option<u8> {
        (self.total_kib > 0)
            .then(|| ((self.used_kib() * 100 + self.total_kib / 2) / self.total_kib) as u8)
    }
}

#[must_use]
pub fn parse_meminfo(meminfo: &str) -> Option<MemoryUsage> {
    let field = |name: &str| {
        meminfo.lines().find_map(|line| {
            let rest = line.strip_prefix(name)?.strip_prefix(':')?;
            rest.split_whitespace().next()?.parse::<u64>().ok()
        })
    };
    let total_kib = field("MemTotal")?;
    let available_kib = field("MemAvailable").or_else(|| {
        Some(field("MemFree")? + field("Buffers").unwrap_or(0) + field("Cached").unwrap_or(0))
    })?;
    Some(MemoryUsage {
        total_kib,
        available_kib: available_kib.min(total_kib),
    })
}

#[must_use]
pub fn format_gib(kib: u64) -> String {
    format!("{:.1}", kib as f64 / 1_048_576.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn time() -> WallTime {
        WallTime {
            year: 2022,
            month: 6,
            day: 29,
            weekday: 3,
            hour: 22,
            minute: 2,
            second: 7,
        }
    }

    #[test]
    fn clock_formats_both_hour_styles() {
        assert_eq!(format_time(time(), true, false), "22:02");
        assert_eq!(format_time(time(), true, true), "22:02:07");
        assert_eq!(format_time(time(), false, false), "10:02 PM");
        let midnight = WallTime { hour: 0, ..time() };
        assert_eq!(format_time(midnight, false, true), "12:02:07 AM");
        assert_eq!(format_date(time(), true, true), "Wednesday, 29 June 2022");
        assert_eq!(format_date(time(), false, false), "29 June");
    }

    #[test]
    fn placeholders_expand_known_names_and_keep_the_rest() {
        let values = Placeholders {
            username: "syndrizzle".into(),
            cpu_percent: Some(9),
            ..Placeholders::default()
        };
        assert_eq!(
            expand_placeholders("Hi %USERNAME%! %cpu% %ram% 100% %nope% %", &values),
            "Hi syndrizzle! 9% --% 100% %nope% %"
        );
    }

    #[test]
    fn markup_applies_colors_backgrounds_bold_and_closing_tags() {
        let base = SpanStyle::default();
        let lines = parse_markup(
            "<white><red_background>A</red_background>B</white>\\n<b>C</b><#00ff00>D<reset>E",
            base,
        );
        assert_eq!(lines.len(), 2);
        let white = Some(Rgb::new(0xff, 0xff, 0xff));
        assert_eq!(lines[0][0].text, "A");
        assert_eq!(lines[0][0].style.color, white);
        assert_eq!(
            lines[0][0].style.background,
            Some(Rgb::new(0xff, 0x55, 0x55))
        );
        assert_eq!(lines[0][1].text, "B");
        assert_eq!(lines[0][1].style.background, None);
        assert!(lines[1][0].style.bold);
        assert_eq!(lines[1][1].style.color, Some(Rgb::new(0, 0xff, 0)));
        assert_eq!(lines[1][2].text, "E");
        assert_eq!(lines[1][2].style, base);
    }

    #[test]
    fn unknown_and_unbalanced_tags_stay_literal() {
        let lines = parse_markup("a <wave>b</wave> c < d </red>", SpanStyle::default());
        assert_eq!(lines[0].len(), 1);
        assert_eq!(lines[0][0].text, "a <wave>b</wave> c < d </red>");
    }

    #[test]
    fn fit_lines_merges_overflowing_spans_and_drops_extra_lines() {
        let line: Vec<Span> = (0..9)
            .map(|index| Span {
                text: index.to_string(),
                style: SpanStyle {
                    bold: index % 2 == 0,
                    ..SpanStyle::default()
                },
            })
            .collect();
        let fitted = fit_lines(vec![line; MAX_TEMPLATE_LINES + 2]);
        assert_eq!(fitted.len(), MAX_TEMPLATE_LINES);
        assert_eq!(fitted[0].len(), MAX_LINE_SPANS);
        assert_eq!(fitted[0][MAX_LINE_SPANS - 1].text, "5678");
    }

    #[test]
    fn cpu_and_memory_parse_proc_files() {
        let first = parse_cpu_times("cpu  100 0 100 800 0 0 0 0 0 0\ncpu0 1 2 3 4\ncpu1 1 2 3 4\n")
            .expect("first");
        let second = parse_cpu_times("cpu  150 0 150 900 0 0 0 0 0 0\n").expect("second");
        assert_eq!(cpu_percent(first, second), Some(50));
        assert_eq!(cpu_percent(second, second), None);
        assert_eq!(count_cpus("cpu  1 2 3 4\ncpu0 1\ncpu1 1\nintr 5\n"), 2);
        let memory = parse_meminfo(
            "MemTotal:       16000000 kB\nMemFree: 1 kB\nMemAvailable:    4000000 kB\n",
        )
        .expect("meminfo");
        assert_eq!(memory.percent(), Some(75));
        assert_eq!(format_gib(memory.used_kib()), "11.4");
        assert!(parse_meminfo("MemFree: 1 kB\n").is_none());
    }

    #[test]
    fn uptime_uses_the_two_largest_units() {
        assert_eq!(format_uptime(59), "0m");
        assert_eq!(format_uptime(3_700), "1h 1m");
        assert_eq!(format_uptime(200_000), "2d 7h");
    }
}

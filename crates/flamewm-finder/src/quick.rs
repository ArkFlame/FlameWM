//! Launcher rows that answer or act without a file: arithmetic and unit conversion, application
//! ranking with frecency, and a few system actions. Pure logic; the window paints and runs it.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;

use std::thread;
use std::time::Duration;

use flamewm_api::applications::DesktopApplication;
use flamewm_api::session::{SessionAction, SessionCapabilities};
use flamewm_control_core::{ControlRequest, ControlResponse};
use flamewm_control_dbus::ControlClient;
use flamewm_dbus_reactor::BusKind;

/// Longest wait for the session capabilities; a silent bus leaves logout hidden.
const CAPABILITY_TIMEOUT: Duration = Duration::from_secs(2);
/// Longest wait for a session action to be accepted.
const ACTION_TIMEOUT: Duration = Duration::from_secs(5);

/// Applications shown above the file results.
pub const APP_SLOTS: usize = 6;
/// Frecency rows kept on disk; the least recently used are dropped.
const FRECENCY_LIMIT: usize = 512;
/// Longest input the calculator reads, which also bounds its recursion depth.
const MAX_CALC_CHARS: usize = 128;
const DAY: i64 = 86_400;
const WEEK: i64 = 7 * DAY;

/// A calculator answer: the trimmed input and the formatted result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub input: String,
    pub result: String,
}

/// Evaluates `input` as arithmetic (`2+2*3`, `(1+2)^3`) or a unit conversion (`5km in mi`).
#[must_use]
pub fn calculate(input: &str) -> Option<Answer> {
    let text = input.trim();
    if text.is_empty() || text.chars().count() > MAX_CALC_CHARS {
        return None;
    }
    let result = convert(text).or_else(|| arithmetic(text))?;
    Some(Answer {
        input: text.to_owned(),
        result,
    })
}

fn arithmetic(text: &str) -> Option<String> {
    // An operator must follow the first character, so a bare "-5" or "42" is not arithmetic.
    let rest = text.get(1..)?;
    if !rest.bytes().any(|b| b"+-*/^%".contains(&b)) || !text.bytes().any(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut parser = Parser {
        chars: text.chars().filter(|c| !c.is_whitespace()).collect(),
        pos: 0,
    };
    let value = parser.expr()?;
    if parser.pos != parser.chars.len() {
        return None;
    }
    format_number(value)
}

/// Recursive descent over `+ - * / % ^`, parentheses and unary signs; `^` binds right-to-left.
struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn expr(&mut self) -> Option<f64> {
        let mut value = self.term()?;
        while let Some(op @ ('+' | '-')) = self.peek() {
            self.pos += 1;
            let rhs = self.term()?;
            value = if op == '+' { value + rhs } else { value - rhs };
        }
        Some(value)
    }

    fn term(&mut self) -> Option<f64> {
        let mut value = self.power()?;
        while let Some(op @ ('*' | '/' | '%')) = self.peek() {
            self.pos += 1;
            let rhs = self.power()?;
            value = match op {
                '*' => value * rhs,
                _ if rhs == 0.0 => return None,
                '/' => value / rhs,
                _ => value % rhs,
            };
        }
        Some(value)
    }

    fn power(&mut self) -> Option<f64> {
        let base = self.unary()?;
        if self.peek() == Some('^') {
            self.pos += 1;
            return Some(base.powf(self.power()?));
        }
        Some(base)
    }

    fn unary(&mut self) -> Option<f64> {
        match self.peek() {
            Some('-') => {
                self.pos += 1;
                Some(-self.unary()?)
            }
            Some('+') => {
                self.pos += 1;
                self.unary()
            }
            _ => self.atom(),
        }
    }

    fn atom(&mut self) -> Option<f64> {
        if self.peek() == Some('(') {
            self.pos += 1;
            let value = self.expr()?;
            if self.peek() != Some(')') {
                return None;
            }
            self.pos += 1;
            return Some(value);
        }
        let start = self.pos;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit() || c == '.') {
            self.pos += 1;
        }
        if start == self.pos {
            return None;
        }
        self.chars[start..self.pos].iter().collect::<String>().parse().ok()
    }
}

/// Integers print without a fraction; other values keep up to eight decimals, trailing zeros cut.
fn format_number(value: f64) -> Option<String> {
    if !value.is_finite() {
        return None;
    }
    if value == 0.0 {
        return Some("0".to_owned());
    }
    if value.fract() == 0.0 && value.abs() < 1e15 {
        return Some(format!("{value:.0}"));
    }
    let text = format!("{value:.8}");
    Some(text.trim_end_matches('0').trim_end_matches('.').to_owned())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Length,
    Mass,
    Volume,
    Time,
    Data,
    Temperature,
}

struct Unit {
    label: &'static str,
    aliases: &'static [&'static str],
    kind: Kind,
    /// Multiplier to the kind's base unit (metre, gram, millilitre, second, byte).
    scale: f64,
}

const UNITS: &[Unit] = &[
    Unit { label: "mm", aliases: &["millimeter", "millimeters", "millimetre", "millimetres"], kind: Kind::Length, scale: 0.001 },
    Unit { label: "cm", aliases: &["centimeter", "centimeters", "centimetre", "centimetres"], kind: Kind::Length, scale: 0.01 },
    Unit { label: "m", aliases: &["meter", "meters", "metre", "metres"], kind: Kind::Length, scale: 1.0 },
    Unit { label: "km", aliases: &["kilometer", "kilometers", "kilometre", "kilometres"], kind: Kind::Length, scale: 1000.0 },
    Unit { label: "in", aliases: &["inch", "inches"], kind: Kind::Length, scale: 0.0254 },
    Unit { label: "ft", aliases: &["foot", "feet"], kind: Kind::Length, scale: 0.3048 },
    Unit { label: "yd", aliases: &["yard", "yards"], kind: Kind::Length, scale: 0.9144 },
    Unit { label: "mi", aliases: &["mile", "miles"], kind: Kind::Length, scale: 1609.344 },
    Unit { label: "mg", aliases: &["milligram", "milligrams"], kind: Kind::Mass, scale: 0.001 },
    Unit { label: "g", aliases: &["gram", "grams"], kind: Kind::Mass, scale: 1.0 },
    Unit { label: "kg", aliases: &["kilogram", "kilograms"], kind: Kind::Mass, scale: 1000.0 },
    Unit { label: "oz", aliases: &["ounce", "ounces"], kind: Kind::Mass, scale: 28.349_523_125 },
    Unit { label: "lb", aliases: &["lbs", "pound", "pounds"], kind: Kind::Mass, scale: 453.592_37 },
    Unit { label: "ml", aliases: &["milliliter", "milliliters", "millilitre"], kind: Kind::Volume, scale: 1.0 },
    Unit { label: "l", aliases: &["liter", "liters", "litre", "litres"], kind: Kind::Volume, scale: 1000.0 },
    Unit { label: "gal", aliases: &["gallon", "gallons"], kind: Kind::Volume, scale: 3785.411_784 },
    Unit { label: "s", aliases: &["sec", "secs", "second", "seconds"], kind: Kind::Time, scale: 1.0 },
    Unit { label: "min", aliases: &["minute", "minutes"], kind: Kind::Time, scale: 60.0 },
    Unit { label: "h", aliases: &["hr", "hrs", "hour", "hours"], kind: Kind::Time, scale: 3600.0 },
    Unit { label: "d", aliases: &["day", "days"], kind: Kind::Time, scale: 86_400.0 },
    Unit { label: "b", aliases: &["byte", "bytes"], kind: Kind::Data, scale: 1.0 },
    Unit { label: "kb", aliases: &[], kind: Kind::Data, scale: 1e3 },
    Unit { label: "mb", aliases: &[], kind: Kind::Data, scale: 1e6 },
    Unit { label: "gb", aliases: &[], kind: Kind::Data, scale: 1e9 },
    Unit { label: "tb", aliases: &[], kind: Kind::Data, scale: 1e12 },
    Unit { label: "kib", aliases: &[], kind: Kind::Data, scale: 1024.0 },
    Unit { label: "mib", aliases: &[], kind: Kind::Data, scale: 1_048_576.0 },
    Unit { label: "gib", aliases: &[], kind: Kind::Data, scale: 1_073_741_824.0 },
    Unit { label: "c", aliases: &["celsius"], kind: Kind::Temperature, scale: 0.0 },
    Unit { label: "f", aliases: &["fahrenheit"], kind: Kind::Temperature, scale: 0.0 },
    Unit { label: "k", aliases: &["kelvin"], kind: Kind::Temperature, scale: 0.0 },
];

fn unit(word: &str) -> Option<&'static Unit> {
    UNITS
        .iter()
        .find(|unit| unit.label == word || unit.aliases.contains(&word))
}

/// `5km in mi`, `5 km to mi`, or `100c in f`: a number, a unit, `in`/`to`, and a unit of one kind.
fn convert(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();
    let (number, from, separator, to) = match words.as_slice() {
        [glued, separator, to] => {
            let split = glued.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))?;
            (&glued[..split], &glued[split..], *separator, *to)
        }
        [number, from, separator, to] => (*number, *from, *separator, *to),
        _ => return None,
    };
    if separator != "in" && separator != "to" {
        return None;
    }
    let value: f64 = number.parse().ok()?;
    let (from, to) = (unit(from)?, unit(to)?);
    if from.kind != to.kind {
        return None;
    }
    let converted = if from.kind == Kind::Temperature {
        let celsius = match from.label {
            "f" => (value - 32.0) * 5.0 / 9.0,
            "k" => value - 273.15,
            _ => value,
        };
        match to.label {
            "f" => celsius * 9.0 / 5.0 + 32.0,
            "k" => celsius + 273.15,
            _ => celsius,
        }
    } else {
        value * from.scale / to.scale
    };
    Some(format!("{} {}", format_number(converted)?, to.label))
}

/// Score of `query` (lowercase) against `name`: prefix, then substring, then in-order subsequence.
fn fuzzy(name: &str, query: &str) -> Option<i64> {
    let name = name.to_lowercase();
    if name.starts_with(query) {
        return Some(10_000 - len_i64(name.len()));
    }
    if let Some(position) = name.find(query) {
        return Some(5_000 - len_i64(position));
    }
    let mut haystack = name.char_indices();
    let mut first = None;
    let mut last = 0;
    for needle in query.chars() {
        let (index, _) = haystack.find(|(_, c)| *c == needle)?;
        first.get_or_insert(index);
        last = index;
    }
    let first = first.unwrap_or(0);
    let gaps = len_i64(last - first + 1).saturating_sub(len_i64(query.chars().count()));
    Some(1_000 - 10 * gaps - len_i64(first))
}

fn len_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Use counts and last-use times per desktop ID, persisted as TSV beside the file index.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Frecency {
    uses: HashMap<String, (u32, i64)>,
}

impl Frecency {
    /// Reads `id<TAB>count<TAB>last` rows; malformed rows are skipped and a missing file is empty.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        let Ok(text) = fs::read_to_string(path) else {
            return Self::default();
        };
        let uses = text
            .lines()
            .filter_map(|line| {
                let mut parts = line.split('\t');
                let id = parts.next().filter(|id| !id.is_empty())?;
                let count = parts.next()?.parse().ok()?;
                let last = parts.next()?.parse().ok()?;
                Some((id.to_owned(), (count, last)))
            })
            .take(FRECENCY_LIMIT)
            .collect();
        Self { uses }
    }

    /// Writes the most recently used rows through a temporary file, so a crash keeps the old file.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let mut rows: Vec<(&String, &(u32, i64))> = self.uses.iter().collect();
        rows.sort_by(|a, b| b.1.1.cmp(&a.1.1).then(a.0.cmp(b.0)));
        let mut text = String::new();
        for (id, (count, last)) in rows.into_iter().take(FRECENCY_LIMIT) {
            let _ = writeln!(text, "{id}\t{count}\t{last}");
        }
        let temporary = path.with_extension("tsv.tmp");
        fs::write(&temporary, text)?;
        fs::rename(temporary, path)
    }

    pub fn record(&mut self, id: &str, now: i64) {
        let entry = self.uses.entry(id.to_owned()).or_insert((0, now));
        entry.0 = entry.0.saturating_add(1);
        entry.1 = now;
    }

    /// Ranking boost: frequent use adds a little, and use within the last day or week adds more.
    #[must_use]
    pub fn bonus(&self, id: &str, now: i64) -> i64 {
        let Some(&(uses, last)) = self.uses.get(id) else {
            return 0;
        };
        let age = now.saturating_sub(last);
        let recent = if age < DAY {
            300
        } else if age < WEEK {
            150
        } else {
            0
        };
        i64::from(uses.min(20)) * 40 + recent
    }
}

/// Indices into `apps` of the best matches for `query`, best first, at most `APP_SLOTS`.
#[must_use]
pub fn rank_apps(
    apps: &[DesktopApplication],
    query: &str,
    frecency: &Frecency,
    now: i64,
) -> Vec<usize> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(i64, usize)> = apps
        .iter()
        .enumerate()
        .filter_map(|(index, app)| {
            let base = fuzzy(&app.name, &needle)?;
            Some((base + frecency.bonus(app.id.as_str(), now), index))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.truncate(APP_SLOTS);
    scored.into_iter().map(|(_, index)| index).collect()
}

/// How a system action runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Run {
    /// Program and arguments, run as the signed-in user.
    Argv(&'static [&'static str]),
    /// Ends the session through the control bus; shown only when the session allows it.
    Logout,
}

/// A system action the user reaches by typing its name and pressing Enter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemAction {
    pub label: &'static str,
    /// Space-separated words that the query may start with.
    pub words: &'static str,
    pub run: Run,
}

pub const ACTIONS: [SystemAction; 3] = [
    SystemAction {
        label: "Lock screen",
        words: "lock screen",
        run: Run::Argv(&["loginctl", "lock-session"]),
    },
    SystemAction {
        label: "Suspend",
        words: "suspend sleep",
        run: Run::Argv(&["systemctl", "suspend"]),
    },
    SystemAction {
        label: "Log out",
        words: "log logout sign",
        run: Run::Logout,
    },
];

/// Actions whose label contains `query` or whose word starts with it; needs two characters.
/// Log out is listed only when `logout_allowed` is set.
#[must_use]
pub fn actions_matching(query: &str, logout_allowed: bool) -> Vec<&'static SystemAction> {
    let needle = query.trim().to_lowercase();
    if needle.chars().count() < 2 {
        return Vec::new();
    }
    ACTIONS
        .iter()
        .filter(|action| logout_allowed || action.run != Run::Logout)
        .filter(|action| {
            action.label.to_lowercase().contains(&needle)
                || action.words.split(' ').any(|word| word.starts_with(&needle))
        })
        .collect()
}

/// Whether the session allows logout. A missing or silent control bus answers no (fail closed).
#[must_use]
pub fn logout_allowed() -> bool {
    ControlClient::connect(BusKind::Session)
        .ok()
        .and_then(|client| {
            match client
                .call_with_timeout(&ControlRequest::GetSessionCapabilities, CAPABILITY_TIMEOUT)
                .ok()?
            {
                ControlResponse::SessionCapabilities(SessionCapabilities { logout, .. }) => {
                    Some(logout)
                }
                _ => None,
            }
        })
        .unwrap_or(false)
}

/// Starts the action without waiting for it, as the finder does for file openers. Logout runs on
/// its own thread, so a slow control bus never blocks the window.
pub fn run_action(action: &SystemAction) -> io::Result<()> {
    match action.run {
        Run::Argv(argv) => {
            let (program, args) = argv
                .split_first()
                .ok_or_else(|| io::Error::other("system action has no program"))?;
            std::process::Command::new(program)
                .args(args)
                .spawn()
                .map(drop)
        }
        Run::Logout => thread::Builder::new()
            .name("flamewm-finder-logout".to_owned())
            .spawn(|| {
                let client = ControlClient::connect(BusKind::Session);
                let outcome = client.map_err(|_| "control bus unavailable".to_owned()).and_then(
                    |client| {
                        client
                            .call_with_timeout(
                                &ControlRequest::SessionAction(SessionAction::Logout),
                                ACTION_TIMEOUT,
                            )
                            .map(drop)
                            .map_err(|_| "control call failed".to_owned())
                    },
                );
                if let Err(error) = outcome {
                    eprintln!("flamewm-finder: log out failed: {error}");
                }
            })
            .map(drop),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, name: &str) -> DesktopApplication {
        DesktopApplication {
            id: flamewm_api::DesktopAppId::new(id),
            name: name.to_owned(),
            generic_name: String::new(),
            comment: String::new(),
            startup_wm_class: String::new(),
            argv: vec![id.to_owned()],
            keywords: Vec::new(),
            categories: Vec::new(),
            icon_name: String::new(),
        }
    }

    fn calc(input: &str) -> Option<String> {
        calculate(input).map(|answer| answer.result)
    }

    #[test]
    fn arithmetic_follows_precedence_and_associativity() {
        assert_eq!(calc("2+2*3").as_deref(), Some("8"));
        assert_eq!(calc("(1+2)^3").as_deref(), Some("27"));
        assert_eq!(calc("2^3^2").as_deref(), Some("512"));
        assert_eq!(calc("10/4").as_deref(), Some("2.5"));
        assert_eq!(calc("-3 + 5").as_deref(), Some("2"));
        assert_eq!(calc("2^-1").as_deref(), Some("0.5"));
        assert_eq!(calc("7 % 4").as_deref(), Some("3"));
    }

    #[test]
    fn arithmetic_rejects_plain_numbers_words_and_bad_syntax() {
        assert_eq!(calc("42"), None);
        assert_eq!(calc("-5"), None);
        assert_eq!(calc("1/0"), None);
        assert_eq!(calc("2+"), None);
        assert_eq!(calc("(2+3"), None);
        assert_eq!(calc("2+3)"), None);
        assert_eq!(calc("report-2024 draft"), None);
        assert_eq!(calc(""), None);
        assert_eq!(calc(&"(".repeat(MAX_CALC_CHARS + 1)), None);
    }

    #[test]
    fn unit_conversion_handles_each_kind_and_temperature() {
        assert_eq!(calc("5km in mi").as_deref(), Some("3.10685596 mi"));
        assert_eq!(calc("2 h to min").as_deref(), Some("120 min"));
        assert_eq!(calc("1 gib in mib").as_deref(), Some("1024 mib"));
        assert_eq!(calc("100c in f").as_deref(), Some("212 f"));
        assert_eq!(calc("32 f in c").as_deref(), Some("0 c"));
        assert_eq!(calc("0 k in c").as_deref(), Some("-273.15 c"));
        assert_eq!(calc("5 kg in km"), None, "different kinds never convert");
        assert_eq!(calc("5 zz in mi"), None, "unknown units are not answers");
        assert_eq!(calc("5 km of mi"), None, "only in/to connect units");
    }

    #[test]
    fn fuzzy_prefers_prefix_then_substring_then_subsequence() {
        assert!(fuzzy("Firefox", "fire").unwrap() > fuzzy("Mozilla Firefox", "fire").unwrap());
        assert!(fuzzy("Mozilla Firefox", "fire").unwrap() > fuzzy("Fine Reader Xtra", "fire").unwrap());
        assert!(fuzzy("Firefox", "frfx").is_some());
        assert_eq!(fuzzy("Firefox", "xyz"), None);
    }

    #[test]
    fn ranking_uses_frecency_only_to_break_fuzzy_ties() {
        let apps = vec![app("alpha.desktop", "Text Editor"), app("beta.desktop", "Terminal"), app("gamma.desktop", "Tertiary")];
        let mut frecency = Frecency::default();
        // Two prefix matches tie on score and break by list order; the subsequence match ranks last.
        assert_eq!(rank_apps(&apps, "ter", &frecency, 0), vec![1, 2, 0]);
        frecency.record("gamma.desktop", 1_000);
        frecency.record("gamma.desktop", 1_001);
        let ranked = rank_apps(&apps, "ter", &frecency, 1_002);
        assert_eq!(ranked[0], 2, "recently used app moves up among equal matches");
        assert!(rank_apps(&apps, "", &frecency, 0).is_empty());
    }

    #[test]
    fn frecency_round_trips_and_skips_malformed_rows() {
        let dir = std::env::temp_dir().join(format!("finder-frecency-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("frecency.tsv");
        fs::write(&path, "good.desktop\t3\t100\nbroken\nbad\tx\t1\n").unwrap();
        let mut loaded = Frecency::load(&path);
        assert_eq!(loaded.bonus("good.desktop", 100), 3 * 40 + 300);
        assert_eq!(loaded.bonus("broken", 100), 0);
        loaded.record("other.desktop", 50);
        loaded.save(&path).unwrap();
        assert_eq!(Frecency::load(&path), loaded);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn actions_need_two_characters_and_match_words() {
        assert!(actions_matching("s", true).is_empty());
        assert_eq!(actions_matching("lo", false)[0].label, "Lock screen");
        assert_eq!(actions_matching("sleep", false)[0].label, "Suspend");
        assert_eq!(actions_matching("sus", false)[0].label, "Suspend");
    }

    #[test]
    fn log_out_appears_only_when_the_session_allows_it() {
        assert!(actions_matching("log", false)
            .iter()
            .all(|action| action.label != "Log out"));
        assert_eq!(actions_matching("log", true)[0].label, "Log out");
        assert_eq!(actions_matching("logout", true)[0].label, "Log out");
        assert_eq!(actions_matching("sign", true)[0].label, "Log out");
        assert!(actions_matching("out", false).is_empty());
    }

    #[test]
    fn log_out_is_a_control_bus_action_not_a_program() {
        let logout = ACTIONS.iter().find(|action| action.label == "Log out").unwrap();
        assert_eq!(logout.run, Run::Logout);
    }
}

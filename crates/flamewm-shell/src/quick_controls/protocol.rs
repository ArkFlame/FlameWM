//! Quick-control line protocol: parent -> host child over stdin/stdout lines.
//!
//! The authentication secret travels out-of-band (environment) and must never
//! appear in [`Command`]. This is enforced by type design: [`Command`] has no
//! secret field, and unit tests assert encoded lines never echo a secret token.

use flamewm_api::{PanelEdge, Rect};
use flamewm_shell_core::weather::WeatherFeed;

pub const MAX_LINE_LEN: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QuickControlKind {
    Audio,
    Network,
    Calendar,
    Battery,
    Weather,
}

impl QuickControlKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Network => "network",
            Self::Calendar => "calendar",
            Self::Battery => "battery",
            Self::Weather => "weather",
        }
    }

    pub fn parse(s: &str) -> Result<Self, ProtocolError> {
        match s {
            "audio" => Ok(Self::Audio),
            "network" => Ok(Self::Network),
            "calendar" => Ok(Self::Calendar),
            "battery" => Ok(Self::Battery),
            "weather" => Ok(Self::Weather),
            _ => Err(ProtocolError::UnknownKind(s.to_owned())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenRequest {
    pub kind: QuickControlKind,
    pub anchor: Rect,
    pub work_area: Rect,
    pub panel_edge: PanelEdge,
    /// Root rect of the parent's panel dock, so the helper can clip slide transit frames.
    pub dock: Option<Rect>,
}

/// Parent -> child commands. Deliberately secret-free by construction.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Open(OpenRequest),
    Close,
    Shutdown,
    /// Main-shell weather settings and state for the popup to project.
    Weather(WeatherFeed),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    TooLong(usize),
    Empty,
    UnknownCommand(String),
    UnknownKind(String),
    UnknownEdge(String),
    BadArity { expected: usize, got: usize },
    BadInt(String),
    BadWeather,
}

impl core::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TooLong(n) => write!(f, "line too long: {n} bytes"),
            Self::Empty => write!(f, "empty line"),
            Self::UnknownCommand(c) => write!(f, "unknown command: {c}"),
            Self::UnknownKind(k) => write!(f, "unknown kind: {k}"),
            Self::UnknownEdge(e) => write!(f, "unknown edge: {e}"),
            Self::BadArity { expected, got } => {
                write!(f, "bad arity: expected {expected} fields, got {got}")
            }
            Self::BadInt(v) => write!(f, "bad integer: {v}"),
            Self::BadWeather => write!(f, "bad weather feed"),
        }
    }
}

impl std::error::Error for ProtocolError {}

fn parse_edge(s: &str) -> Result<PanelEdge, ProtocolError> {
    match s {
        "top" => Ok(PanelEdge::Top),
        "bottom" => Ok(PanelEdge::Bottom),
        "left" => Ok(PanelEdge::Left),
        "right" => Ok(PanelEdge::Right),
        _ => Err(ProtocolError::UnknownEdge(s.to_owned())),
    }
}

fn edge_str(edge: PanelEdge) -> &'static str {
    match edge {
        PanelEdge::Top => "top",
        PanelEdge::Bottom => "bottom",
        PanelEdge::Left => "left",
        PanelEdge::Right => "right",
    }
}

/// Strict i32 parsing: no floats, no empty, no overflow, no shell expansion.
fn parse_int(s: &str) -> Result<i32, ProtocolError> {
    if s.is_empty() {
        return Err(ProtocolError::BadInt(s.to_owned()));
    }
    let neg = s.starts_with('-');
    let digits = if neg { &s[1..] } else { s };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ProtocolError::BadInt(s.to_owned()));
    }
    s.parse::<i32>()
        .map_err(|_| ProtocolError::BadInt(s.to_owned()))
}

fn parse_rect(parts: &[&str]) -> Result<Rect, ProtocolError> {
    if parts.len() != 4 {
        return Err(ProtocolError::BadArity {
            expected: 4,
            got: parts.len(),
        });
    }
    Ok(Rect::new(
        parse_int(parts[0])?,
        parse_int(parts[1])?,
        parse_int(parts[2])?,
        parse_int(parts[3])?,
    ))
}

#[must_use]
pub fn encode_command(cmd: &Command) -> String {
    match cmd {
        Command::Close => "CLOSE".to_owned(),
        Command::Shutdown => "SHUTDOWN".to_owned(),
        Command::Weather(feed) => format!("WEATHER {}", feed.encode()),
        Command::Open(req) => {
            // A missing dock encodes as a zero-width rect, which decodes back to `None`.
            let dock = req.dock.unwrap_or(Rect::new(0, 0, 0, 0));
            format!(
                "OPEN {} {} {} {} {} {} {} {} {} {} {} {} {} {}",
                req.kind.as_str(),
                req.anchor.x,
                req.anchor.y,
                req.anchor.width,
                req.anchor.height,
                req.work_area.x,
                req.work_area.y,
                req.work_area.width,
                req.work_area.height,
                edge_str(req.panel_edge),
                dock.x,
                dock.y,
                dock.width,
                dock.height,
            )
        }
    }
}

/// Decode one protocol line. No shell execution: whitespace split only.
pub fn decode_line(line: &str) -> Result<Command, ProtocolError> {
    if line.len() > MAX_LINE_LEN {
        return Err(ProtocolError::TooLong(line.len()));
    }
    let line = line.trim();
    if line.is_empty() {
        return Err(ProtocolError::Empty);
    }
    // Feed payloads use control-character separators and are not whitespace-split.
    if let Some(payload) = line.strip_prefix("WEATHER ") {
        return WeatherFeed::decode(payload)
            .map(Command::Weather)
            .ok_or(ProtocolError::BadWeather);
    }
    let parts: Vec<&str> = line.split_ascii_whitespace().collect();
    match parts[0] {
        "CLOSE" if parts.len() == 1 => Ok(Command::Close),
        "SHUTDOWN" if parts.len() == 1 => Ok(Command::Shutdown),
        "OPEN" => {
            // OPEN <kind> <anchor x4> <work_area x4> <edge> <dock x4> = 15 tokens.
            if parts.len() != 15 {
                return Err(ProtocolError::BadArity {
                    expected: 15,
                    got: parts.len(),
                });
            }
            let kind = QuickControlKind::parse(parts[1])?;
            let anchor = parse_rect(&parts[2..6])?;
            let work_area = parse_rect(&parts[6..10])?;
            let panel_edge = parse_edge(parts[10])?;
            let dock = parse_rect(&parts[11..15])?;
            Ok(Command::Open(OpenRequest {
                kind,
                anchor,
                work_area,
                panel_edge,
                dock: (dock.width > 0 && dock.height > 0).then_some(dock),
            }))
        }
        other => Err(ProtocolError::UnknownCommand(other.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Command {
        Command::Open(OpenRequest {
            kind: QuickControlKind::Audio,
            anchor: Rect::new(10, 20, 100, 30),
            work_area: Rect::new(0, 0, 1920, 1040),
            panel_edge: PanelEdge::Bottom,
            dock: None,
        })
    }

    #[test]
    fn round_trip_open_close_shutdown() {
        for cmd in [
            sample(),
            Command::Close,
            Command::Shutdown,
            Command::Open(OpenRequest {
                kind: QuickControlKind::Calendar,
                anchor: Rect::new(-5, -5, 1, 1),
                work_area: Rect::new(0, 0, 800, 600),
                panel_edge: PanelEdge::Top,
                dock: Some(Rect::new(0, 1040, 1920, 40)),
            }),
        ] {
            let line = encode_command(&cmd);
            assert!(line.len() <= MAX_LINE_LEN);
            assert_eq!(decode_line(&line), Ok(cmd));
        }
    }

    #[test]
    fn malformed_lines_rejected() {
        for bad in [
            "",
            "   ",
            "LAUNCH xterm",
            "OPEN audio 1 2 3",
            "OPEN bluetooth 0 0 1 1 0 0 2 2 bottom",
            "OPEN audio 1.5 2 3 4 0 0 2 2 bottom",
            "OPEN audio 1 2 3 4 0 0 2 2 nowhere",
            "OPEN audio $(rm -rf ~) 2 3 4 0 0 2 2 bottom",
            "CLOSE now",
            "SHUTDOWN 1",
            "OPEN audio 99999999999999999999 2 3 4 0 0 2 2 bottom",
        ] {
            assert!(decode_line(bad).is_err(), "accepted: {bad:?}");
        }
    }

    #[test]
    fn battery_kind_round_trips_through_its_wire_name() {
        assert_eq!(QuickControlKind::Battery.as_str(), "battery");
        assert_eq!(QuickControlKind::parse("battery"), Ok(QuickControlKind::Battery));
        let line = encode_command(&Command::Open(OpenRequest {
            kind: QuickControlKind::Battery,
            anchor: Rect::new(10, 20, 100, 30),
            work_area: Rect::new(0, 0, 1920, 1040),
            panel_edge: PanelEdge::Bottom,
            dock: None,
        }));
        assert!(line.starts_with("OPEN battery "), "{line}");
        assert!(decode_line(&line).is_ok());
    }

    #[test]
    fn weather_kind_round_trips_through_its_wire_name() {
        assert_eq!(QuickControlKind::Weather.as_str(), "weather");
        assert_eq!(QuickControlKind::parse("weather"), Ok(QuickControlKind::Weather));
        for cmd in [Command::Open(OpenRequest {
            kind: QuickControlKind::Weather,
            anchor: Rect::new(10, 20, 100, 30),
            work_area: Rect::new(0, 0, 1920, 1040),
            panel_edge: PanelEdge::Bottom,
            dock: None,
        })] {
            let line = encode_command(&cmd);
            assert!(line.starts_with("OPEN weather "), "{line}");
            assert_eq!(decode_line(&line), Ok(cmd));
        }
        assert!(QuickControlKind::parse("Weather").is_err());
    }

    #[test]
    fn weather_feed_round_trips_as_one_line() {
        use flamewm_shell_core::weather::{TemperatureUnit, WeatherFeed, WeatherSettings, WeatherState};
        let cmd = Command::Weather(WeatherFeed {
            settings: WeatherSettings {
                enabled: true,
                location: None,
                units: TemperatureUnit::Fahrenheit,
            },
            state: WeatherState::Unconfigured,
        });
        let line = encode_command(&cmd);
        assert!(line.starts_with("WEATHER "), "{line:?}");
        assert!(!line.contains('\n'));
        assert_eq!(decode_line(&line), Ok(cmd));
        assert_eq!(decode_line("WEATHER 9\u{1f}x"), Err(ProtocolError::BadWeather));
    }

    #[test]
    fn oversized_line_rejected() {
        let big = "X".repeat(MAX_LINE_LEN + 1);
        assert_eq!(decode_line(&big), Err(ProtocolError::TooLong(big.len())));
    }

    #[test]
    fn secret_never_appears_in_command_encoding() {
        // Type design: Command carries no secret; encoded lines must never
        // echo an out-of-band token.
        let secret = "s3cr3t-token-xyz-9f8e7d6c5b4a";
        for cmd in [sample(), Command::Close, Command::Shutdown] {
            let line = encode_command(&cmd);
            assert!(!line.contains(secret), "secret leaked: {line:?}");
        }
        // Compile-time shape check: Command must stay secret-free with
        // exactly these three variants.
        fn assert_shape(c: &Command) {
            match *c {
                Command::Open(_) | Command::Close | Command::Shutdown | Command::Weather(_) => {}
            }
        }
        let c = sample();
        assert_shape(&c);
    }
}

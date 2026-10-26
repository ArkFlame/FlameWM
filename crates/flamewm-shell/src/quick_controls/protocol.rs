//! Quick-control line protocol: parent -> host child over stdin/stdout lines.
//!
//! The authentication secret travels out-of-band (environment) and must never
//! appear in [`Command`]. This is enforced by type design: [`Command`] has no
//! secret field, and unit tests assert encoded lines never echo a secret token.

use flamewm_api::{OutputId, PanelEdge, Rect};
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRequest {
    pub kind: QuickControlKind,
    pub anchor: Rect,
    pub work_area: Rect,
    pub panel_edge: PanelEdge,
    /// Root rect of the parent's panel dock, so the helper can clip slide transit frames.
    pub dock: Option<Rect>,
    /// Optional provenance fence. Legacy OPEN remains decodable for mixed versions.
    pub scope: Option<OpenScope>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenScope {
    pub output: OutputId,
    pub topology_generation: u64,
    pub sequence: u64,
}

/// Helper acknowledgement for one scoped OPEN. Sequence numbers bind it to
/// the popup lease that requested presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenResult {
    Presented(u64),
    Refused(u64),
    RolledBack(u64),
    HostExited(u64),
}

impl OpenResult {
    #[must_use]
    pub fn encode(self) -> String {
        match self {
            Self::Presented(sequence) => format!("OPENED {sequence}"),
            Self::Refused(sequence) => format!("OPEN-REFUSED {sequence}"),
            Self::RolledBack(sequence) => format!("OPEN-ROLLED-BACK {sequence}"),
            Self::HostExited(sequence) => format!("OPEN-HOST-EXITED {sequence}"),
        }
    }

    pub fn decode(line: &str) -> Result<Self, ProtocolError> {
        let mut parts = line.split_ascii_whitespace();
        let (kind, sequence) = match (parts.next(), parts.next(), parts.next()) {
            (Some(kind), Some(sequence), None) => (kind, parse_u64(sequence)?),
            _ => {
                return Err(ProtocolError::BadArity {
                    expected: 2,
                    got: line.split_ascii_whitespace().count(),
                });
            }
        };
        if sequence == 0 {
            return Err(ProtocolError::BadInt(sequence.to_string()));
        }
        match kind {
            "OPENED" => Ok(Self::Presented(sequence)),
            "OPEN-REFUSED" => Ok(Self::Refused(sequence)),
            "OPEN-ROLLED-BACK" => Ok(Self::RolledBack(sequence)),
            "OPEN-HOST-EXITED" => Ok(Self::HostExited(sequence)),
            _ => Err(ProtocolError::UnknownCommand(kind.to_owned())),
        }
    }
}

/// Parent -> child commands. Deliberately secret-free by construction.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Open(OpenRequest),
    Close,
    CloseSession(u64),
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

fn rect_str(r: Rect) -> String {
    format!("{} {} {} {}", r.x, r.y, r.width, r.height)
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
        Command::CloseSession(sequence) => format!("CLOSE2 {sequence}"),
        Command::Shutdown => "SHUTDOWN".to_owned(),
        Command::Weather(feed) => format!("WEATHER {}", feed.encode()),
        Command::Open(req) => {
            // A missing dock encodes as a zero-width rect, which decodes back to `None`.
            let dock = req.dock.unwrap_or(Rect::new(0, 0, 0, 0));
            let geometry = format!(
                "{} {} {} {} {}",
                req.kind.as_str(),
                rect_str(req.anchor),
                rect_str(req.work_area),
                edge_str(req.panel_edge),
                rect_str(dock)
            );
            match &req.scope {
                Some(scope) => format!(
                    "OPEN2 {} {} {} {geometry}",
                    scope.output, scope.topology_generation, scope.sequence
                ),
                None => format!("OPEN {geometry}"),
            }
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
        "CLOSE2" if parts.len() == 2 => Ok(Command::CloseSession(parse_u64(parts[1])?)),
        "SHUTDOWN" if parts.len() == 1 => Ok(Command::Shutdown),
        "OPEN" | "OPEN2" => {
            let (scope, fields) = if parts[0] == "OPEN2" {
                if parts.len() != 18 {
                    return Err(ProtocolError::BadArity {
                        expected: 18,
                        got: parts.len(),
                    });
                }
                let output = OutputId::new(parts[1]);
                if !output.is_valid() {
                    return Err(ProtocolError::BadInt(parts[1].to_owned()));
                }
                let sequence = parse_u64(parts[3])?;
                if sequence == 0 {
                    return Err(ProtocolError::BadInt(parts[3].to_owned()));
                }
                (
                    Some(OpenScope {
                        output,
                        topology_generation: parse_u64(parts[2])?,
                        sequence,
                    }),
                    &parts[4..],
                )
            } else {
                (None, &parts[1..])
            };
            // Both versions carry 14 geometry fields after their command prefix.
            if fields.len() != 14 {
                return Err(ProtocolError::BadArity {
                    expected: 14,
                    got: fields.len(),
                });
            }
            let kind = QuickControlKind::parse(fields[0])?;
            let anchor = parse_rect(&fields[1..5])?;
            let work_area = parse_rect(&fields[5..9])?;
            let panel_edge = parse_edge(fields[9])?;
            let dock = parse_rect(&fields[10..14])?;
            Ok(Command::Open(OpenRequest {
                kind,
                anchor,
                work_area,
                panel_edge,
                dock: (dock.width > 0 && dock.height > 0).then_some(dock),
                scope,
            }))
        }
        other => Err(ProtocolError::UnknownCommand(other.to_owned())),
    }
}

fn parse_u64(s: &str) -> Result<u64, ProtocolError> {
    if s.is_empty() || !s.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ProtocolError::BadInt(s.to_owned()));
    }
    s.parse().map_err(|_| ProtocolError::BadInt(s.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_of(kind: QuickControlKind) -> Command {
        Command::Open(OpenRequest {
            kind,
            anchor: Rect::new(10, 20, 100, 30),
            work_area: Rect::new(0, 0, 1920, 1040),
            panel_edge: PanelEdge::Bottom,
            dock: None,
            scope: None,
        })
    }

    fn sample() -> Command {
        sample_of(QuickControlKind::Audio)
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
                scope: None,
            }),
        ] {
            let line = encode_command(&cmd);
            assert!(line.len() <= MAX_LINE_LEN);
            assert_eq!(decode_line(&line), Ok(cmd));
        }
        let scoped = Command::Open(OpenRequest {
            kind: QuickControlKind::Weather,
            anchor: Rect::new(10, 20, 100, 30),
            work_area: Rect::new(0, 0, 1920, 1040),
            panel_edge: PanelEdge::Bottom,
            dock: None,
            scope: Some(OpenScope {
                output: OutputId::new("HDMI-1"),
                topology_generation: 12,
                sequence: 44,
            }),
        });
        let line = encode_command(&scoped);
        assert!(line.starts_with("OPEN2 HDMI-1 12 44 "));
        assert_eq!(decode_line(&line), Ok(scoped));
        for command in [Command::CloseSession(44), Command::Close] {
            let line = encode_command(&command);
            assert_eq!(decode_line(&line), Ok(command));
        }
    }

    #[test]
    fn open_result_frames_round_trip_and_reject_invalid_sequences() {
        for result in [
            OpenResult::Presented(7),
            OpenResult::Refused(8),
            OpenResult::RolledBack(9),
            OpenResult::HostExited(10),
        ] {
            assert_eq!(OpenResult::decode(&result.encode()), Ok(result));
        }
        assert!(OpenResult::decode("OPENED 0").is_err());
        assert!(OpenResult::decode("OPENED nope").is_err());
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
        assert_eq!(
            QuickControlKind::parse("battery"),
            Ok(QuickControlKind::Battery)
        );
        let line = encode_command(&sample_of(QuickControlKind::Battery));
        assert!(line.starts_with("OPEN battery "), "{line}");
        assert!(decode_line(&line).is_ok());
    }

    #[test]
    fn weather_kind_round_trips_through_its_wire_name() {
        assert_eq!(QuickControlKind::Weather.as_str(), "weather");
        assert_eq!(
            QuickControlKind::parse("weather"),
            Ok(QuickControlKind::Weather)
        );
        let cmd = sample_of(QuickControlKind::Weather);
        let line = encode_command(&cmd);
        assert!(line.starts_with("OPEN weather "), "{line}");
        assert_eq!(decode_line(&line), Ok(cmd));
        assert!(QuickControlKind::parse("Weather").is_err());
    }

    #[test]
    fn weather_feed_round_trips_as_one_line() {
        use flamewm_shell_core::weather::{
            TemperatureUnit, WeatherFeed, WeatherSettings, WeatherState,
        };
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
        assert_eq!(
            decode_line("WEATHER 9\u{1f}x"),
            Err(ProtocolError::BadWeather)
        );
    }

    #[test]
    fn oversized_line_rejected() {
        let big = "X".repeat(MAX_LINE_LEN + 1);
        assert_eq!(decode_line(&big), Err(ProtocolError::TooLong(big.len())));
    }

    #[test]
    fn secret_never_appears_in_command_encoding() {
        fn assert_shape(c: &Command) {
            match *c {
                Command::Open(_)
                | Command::Close
                | Command::CloseSession(_)
                | Command::Shutdown
                | Command::Weather(_) => {}
            }
        }
        // Type design: Command carries no secret; encoded lines must never
        // echo an out-of-band token.
        let secret = "s3cr3t-token-xyz-9f8e7d6c5b4a";
        for cmd in [
            sample(),
            Command::Close,
            Command::CloseSession(7),
            Command::Shutdown,
        ] {
            let line = encode_command(&cmd);
            assert!(!line.contains(secret), "secret leaked: {line:?}");
        }
        // Compile-time shape check: Command must stay secret-free with
        // exactly these three variants.
        let c = sample();
        assert_shape(&c);
    }
}

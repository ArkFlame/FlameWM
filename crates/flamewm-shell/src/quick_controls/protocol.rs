//! Quick-control line protocol: parent -> host child over stdin/stdout lines.
//!
//! The authentication secret travels out-of-band (environment) and must never
//! appear in [`Command`]. This is enforced by type design: [`Command`] has no
//! secret field, and unit tests assert encoded lines never echo a secret token.

use flamewm_api::{PanelEdge, Rect};

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
}

/// Parent -> child commands. Deliberately secret-free by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Open(OpenRequest),
    Close,
    Shutdown,
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
    match *cmd {
        Command::Close => "CLOSE".to_owned(),
        Command::Shutdown => "SHUTDOWN".to_owned(),
        Command::Open(req) => format!(
            "OPEN {} {} {} {} {} {} {} {} {} {}",
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
        ),
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
    let parts: Vec<&str> = line.split_ascii_whitespace().collect();
    match parts[0] {
        "CLOSE" if parts.len() == 1 => Ok(Command::Close),
        "SHUTDOWN" if parts.len() == 1 => Ok(Command::Shutdown),
        "OPEN" => {
            // OPEN <kind> <anchor x4> <work_area x4> <edge> = 11 tokens.
            if parts.len() != 11 {
                return Err(ProtocolError::BadArity {
                    expected: 11,
                    got: parts.len(),
                });
            }
            let kind = QuickControlKind::parse(parts[1])?;
            let anchor = parse_rect(&parts[2..6])?;
            let work_area = parse_rect(&parts[6..10])?;
            let panel_edge = parse_edge(parts[10])?;
            Ok(Command::Open(OpenRequest {
                kind,
                anchor,
                work_area,
                panel_edge,
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
        })] {
            let line = encode_command(&cmd);
            assert!(line.starts_with("OPEN weather "), "{line}");
            assert_eq!(decode_line(&line), Ok(cmd));
        }
        assert!(QuickControlKind::parse("Weather").is_err());
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
                Command::Open(_) | Command::Close | Command::Shutdown => {}
            }
        }
        let c = sample();
        assert_shape(&c);
    }
}

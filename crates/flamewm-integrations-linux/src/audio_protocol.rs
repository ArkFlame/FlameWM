//! Line protocol between the WM and the `flamewm-audio-helper` process.
//!
//! Requests (WM to helper), one per line:
//! - `get`
//! - `volume <target> <percent> <generation> <server_generation>`
//! - `mute <target> <0|1> <generation> <server_generation>`
//!
//! `<target>` is `sink:<id>`, `source:<id>` or `stream:<id>`.
//!
//! Events (helper to WM): one block per published snapshot, terminated by `end`:
//! - `snapshot <availability> <generation> <server_generation> <volume> <muted> <sink_name>`
//! - `endpoint <sink|source> <id> <volume> <muted> <is_default> <name> <description>`
//! - `stream <id> <endpoint_id> <volume> <muted> <name> <app_name> <app_id> <icon_name> <process_binary> <media_name>`
//! - `end`
//!
//! Text fields escape `\`, space, newline, tab and carriage return, so every field is one token.
//! Optional stream fields are `-` for `None` and `=<text>` for `Some`.

use flamewm_api::system::{
    AudioEndpointKind, AudioEndpointSnapshot, AudioMuteAction, AudioSnapshot,
    AudioStreamSnapshot, AudioTarget, AudioVolumeAction, ServiceAvailability,
};
use flamewm_api::{ErrorCode, FlameError, FlameResult};

/// One WM request to the audio helper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioRequest {
    Get,
    Volume(AudioVolumeAction),
    Mute(AudioMuteAction),
}

fn protocol_error(message: impl Into<String>) -> FlameError {
    FlameError::new(ErrorCode::InvalidArgument, message)
}

/// Format one request as a single line without the trailing newline.
#[must_use]
pub fn encode_request(request: &AudioRequest) -> String {
    match request {
        AudioRequest::Get => "get".to_owned(),
        AudioRequest::Volume(action) => format!(
            "volume {} {} {} {}",
            encode_target(action.target),
            action.percent,
            action.generation,
            action.server_generation
        ),
        AudioRequest::Mute(action) => format!(
            "mute {} {} {} {}",
            encode_target(action.target),
            u8::from(action.muted),
            action.generation,
            action.server_generation
        ),
    }
}

/// Parse one request line produced by [`encode_request`].
pub fn parse_request(line: &str) -> FlameResult<AudioRequest> {
    let fields: Vec<&str> = line.split(' ').collect();
    match fields.as_slice() {
        ["get"] => Ok(AudioRequest::Get),
        ["volume", target, percent, generation, server_generation] => {
            Ok(AudioRequest::Volume(AudioVolumeAction {
                target: parse_target(target)?,
                percent: parse_number(percent, "volume percent")?,
                generation: parse_number(generation, "generation")?,
                server_generation: parse_number(server_generation, "server generation")?,
            }))
        }
        ["mute", target, muted, generation, server_generation] => {
            Ok(AudioRequest::Mute(AudioMuteAction {
                target: parse_target(target)?,
                muted: parse_bool(muted)?,
                generation: parse_number(generation, "generation")?,
                server_generation: parse_number(server_generation, "server generation")?,
            }))
        }
        _ => Err(protocol_error(format!("unknown audio request: {line}"))),
    }
}

/// Format a whole snapshot block, including the `end` line and trailing newlines.
#[must_use]
pub fn encode_snapshot(snapshot: &AudioSnapshot) -> String {
    let mut out = format!(
        "snapshot {} {} {} {} {} {}\n",
        encode_availability(snapshot.availability),
        snapshot.generation,
        snapshot.server_generation,
        snapshot.volume_percent,
        u8::from(snapshot.muted),
        escape(&snapshot.sink_name)
    );
    for endpoint in &snapshot.endpoints {
        out.push_str(&format!(
            "endpoint {} {} {} {} {} {} {}\n",
            encode_endpoint_kind(endpoint.kind),
            endpoint.id,
            endpoint.volume_percent,
            u8::from(endpoint.muted),
            u8::from(endpoint.is_default),
            escape(&endpoint.name),
            escape(&endpoint.description)
        ));
    }
    for stream in &snapshot.streams {
        out.push_str(&format!(
            "stream {} {} {} {} {} {} {} {} {} {}\n",
            stream.id,
            stream.endpoint_id,
            stream.volume_percent,
            u8::from(stream.muted),
            escape(&stream.name),
            encode_optional(stream.app_name.as_deref()),
            encode_optional(stream.app_id.as_deref()),
            encode_optional(stream.icon_name.as_deref()),
            encode_optional(stream.process_binary.as_deref()),
            encode_optional(stream.media_name.as_deref()),
        ));
    }
    out.push_str("end\n");
    out
}

/// Incremental decoder for snapshot blocks read line by line from the helper.
#[derive(Debug, Default)]
pub struct SnapshotDecoder {
    pending: Option<AudioSnapshot>,
}

impl SnapshotDecoder {
    /// Feed one line. Returns the completed snapshot on `end`.
    pub fn push_line(&mut self, line: &str) -> FlameResult<Option<AudioSnapshot>> {
        let fields: Vec<&str> = line.split(' ').collect();
        match fields.as_slice() {
            ["snapshot", availability, generation, server_generation, volume, muted, sink_name] => {
                if self.pending.is_some() {
                    return Err(protocol_error("snapshot started before end"));
                }
                self.pending = Some(AudioSnapshot {
                    availability: parse_availability(availability)?,
                    generation: parse_number(generation, "generation")?,
                    server_generation: parse_number(server_generation, "server generation")?,
                    sink_name: unescape(sink_name)?,
                    volume_percent: parse_number(volume, "volume percent")?,
                    muted: parse_bool(muted)?,
                    endpoints: Vec::new(),
                    streams: Vec::new(),
                });
                Ok(None)
            }
            ["endpoint", kind, id, volume, muted, is_default, name, description] => {
                let snapshot = self.open()?;
                snapshot.endpoints.push(AudioEndpointSnapshot {
                    id: parse_number(id, "endpoint id")?,
                    kind: parse_endpoint_kind(kind)?,
                    name: unescape(name)?,
                    description: unescape(description)?,
                    volume_percent: parse_number(volume, "volume percent")?,
                    muted: parse_bool(muted)?,
                    is_default: parse_bool(is_default)?,
                });
                Ok(None)
            }
            [
                "stream",
                id,
                endpoint_id,
                volume,
                muted,
                name,
                app_name,
                app_id,
                icon_name,
                process_binary,
                media_name,
            ] => {
                let snapshot = self.open()?;
                snapshot.streams.push(AudioStreamSnapshot {
                    id: parse_number(id, "stream id")?,
                    endpoint_id: parse_number(endpoint_id, "endpoint id")?,
                    name: unescape(name)?,
                    volume_percent: parse_number(volume, "volume percent")?,
                    muted: parse_bool(muted)?,
                    app_name: decode_optional(app_name)?,
                    app_id: decode_optional(app_id)?,
                    icon_name: decode_optional(icon_name)?,
                    process_binary: decode_optional(process_binary)?,
                    media_name: decode_optional(media_name)?,
                });
                Ok(None)
            }
            ["end"] => self
                .pending
                .take()
                .map(Some)
                .ok_or_else(|| protocol_error("end without snapshot")),
            _ => Err(protocol_error(format!("unknown audio event: {line}"))),
        }
    }

    fn open(&mut self) -> FlameResult<&mut AudioSnapshot> {
        self.pending
            .as_mut()
            .ok_or_else(|| protocol_error("snapshot item before snapshot header"))
    }
}

/// Escape one text field so it contains no separator, newline or tab.
#[must_use]
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            ' ' => out.push_str("\\s"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out
}

/// Reverse [`escape`].
pub fn unescape(text: &str) -> FlameResult<String> {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            _ => return Err(protocol_error("invalid escape in audio field")),
        }
    }
    Ok(out)
}

fn encode_optional(value: Option<&str>) -> String {
    match value {
        None => "-".to_owned(),
        Some(text) => format!("={}", escape(text)),
    }
}

fn decode_optional(field: &str) -> FlameResult<Option<String>> {
    if field == "-" {
        return Ok(None);
    }
    let Some(rest) = field.strip_prefix('=') else {
        return Err(protocol_error("optional audio field lacks a presence marker"));
    };
    unescape(rest).map(Some)
}

fn encode_target(target: AudioTarget) -> String {
    match target {
        AudioTarget::Endpoint {
            id,
            kind: AudioEndpointKind::Sink,
        } => format!("sink:{id}"),
        AudioTarget::Endpoint {
            id,
            kind: AudioEndpointKind::Source,
        } => format!("source:{id}"),
        AudioTarget::Stream { id } => format!("stream:{id}"),
    }
}

fn parse_target(field: &str) -> FlameResult<AudioTarget> {
    let (kind, id) = field
        .split_once(':')
        .ok_or_else(|| protocol_error("audio target lacks a kind"))?;
    let id = parse_number(id, "audio target id")?;
    match kind {
        "sink" => Ok(AudioTarget::Endpoint {
            id,
            kind: AudioEndpointKind::Sink,
        }),
        "source" => Ok(AudioTarget::Endpoint {
            id,
            kind: AudioEndpointKind::Source,
        }),
        "stream" => Ok(AudioTarget::Stream { id }),
        _ => Err(protocol_error("unknown audio target kind")),
    }
}

const fn encode_availability(availability: ServiceAvailability) -> &'static str {
    match availability {
        ServiceAvailability::Unknown => "unknown",
        ServiceAvailability::Available => "available",
        ServiceAvailability::Unavailable => "unavailable",
    }
}

fn parse_availability(field: &str) -> FlameResult<ServiceAvailability> {
    match field {
        "unknown" => Ok(ServiceAvailability::Unknown),
        "available" => Ok(ServiceAvailability::Available),
        "unavailable" => Ok(ServiceAvailability::Unavailable),
        _ => Err(protocol_error("unknown audio availability")),
    }
}

const fn encode_endpoint_kind(kind: AudioEndpointKind) -> &'static str {
    match kind {
        AudioEndpointKind::Sink => "sink",
        AudioEndpointKind::Source => "source",
    }
}

fn parse_endpoint_kind(field: &str) -> FlameResult<AudioEndpointKind> {
    match field {
        "sink" => Ok(AudioEndpointKind::Sink),
        "source" => Ok(AudioEndpointKind::Source),
        _ => Err(protocol_error("unknown audio endpoint kind")),
    }
}

fn parse_bool(field: &str) -> FlameResult<bool> {
    match field {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(protocol_error("audio flag must be 0 or 1")),
    }
}

fn parse_number<T: std::str::FromStr>(field: &str, what: &str) -> FlameResult<T> {
    field
        .parse()
        .map_err(|_| protocol_error(format!("invalid {what} in audio protocol")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_snapshot() -> AudioSnapshot {
        AudioSnapshot {
            availability: ServiceAvailability::Available,
            generation: 7,
            server_generation: 3,
            sink_name: "alsa output\tmain\\x".to_owned(),
            volume_percent: 42,
            muted: true,
            endpoints: vec![AudioEndpointSnapshot {
                id: 4,
                kind: AudioEndpointKind::Sink,
                name: "sink.one".to_owned(),
                description: "Speakers (line)".to_owned(),
                volume_percent: 42,
                muted: true,
                is_default: true,
            }],
            streams: vec![
                AudioStreamSnapshot {
                    id: 9,
                    endpoint_id: 4,
                    name: "music".to_owned(),
                    volume_percent: 80,
                    muted: false,
                    app_name: Some(String::new()),
                    app_id: None,
                    icon_name: Some("a b".to_owned()),
                    process_binary: None,
                    media_name: Some("x\ny".to_owned()),
                },
            ],
        }
    }

    #[test]
    fn requests_round_trip_for_every_target_kind() {
        let targets = [
            AudioTarget::Endpoint {
                id: 1,
                kind: AudioEndpointKind::Sink,
            },
            AudioTarget::Endpoint {
                id: 2,
                kind: AudioEndpointKind::Source,
            },
            AudioTarget::Stream { id: 3 },
        ];
        for target in targets {
            let volume = AudioRequest::Volume(AudioVolumeAction {
                target,
                percent: 150,
                generation: 11,
                server_generation: 12,
            });
            let mute = AudioRequest::Mute(AudioMuteAction {
                target,
                muted: true,
                generation: 13,
                server_generation: 14,
            });
            for request in [volume, mute, AudioRequest::Get] {
                let line = encode_request(&request);
                assert!(!line.contains('\n'));
                assert_eq!(parse_request(&line).expect("parse"), request, "{line}");
            }
        }
    }

    #[test]
    fn malformed_requests_are_rejected() {
        for line in [
            "",
            "set 50",
            "volume sink:1 101 1",
            "volume bogus:1 5 1 1",
            "volume sink:x 5 1 1",
            "mute sink:1 2 1 1",
            "get extra",
        ] {
            assert!(parse_request(line).is_err(), "{line:?} should be rejected");
        }
    }

    #[test]
    fn snapshot_round_trips_with_escaped_and_optional_fields() {
        let original = sample_snapshot();
        let text = encode_snapshot(&original);
        assert!(text.ends_with("end\n"));
        let mut decoder = SnapshotDecoder::default();
        let mut decoded = None;
        for line in text.lines() {
            if let Some(snapshot) = decoder.push_line(line).expect("decode") {
                decoded = Some(snapshot);
            }
        }
        assert_eq!(decoded.expect("completed snapshot"), original);
    }

    #[test]
    fn decoder_rejects_items_outside_a_block_and_recovers_after_reset() {
        let mut decoder = SnapshotDecoder::default();
        assert!(decoder.push_line("end").is_err());
        assert!(decoder.push_line("endpoint sink 1 1 0 0 a b").is_err());
        assert!(decoder.push_line("garbage").is_err());
        let mut decoder = SnapshotDecoder::default();
        assert!(decoder.push_line("snapshot unknown 0 0 0 0 ").expect("header").is_none());
        assert!(decoder.push_line("snapshot unknown 0 0 0 0 ").is_err());
    }

    #[test]
    fn escape_round_trips_separators_and_rejects_bad_escapes() {
        for text in ["", "a b", "tab\there", "back\\slash", "line\nbreak\r", "=lead"] {
            let escaped = escape(text);
            assert!(!escaped.contains(' ') && !escaped.contains('\t') && !escaped.contains('\n'));
            assert_eq!(unescape(&escaped).expect("unescape"), text);
        }
        assert!(unescape("bad\\q").is_err());
        assert!(unescape("trailing\\").is_err());
    }

    #[test]
    fn optional_fields_distinguish_none_from_empty_text() {
        assert_eq!(decode_optional("-").expect("none"), None);
        assert_eq!(decode_optional("=").expect("empty"), Some(String::new()));
        assert!(decode_optional("text").is_err());
    }
}

//! XDND protocol v5 core: message decoding, version negotiation, status bits, `text/uri-list`
//! codec and the drop-target state machine. Pure logic only; every X call lives in
//! `app/xdnd.rs`. Atoms and windows are plain `u64` ids here so the state machine is testable
//! without a display.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

/// Highest protocol version this implementation speaks.
pub const XDND_VERSION: u64 = 5;
pub const URI_LIST: &str = "text/uri-list";
pub const TEXT_PLAIN: &str = "text/plain";
/// `XdndStatus` data[1] bit 0: the target accepts the drop.
pub const STATUS_ACCEPT: i64 = 1;

/// `XdndStatus`/`XdndFinished` data[1] bits for an accept or decline.
pub fn status_flags(accepted: bool) -> i64 {
    if accepted { STATUS_ACCEPT } else { 0 }
}

/// Version agreed with a source. A source newer than this implementation is ignored (`None`).
pub fn negotiate_version(source_version: u64) -> Option<u64> {
    (source_version <= XDND_VERSION).then_some(source_version)
}

/// Source side: the version to advertise to a target that declares `target_aware`.
pub fn source_version_for(target_aware: u64) -> u64 {
    target_aware.min(XDND_VERSION)
}

/// `XdndPosition` packs root coordinates as signed 16-bit halves: x in the high word.
pub fn pack_position(x: i32, y: i32) -> i64 {
    (((x as i16 as u16) as i64) << 16) | ((y as i16 as u16) as i64)
}

pub fn unpack_position(packed: i64) -> (i32, i32) {
    let x = ((packed >> 16) & 0xffff) as u16 as i16 as i32;
    let y = (packed & 0xffff) as u16 as i16 as i32;
    (x, y)
}

/// Decoded `XdndEnter` payload. `types` holds the inline type atoms (zero entries removed);
/// when `more_types` is set the complete list lives in the source's `XdndTypeList` property.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Enter {
    pub source: u64,
    pub version: u64,
    pub more_types: bool,
    pub types: Vec<u64>,
}

pub fn decode_enter(data: [i64; 5]) -> Enter {
    Enter {
        source: data[0] as u64,
        version: ((data[1] as u64) >> 24) & 0xff,
        more_types: data[1] & 1 != 0,
        types: data[2..5]
            .iter()
            .filter(|atom| **atom != 0)
            .map(|atom| *atom as u64)
            .collect(),
    }
}

/// Prefers `text/uri-list`, then `text/plain`. Both are file-path carriers; the caller
/// converts the chosen format and parses it with `parse_uri_list`.
pub fn choose_format<T: Copy + PartialEq>(offered: &[T], uri_list: T, text_plain: T) -> Option<T> {
    if offered.contains(&uri_list) {
        Some(uri_list)
    } else if offered.contains(&text_plain) {
        Some(text_plain)
    } else {
        None
    }
}

/// Encodes `paths` as RFC 2483 `text/uri-list`: one `file://` URI per CRLF-terminated line.
pub fn encode_uri_list(paths: &[PathBuf]) -> Vec<u8> {
    let mut out = Vec::new();
    for path in paths {
        out.extend_from_slice(b"file://");
        for &byte in path.as_os_str().as_bytes() {
            if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
                out.push(byte);
            } else {
                out.extend_from_slice(format!("%{byte:02X}").as_bytes());
            }
        }
        out.extend_from_slice(b"\r\n");
    }
    out
}

/// Parses local `file://` URIs (empty host or `localhost`), skipping blanks, `#` comments,
/// non-file URIs and relative paths.
pub fn parse_uri_list(bytes: &[u8]) -> Vec<PathBuf> {
    bytes
        .split(|byte| *byte == b'\n')
        .filter_map(|line| {
            let line = line.trim_ascii();
            if line.is_empty() || line.starts_with(b"#") {
                return None;
            }
            let rest = line.strip_prefix(b"file://")?;
            let path = rest.strip_prefix(b"localhost").unwrap_or(rest);
            if !path.starts_with(b"/") {
                return None;
            }
            let decoded = percent_decode(path)?;
            Some(PathBuf::from(OsStr::from_bytes(&decoded)))
        })
        .collect()
}

fn percent_decode(input: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%' {
            let hex = input.get(i + 1..i + 3)?;
            let text = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(text, 16).ok()?);
            i += 3;
        } else {
            out.push(input[i]);
            i += 1;
        }
    }
    Some(out)
}

/// Drop-target events for the window owner. Coordinates are window-relative logical px.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropEvent {
    Enter { x: i32, y: i32 },
    Over { x: i32, y: i32 },
    Leave,
    Drop { x: i32, y: i32, paths: Vec<PathBuf> },
}

/// Protocol messages the drop target must send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    /// `XdndStatus` to `source`. The action atom is chosen by the caller (copy).
    Status { source: u64, accepted: bool },
    /// `XConvertSelection(XdndSelection, format, time)` for the dropped data.
    Convert { format: u64, time: u64 },
    /// `XdndFinished` to `source`.
    Finished { source: u64, accepted: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Session {
    source: u64,
    format: Option<u64>,
    position: (i32, i32),
    entered: bool,
    /// Set once `XdndDrop` asked for the data; cleared by `selection`.
    pending_drop: Option<u64>,
}

/// Drop-target state for one window. A message from a source other than the current session
/// is ignored, as is a message from a source that advertised a newer protocol version.
#[derive(Debug, Default)]
pub struct DropTarget {
    session: Option<Session>,
}

impl DropTarget {
    /// `XdndEnter`. Replies nothing; the first position reply carries the status.
    pub fn enter(&mut self, enter: &Enter, format: Option<u64>) {
        if negotiate_version(enter.version).is_none() {
            self.session = None;
            return;
        }
        self.session = Some(Session {
            source: enter.source,
            format,
            position: (0, 0),
            entered: false,
            pending_drop: None,
        });
    }

    /// `XdndPosition` with root-relative packed coordinates already converted to window space.
    pub fn position(&mut self, source: u64, x: i32, y: i32) -> (Vec<DropEvent>, Vec<Reply>) {
        let Some(session) = self.session.as_mut() else {
            return (Vec::new(), Vec::new());
        };
        if session.source != source || session.pending_drop.is_some() {
            return (Vec::new(), Vec::new());
        }
        session.position = (x, y);
        let event = if session.entered {
            DropEvent::Over { x, y }
        } else {
            session.entered = true;
            DropEvent::Enter { x, y }
        };
        let reply = Reply::Status {
            source,
            accepted: session.format.is_some(),
        };
        (vec![event], vec![reply])
    }

    /// `XdndLeave`.
    pub fn leave(&mut self, source: u64) -> Vec<DropEvent> {
        match &self.session {
            Some(session) if session.source == source && session.pending_drop.is_none() => {
                self.session = None;
                vec![DropEvent::Leave]
            }
            _ => Vec::new(),
        }
    }

    /// `XdndDrop`. Accepted drops start a conversion; the caller completes the drop in
    /// `selection`.
    pub fn drop_requested(&mut self, source: u64, time: u64) -> Vec<Reply> {
        let Some(session) = self.session.as_mut() else {
            return Vec::new();
        };
        if session.source != source {
            return Vec::new();
        }
        match session.format {
            Some(format) => {
                session.pending_drop = Some(time);
                vec![Reply::Convert { format, time }]
            }
            None => {
                self.session = None;
                vec![Reply::Finished {
                    source,
                    accepted: false,
                }]
            }
        }
    }

    /// Converted selection data for a pending drop (`None` when the owner refused).
    pub fn selection(&mut self, data: Option<Vec<PathBuf>>) -> (Vec<DropEvent>, Vec<Reply>) {
        let Some(session) = self.session.take() else {
            return (Vec::new(), Vec::new());
        };
        if session.pending_drop.is_none() {
            return (Vec::new(), Vec::new());
        }
        let finished = |accepted| Reply::Finished {
            source: session.source,
            accepted,
        };
        match data.filter(|paths| !paths.is_empty()) {
            Some(paths) => {
                let (x, y) = session.position;
                (
                    vec![DropEvent::Drop { x, y, paths }],
                    vec![finished(true)],
                )
            }
            None => (Vec::new(), vec![finished(false)]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enter(source: u64, version: i64, types: [i64; 3]) -> Enter {
        decode_enter([
            source as i64,
            version << 24,
            types[0],
            types[1],
            types[2],
        ])
    }

    const URI: u64 = 100;
    const PLAIN: u64 = 101;

    #[test]
    fn version_five_is_accepted_newer_is_ignored() {
        assert_eq!(negotiate_version(5), Some(5));
        assert_eq!(negotiate_version(3), Some(3));
        assert_eq!(negotiate_version(6), None);
        assert_eq!(source_version_for(4), 4);
        assert_eq!(source_version_for(9), 5);
    }

    #[test]
    fn enter_decodes_version_and_inline_types() {
        let decoded = enter(7, 5, [URI as i64, 0, 0]);
        assert_eq!(decoded.source, 7);
        assert_eq!(decoded.version, 5);
        assert!(!decoded.more_types);
        assert_eq!(decoded.types, vec![URI]);
    }

    #[test]
    fn enter_with_type_list_flag_defers_to_property() {
        let decoded = decode_enter([7, (5 << 24) | 1, 0, 0, 0]);
        assert!(decoded.more_types);
        assert!(decoded.types.is_empty());
    }

    #[test]
    fn packed_position_round_trips_signed_root_coordinates() {
        for (x, y) in [(0, 0), (1920, 1080), (-5, -7), (32767, -32768)] {
            assert_eq!(unpack_position(pack_position(x, y)), (x, y));
        }
    }

    #[test]
    fn choose_format_prefers_uri_list_then_plain() {
        assert_eq!(choose_format(&[PLAIN, URI], URI, PLAIN), Some(URI));
        assert_eq!(choose_format(&[PLAIN], URI, PLAIN), Some(PLAIN));
        assert_eq!(choose_format(&[999u64], URI, PLAIN), None);
    }

    #[test]
    fn position_replies_status_with_accept_bit_for_offered_format() {
        let mut target = DropTarget::default();
        target.enter(&enter(7, 5, [URI as i64, 0, 0]), Some(URI));
        let (events, replies) = target.position(7, 10, 20);
        assert_eq!(events, vec![DropEvent::Enter { x: 10, y: 20 }]);
        assert_eq!(
            replies,
            vec![Reply::Status {
                source: 7,
                accepted: true
            }]
        );
        let (events, _) = target.position(7, 11, 21);
        assert_eq!(events, vec![DropEvent::Over { x: 11, y: 21 }]);
    }

    #[test]
    fn unoffered_format_is_declined_and_drop_finishes_without_conversion() {
        let mut target = DropTarget::default();
        target.enter(&enter(7, 5, [999, 0, 0]), None);
        let (_, replies) = target.position(7, 1, 1);
        assert_eq!(
            replies,
            vec![Reply::Status {
                source: 7,
                accepted: false
            }]
        );
        assert_eq!(
            target.drop_requested(7, 42),
            vec![Reply::Finished {
                source: 7,
                accepted: false
            }]
        );
    }

    #[test]
    fn messages_from_other_sources_and_newer_versions_are_ignored() {
        let mut target = DropTarget::default();
        target.enter(&enter(7, 6, [URI as i64, 0, 0]), Some(URI));
        assert_eq!(target.position(7, 1, 1), (Vec::new(), Vec::new()));

        target.enter(&enter(7, 5, [URI as i64, 0, 0]), Some(URI));
        assert_eq!(target.position(8, 1, 1), (Vec::new(), Vec::new()));
        assert!(target.leave(8).is_empty());
        assert_eq!(target.leave(7), vec![DropEvent::Leave]);
        assert_eq!(target.position(7, 1, 1), (Vec::new(), Vec::new()));
    }

    #[test]
    fn drop_converts_then_selection_delivers_paths_and_finishes_accepted() {
        let mut target = DropTarget::default();
        target.enter(&enter(7, 5, [URI as i64, 0, 0]), Some(URI));
        target.position(7, 30, 40);
        assert_eq!(
            target.drop_requested(7, 99),
            vec![Reply::Convert {
                format: URI,
                time: 99
            }]
        );
        let (events, replies) = target.selection(Some(vec![PathBuf::from("/a b")]));
        assert_eq!(
            events,
            vec![DropEvent::Drop {
                x: 30,
                y: 40,
                paths: vec![PathBuf::from("/a b")]
            }]
        );
        assert_eq!(
            replies,
            vec![Reply::Finished {
                source: 7,
                accepted: true
            }]
        );
    }

    #[test]
    fn refused_or_empty_selection_finishes_not_accepted_without_drop_event() {
        for data in [None, Some(Vec::new())] {
            let mut target = DropTarget::default();
            target.enter(&enter(7, 5, [URI as i64, 0, 0]), Some(URI));
            target.position(7, 1, 1);
            target.drop_requested(7, 5);
            let (events, replies) = target.selection(data);
            assert!(events.is_empty());
            assert_eq!(
                replies,
                vec![Reply::Finished {
                    source: 7,
                    accepted: false
                }]
            );
        }
    }

    #[test]
    fn selection_without_pending_drop_is_ignored() {
        let mut target = DropTarget::default();
        target.enter(&enter(7, 5, [URI as i64, 0, 0]), Some(URI));
        assert_eq!(
            target.selection(Some(vec![PathBuf::from("/x")])),
            (Vec::new(), Vec::new())
        );
    }

    #[test]
    fn uri_list_encodes_crlf_and_percent_escapes_and_round_trips() {
        assert_eq!(encode_uri_list(&[]), b"");
        assert_eq!(
            encode_uri_list(&[PathBuf::from("/a b/c.txt")]),
            b"file:///a%20b/c.txt\r\n"
        );
        let paths = vec![
            PathBuf::from("/tmp/x y%z/ünï"),
            PathBuf::from("/plain"),
        ];
        assert_eq!(parse_uri_list(&encode_uri_list(&paths)), paths);
    }

    #[test]
    fn uri_list_parse_skips_comments_foreign_hosts_and_relative_paths() {
        let input = b"# comment\r\n\r\nfile://localhost/a\r\nhttp://x/y\r\nfile://remote/b\r\nfile:relative\r\nfile:///c%2Fd\n";
        assert_eq!(
            parse_uri_list(input),
            vec![PathBuf::from("/a"), PathBuf::from("/c/d")]
        );
    }

    #[test]
    fn uri_list_parse_rejects_bad_escape_without_panicking() {
        assert!(parse_uri_list(b"file:///bad%zz\r\n").is_empty());
        assert!(parse_uri_list(b"file:///trunc%2\r\n").is_empty());
    }
}

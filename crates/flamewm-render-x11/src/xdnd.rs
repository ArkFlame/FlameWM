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

/// Largest accepted drop payload (uri-list, ark service or path), in bytes.
pub const MAX_DROP_PAYLOAD_BYTES: usize = 256 * 1024;
/// Target-side wait for one `XConvertSelection` reply (500 ms stall guard).
#[cfg(test)]
pub const SELECTION_TIMEOUT_MS: u64 = 500;
/// Ark's private drag-out MIME types (`application/x-kde-ark-dndextract-{service,path}`).
pub const ARK_SERVICE_TYPE: &str = "application/x-kde-ark-dndextract-service";
pub const ARK_PATH_TYPE: &str = "application/x-kde-ark-dndextract-path";

/// Atom ids of the formats this target can receive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormatAtoms {
    pub uri_list: u64,
    pub ark_service: u64,
    pub ark_path: u64,
    pub text_plain: u64,
}

/// The format chosen for one drag, with the atom(s) to convert.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Offer {
    /// `text/uri-list` (or `text/plain` fallback) carrying local file paths.
    Paths(u64),
    /// Ark drag-out: a DBus service name and object path, converted in two steps.
    Ark { service: u64, path: u64 },
}

/// Best offer in priority order: `text/uri-list`, Ark service+path, `text/plain`.
/// `XDS` (XdndDirectSave0) is intentionally absent: its X wiring is not complete.
pub fn choose_offer(offered: &[u64], atoms: &FormatAtoms) -> Option<Offer> {
    if offered.contains(&atoms.uri_list) {
        Some(Offer::Paths(atoms.uri_list))
    } else if offered.contains(&atoms.ark_service) && offered.contains(&atoms.ark_path) {
        Some(Offer::Ark {
            service: atoms.ark_service,
            path: atoms.ark_path,
        })
    } else if offered.contains(&atoms.text_plain) {
        Some(Offer::Paths(atoms.text_plain))
    } else {
        None
    }
}

/// Valid DBus well-known bus name (`org.kde.ark-123`): at least two non-empty elements of
/// `[A-Za-z0-9_-]`, no element starting with a digit, at most 255 bytes, no unique `:` name.
pub fn valid_bus_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 255 {
        return false;
    }
    let parts: Vec<&str> = name.split('.').collect();
    parts.len() >= 2
        && parts.iter().all(|part| {
            !part.is_empty()
                && !part.as_bytes()[0].is_ascii_digit()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
}

/// Valid DBus object path: `/` or `/`-separated non-empty elements of `[A-Za-z0-9_]`.
pub fn valid_object_path(path: &str) -> bool {
    if path == "/" {
        return true;
    }
    path.len() <= 1024
        && path.starts_with('/')
        && !path.ends_with('/')
        && path[1..].split('/').all(|part| {
            !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
}

/// A single file name that cannot escape its directory.
pub fn safe_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name != "."
        && name != ".."
        && !name.bytes().any(|b| b == b'/' || b == 0)
}

/// Rejects oversized payloads before any parsing.
pub fn payload_within_limit(len: usize) -> bool {
    len <= MAX_DROP_PAYLOAD_BYTES
}

#[cfg(test)]
/// Monotonic deadline on an injected millisecond clock, so timeouts are testable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Deadline {
    started_ms: u64,
    limit_ms: u64,
}

#[cfg(test)]
impl Deadline {
    pub fn new(now_ms: u64, limit_ms: u64) -> Self {
        Self {
            started_ms: now_ms,
            limit_ms,
        }
    }

    pub fn expired(&self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.started_ms) >= self.limit_ms
    }
}

#[cfg(test)]
/// Outcome of an XDS (XdndDirectSave0) reply: the source's first byte is `S`, `E` or `F`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectSaveOutcome {
    Saved,
    Error,
    Failed,
}

#[cfg(test)]
pub fn parse_direct_save_reply(bytes: &[u8]) -> DirectSaveOutcome {
    match bytes.first() {
        Some(b'S') => DirectSaveOutcome::Saved,
        Some(b'E') => DirectSaveOutcome::Error,
        _ => DirectSaveOutcome::Failed,
    }
}

#[cfg(test)]
/// Reply already received, or a timeout failure once `deadline` has passed.
pub fn direct_save_poll(
    reply: Option<&[u8]>,
    deadline: &Deadline,
    now_ms: u64,
) -> Option<DirectSaveOutcome> {
    match reply {
        Some(bytes) => Some(parse_direct_save_reply(bytes)),
        None if deadline.expired(now_ms) => Some(DirectSaveOutcome::Failed),
        None => None,
    }
}

#[cfg(test)]
/// Destination `file://` URI for an XDS save into `dir` (absolute) named `name`.
pub fn direct_save_uri(dir: &std::path::Path, name: &str) -> Option<String> {
    if !dir.is_absolute() || !safe_file_name(name) {
        return None;
    }
    let mut uri = String::from("file://");
    let joined = dir.join(name);
    for &byte in joined.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            uri.push(byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    Some(uri)
}

/// Drop-target events for the window owner. Coordinates are window-relative logical px.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropEvent {
    Enter {
        x: i32,
        y: i32,
    },
    Over {
        x: i32,
        y: i32,
    },
    Leave,
    Drop {
        x: i32,
        y: i32,
        paths: Vec<PathBuf>,
    },
    /// Ark drag-out: the source offered a DBus service and object path to extract into.
    Extract {
        x: i32,
        y: i32,
        service: String,
        path: String,
    },
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

/// Where a accepted drop is in its data transfer.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Stage {
    Hover,
    Paths { time: u64 },
    ArkService { time: u64, path_atom: u64 },
    ArkPath { time: u64, service: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Session {
    source: u64,
    offer: Option<Offer>,
    position: (i32, i32),
    entered: bool,
    stage: Stage,
}

/// Drop-target state for one window. A message from a source other than the current session
/// is ignored, as is a message from a source that advertised a newer protocol version.
#[derive(Debug, Default)]
pub struct DropTarget {
    session: Option<Session>,
}

impl DropTarget {
    #[cfg(test)]
    /// `XdndEnter` with a `text/uri-list`/`text/plain` format (or none). Replies nothing.
    pub fn enter(&mut self, enter: &Enter, format: Option<u64>) {
        self.enter_offer(enter, format.map(Offer::Paths));
    }

    /// `XdndEnter` with any offer. Replies nothing; the first position reply carries status.
    pub fn enter_offer(&mut self, enter: &Enter, offer: Option<Offer>) {
        if negotiate_version(enter.version).is_none() {
            self.session = None;
            return;
        }
        self.session = Some(Session {
            source: enter.source,
            offer,
            position: (0, 0),
            entered: false,
            stage: Stage::Hover,
        });
    }

    /// `XdndPosition` with root-relative packed coordinates already converted to window space.
    pub fn position(&mut self, source: u64, x: i32, y: i32) -> (Vec<DropEvent>, Vec<Reply>) {
        let Some(session) = self.session.as_mut() else {
            return (Vec::new(), Vec::new());
        };
        if session.source != source || session.stage != Stage::Hover {
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
            accepted: session.offer.is_some(),
        };
        (vec![event], vec![reply])
    }

    /// `XdndLeave`.
    pub fn leave(&mut self, source: u64) -> Vec<DropEvent> {
        match &self.session {
            Some(session) if session.source == source && session.stage == Stage::Hover => {
                self.session = None;
                vec![DropEvent::Leave]
            }
            _ => Vec::new(),
        }
    }

    /// `XdndDrop`. Accepted drops start the first conversion; the caller continues in
    /// `selection_bytes` until the drop completes or is refused.
    pub fn drop_requested(&mut self, source: u64, time: u64) -> Vec<Reply> {
        let Some(session) = self.session.as_mut() else {
            return Vec::new();
        };
        if session.source != source {
            return Vec::new();
        }
        match session.offer {
            Some(Offer::Paths(format)) => {
                session.stage = Stage::Paths { time };
                vec![Reply::Convert { format, time }]
            }
            Some(Offer::Ark { service, path }) => {
                session.stage = Stage::ArkService {
                    time,
                    path_atom: path,
                };
                vec![Reply::Convert {
                    format: service,
                    time,
                }]
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

    /// Converted `text/uri-list` (or `text/plain`) paths for a pending drop.
    #[cfg(test)]
    pub fn selection(&mut self, data: Option<Vec<PathBuf>>) -> (Vec<DropEvent>, Vec<Reply>) {
        if !matches!(
            self.session.as_ref().map(|s| &s.stage),
            Some(Stage::Paths { .. })
        ) {
            return (Vec::new(), Vec::new());
        }
        self.complete_paths(data)
    }

    /// Raw converted bytes for the current conversion of a pending drop. Handles every stage:
    /// paths, then Ark service, then Ark path. `None` means the owner refused the conversion.
    pub fn selection_bytes(&mut self, data: Option<Vec<u8>>) -> (Vec<DropEvent>, Vec<Reply>) {
        let Some(session) = self.session.as_ref() else {
            return (Vec::new(), Vec::new());
        };
        let data = data.filter(|bytes| payload_within_limit(bytes.len()));
        let source = session.source;
        match session.stage.clone() {
            Stage::Hover => (Vec::new(), Vec::new()),
            Stage::Paths { .. } => {
                let paths = data.map(|bytes| parse_uri_list(&bytes));
                self.complete_paths(paths)
            }
            Stage::ArkService { time, path_atom } => {
                match data
                    .and_then(|bytes| ark_text(&bytes))
                    .filter(|s| valid_bus_name(s))
                {
                    Some(service) => {
                        if let Some(session) = self.session.as_mut() {
                            session.stage = Stage::ArkPath { time, service };
                        }
                        (
                            Vec::new(),
                            vec![Reply::Convert {
                                format: path_atom,
                                time,
                            }],
                        )
                    }
                    None => self.fail(source),
                }
            }
            Stage::ArkPath { service, .. } => {
                match data
                    .and_then(|bytes| ark_text(&bytes))
                    .filter(|p| valid_object_path(p))
                {
                    Some(path) => {
                        let session = self.session.take().expect("session present");
                        let (x, y) = session.position;
                        (
                            vec![DropEvent::Extract {
                                x,
                                y,
                                service,
                                path,
                            }],
                            vec![Reply::Finished {
                                source,
                                accepted: true,
                            }],
                        )
                    }
                    None => self.fail(source),
                }
            }
        }
    }

    fn fail(&mut self, source: u64) -> (Vec<DropEvent>, Vec<Reply>) {
        self.session = None;
        (
            Vec::new(),
            vec![Reply::Finished {
                source,
                accepted: false,
            }],
        )
    }

    fn complete_paths(&mut self, data: Option<Vec<PathBuf>>) -> (Vec<DropEvent>, Vec<Reply>) {
        let Some(session) = self.session.take() else {
            return (Vec::new(), Vec::new());
        };
        let finished = |accepted| Reply::Finished {
            source: session.source,
            accepted,
        };
        match data.filter(|paths| !paths.is_empty()) {
            Some(paths) => {
                let (x, y) = session.position;
                (vec![DropEvent::Drop { x, y, paths }], vec![finished(true)])
            }
            None => (Vec::new(), vec![finished(false)]),
        }
    }
}

/// Ark payloads are plain UTF-8 text without NUL or surrounding whitespace.
fn ark_text(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?.trim();
    (!text.is_empty() && !text.contains('\0')).then(|| text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enter(source: u64, version: i64, types: [i64; 3]) -> Enter {
        decode_enter([source as i64, version << 24, types[0], types[1], types[2]])
    }

    const URI: u64 = 100;

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
        let paths = vec![PathBuf::from("/tmp/x y%z/ünï"), PathBuf::from("/plain")];
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

#[cfg(test)]
mod ark_and_guard_tests {
    use super::*;

    const ATOMS: FormatAtoms = FormatAtoms {
        uri_list: 1,
        ark_service: 2,
        ark_path: 3,
        text_plain: 4,
    };

    fn enter(source: u64, types: [i64; 3]) -> Enter {
        decode_enter([source as i64, 5 << 24, types[0], types[1], types[2]])
    }

    #[test]
    fn offer_prefers_uri_list_then_ark_pair_then_plain() {
        assert_eq!(choose_offer(&[4, 1, 2, 3], &ATOMS), Some(Offer::Paths(1)));
        assert_eq!(
            choose_offer(&[2, 3, 4], &ATOMS),
            Some(Offer::Ark {
                service: 2,
                path: 3
            })
        );
        assert_eq!(choose_offer(&[2, 4], &ATOMS), Some(Offer::Paths(4)));
        assert_eq!(
            choose_offer(&[3], &ATOMS),
            None,
            "ark path alone is not usable"
        );
        assert_eq!(choose_offer(&[999], &ATOMS), None);
    }

    #[test]
    fn bus_name_and_object_path_validation() {
        assert!(valid_bus_name("org.kde.ark-4242"));
        assert!(valid_bus_name("org.kde.ark"));
        assert!(!valid_bus_name("ark"));
        assert!(!valid_bus_name("org..kde"));
        assert!(!valid_bus_name("org.1kde.ark"));
        assert!(!valid_bus_name(":1.42"), "unique names are not accepted");
        assert!(!valid_bus_name("org.kde.ark;rm"));
        assert!(valid_object_path("/DndExtract"));
        assert!(valid_object_path("/"));
        assert!(!valid_object_path("DndExtract"));
        assert!(!valid_object_path("/a//b"));
        assert!(!valid_object_path("/a/"));
        assert!(!valid_object_path("/a b"));
    }

    #[test]
    fn file_names_cannot_escape_directory() {
        assert!(safe_file_name("report.txt"));
        assert!(!safe_file_name(".."));
        assert!(!safe_file_name("."));
        assert!(!safe_file_name("a/b"));
        assert!(!safe_file_name(""));
        assert!(!safe_file_name("nul\0"));
        assert!(!safe_file_name(&"x".repeat(256)));
    }

    #[test]
    fn deadline_uses_injected_clock() {
        let deadline = Deadline::new(1_000, SELECTION_TIMEOUT_MS);
        assert!(!deadline.expired(1_499));
        assert!(deadline.expired(1_500));
        assert!(
            !deadline.expired(900),
            "a clock behind the start never reports expiry"
        );
    }

    #[test]
    fn xds_reply_parsing_and_timeout() {
        assert_eq!(parse_direct_save_reply(b"S"), DirectSaveOutcome::Saved);
        assert_eq!(parse_direct_save_reply(b"E"), DirectSaveOutcome::Error);
        assert_eq!(parse_direct_save_reply(b"F"), DirectSaveOutcome::Failed);
        assert_eq!(parse_direct_save_reply(b""), DirectSaveOutcome::Failed);
        let deadline = Deadline::new(0, SELECTION_TIMEOUT_MS);
        assert_eq!(direct_save_poll(None, &deadline, 10), None);
        assert_eq!(
            direct_save_poll(None, &deadline, 600),
            Some(DirectSaveOutcome::Failed)
        );
        assert_eq!(
            direct_save_poll(Some(b"S"), &deadline, 600),
            Some(DirectSaveOutcome::Saved)
        );
    }

    #[test]
    fn direct_save_uri_encodes_and_rejects_bad_names() {
        let dir = std::path::Path::new("/home/u/My Docs");
        assert_eq!(
            direct_save_uri(dir, "a b.txt").as_deref(),
            Some("file:///home/u/My%20Docs/a%20b.txt")
        );
        assert_eq!(direct_save_uri(std::path::Path::new("rel"), "a"), None);
        assert_eq!(direct_save_uri(dir, "../etc"), None);
    }

    #[test]
    fn ark_drop_converts_service_then_path_then_extracts() {
        let mut target = DropTarget::default();
        target.enter_offer(
            &enter(7, [2, 3, 0]),
            Some(Offer::Ark {
                service: 2,
                path: 3,
            }),
        );
        let (_, replies) = target.position(7, 10, 20);
        assert_eq!(
            replies,
            vec![Reply::Status {
                source: 7,
                accepted: true
            }]
        );
        assert_eq!(
            target.drop_requested(7, 9),
            vec![Reply::Convert { format: 2, time: 9 }]
        );
        let (events, replies) = target.selection_bytes(Some(b"org.kde.ark-42\n".to_vec()));
        assert!(events.is_empty());
        assert_eq!(replies, vec![Reply::Convert { format: 3, time: 9 }]);
        let (events, replies) = target.selection_bytes(Some(b"/DndExtract".to_vec()));
        assert_eq!(
            events,
            vec![DropEvent::Extract {
                x: 10,
                y: 20,
                service: "org.kde.ark-42".into(),
                path: "/DndExtract".into(),
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
    fn ark_invalid_service_or_path_is_refused_without_event() {
        let mut target = DropTarget::default();
        target.enter_offer(
            &enter(7, [2, 3, 0]),
            Some(Offer::Ark {
                service: 2,
                path: 3,
            }),
        );
        target.drop_requested(7, 1);
        let (events, replies) = target.selection_bytes(Some(b"../evil".to_vec()));
        assert!(events.is_empty());
        assert_eq!(
            replies,
            vec![Reply::Finished {
                source: 7,
                accepted: false
            }]
        );
        // session is gone: a further selection does nothing
        assert_eq!(
            target.selection_bytes(Some(b"org.kde.ark".to_vec())),
            (vec![], vec![])
        );
    }

    #[test]
    fn ark_refused_conversion_or_oversized_payload_finishes_not_accepted() {
        let mut target = DropTarget::default();
        target.enter_offer(
            &enter(7, [2, 3, 0]),
            Some(Offer::Ark {
                service: 2,
                path: 3,
            }),
        );
        target.drop_requested(7, 1);
        assert_eq!(
            target.selection_bytes(None).1,
            vec![Reply::Finished {
                source: 7,
                accepted: false
            }]
        );

        let mut target = DropTarget::default();
        target.enter_offer(&enter(7, [1, 0, 0]), Some(Offer::Paths(1)));
        target.drop_requested(7, 1);
        let huge = vec![b'a'; MAX_DROP_PAYLOAD_BYTES + 1];
        assert_eq!(
            target.selection_bytes(Some(huge)).1,
            vec![Reply::Finished {
                source: 7,
                accepted: false
            }]
        );
    }

    #[test]
    fn uri_drop_still_completes_through_selection_bytes() {
        let mut target = DropTarget::default();
        target.enter_offer(&enter(7, [1, 0, 0]), Some(Offer::Paths(1)));
        target.drop_requested(7, 1);
        let (events, _) = target.selection_bytes(Some(b"file:///tmp/a%20b\r\n".to_vec()));
        assert_eq!(
            events,
            vec![DropEvent::Drop {
                x: 0,
                y: 0,
                paths: vec![PathBuf::from("/tmp/a b")]
            }]
        );
    }

    #[test]
    fn stale_selection_without_drop_does_nothing() {
        let mut target = DropTarget::default();
        target.enter_offer(
            &enter(7, [2, 3, 0]),
            Some(Offer::Ark {
                service: 2,
                path: 3,
            }),
        );
        assert_eq!(
            target.selection_bytes(Some(b"org.kde.ark-1".to_vec())),
            (vec![], vec![])
        );
    }
}

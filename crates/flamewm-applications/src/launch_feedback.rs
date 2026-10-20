//! Freedesktop startup notification: bounded wire parsing and Flame-launched sequences.
//! Window readiness is observed by the shell, never inferred from a successful spawn.
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ClientMessageEvent, ConnectionExt, CreateWindowAux, EventMask, WindowClass};

const MAX_MESSAGE: usize = 4096;
const MAX_SEQUENCES: usize = 64;
const TIMEOUT: Duration = Duration::from_secs(15);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartupMessage {
    pub kind: MessageKind,
    pub values: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageKind { New, Change, Remove }

/// Values use the startup-notification quoting/escaping rules, not shell syntax.
pub fn parse_message(message: &str) -> Option<StartupMessage> {
    if message.len() > MAX_MESSAGE || message.as_bytes().contains(&0) { return None; }
    let (kind, body) = message.split_once(':')?;
    let kind = match kind { "new" => MessageKind::New, "change" => MessageKind::Change, "remove" => MessageKind::Remove, _ => return None };
    let bytes = body.as_bytes();
    let mut i = 0;
    let mut values = BTreeMap::new();
    while i < bytes.len() {
        while i < bytes.len() && bytes[i] == b' ' { i += 1; }
        if i == bytes.len() { break; }
        let start = i;
        while i < bytes.len() && bytes[i] != b'=' && bytes[i] != b' ' { i += 1; }
        if i == start || bytes.get(i) != Some(&b'=') { return None; }
        let key = std::str::from_utf8(&bytes[start..i]).ok()?;
        i += 1;
        let mut output = Vec::new();
        let mut quote = false;
        let mut escape = false;
        while i < bytes.len() {
            let byte = bytes[i];
            i += 1;
            if escape { output.push(byte); escape = false; }
            else if byte == b'\\' { escape = true; }
            else if byte == b'"' { quote = !quote; }
            else if byte == b' ' && !quote { break; }
            else { output.push(byte); }
        }
        if quote || escape { return None; }
        values.insert(key.to_string(), String::from_utf8(output).ok()?);
    }
    if values.get("ID").is_none_or(|id| id.is_empty() || id.len() > 256) { return None; }
    Some(StartupMessage { kind, values })
}

#[derive(Debug, Default)]
pub struct Fragments { messages: HashMap<u32, Vec<u8>> }
impl Fragments {
    pub fn push(&mut self, sender: u32, begin: bool, bytes: [u8; 20]) -> Option<StartupMessage> {
        if begin {
            if self.messages.len() >= MAX_SEQUENCES { self.messages.clear(); }
            self.messages.insert(sender, Vec::new());
        }
        let partial = self.messages.get_mut(&sender)?;
        let chunk = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
        if partial.len() + chunk > MAX_MESSAGE {
            self.messages.remove(&sender);
            return None;
        }
        partial.extend_from_slice(&bytes[..chunk]);
        if chunk == 20 { return None; }
        let full = self.messages.remove(&sender)?;
        parse_message(std::str::from_utf8(&full).ok()?)
    }
}

#[derive(Debug, Clone)]
pub struct Sequence {
    pub id: String,
    pub icon: String,
    pub wm_class: String,
    pub silent: bool,
    pub started: Instant,
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counters { pub started: u64, pub completed: u64, pub expired: u64 }
#[derive(Default)]
pub struct Sequences {
    active: BTreeMap<String, Sequence>,
    completed: VecDeque<String>,
    pending_changes: BTreeMap<String, (Instant, BTreeMap<String, String>)>,
    pub counters: Counters,
}
impl Sequences {
    pub fn accept(&mut self, mut message: StartupMessage, now: Instant) {
        let id = message.values["ID"].clone();
        if message.kind == MessageKind::Remove {
            self.pending_changes.remove(&id);
            if self.active.remove(&id).is_some() { self.counters.completed += 1; }
            if !self.completed.contains(&id) {
                self.completed.push_back(id);
                if self.completed.len() > 128 { self.completed.pop_front(); }
            }
            return;
        }
        if self.completed.contains(&id) { return; }
        if message.kind == MessageKind::Change && !self.active.contains_key(&id) {
            if self.pending_changes.len() < MAX_SEQUENCES {
                self.pending_changes.insert(id, (now, message.values));
            }
            return;
        }
        if message.kind == MessageKind::New {
            if let Some((at, pending)) = self.pending_changes.remove(&id) {
                if now.saturating_duration_since(at) <= Duration::from_secs(60) {
                    for (key, value) in pending {
                        message.values.entry(key).or_insert(value);
                    }
                }
            }
        }
        if self.active.len() >= MAX_SEQUENCES && !self.active.contains_key(&id) { return; }
        if !self.active.contains_key(&id) {
            self.counters.started += 1;
            self.active.insert(id.clone(), Sequence { id: id.clone(), icon: String::new(),
                wm_class: String::new(), silent: false, started: now });
        }
        let Some(sequence) = self.active.get_mut(&id) else { return; };
        if let Some(icon) = message.values.get("ICON") { sequence.icon.clone_from(icon); }
        if let Some(wm_class) = message.values.get("WMCLASS") { sequence.wm_class.clone_from(wm_class); }
        if let Some(silent) = message.values.get("SILENT") { sequence.silent = silent == "1"; }
        if sequence.icon.is_empty() {
            if !sequence.wm_class.is_empty() {
                sequence.icon.clone_from(&sequence.wm_class);
            } else if let Some(bin) = message.values.get("BIN") {
                // Executable paths are not theme icon names; use their basename.
                sequence.icon = std::path::Path::new(bin).file_name()
                    .and_then(|name| name.to_str()).unwrap_or(bin).to_owned();
            }
        }
    }
    pub fn complete_for_window(&mut self, id: Option<&str>, wm_class: Option<&str>) -> Option<String> {
        let match_id = id.filter(|id| self.active.contains_key(*id)).map(str::to_owned)
            .or_else(|| wm_class.and_then(|class| self.active.values()
                .filter(|s| !s.wm_class.is_empty() && s.wm_class.eq_ignore_ascii_case(class))
                .min_by_key(|s| s.started).map(|s| s.id.clone())));
        if let Some(id) = match_id.as_deref() {
            self.accept(StartupMessage { kind: MessageKind::Remove, values: BTreeMap::from([("ID".into(), id.into())]) }, Instant::now());
        }
        match_id
    }
    pub fn expire(&mut self, now: Instant) {
        let before = self.active.len();
        self.active.retain(|_, sequence| now.saturating_duration_since(sequence.started) < TIMEOUT);
        self.counters.expired += (before - self.active.len()) as u64;
        self.pending_changes.retain(|_, (at, _)|
            now.saturating_duration_since(*at) <= Duration::from_secs(60));
    }
    pub fn visible(&self) -> Option<&Sequence> {
        self.active.values().filter(|seq| !seq.silent).max_by_key(|seq| seq.started)
    }
    pub fn active(&self) -> bool { !self.active.is_empty() }
    pub fn next_deadline(&self) -> Option<Instant> {
        self.active.values().map(|s| s.started + TIMEOUT).min()
    }
}

fn quote_field(value: &str) -> String {
    // Build the wire field directly: vec! per character creates avoidable
    // temporary heap allocations for every startup notification.
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for c in value.chars().filter(|c| !c.is_control()) {
        if c == '"' || c == '\\' { quoted.push('\\'); }
        quoted.push(c);
    }
    quoted.push('"');
    quoted
}

pub fn new_id() -> String {
    let millis = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis();
    format!("flamewm-{}-{}-{millis}_TIME0", std::process::id(), NEXT_ID.fetch_add(1, Ordering::Relaxed))
}

/// Native broadcast uses a unique unmapped sender window; no cursor mutation or X grab.
/// Failure to contact X is nonfatal and must never prevent the actual launch.
pub fn broadcast(kind: MessageKind, id: &str, name: &str, icon: &str, wm_class: &str) -> bool {
    let Ok((conn, screen)) = x11rb::connect(None) else { return false; };
    let root = conn.setup().roots[screen].root;
    let atom = |name: &[u8]| conn.intern_atom(false, name).ok()?.reply().ok().map(|r| r.atom);
    let (Some(begin), Some(cont)) = (atom(b"_NET_STARTUP_INFO_BEGIN"), atom(b"_NET_STARTUP_INFO")) else { return false; };
    let Ok(sender) = conn.generate_id() else { return false; };
    if conn.create_window(0, sender, root, 0, 0, 1, 1, 0,
        WindowClass::INPUT_ONLY, 0, &CreateWindowAux::new()).is_err() { return false; }
    let verb = match kind { MessageKind::New => "new", MessageKind::Change => "change", MessageKind::Remove => "remove" };
    let msg = if kind == MessageKind::Remove { format!("remove: ID={}", quote_field(id)) }
    else {
        format!("{verb}: ID={} NAME={} SCREEN={screen} ICON={} WMCLASS={}",
            quote_field(id), quote_field(name), quote_field(icon), quote_field(wm_class))
    };
    let mut payload = msg.into_bytes();
    payload.push(0);
    let ok = payload.chunks(20).enumerate().all(|(index, chunk)| {
        let mut data = [0_u8; 20];
        data[..chunk.len()].copy_from_slice(chunk);
        conn.send_event(false, root, EventMask::PROPERTY_CHANGE,
            ClientMessageEvent::new(8, sender, if index == 0 { begin } else { cont }, data)).is_ok()
    });
    let _ = conn.destroy_window(sender);
    ok && conn.flush().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragments_parse_escaped_quoted_multi_chunk() {
        let wire = b"new: ID=launch-42 NAME=\"Foo \\\"Bar\\\"\" ICON=org.kde.dolphin WMCLASS=dolphin\0";
        let mut assembler = Fragments::default();
        let mut result = None;
        for (i, chunk) in wire.chunks(20).enumerate() {
            let mut data = [0; 20]; data[..chunk.len()].copy_from_slice(chunk);
            result = assembler.push(99, i == 0, data);
        }
        let parsed = result.expect("reassembled");
        assert_eq!(parsed.values["NAME"], "Foo \"Bar\"");
        assert_eq!(parsed.values["ICON"], "org.kde.dolphin");
    }
    #[test]
    fn overlapping_launches_and_tombstones() {
        let mut sequences = Sequences::default();
        let now = Instant::now();
        for id in ["first", "second"] {
            sequences.accept(parse_message(&format!("new: ID={id} ICON={id}" )).unwrap(), now);
        }
        assert_eq!(sequences.counters.started, 2);
        sequences.complete_for_window(Some("first"), None);
        assert_eq!(sequences.visible().unwrap().id, "second");
        sequences.accept(parse_message("new: ID=first ICON=stale").unwrap(), now);
        assert_eq!(sequences.counters.started, 2);
        sequences.expire(now + Duration::from_secs(16));
        assert_eq!(sequences.counters.expired, 1);
        assert!(!sequences.active());
    }
    #[test]
    fn quote_field_escapes_without_changing_startup_wire_format() {
        assert_eq!(quote_field("Dolphin"), "\"Dolphin\"");
        assert_eq!(quote_field("say \"hello\"\\now\n"),
            "\"say \\\"hello\\\"\\\\now\"");
        assert_eq!(quote_field("café"), "\"café\"");
    }

    #[test]
    fn malformed_does_not_start() {
        assert!(parse_message("new: ICON=foo").is_none());
        assert!(parse_message("new: ID=x NAME=\"unterminated").is_none());
        let mut partial = Fragments::default();
        for i in 0..210 {
            let _ = partial.push(7, i == 0, [b'x'; 20]);
        }
        assert!(partial.messages.is_empty());
        assert!(parse_message("change: ID=a").is_some());
    }
    #[test]
    fn change_before_new_merges_without_extra_start() {
        let now = Instant::now();
        let mut model = Sequences::default();
        model.accept(parse_message("change: ID=abc ICON=preloaded").unwrap(), now);
        assert_eq!(model.counters.started, 0);
        model.accept(parse_message("new: ID=abc NAME=Example SCREEN=0").unwrap(), now);
        assert_eq!(model.visible().unwrap().icon, "preloaded");
        assert_eq!(model.counters.started, 1);
    }
    #[test]
    fn missing_icon_prefers_wmclass_over_executable_path() {
        let mut model = Sequences::default();
        let now = Instant::now();
        model.accept(parse_message("new: ID=alpha WMCLASS=Dolphin BIN=/usr/bin/dolphin").unwrap(), now);
        assert_eq!(model.visible().unwrap().icon, "Dolphin");
        model.accept(parse_message("new: ID=beta BIN=/usr/bin/kate").unwrap(), now);
        assert_eq!(model.visible().unwrap().icon, "kate");
    }
    #[test]
    fn matching_class_completes_only_one_sequence() {
        let now = Instant::now();
        let mut model = Sequences::default();
        for id in ["a", "b"] { model.accept(parse_message(&format!("new: ID={id} WMCLASS=dolphin")).unwrap(), now); }
        model.complete_for_window(None, Some("Dolphin"));
        assert_eq!(model.counters.completed, 1);
        assert!(model.active());
    }
}

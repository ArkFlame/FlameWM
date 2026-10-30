//! Durable chat history: append-only JSONL plus a fixed-width index, a bounded
//! in-memory window, and small atomically replaced session state.
//!
//! Files in the store directory:
//! - `history.jsonl`: one JSON object per line. Its line number is the message `seq`.
//! - `history.idx`: 28-byte little-endian records `{offset u64, len u32, seq u64, ts_ms u64}`.
//!   Rebuilt from the JSONL on open whenever it disagrees.
//! - `state.json`: `{hermes_session, hermes_url}`, replaced via temp file and rename.
//!
//! Crash behaviour: a line is written and fsynced before its index record. On open, a
//! trailing line without `\n` or an unparseable line is truncated away, and the index is
//! rebuilt if it no longer matches.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::AssistantError;

const HISTORY_FILE: &str = "history.jsonl";
const INDEX_FILE: &str = "history.idx";
const STATE_FILE: &str = "state.json";
const INDEX_RECORD: usize = 28;
const DEFAULT_CACHE: usize = 120;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    SystemNote,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::SystemNote => "system_note",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "user" => Self::User,
            "assistant" => Self::Assistant,
            "system_note" => Self::SystemNote,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MsgState {
    Complete,
    Partial,
    Cancelled,
    Error,
}

impl MsgState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Partial => "partial",
            Self::Cancelled => "cancelled",
            Self::Error => "error",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "complete" => Self::Complete,
            "partial" => Self::Partial,
            "cancelled" => Self::Cancelled,
            "error" => Self::Error,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub seq: u64,
    pub ts_ms: u64,
    pub role: Role,
    pub text: String,
    pub session: Option<String>,
    pub turn: Option<String>,
    pub state: MsgState,
}

/// Hermes session bookkeeping persisted across restarts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionState {
    pub hermes_session: Option<String>,
    pub hermes_url: Option<String>,
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    offset: u64,
    len: u32,
    ts_ms: u64,
}

pub struct ChatStore {
    dir: PathBuf,
    log: File,
    index: File,
    reader: File,
    entries: Vec<Entry>,
    log_len: u64,
    cache: BTreeMap<u64, Message>,
    cache_cap: usize,
    /// Window centre for eviction: the newest append or the first message of the last page.
    anchor: u64,
    session: SessionState,
}

impl ChatStore {
    pub fn open(dir: &Path) -> Result<Self, AssistantError> {
        Self::open_with_cache(dir, DEFAULT_CACHE)
    }

    /// Opens or creates the store, recovering from a torn tail or a stale index.
    pub fn open_with_cache(dir: &Path, cache_cap: usize) -> Result<Self, AssistantError> {
        fs::create_dir_all(dir)?;
        let log_path = dir.join(HISTORY_FILE);
        let idx_path = dir.join(INDEX_FILE);

        let entries = scan(&log_path)?;
        let valid_len = entries.last().map_or(0, |e| e.offset + u64::from(e.len));
        if fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0) != valid_len {
            let file = OpenOptions::new().write(true).open(&log_path)?;
            file.set_len(valid_len)?;
            file.sync_all()?;
        }
        let expected = encode_index(&entries);
        if fs::read(&idx_path).ok().as_deref() != Some(expected.as_slice()) {
            write_atomic(&idx_path, &expected)?;
        }

        let log = OpenOptions::new().create(true).append(true).open(&log_path)?;
        let index = OpenOptions::new().create(true).append(true).open(&idx_path)?;
        let reader = File::open(&log_path)?;
        Ok(Self {
            dir: dir.to_path_buf(),
            log,
            index,
            reader,
            log_len: valid_len,
            entries,
            cache: BTreeMap::new(),
            cache_cap: cache_cap.max(1),
            anchor: 0,
            session: read_session(dir),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Number of messages currently held in memory.
    pub fn cached(&self) -> usize {
        self.cache.len()
    }

    /// Appends one message, durable on return. Returns its `seq`.
    /// After an error the store must be reopened before further use.
    pub fn append(
        &mut self,
        role: Role,
        text: &str,
        session: Option<&str>,
        turn: Option<&str>,
        state: MsgState,
    ) -> Result<u64, AssistantError> {
        let seq = self.entries.len() as u64;
        let ts_ms = now_ms();
        let mut line = json!({
            "v": 1,
            "seq": seq,
            "ts_ms": ts_ms,
            "role": role.as_str(),
            "text": text,
            "session": session,
            "turn": turn,
            "state": state.as_str(),
        })
        .to_string()
        .into_bytes();
        line.push(b'\n');
        let len = u32::try_from(line.len())
            .map_err(|_| AssistantError::Store("message exceeds 4 GiB".into()))?;
        let entry = Entry { offset: self.log_len, len, ts_ms };

        self.log.write_all(&line)?;
        self.log.sync_data()?;
        self.index.write_all(&encode_record(entry, seq))?;
        self.index.sync_data()?;

        self.log_len += u64::from(len);
        self.entries.push(entry);
        self.anchor = seq;
        self.cache_insert(Message {
            seq,
            ts_ms,
            role,
            text: text.to_owned(),
            session: session.map(str::to_owned),
            turn: turn.map(str::to_owned),
            state,
        });
        Ok(seq)
    }

    /// Up to `limit` messages strictly older than `before` (all messages when `None`), oldest first.
    pub fn load_page(&mut self, before: Option<u64>, limit: usize) -> Result<Vec<Message>, AssistantError> {
        let total = self.entries.len() as u64;
        let end = before.map_or(total, |b| b.min(total));
        let start = end.saturating_sub(limit as u64);
        self.anchor = start;
        (start..end).map(|seq| self.get(seq)).collect()
    }

    /// The newest `limit` messages, oldest first.
    pub fn tail(&mut self, limit: usize) -> Result<Vec<Message>, AssistantError> {
        self.load_page(None, limit)
    }

    pub fn session(&self) -> &SessionState {
        &self.session
    }

    /// Deletes every message and the Hermes session. Durable on return.
    pub fn clear(&mut self) -> Result<(), AssistantError> {
        self.log.set_len(0)?;
        self.log.sync_all()?;
        self.index.set_len(0)?;
        self.index.sync_all()?;
        self.set_session(SessionState::default())?;
        self.entries.clear();
        self.cache.clear();
        self.log_len = 0;
        self.anchor = 0;
        Ok(())
    }

    /// Replaces the session state atomically.
    pub fn set_session(&mut self, state: SessionState) -> Result<(), AssistantError> {
        let value = json!({"hermes_session": state.hermes_session, "hermes_url": state.hermes_url});
        write_atomic(&self.dir.join(STATE_FILE), value.to_string().as_bytes())?;
        self.session = state;
        Ok(())
    }

    fn get(&mut self, seq: u64) -> Result<Message, AssistantError> {
        if let Some(message) = self.cache.get(&seq) {
            return Ok(message.clone());
        }
        let entry = self.entries[seq as usize];
        let mut buf = vec![0u8; entry.len as usize];
        self.reader.read_exact_at(&mut buf, entry.offset)?;
        let message = parse_line(&buf[..buf.len() - 1], seq)?;
        self.cache_insert(message.clone());
        Ok(message)
    }

    /// Inserts a message and evicts the entry farthest from the anchor once over capacity.
    fn cache_insert(&mut self, message: Message) {
        self.cache.insert(message.seq, message);
        while self.cache.len() > self.cache_cap {
            let anchor = self.anchor;
            let victim = self.cache.keys().copied().max_by_key(|k| k.abs_diff(anchor));
            if let Some(victim) = victim {
                self.cache.remove(&victim);
            } else {
                break;
            }
        }
    }
}

/// Reads the valid prefix of the JSONL file and returns its index entries.
fn scan(path: &Path) -> Result<Vec<Entry>, AssistantError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut reader = BufReader::new(file);
    let mut entries = Vec::new();
    let mut offset = 0u64;
    let mut line = Vec::new();
    loop {
        line.clear();
        let n = reader.read_until(b'\n', &mut line)?;
        if n == 0 || line.last() != Some(&b'\n') {
            break;
        }
        let seq = entries.len() as u64;
        match parse_line(&line[..n - 1], seq) {
            Ok(message) => entries.push(Entry { offset, len: n as u32, ts_ms: message.ts_ms }),
            Err(_) => break,
        }
        offset += n as u64;
    }
    Ok(entries)
}

fn parse_line(bytes: &[u8], expected_seq: u64) -> Result<Message, AssistantError> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|e| AssistantError::Store(format!("corrupt history line: {e}")))?;
    let bad = || AssistantError::Store("history line has missing or invalid fields".into());
    let text_of = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    if value.get("v").and_then(Value::as_u64) != Some(1) {
        return Err(bad());
    }
    let seq = value.get("seq").and_then(Value::as_u64).ok_or_else(bad)?;
    if seq != expected_seq {
        return Err(bad());
    }
    Ok(Message {
        seq,
        ts_ms: value.get("ts_ms").and_then(Value::as_u64).ok_or_else(bad)?,
        role: text_of("role").as_deref().and_then(Role::parse).ok_or_else(bad)?,
        text: text_of("text").ok_or_else(bad)?,
        session: text_of("session"),
        turn: text_of("turn"),
        state: text_of("state").as_deref().and_then(MsgState::parse).ok_or_else(bad)?,
    })
}

fn encode_record(entry: Entry, seq: u64) -> [u8; INDEX_RECORD] {
    let mut out = [0u8; INDEX_RECORD];
    out[0..8].copy_from_slice(&entry.offset.to_le_bytes());
    out[8..12].copy_from_slice(&entry.len.to_le_bytes());
    out[12..20].copy_from_slice(&seq.to_le_bytes());
    out[20..28].copy_from_slice(&entry.ts_ms.to_le_bytes());
    out
}

fn encode_index(entries: &[Entry]) -> Vec<u8> {
    entries
        .iter()
        .enumerate()
        .flat_map(|(seq, entry)| encode_record(*entry, seq as u64))
        .collect()
}

fn read_session(dir: &Path) -> SessionState {
    let value = fs::read(dir.join(STATE_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    let Some(value) = value else {
        return SessionState::default();
    };
    let field = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    SessionState { hermes_session: field("hermes_session"), hermes_url: field("hermes_url") }
}

/// Writes through a temp file, fsyncs it, renames over `path`, then fsyncs the directory.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), AssistantError> {
    let tmp = path.with_extension("tmp");
    {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_dir;

    fn texts(messages: &[Message]) -> Vec<&str> {
        messages.iter().map(|m| m.text.as_str()).collect()
    }

    #[test]
    fn append_reopen_tail_and_paging() {
        let dir = test_dir("store-roundtrip");
        {
            let mut store = ChatStore::open(&dir).unwrap();
            for i in 0..5 {
                let seq = store
                    .append(Role::User, &format!("m{i}"), Some("s1"), Some("t"), MsgState::Complete)
                    .unwrap();
                assert_eq!(seq, i);
            }
            store.append(Role::Assistant, "héllo\nworld", None, None, MsgState::Partial).unwrap();
        }
        let mut store = ChatStore::open(&dir).unwrap();
        assert_eq!(store.len(), 6);
        let tail = store.tail(2).unwrap();
        assert_eq!(texts(&tail), vec!["m4", "héllo\nworld"]);
        assert_eq!(tail[1].role, Role::Assistant);
        assert_eq!(tail[1].state, MsgState::Partial);
        assert_eq!(tail[0].session.as_deref(), Some("s1"));
        let older = store.load_page(Some(4), 3).unwrap();
        assert_eq!(texts(&older), vec!["m1", "m2", "m3"]);
        assert_eq!(store.load_page(Some(0), 3).unwrap(), vec![]);
    }

    #[test]
    fn torn_tail_is_truncated_and_store_keeps_working() {
        let dir = test_dir("store-torn");
        {
            let mut store = ChatStore::open(&dir).unwrap();
            store.append(Role::User, "kept", None, None, MsgState::Complete).unwrap();
        }
        let log = dir.join(HISTORY_FILE);
        let mut bytes = fs::read(&log).unwrap();
        bytes.extend_from_slice(b"{\"v\":1,\"seq\":1,\"ts_m");
        fs::write(&log, bytes).unwrap();

        let mut store = ChatStore::open(&dir).unwrap();
        assert_eq!(store.len(), 1);
        let seq = store.append(Role::Assistant, "next", None, None, MsgState::Complete).unwrap();
        assert_eq!(seq, 1);
        drop(store);
        let mut store = ChatStore::open(&dir).unwrap();
        assert_eq!(texts(&store.tail(10).unwrap()), vec!["kept", "next"]);
    }

    #[test]
    fn stale_or_missing_index_is_rebuilt() {
        let dir = test_dir("store-index");
        {
            let mut store = ChatStore::open(&dir).unwrap();
            store.append(Role::User, "a", None, None, MsgState::Complete).unwrap();
            store.append(Role::User, "b", None, None, MsgState::Complete).unwrap();
        }
        fs::remove_file(dir.join(INDEX_FILE)).unwrap();
        let mut store = ChatStore::open(&dir).unwrap();
        assert_eq!(texts(&store.tail(10).unwrap()), vec!["a", "b"]);
        assert_eq!(fs::metadata(dir.join(INDEX_FILE)).unwrap().len(), 2 * INDEX_RECORD as u64);
        drop(store);

        fs::write(dir.join(INDEX_FILE), b"garbage").unwrap();
        let mut store = ChatStore::open(&dir).unwrap();
        assert_eq!(texts(&store.tail(10).unwrap()), vec!["a", "b"]);
    }

    #[test]
    fn window_is_bounded_and_pages_reload_from_disk() {
        let dir = test_dir("store-window");
        let mut store = ChatStore::open_with_cache(&dir, 3).unwrap();
        for i in 0..10 {
            store.append(Role::User, &format!("n{i}"), None, None, MsgState::Complete).unwrap();
        }
        assert!(store.cached() <= 3);
        let first = store.load_page(Some(4), 4).unwrap();
        assert_eq!(texts(&first), vec!["n0", "n1", "n2", "n3"]);
        assert!(store.cached() <= 3);
        assert_eq!(texts(&store.load_page(None, 2).unwrap()), vec!["n8", "n9"]);
    }

    #[test]
    fn session_state_round_trip() {
        let dir = test_dir("store-session");
        {
            let mut store = ChatStore::open(&dir).unwrap();
            assert_eq!(store.session(), &SessionState::default());
            store
                .set_session(SessionState {
                    hermes_session: Some("sess-42".into()),
                    hermes_url: Some("http://127.0.0.1:8642".into()),
                })
                .unwrap();
        }
        let store = ChatStore::open(&dir).unwrap();
        assert_eq!(store.session().hermes_session.as_deref(), Some("sess-42"));
        assert_eq!(store.session().hermes_url.as_deref(), Some("http://127.0.0.1:8642"));
        assert!(!dir.join("state.tmp").exists());
    }

    #[test]
    fn clear_removes_messages_and_session_durably() {
        let dir = test_dir("store-clear");
        {
            let mut store = ChatStore::open(&dir).unwrap();
            store.append(Role::User, "gone", Some("s"), None, MsgState::Complete).unwrap();
            store.set_session(SessionState { hermes_session: Some("s".into()), hermes_url: None }).unwrap();
            store.clear().unwrap();
            assert!(store.is_empty());
            assert_eq!(store.session().hermes_session, None);
            let seq = store.append(Role::User, "fresh", None, None, MsgState::Complete).unwrap();
            assert_eq!(seq, 0);
        }
        let mut store = ChatStore::open(&dir).unwrap();
        assert_eq!(texts(&store.tail(10).unwrap()), vec!["fresh"]);
        assert_eq!(store.session().hermes_session, None);
    }
}

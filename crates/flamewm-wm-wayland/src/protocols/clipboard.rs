//! Text selections outlive their client, as in KWin/Klipper and the smithay anvil example.
//!
//! When a client offers a text selection, the compositor reads the text once and re-publishes it as a
//! compositor-owned selection. The owner can then exit and paste still works. Smithay clears a dead
//! client's selection silently, so owner exit never reaches `SelectionHandler::new_selection`; only an
//! explicit clear (a focused client sending a null selection) does, which drops the cached text. A
//! selection larger than `MAX_TEXT_BYTES` or unreadable within `READ_TIMEOUT` is not kept.

use std::cell::Cell;
use std::collections::VecDeque;
use std::fs::File;
use std::io::Write;
use std::os::fd::OwnedFd;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use flamewm_api::clipboard::ClipboardHistoryEntry;
use smithay::input::Seat;
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{Interest, Mode, PostAction, RegistrationToken};
use smithay::wayland::selection::SelectionTarget;
use smithay::wayland::selection::data_device::{
    request_data_device_client_selection, set_data_device_selection,
};
use smithay::wayland::selection::primary_selection::{
    request_primary_client_selection, set_primary_selection,
};

use crate::state::State;

/// Text mime types in preference order; the first one a source offers is the one read.
pub const TEXT_MIMES: [&str; 3] = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING"];
/// Largest selection kept. Bigger selections are dropped and the selection is not re-published.
pub const MAX_TEXT_BYTES: usize = 1 << 20;
/// A source that has not finished sending within this time is not kept.
pub const READ_TIMEOUT: Duration = Duration::from_secs(2);
/// Clipboard history length. Klipper's default (`klipper/historymodel.cpp`, `MaxClipItems`).
pub const HISTORY_LEN: usize = 20;
/// Total bytes across history entries. Entries are shared with the current selection, so this cap
/// bounds retained clipboard RAM without copying anything when an entry is kept or re-selected.
pub const MAX_HISTORY_BYTES: usize = 8 << 20;
/// Preview length in characters for one history entry.
pub const PREVIEW_CHARS: usize = 120;
/// KDE/KeePassXC convention (`x-kde-passwordManagerHint`): a source offering it holds a secret.
pub const PASSWORD_HINT: &str = "x-kde-passwordManagerHint";

/// True when a selection's offered mime types mark it as a password-manager secret.
pub fn is_sensitive(offered: &[String]) -> bool {
    offered.iter().any(|mime| mime == PASSWORD_HINT)
}

/// Single-line preview of a history entry: the first non-empty line, trimmed, at most
/// `PREVIEW_CHARS` characters (ellipsis when cut).
pub fn preview(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    if line.chars().count() <= PREVIEW_CHARS {
        return line.to_owned();
    }
    flamewm_api::format::truncate_chars(line, PREVIEW_CHARS - 1)
}

/// Newest-first history of distinct text entries on CLIPBOARD. Each entry has a stable id.
/// Pure memory: no I/O, no timers.
#[derive(Default)]
pub struct TextHistory {
    next_id: u64,
    items: VecDeque<HistoryItem>,
    bytes: usize,
}

/// One kept entry. The preview is computed once at push, so listing never rescans the text.
/// The bytes are shared with the selection slot, so keeping an entry copies no payload.
struct HistoryItem {
    id: u64,
    bytes: Arc<[u8]>,
    preview: String,
}

impl TextHistory {
    /// Adds `bytes` at the front, moving an equal entry there with a fresh id. Returns false for
    /// empty text, non-UTF-8 bytes, or text above `MAX_TEXT_BYTES`. The caller passes the slot's
    /// shared bytes, so history never duplicates the text.
    pub fn push(&mut self, bytes: Arc<[u8]>) -> bool {
        if bytes.is_empty() || bytes.len() > MAX_TEXT_BYTES {
            return false;
        }
        let Ok(text) = std::str::from_utf8(&bytes) else {
            return false;
        };
        if let Some(pos) = self.items.iter().position(|item| item.bytes == bytes) {
            self.bytes -= self.items[pos].bytes.len();
            self.items.remove(pos);
        }
        self.next_id += 1;
        let preview = preview(text);
        self.bytes += bytes.len();
        self.items.push_front(HistoryItem {
            id: self.next_id,
            bytes,
            preview,
        });
        while self.items.len() > HISTORY_LEN || self.bytes > MAX_HISTORY_BYTES {
            let Some(old) = self.items.pop_back() else {
                break;
            };
            self.bytes -= old.bytes.len();
        }
        true
    }

    /// Listed entries newest first: id, preview and byte length. Never the full text.
    pub fn summaries(&self) -> Vec<ClipboardHistoryEntry> {
        self.items
            .iter()
            .map(|item| ClipboardHistoryEntry {
                id: item.id,
                preview: item.preview.clone(),
                len: item.bytes.len(),
            })
            .collect()
    }

    /// Shared bytes of entry `id`; re-selecting it copies a refcount, not the text.
    pub fn get(&self, id: u64) -> Option<Arc<[u8]>> {
        self.items
            .iter()
            .find(|item| item.id == id)
            .map(|item| Arc::clone(&item.bytes))
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.bytes = 0;
    }
}

/// The preferred text mime type among those a selection offers, or None for a non-text selection.
pub fn pick_text_mime(offered: &[String]) -> Option<&'static str> {
    TEXT_MIMES
        .into_iter()
        .find(|mime| offered.iter().any(|o| o == mime))
}

/// Bytes kept for one selection target, with the UTF-8 verdict cached at completion so serving a
/// paste never rescans a large payload on the compositor loop.
struct Kept {
    mime: &'static str,
    bytes: Arc<[u8]>,
    utf8: bool,
}

fn kept_bytes(mime: &'static str, bytes: Arc<[u8]>) -> Kept {
    let utf8 = std::str::from_utf8(&bytes).is_ok();
    Kept { mime, bytes, utf8 }
}

/// Text kept for one selection target. Generations invalidate reads that were started for an
/// earlier selection, so a late read cannot replace a newer one.
#[derive(Default)]
pub struct TextSlot {
    generation: u64,
    kept: Option<Kept>,
}

impl TextSlot {
    /// A new selection replaces the old text. Returns the generation the read must present.
    pub fn begin(&mut self) -> u64 {
        self.clear();
        self.generation
    }

    /// True while no newer selection or clear has happened since `generation` was issued.
    pub fn is_current(&self, generation: u64) -> bool {
        generation == self.generation
    }

    /// Explicit clear, or the start of a replacement. Invalidates any read in flight.
    pub fn clear(&mut self) {
        self.generation += 1;
        self.kept = None;
    }

    /// Keeps the bytes of a finished read. Returns false, and keeps nothing, when the read is stale,
    /// the source was unreadable (`None`), empty, or larger than `MAX_TEXT_BYTES`.
    pub fn complete(
        &mut self,
        generation: u64,
        mime: &'static str,
        bytes: Option<Vec<u8>>,
    ) -> bool {
        if generation != self.generation {
            return false;
        }
        self.kept = bytes
            .filter(|bytes| !bytes.is_empty() && bytes.len() <= MAX_TEXT_BYTES)
            .map(|bytes| kept_bytes(mime, Arc::from(bytes)));
        self.kept.is_some()
    }

    /// Keeps shared bytes already known to be UTF-8 text (a history entry). No copy, no rescan.
    pub fn complete_utf8(
        &mut self,
        generation: u64,
        mime: &'static str,
        bytes: Arc<[u8]>,
    ) -> bool {
        if generation != self.generation {
            return false;
        }
        self.kept = (!bytes.is_empty() && bytes.len() <= MAX_TEXT_BYTES).then_some(Kept {
            mime,
            bytes,
            utf8: true,
        });
        self.kept.is_some()
    }

    /// The kept bytes, when `mime` is the stored mime or, for UTF-8 text, any other text target.
    pub fn text(&self, mime: &str) -> Option<&[u8]> {
        self.matching(mime).map(|kept| kept.bytes.as_ref())
    }

    /// Shared handle to the kept bytes for a paste, so serving does not copy the payload on the loop.
    pub fn bytes(&self, mime: &str) -> Option<Arc<[u8]>> {
        self.matching(mime).map(|kept| Arc::clone(&kept.bytes))
    }

    fn matching(&self, mime: &str) -> Option<&Kept> {
        self.kept.as_ref().filter(|kept| {
            kept.mime == mime
                || (kept.utf8
                    && TEXT_MIMES.contains(&kept.mime)
                    && TEXT_MIMES.contains(&mime))
        })
    }

    /// Mime types a re-published selection offers. UTF-8 text advertises every text target, so a
    /// client that asks for `text/plain`, `UTF8_STRING` or `text/plain;charset=utf-8` finds it.
    pub fn offers(&self) -> Vec<String> {
        match self.kept.as_ref() {
            Some(kept) if kept.utf8 && TEXT_MIMES.contains(&kept.mime) => {
                TEXT_MIMES.iter().map(|m| (*m).to_owned()).collect()
            }
            Some(kept) => vec![kept.mime.to_owned()],
            None => Vec::new(),
        }
    }
}

/// Clipboard and PRIMARY text slots, held by `CompatState`.
#[derive(Default)]
pub struct TextPersistence {
    clipboard: TextSlot,
    primary: TextSlot,
    /// Clipboard text history (PRIMARY is not recorded).
    pub history: TextHistory,
}

impl TextPersistence {
    pub fn slot(&self, ty: SelectionTarget) -> &TextSlot {
        match ty {
            SelectionTarget::Clipboard => &self.clipboard,
            SelectionTarget::Primary => &self.primary,
        }
    }

    pub fn slot_mut(&mut self, ty: SelectionTarget) -> &mut TextSlot {
        match ty {
            SelectionTarget::Clipboard => &mut self.clipboard,
            SelectionTarget::Primary => &mut self.primary,
        }
    }
}

impl State {
    /// A client offered `offered` on `ty`: read its text off the loop and keep it. Any other selection
    /// drops the previous text immediately.
    pub(crate) fn read_client_selection(
        &mut self,
        ty: SelectionTarget,
        offered: &[String],
        seat: Seat<Self>,
    ) {
        // Password-manager secrets are never kept, in the slot or in the history.
        let Some(mime) = pick_text_mime(offered).filter(|_| !is_sensitive(offered)) else {
            self.compat.clipboard.slot_mut(ty).clear();
            return;
        };
        let generation = self.compat.clipboard.slot_mut(ty).begin();
        // Smithay reports the new owner before it stores the selection, so the request waits for idle.
        self.loop_handle.insert_idle(move |state| {
            state.start_text_read(ty, generation, mime, seat);
        });
    }

    fn start_text_read(
        &mut self,
        ty: SelectionTarget,
        generation: u64,
        mime: &'static str,
        seat: Seat<Self>,
    ) {
        if !self.compat.clipboard.slot(ty).is_current(generation) {
            return;
        }
        let Ok((reader, writer)) = std::io::pipe() else {
            return;
        };
        let writer = OwnedFd::from(writer);
        let requested = match ty {
            SelectionTarget::Clipboard => {
                request_data_device_client_selection(&seat, mime.to_owned(), writer)
                    .map_err(|error| error.to_string())
            }
            SelectionTarget::Primary => {
                request_primary_client_selection(&seat, mime.to_owned(), writer)
                    .map_err(|error| error.to_string())
            }
        };
        if let Err(error) = requested {
            eprintln!("flamewm-wayland: keep {ty:?} text: {error}");
            return;
        }

        // Both sources share their token so whichever finishes first removes the other.
        let timer_token = Rc::new(Cell::new(None::<RegistrationToken>));
        let read_token = Rc::new(Cell::new(None::<RegistrationToken>));
        let handle = self.loop_handle.clone();
        let timer = handle.insert_source(Timer::from_duration(READ_TIMEOUT), {
            let read_token = read_token.clone();
            move |_, (), state: &mut State| {
                if let Some(token) = read_token.take() {
                    state.loop_handle.remove(token);
                }
                state.finish_text_read(ty, generation, mime, None);
                TimeoutAction::Drop
            }
        });
        let Ok(timer) = timer else { return };
        timer_token.set(Some(timer));

        let mut bytes = Vec::new();
        let read = handle.insert_source(
            Generic::new(reader, Interest::READ, Mode::Level),
            move |_, reader, state: &mut State| {
                let mut chunk = [0_u8; 16 * 1024];
                // One read per readiness event: the fd is readable, so this returns without blocking.
                let outcome = match rustix::io::read(&**reader, &mut chunk) {
                    Ok(0) => Some(Some(std::mem::take(&mut bytes))),
                    Ok(n) => {
                        bytes.extend_from_slice(&chunk[..n]);
                        (bytes.len() > MAX_TEXT_BYTES).then_some(None)
                    }
                    Err(error) if error == rustix::io::Errno::INTR => {
                        return Ok(PostAction::Continue);
                    }
                    Err(_) => Some(None),
                };
                let Some(bytes) = outcome else {
                    return Ok(PostAction::Continue);
                };
                if let Some(token) = timer_token.take() {
                    state.loop_handle.remove(token);
                }
                state.finish_text_read(ty, generation, mime, bytes);
                Ok(PostAction::Remove)
            },
        );
        if let Ok(token) = read {
            read_token.set(Some(token));
        }
    }

    /// Keeps a finished read and re-publishes its text as a compositor-owned selection.
    fn finish_text_read(
        &mut self,
        ty: SelectionTarget,
        generation: u64,
        mime: &'static str,
        bytes: Option<Vec<u8>>,
    ) {
        if !self
            .compat
            .clipboard
            .slot_mut(ty)
            .complete(generation, mime, bytes)
        {
            return;
        }
        if ty == SelectionTarget::Clipboard
            && let Some(bytes) = self.compat.clipboard.slot(ty).bytes(mime)
        {
            // Shared with the slot: keeping history costs a refcount, not another copy of the text.
            self.compat.clipboard.history.push(bytes);
        }
        let seat = self.seat.clone();
        let offers = self.compat.clipboard.slot(ty).offers();
        match ty {
            SelectionTarget::Clipboard => {
                set_data_device_selection(&self.display, &seat, offers, ());
            }
            SelectionTarget::Primary => set_primary_selection(&self.display, &seat, offers, ()),
        }
    }
}

impl State {
    /// Clipboard history as the control surface lists it (ids and previews).
    pub(crate) fn clipboard_history(&self) -> Vec<ClipboardHistoryEntry> {
        self.compat.clipboard.history.summaries()
    }

    /// Makes history entry `id` the CLIPBOARD selection, served like a kept owner's text. False when
    /// the id is unknown.
    pub(crate) fn select_clipboard_history(&mut self, id: u64) -> bool {
        let Some(bytes) = self.compat.clipboard.history.get(id) else {
            return false;
        };
        let slot = self.compat.clipboard.slot_mut(SelectionTarget::Clipboard);
        let generation = slot.begin();
        slot.complete_utf8(generation, TEXT_MIMES[0], bytes);
        let seat = self.seat.clone();
        set_data_device_selection(&self.display, &seat, vec![TEXT_MIMES[0].to_owned()], ());
        true
    }

    /// Empties the clipboard history. The current selection is left as it is.
    pub(crate) fn clear_clipboard_history(&mut self) {
        self.compat.clipboard.history.clear();
    }
}

/// Writes kept text into a paste's fd. Off the loop so a slow reader cannot stall the compositor.
/// Takes the slot's shared bytes, so the compositor thread never copies the payload.
pub fn write_off_loop(bytes: Arc<[u8]>, fd: OwnedFd) {
    std::thread::spawn(move || {
        let _ = File::from(fd).write_all(&bytes);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mimes(list: &[&str]) -> Vec<String> {
        list.iter().map(|m| (*m).to_owned()).collect()
    }

    #[test]
    fn utf8_mime_is_preferred_over_plain_and_utf8_string() {
        assert_eq!(
            pick_text_mime(&mimes(&[
                "UTF8_STRING",
                "text/plain",
                "text/plain;charset=utf-8"
            ])),
            Some("text/plain;charset=utf-8")
        );
        assert_eq!(
            pick_text_mime(&mimes(&["text/plain", "UTF8_STRING"])),
            Some("text/plain")
        );
        assert_eq!(
            pick_text_mime(&mimes(&["UTF8_STRING"])),
            Some("UTF8_STRING")
        );
        assert_eq!(pick_text_mime(&mimes(&["image/png", "text/html"])), None);
        assert_eq!(pick_text_mime(&[]), None);
    }

    #[test]
    fn selection_at_cap_is_kept_and_one_byte_over_is_dropped() {
        let mut slot = TextSlot::default();
        let generation = slot.begin();
        assert!(slot.complete(generation, "text/plain", Some(vec![b'a'; MAX_TEXT_BYTES])));
        assert_eq!(
            slot.text("text/plain").map(<[u8]>::len),
            Some(MAX_TEXT_BYTES)
        );

        let generation = slot.begin();
        assert!(!slot.complete(
            generation,
            "text/plain",
            Some(vec![b'a'; MAX_TEXT_BYTES + 1])
        ));
        assert!(
            slot.text("text/plain").is_none(),
            "an oversize selection must not keep an older text"
        );
    }

    #[test]
    fn unreadable_or_empty_source_keeps_nothing() {
        let mut slot = TextSlot::default();
        let generation = slot.begin();
        assert!(
            !slot.complete(generation, "text/plain", None),
            "timeout or io error"
        );
        let generation = slot.begin();
        assert!(
            !slot.complete(generation, "text/plain", Some(Vec::new())),
            "empty text"
        );
    }

    #[test]
    fn owner_exit_keeps_text_while_explicit_clear_drops_it() {
        let mut slot = TextSlot::default();
        let generation = slot.begin();
        assert!(slot.complete(
            generation,
            "text/plain;charset=utf-8",
            Some(b"kept".to_vec())
        ));
        // Owner exit delivers no event, so nothing touches the slot: the text stays available.
        assert_eq!(slot.text("text/plain;charset=utf-8"), Some(&b"kept"[..]));
        // An explicit clear from a focused client drops it, and a read started before the clear is stale.
        let in_flight = slot.begin();
        slot.clear();
        assert!(!slot.complete(in_flight, "text/plain", Some(b"late".to_vec())));
        assert!(slot.text("text/plain").is_none());
        assert!(slot.text("text/plain;charset=utf-8").is_none());
    }

    #[test]
    fn stale_read_cannot_replace_a_newer_selection() {
        let mut slot = TextSlot::default();
        let old = slot.begin();
        let new = slot.begin();
        assert!(!slot.complete(old, "text/plain", Some(b"old".to_vec())));
        assert!(slot.complete(new, "text/plain", Some(b"new".to_vec())));
        assert_eq!(slot.text("text/plain"), Some(&b"new"[..]));
    }

    fn arc(text: &str) -> Arc<[u8]> {
        Arc::from(text.as_bytes())
    }

    fn text_entry(history: &TextHistory) -> Vec<String> {
        history
            .summaries()
            .into_iter()
            .map(|entry| entry.preview)
            .collect()
    }

    #[test]
    fn history_is_newest_first_and_dedupe_moves_to_front() {
        let mut history = TextHistory::default();
        assert!(history.push(arc("one")));
        assert!(history.push(arc("two")));
        assert!(history.push(arc("three")));
        assert_eq!(text_entry(&history), ["three", "two", "one"]);
        assert!(history.push(arc("one")));
        assert_eq!(text_entry(&history), ["one", "three", "two"]);
        let ids: Vec<u64> = history.summaries().iter().map(|entry| entry.id).collect();
        assert_eq!(ids.len(), 3);
        assert_eq!(
            ids.iter().collect::<std::collections::HashSet<_>>().len(),
            3
        );
    }

    #[test]
    fn history_keeps_twenty_entries_and_evicts_oldest() {
        let mut history = TextHistory::default();
        for tag in 0..=HISTORY_LEN {
            assert!(history.push(arc(&format!("item {tag}"))));
        }
        assert_eq!(history.summaries().len(), HISTORY_LEN);
        assert_eq!(
            history.summaries().first().map(|e| e.preview.as_str()),
            Some("item 20")
        );
        assert!(history.summaries().iter().all(|e| e.preview != "item 0"));
    }

    #[test]
    fn history_rejects_empty_and_oversize_and_clears() {
        let mut history = TextHistory::default();
        assert!(!history.push(String::new()));
        assert!(!history.push("x".repeat(MAX_TEXT_BYTES + 1)));
        assert!(history.push("x".repeat(MAX_TEXT_BYTES)));
        assert_eq!(history.summaries().len(), 1);
        history.clear();
        assert_eq!(history.summaries().len(), 0);
    }

    #[test]
    fn history_select_by_id_and_unknown_id() {
        let mut history = TextHistory::default();
        history.push("first".into());
        history.push("second".into());
        let first_id = history.summaries().last().unwrap().id;
        assert_eq!(history.get(first_id), Some("first"));
        assert_eq!(history.get(u64::MAX), None);
    }

    #[test]
    fn sensitive_hint_marks_selection_secret() {
        assert!(is_sensitive(&mimes(&["text/plain", PASSWORD_HINT])));
        assert!(!is_sensitive(&mimes(&["text/plain;charset=utf-8"])));
    }

    #[test]
    fn preview_is_one_line_and_capped() {
        assert_eq!(preview("  first line \nsecond"), "first line");
        assert_eq!(preview("\n\n  \nreal"), "real");
        assert_eq!(preview(""), "");
        let long = "a".repeat(300);
        let shown = preview(&long);
        assert_eq!(shown.chars().count(), PREVIEW_CHARS);
        assert!(shown.ends_with('\u{2026}'));
    }

    #[test]
    fn utf8_text_is_served_and_offered_for_every_text_target() {
        // Browser-style owner offers only text/plain; the re-published selection must still satisfy
        // a terminal that asks for text/plain;charset=utf-8 (repro: scripts in .agents/runtime/paste).
        let mut slot = TextSlot::default();
        let generation = slot.begin();
        slot.complete(generation, "text/plain", Some(b"x".to_vec()));
        assert_eq!(slot.text("text/plain"), Some(&b"x"[..]));
        assert_eq!(slot.text("UTF8_STRING"), Some(&b"x"[..]));
        assert_eq!(slot.text("text/plain;charset=utf-8"), Some(&b"x"[..]));
        assert_eq!(slot.offers(), TEXT_MIMES.map(str::to_owned).to_vec());
        assert!(slot.text("text/html").is_none());
    }

    #[test]
    fn non_utf8_text_is_served_and_offered_only_as_its_own_mime() {
        let mut slot = TextSlot::default();
        let generation = slot.begin();
        slot.complete(generation, "text/plain", Some(vec![0xff, b'a']));
        assert_eq!(slot.offers(), vec!["text/plain".to_owned()]);
        assert_eq!(slot.text("text/plain"), Some(&[0xff, b'a'][..]));
        assert!(slot.text("UTF8_STRING").is_none());
    }
}

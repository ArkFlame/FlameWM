//! Pure view-model for the assistant window: bounded, lazily paged history
//! for the chat list and the key rules for the composer. No UI or I/O types live here, so
//! every rule is unit-tested without an X11 session.

use std::collections::VecDeque;

use flamewm_assistant_core::{Message, Provider, Role};

/// Bubble slots that exist in the document. Node count stays bounded by this.
pub const SLOTS: usize = 12;
/// Messages fetched per page from the store.
pub const PAGE: usize = 40;
/// Messages kept in memory. Messages beyond the viewport are dropped from the far side.
pub const MAX_LOADED: usize = 96;
/// Fetch older messages when the viewport is this close to the top.
const NEAR_TOP: usize = 3;
/// Input length limit in characters.
pub const MAX_INPUT: usize = 4_000;
/// Pixels the bubble list may use. The window is 680 px tall; the composer, notice and working
/// row take the rest. Bubble heights are estimates, so the budget leaves room for error.
const LIST_BUDGET: usize = 400;
/// Body font size and the text width inside a bubble (max-width 330 px less 13 px padding a side).
/// Heights come from the renderer's own text layout, so the budget matches what is drawn.
const BODY_PX: f32 = 14.0;
const BUBBLE_TEXT_PX: f32 = 304.0;
/// Bubble vertical padding plus the row gap.
const BUBBLE_PAD_PX: usize = 26;
/// Longest text drawn in one bubble. Longer replies are cut with an ellipsis so a single bubble
/// always fits the budget. The full text stays in the store.
const MAX_SHOWN_CHARS: usize = 600;

/// Text drawn in a bubble.
fn shown(text: &str) -> String {
    if text.chars().count() <= MAX_SHOWN_CHARS {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(MAX_SHOWN_CHARS).collect();
    cut.push_str(" \u{2026}");
    cut
}

/// Bubble height in pixels for a message text, measured by the renderer's text layout.
fn bubble_px(text: impl AsRef<str>) -> usize {
    let layout = flamewm_render_core::layout_text(
        text.as_ref(),
        BODY_PX,
        BUBBLE_TEXT_PX,
        flamewm_render_core::TextWrap::Wrap,
        false,
    );
    layout.height.ceil() as usize + BUBBLE_PAD_PX
}

/// How many messages, taken in order, fit `slots` and the budget after `base` pixels.
fn fitting<'a>(msgs: impl Iterator<Item = &'a Message>, base: usize, slots: usize) -> usize {
    let mut total = base;
    let mut count = 0;
    for msg in msgs {
        if count >= slots {
            break;
        }
        let height = bubble_px(shown(&msg.text));
        if total + height > LIST_BUDGET {
            break;
        }
        total += height;
        count += 1;
    }
    count
}

/// Source of stored messages, oldest first. `before` is exclusive; `None` means the newest.
pub trait Pager {
    fn page(&mut self, before: Option<u64>, limit: usize) -> Result<Vec<Message>, String>;
}

/// One bubble to draw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub role: Role,
    pub text: String,
}

/// Loaded slice of the chat (`lo..hi` in seq numbers), the viewport, and the streaming reply.
#[derive(Debug, Default)]
pub struct History {
    msgs: VecDeque<Message>,
    lo: u64,
    hi: u64,
    total: u64,
    /// Index into `msgs` of the first message in the viewport.
    top: usize,
    /// True when the viewport sits on the newest message and new messages should scroll in.
    follow: bool,
    /// Streaming assistant text for the turn in flight.
    pending: Option<String>,
}

impl History {
    /// Loads the newest page of a store holding `total` messages.
    pub fn open(pager: &mut impl Pager, total: u64) -> Result<Self, String> {
        let msgs: VecDeque<Message> = pager.page(None, PAGE)?.into();
        let len = msgs.len();
        let mut history = Self {
            msgs,
            lo: total.saturating_sub(len as u64),
            hi: total,
            total,
            top: 0,
            follow: true,
            pending: None,
        };
        history.top = history.max_top();
        Ok(history)
    }

    /// Bubbles the document can show: slots minus the streaming bubble when it has text.
    fn capacity(&self) -> usize {
        SLOTS - usize::from(self.pending_visible())
    }

    fn pending_visible(&self) -> bool {
        self.pending.as_deref().is_some_and(|text| !text.is_empty())
    }

    /// Smallest viewport start whose messages (and the streaming bubble) fit the list budget,
    /// counted from the newest message. Keeps the newest message and the composer on screen.
    fn max_top(&self) -> usize {
        let base = self.pending_height();
        let count = fitting(self.msgs.iter().rev(), base, self.capacity());
        self.msgs.len() - count
    }

    fn pending_height(&self) -> usize {
        self.pending.as_deref().filter(|text| !text.is_empty()).map_or(0, |text| bubble_px(shown(text)))
    }

    /// Records a message that was just appended to the store with seq `msg.seq`.
    pub fn push(&mut self, msg: Message) {
        self.total = self.total.max(msg.seq + 1);
        if msg.seq != self.hi {
            // The viewport is far from the tail, so the message loads later from the store.
            return;
        }
        self.msgs.push_back(msg);
        self.hi += 1;
        if self.follow {
            self.top = self.max_top();
        }
        self.trim();
    }

    /// Forgets every loaded message and the viewport, as after a clear.
    pub fn clear(&mut self) {
        *self = Self { follow: true, ..Self::default() };
    }

    /// Replaces the streaming text. `None` ends the turn.
    pub fn set_pending(&mut self, text: Option<String>) {
        self.pending = text;
        if self.follow {
            self.top = self.max_top();
        }
    }

    pub fn append_pending(&mut self, delta: &str) {
        self.pending.get_or_insert_with(String::new).push_str(delta);
        if self.follow {
            self.top = self.max_top();
        }
    }

    /// Ends the streaming turn and returns the text streamed so far.
    pub fn take_pending(&mut self) -> Option<String> {
        let text = self.pending.take();
        if self.follow {
            self.top = self.max_top();
        }
        text
    }

    /// Moves the viewport by `delta` messages (negative is older). Loads a neighbour page when
    /// the viewport reaches the edge of the loaded slice.
    pub fn scroll(&mut self, pager: &mut impl Pager, delta: isize) -> Result<(), String> {
        if delta < 0 && self.top <= NEAR_TOP && self.lo > 0 {
            self.load_older(pager)?;
        }
        if delta > 0 && self.hi < self.total && self.top + self.capacity() + NEAR_TOP >= self.msgs.len() {
            self.load_newer(pager)?;
        }
        let max = self.max_top() as isize;
        self.top = (self.top as isize + delta).clamp(0, max) as usize;
        self.follow = self.hi == self.total && self.top as isize >= max;
        self.trim();
        Ok(())
    }

    fn load_older(&mut self, pager: &mut impl Pager) -> Result<(), String> {
        let page = pager.page(Some(self.lo), PAGE)?;
        let count = page.len();
        for msg in page.into_iter().rev() {
            self.msgs.push_front(msg);
        }
        self.lo -= count as u64;
        self.top += count;
        Ok(())
    }

    fn load_newer(&mut self, pager: &mut impl Pager) -> Result<(), String> {
        let end = (self.hi + PAGE as u64).min(self.total);
        let want = (end - self.hi) as usize;
        let page = pager.page(Some(end), want)?;
        self.hi += page.len() as u64;
        self.msgs.extend(page);
        Ok(())
    }

    /// Drops loaded messages beyond `MAX_LOADED`. Messages above the viewport go first, then
    /// messages below it. Dropped messages reload through `scroll`.
    fn trim(&mut self) {
        let excess = self.msgs.len().saturating_sub(MAX_LOADED);
        if excess == 0 {
            return;
        }
        let front = excess.min(self.top);
        for _ in 0..front {
            self.msgs.pop_front();
        }
        self.lo += front as u64;
        self.top -= front;
        let back = excess - front;
        for _ in 0..back {
            self.msgs.pop_back();
        }
        self.hi -= back as u64;
        if back > 0 {
            self.follow = false;
        }
    }

    /// Bubbles for the viewport, oldest first, at most `SLOTS` in total.
    pub fn rows(&self) -> Vec<Row> {
        let count = fitting(self.msgs.iter().skip(self.top), self.pending_height(), self.capacity());
        let mut rows: Vec<Row> = self
            .msgs
            .iter()
            .skip(self.top)
            .take(count)
            .map(|msg| Row { role: msg.role, text: shown(&msg.text) })
            .collect();
        if let Some(text) = self.pending.as_deref().filter(|text| !text.is_empty()) {
            rows.push(Row { role: Role::Assistant, text: shown(text) });
        }
        rows
    }
}

/// Composer input, decoded from a key press.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyCmd {
    Insert(String),
    Newline,
    Backspace,
    Submit,
    Cancel,
    Ignore,
}

/// Enter sends, Shift+Enter inserts a newline, Escape cancels a running turn.
pub fn key_command(key: &str, text: Option<&str>, shift: bool, ctrl_or_alt: bool) -> KeyCmd {
    // The X11 backend reports canonical keysym names ("Return", "Escape"); match them case-free.
    let key = key.to_ascii_lowercase();
    match key.as_str() {
        "return" | "enter" | "kp_enter" if shift => KeyCmd::Newline,
        "return" | "enter" | "kp_enter" => KeyCmd::Submit,
        "escape" | "esc" => KeyCmd::Cancel,
        "backspace" => KeyCmd::Backspace,
        _ if ctrl_or_alt => KeyCmd::Ignore,
        _ => match text {
            Some(text) if !text.is_empty() && text.chars().all(|c| !c.is_control()) => {
                KeyCmd::Insert(text.to_owned())
            }
            _ => KeyCmd::Ignore,
        },
    }
}

/// Applies an insert, keeping the input within `MAX_INPUT` characters.
pub fn insert_text(input: &mut String, text: &str) {
    let room = MAX_INPUT.saturating_sub(input.chars().count());
    input.extend(text.chars().take(room));
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_assistant_core::MsgState;

    /// In-memory store with `count` messages, seq 0..count, oldest first.
    struct Fake {
        count: u64,
        calls: usize,
    }

    impl Fake {
        fn new(count: u64) -> Self {
            Self { count, calls: 0 }
        }
    }

    fn msg(seq: u64) -> Message {
        Message {
            seq,
            ts_ms: 0,
            role: if seq % 2 == 0 { Role::User } else { Role::Assistant },
            text: format!("m{seq}"),
            session: None,
            turn: None,
            state: MsgState::Complete,
        }
    }

    impl Pager for Fake {
        fn page(&mut self, before: Option<u64>, limit: usize) -> Result<Vec<Message>, String> {
            self.calls += 1;
            let end = before.map_or(self.count, |b| b.min(self.count));
            let start = end.saturating_sub(limit as u64);
            Ok((start..end).map(msg).collect())
        }
    }

    fn texts(history: &History) -> Vec<String> {
        history.rows().into_iter().map(|row| row.text).collect()
    }

    #[test]
    fn opens_on_the_newest_page_and_shows_only_slots() {
        let mut store = Fake::new(200);
        let history = History::open(&mut store, 200).unwrap();
        assert_eq!(history.msgs.len(), PAGE);
        let rows = history.rows();
        assert!(!rows.is_empty() && rows.len() <= SLOTS);
        assert_eq!(rows.last().unwrap().text, "m199");
    }

    #[test]
    fn scrolling_to_the_top_loads_older_pages_and_stays_bounded() {
        let mut store = Fake::new(400);
        let mut history = History::open(&mut store, 400).unwrap();
        for _ in 0..2_000 {
            if texts(&history).first().map(String::as_str) == Some("m0") {
                break;
            }
            history.scroll(&mut store, -1).unwrap();
            assert!(history.msgs.len() <= MAX_LOADED, "loaded {}", history.msgs.len());
            assert!(history.rows().len() <= SLOTS);
        }
        // Reached the oldest message: the viewport now starts at seq 0.
        assert_eq!(texts(&history).first().map(String::as_str), Some("m0"));
        assert!(store.calls > 1);
    }

    #[test]
    fn scrolling_down_reloads_newer_messages_after_unload() {
        let mut store = Fake::new(400);
        let mut history = History::open(&mut store, 400).unwrap();
        for _ in 0..200 {
            history.scroll(&mut store, -1).unwrap();
        }
        for _ in 0..600 {
            history.scroll(&mut store, 1).unwrap();
        }
        assert_eq!(texts(&history).last().map(String::as_str), Some("m399"));
        assert!(history.msgs.len() <= MAX_LOADED);
    }

    #[test]
    fn new_messages_follow_when_at_the_bottom_and_not_otherwise() {
        let mut store = Fake::new(30);
        let mut history = History::open(&mut store, 30).unwrap();
        history.push(msg(30));
        assert_eq!(texts(&history).last().map(String::as_str), Some("m30"));

        // Scrolled up: a new tail message does not move the viewport.
        history.scroll(&mut store, -5).unwrap();
        let before = texts(&history);
        history.push(msg(31));
        assert_eq!(texts(&history), before);
        assert_eq!(history.total, 32);
    }

    #[test]
    fn streaming_text_takes_the_last_bubble_and_reduces_capacity() {
        let mut store = Fake::new(30);
        let mut history = History::open(&mut store, 30).unwrap();
        let idle = history.rows().len();
        history.set_pending(Some(String::new()));
        assert_eq!(history.rows().len(), idle, "empty stream stays hidden");
        history.append_pending("Hel");
        history.append_pending("lo");
        let rows = history.rows();
        assert!(rows.len() <= SLOTS);
        assert_eq!(rows.last().unwrap().text, "Hello");
        assert_eq!(rows.last().unwrap().role, Role::Assistant);
        history.set_pending(None);
        assert!(history.pending.is_none());
        assert_eq!(history.rows().last().unwrap().text, "m29");
    }

    #[test]
    fn enter_sends_shift_enter_adds_a_line_and_escape_cancels() {
        assert_eq!(key_command("return", None, false, false), KeyCmd::Submit);
        assert_eq!(key_command("kp_enter", None, false, false), KeyCmd::Submit);
        assert_eq!(key_command("return", None, true, false), KeyCmd::Newline);
        assert_eq!(key_command("escape", None, false, false), KeyCmd::Cancel);
        assert_eq!(key_command("backspace", None, false, false), KeyCmd::Backspace);
        assert_eq!(key_command("a", Some("a"), false, false), KeyCmd::Insert("a".into()));
        assert_eq!(key_command("a", Some("a"), false, true), KeyCmd::Ignore);
        assert_eq!(key_command("Shift_L", None, false, false), KeyCmd::Ignore);
    }

    #[test]
    fn backend_key_names_are_matched_case_free() {
        assert_eq!(key_command("Return", None, false, false), KeyCmd::Submit);
        assert_eq!(key_command("Return", None, true, false), KeyCmd::Newline);
        assert_eq!(key_command("Escape", None, false, false), KeyCmd::Cancel);
        assert_eq!(key_command("BackSpace", None, false, false), KeyCmd::Backspace);
    }

    #[test]
    fn input_is_capped() {
        let mut input = "x".repeat(MAX_INPUT - 2);
        insert_text(&mut input, "abcdef");
        assert_eq!(input.chars().count(), MAX_INPUT);
    }

    #[test]
    fn long_replies_are_cut_and_the_newest_always_fits_the_budget() {
        let long = "word ".repeat(2_000);
        let mut store = Fake::new(3);
        let mut history = History::open(&mut store, 3).unwrap();
        history.push(Message { seq: 3, text: long.clone(), ..msg(3) });
        history.set_pending(Some(long.clone()));
        let rows = history.rows();
        let heights: usize = rows.iter().map(|row| bubble_px(&row.text)).sum();
        assert!(heights <= LIST_BUDGET, "list needs {heights} px");
        assert!(rows.iter().all(|row| row.text.chars().count() <= MAX_SHOWN_CHARS + 2));
        assert_eq!(rows.last().unwrap().role, Role::Assistant);
    }

    #[test]
    fn budget_limits_rows_when_bubbles_are_tall() {
        let mut store = Fake::new(40);
        let history = History::open(&mut store, 40).unwrap();
        let heights: usize = history.rows().iter().map(|row| bubble_px(&row.text)).sum();
        assert!(heights <= LIST_BUDGET);
        assert!(history.rows().len() < SLOTS, "rows must stop at the height budget");
    }
}

/// Indices of `models` whose id contains `query`, ignoring case. An empty query keeps every id.
pub fn filter_models(models: &[String], query: &str) -> Vec<usize> {
    let needle = query.trim().to_lowercase();
    models
        .iter()
        .enumerate()
        .filter(|(_, id)| needle.is_empty() || id.to_lowercase().contains(&needle))
        .map(|(index, _)| index)
        .collect()
}

/// One row of the grouped model picker. A provider's models sit under its header row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuRow {
    Header(Provider),
    Model { provider: Provider, id: String },
}

/// Flattened picker rows for `groups`, in order. The query filters ids in every group, and a group
/// with no match is dropped together with its header.
pub fn menu_rows(groups: &[(Provider, &[String])], query: &str) -> Vec<MenuRow> {
    let mut rows = Vec::new();
    for (provider, ids) in groups {
        let matches = filter_models(ids, query);
        if matches.is_empty() {
            continue;
        }
        rows.push(MenuRow::Header(*provider));
        rows.extend(matches.into_iter().map(|index| MenuRow::Model { provider: *provider, id: ids[index].clone() }));
    }
    rows
}

#[cfg(test)]
mod menu_rows_tests {
    use super::{menu_rows, MenuRow};
    use flamewm_assistant_core::Provider;

    fn ark() -> Vec<String> {
        ["lite", "flash", "pro"].iter().map(|s| s.to_string()).collect()
    }

    fn proxy() -> Vec<String> {
        ["a/b:free", "c/d"].iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn each_provider_group_starts_with_its_header() {
        let (ark, proxy) = (ark(), proxy());
        let rows = menu_rows(&[(Provider::Ark, &ark), (Provider::HermesProxy, &proxy)], "");
        assert_eq!(rows.len(), 1 + 3 + 1 + 2);
        assert_eq!(rows[0], MenuRow::Header(Provider::Ark));
        assert_eq!(rows[4], MenuRow::Header(Provider::HermesProxy));
        assert_eq!(rows[5], MenuRow::Model { provider: Provider::HermesProxy, id: "a/b:free".into() });
    }

    #[test]
    fn missing_proxy_group_has_no_header() {
        let ark = ark();
        let rows = menu_rows(&[(Provider::Ark, &ark)], "");
        assert_eq!(rows.len(), 4);
        assert!(rows.iter().all(|row| !matches!(row, MenuRow::Model { provider: Provider::HermesProxy, .. })));
    }

    #[test]
    fn search_spans_groups_and_hides_groups_without_matches() {
        let (ark, proxy) = (ark(), proxy());
        let rows = menu_rows(&[(Provider::Ark, &ark), (Provider::HermesProxy, &proxy)], "free");
        assert_eq!(
            rows,
            vec![
                MenuRow::Header(Provider::HermesProxy),
                MenuRow::Model { provider: Provider::HermesProxy, id: "a/b:free".into() },
            ]
        );
    }

    #[test]
    fn no_match_anywhere_is_empty() {
        let (ark, proxy) = (ark(), proxy());
        assert!(menu_rows(&[(Provider::Ark, &ark), (Provider::HermesProxy, &proxy)], "zzz").is_empty());
    }
}

#[cfg(test)]
mod model_filter_tests {
    use super::filter_models;

    fn ids() -> Vec<String> {
        ["poolside/laguna:free", "openai/gpt-x", "Meta/Llama:FREE", "a/b-free-plus"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn empty_query_keeps_everything_in_order() {
        assert_eq!(filter_models(&ids(), ""), vec![0, 1, 2, 3]);
        assert_eq!(filter_models(&ids(), "   "), vec![0, 1, 2, 3]);
    }

    #[test]
    fn substring_match_is_case_insensitive() {
        assert_eq!(filter_models(&ids(), "free"), vec![0, 2, 3]);
        assert_eq!(filter_models(&ids(), "GPT"), vec![1]);
        assert_eq!(filter_models(&ids(), "llama:f"), vec![2]);
    }

    #[test]
    fn no_match_is_empty() {
        assert!(filter_models(&ids(), "zzz").is_empty());
    }
}

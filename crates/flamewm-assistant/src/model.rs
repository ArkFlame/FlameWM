//! Pure view-model for the assistant window: bounded, lazily paged history
//! for the chat list and the key rules for the composer. No UI or I/O types live here, so
//! every rule is unit-tested without an X11 session.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};

use flamewm_assistant_core::{Message, Provider, Role};

/// Bubble slots that exist in the document. Node count stays bounded by this.
pub const SLOTS: usize = 24;
/// Messages fetched per page from the store.
pub const PAGE: usize = 40;
/// Messages kept in memory. Messages beyond the viewport are dropped from the far side.
pub const MAX_LOADED: usize = 96;
/// Fetch older messages when the viewport is this close to the top.
const NEAR_TOP: usize = 3;
/// Pixels the bubble list may use. The window is 780 px tall; the composer, notice and working
/// row take the rest. Bubble heights are estimates, so the budget leaves room for error.
const LIST_BUDGET: usize = 500;
/// Height of the window around the list: header, composer, notice and paddings.
pub const CHROME_PX: usize = 280;
static LIST_BUDGET_PX: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(LIST_BUDGET);

/// Sets the list budget for a window of `height` pixels (a popover can be shorter than `HEIGHT`).
pub fn set_window_height(height: usize) {
    let budget = height.saturating_sub(CHROME_PX).clamp(120, LIST_BUDGET);
    LIST_BUDGET_PX.store(budget, std::sync::atomic::Ordering::Relaxed);
}

fn list_budget() -> usize {
    LIST_BUDGET_PX.load(std::sync::atomic::Ordering::Relaxed)
}
/// Body font size and the text width inside a bubble (`.bubble` max-width 340 px, border-box, less
/// 13 px padding a side). Heights come from the renderer's own text layout, so the budget matches
/// what is drawn.
const BODY_PX: f32 = 14.0;
const BUBBLE_TEXT_PX: f32 = 314.0;
/// Bubble vertical padding plus the row gap.
const BUBBLE_PAD_PX: usize = 26;
/// Bubble height in pixels for a message text, measured by the renderer's text layout. The whole
/// text is measured: a long reply is taller than the budget and is clipped by the list, not cut.
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

/// True when the text needs more than one line in a bubble.
pub fn wraps(text: &str) -> bool {
    bubble_px(text) > bubble_px("x")
}

/// Composer input font size, and the one-line input box height the pill is designed around.
const INPUT_PX: f32 = 14.0;
const INPUT_BOX_PX: f32 = 38.0;
/// Composer pill height around a one-line input box.
const PILL_PX: f32 = 50.0;
/// Horizontal inset of the input text inside the input area.
const INPUT_PAD_PX: f32 = 8.0;
/// Input lines shown before the box stops growing and scrolls to the tail.
pub const INPUT_MAX_LINES: usize = 5;
/// Caret bar height: a glyph's em box, centred on its line box.
const CARET_PX: f32 = 16.0;

/// Composer input geometry. Offsets are relative to the input area's content box (inside its
/// padding), which is the containing block of the absolutely positioned input box.
#[derive(Clone, Debug, PartialEq)]
pub struct Composer {
    /// Input box height: one line, plus each further shown line.
    pub box_h: f32,
    /// Pill height that holds the box with the same margins as a one-line pill.
    pub pill_h: f32,
    /// Text block width (the area's content width).
    pub text_w: f32,
    /// Laid-out text height.
    pub text_h: f32,
    /// Text block top. Negative once the text overflows and the box shows its tail.
    pub text_top: f32,
    /// Caret top-left relative to the text block, and its height.
    pub caret: (f32, f32),
    pub caret_h: f32,
}

/// Composer geometry for `text` in an input area `area_w` wide, from the renderer's own text layout
/// at the real wrap width, so the caret sits where the painted glyphs end. Painted spaces collapse,
/// so trailing spaces are added to the caret's advance.
#[must_use]
pub fn composer(text: &str, area_w: f32) -> Composer {
    let text_w = (area_w - 2.0 * INPUT_PAD_PX).max(1.0);
    let wrap = flamewm_render_core::TextWrap::Wrap;
    let layout = flamewm_render_core::layout_text(text, INPUT_PX, text_w, wrap, false);
    let space_w = flamewm_render_core::layout_text("\u{a0}", INPUT_PX, text_w, wrap, false).width;
    let line_h = layout.line_height;
    let lines = layout.lines.len().max(1);
    let shown = lines.min(INPUT_MAX_LINES);
    let box_h = INPUT_BOX_PX + (shown - 1) as f32 * line_h;
    let text_h = lines as f32 * line_h;
    // Bottom inset of the box below the last line; a one-line box centres its line.
    let inset = (INPUT_BOX_PX - line_h) / 2.0;
    // Text is bottom-aligned to the box: a short one centres, a tall one shows its tail.
    let text_top = box_h - inset - text_h;
    let trailing = text.len() - text.trim_end_matches(' ').len();
    let last_line = layout.lines.last().map_or(0.0, |line| line.width);
    let caret_x = (last_line + trailing as f32 * space_w).min(text_w);
    let caret_y = (lines - 1) as f32 * line_h + (line_h - CARET_PX) / 2.0;
    Composer {
        box_h,
        pill_h: PILL_PX + (box_h - INPUT_BOX_PX),
        text_w,
        text_h,
        text_top,
        caret: (caret_x, caret_y),
        caret_h: CARET_PX,
    }
}

/// How many messages, taken in order, fit `slots` and the budget after `base` pixels.
fn fitting<'a>(
    msgs: impl Iterator<Item = &'a Message>,
    base: usize,
    slots: usize,
    cache: &RefCell<HashMap<u64, (usize, usize)>>,
) -> usize {
    let mut total = base;
    let mut count = 0;
    for msg in msgs {
        if count >= slots {
            break;
        }
        // Layout is the costly part and runs on every paint, so heights are cached per message.
        let len = msg.text.len();
        let cached = cache
            .borrow()
            .get(&msg.seq)
            .filter(|(l, _)| *l == len)
            .map(|(_, h)| *h);
        let height = cached.unwrap_or_else(|| {
            let h = bubble_px(&msg.text);
            let mut cache = cache.borrow_mut();
            if cache.len() > 4 * MAX_LOADED {
                cache.clear();
            }
            cache.insert(msg.seq, (len, h));
            h
        });
        // The first message always gets a row, so a single reply taller than the budget still shows.
        if count > 0 && total + height > list_budget() {
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
    /// Bubble heights by seq: `(text length, pixels)`.
    heights: RefCell<HashMap<u64, (usize, usize)>>,
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
            heights: RefCell::default(),
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
        let count = fitting(self.msgs.iter().rev(), base, self.capacity(), &self.heights);
        self.msgs.len().saturating_sub(count)
    }

    fn pending_height(&self) -> usize {
        self.pending
            .as_deref()
            .filter(|text| !text.is_empty())
            .map_or(0, bubble_px)
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
        *self = Self {
            follow: true,
            ..Self::default()
        };
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
        if delta > 0
            && self.hi < self.total
            && self.top + self.capacity() + NEAR_TOP >= self.msgs.len()
        {
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
        let count = fitting(
            self.msgs.iter().skip(self.top),
            self.pending_height(),
            self.capacity(),
            &self.heights,
        );
        let mut rows: Vec<Row> = self
            .msgs
            .iter()
            .skip(self.top)
            .take(count)
            .map(|msg| Row {
                role: msg.role,
                text: msg.text.clone(),
            })
            .collect();
        if let Some(text) = self.pending.as_deref().filter(|text| !text.is_empty()) {
            rows.push(Row {
                role: Role::Assistant,
                text: text.to_owned(),
            });
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

/// Applies an insert. The input has no length limit.
pub fn insert_text(input: &mut String, text: &str) {
    input.push_str(text);
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
            role: if seq % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            },
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
            assert!(
                history.msgs.len() <= MAX_LOADED,
                "loaded {}",
                history.msgs.len()
            );
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
        assert_eq!(
            key_command("backspace", None, false, false),
            KeyCmd::Backspace
        );
        assert_eq!(
            key_command("a", Some("a"), false, false),
            KeyCmd::Insert("a".into())
        );
        assert_eq!(key_command("a", Some("a"), false, true), KeyCmd::Ignore);
        assert_eq!(key_command("Shift_L", None, false, false), KeyCmd::Ignore);
    }

    #[test]
    fn backend_key_names_are_matched_case_free() {
        assert_eq!(key_command("Return", None, false, false), KeyCmd::Submit);
        assert_eq!(key_command("Return", None, true, false), KeyCmd::Newline);
        assert_eq!(key_command("Escape", None, false, false), KeyCmd::Cancel);
        assert_eq!(
            key_command("BackSpace", None, false, false),
            KeyCmd::Backspace
        );
    }

    #[test]
    fn input_has_no_length_limit() {
        let mut input = "x".repeat(100_000);
        insert_text(&mut input, "abcdef");
        assert_eq!(input.chars().count(), 100_006);
        assert!(input.ends_with("abcdef"));
    }

    #[test]
    fn long_replies_are_kept_whole_and_the_newest_is_always_a_row() {
        let long = "word ".repeat(2_000);
        let mut store = Fake::new(3);
        let mut history = History::open(&mut store, 3).unwrap();
        history.push(Message {
            seq: 3,
            text: long.clone(),
            ..msg(3)
        });
        history.set_pending(Some(long.clone()));
        let rows = history.rows();
        assert!(
            rows.iter().any(|row| row.text == long),
            "reply text must not be cut"
        );
        assert_eq!(rows.last().unwrap().role, Role::Assistant);
        assert!(!rows.is_empty() && rows.len() <= SLOTS);
    }

    #[test]
    fn budget_limits_rows_when_bubbles_are_tall() {
        let mut store = Fake::new(40);
        let history = History::open(&mut store, 40).unwrap();
        let heights: usize = history.rows().iter().map(|row| bubble_px(&row.text)).sum();
        assert!(heights <= LIST_BUDGET);
        assert!(
            history.rows().len() < SLOTS,
            "rows must stop at the height budget"
        );
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
        rows.extend(matches.into_iter().map(|index| MenuRow::Model {
            provider: *provider,
            id: ids[index].clone(),
        }));
    }
    rows
}

#[cfg(test)]
mod menu_rows_tests {
    use super::{MenuRow, menu_rows};
    use flamewm_assistant_core::Provider;

    fn ark() -> Vec<String> {
        ["lite", "flash", "pro"]
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    }

    fn proxy() -> Vec<String> {
        ["a/b:free", "c/d"]
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    }

    #[test]
    fn each_provider_group_starts_with_its_header() {
        let (ark, proxy) = (ark(), proxy());
        let rows = menu_rows(
            &[(Provider::Ark, &ark), (Provider::HermesProxy, &proxy)],
            "",
        );
        assert_eq!(rows.len(), 1 + 3 + 1 + 2);
        assert_eq!(rows[0], MenuRow::Header(Provider::Ark));
        assert_eq!(rows[4], MenuRow::Header(Provider::HermesProxy));
        assert_eq!(
            rows[5],
            MenuRow::Model {
                provider: Provider::HermesProxy,
                id: "a/b:free".into()
            }
        );
    }

    #[test]
    fn missing_proxy_group_has_no_header() {
        let ark = ark();
        let rows = menu_rows(&[(Provider::Ark, &ark)], "");
        assert_eq!(rows.len(), 4);
        assert!(rows.iter().all(|row| !matches!(
            row,
            MenuRow::Model {
                provider: Provider::HermesProxy,
                ..
            }
        )));
    }

    #[test]
    fn search_spans_groups_and_hides_groups_without_matches() {
        let (ark, proxy) = (ark(), proxy());
        let rows = menu_rows(
            &[(Provider::Ark, &ark), (Provider::HermesProxy, &proxy)],
            "free",
        );
        assert_eq!(
            rows,
            vec![
                MenuRow::Header(Provider::HermesProxy),
                MenuRow::Model {
                    provider: Provider::HermesProxy,
                    id: "a/b:free".into()
                },
            ]
        );
    }

    #[test]
    fn no_match_anywhere_is_empty() {
        let (ark, proxy) = (ark(), proxy());
        assert!(
            menu_rows(
                &[(Provider::Ark, &ark), (Provider::HermesProxy, &proxy)],
                "zzz"
            )
            .is_empty()
        );
    }
}

#[cfg(test)]
mod model_filter_tests {
    use super::filter_models;

    fn ids() -> Vec<String> {
        [
            "poolside/laguna:free",
            "openai/gpt-x",
            "Meta/Llama:FREE",
            "a/b-free-plus",
        ]
        .iter()
        .map(|s| (*s).to_string())
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

    use super::{INPUT_BOX_PX, INPUT_MAX_LINES, INPUT_PX, composer};

    const BOX_W: f32 = 300.0;

    fn line_h() -> f32 {
        flamewm_render_core::layout_text(
            "x",
            INPUT_PX,
            100.0,
            flamewm_render_core::TextWrap::Wrap,
            false,
        )
        .line_height
    }

    #[test]
    fn empty_composer_is_one_centred_line() {
        let c = composer("", BOX_W);
        assert_eq!(c.box_h, INPUT_BOX_PX);
        assert_eq!(c.caret.0, 0.0);
        // The caret bar is centred on the single line box (relative to the text block).
        let centre = line_h() / 2.0;
        assert!((c.caret.1 + c.caret_h / 2.0 - centre).abs() < 0.01);
    }

    #[test]
    fn caret_follows_the_last_glyph_on_its_line() {
        let short = composer("hi", BOX_W);
        let longer = composer("hi there", BOX_W);
        assert!(longer.caret.0 > short.caret.0);
        let trailing = composer("hi ", BOX_W);
        assert!(
            trailing.caret.0 > short.caret.0,
            "a trailing space moves the caret"
        );
        let line = flamewm_render_core::layout_text(
            "hi there",
            INPUT_PX,
            longer.text_w,
            flamewm_render_core::TextWrap::Wrap,
            false,
        );
        assert_eq!(longer.caret.0, line.lines[0].width);
    }

    #[test]
    fn wrapped_input_grows_and_caret_moves_to_the_new_line() {
        let one = composer("word", BOX_W);
        let text = "word ".repeat(40);
        let many = composer(&text, BOX_W);
        assert!(many.box_h > one.box_h, "box grows as text wraps");
        assert!((many.pill_h - (one.pill_h + many.box_h - one.box_h)).abs() < 0.01);
        assert!(many.box_h <= INPUT_BOX_PX + (INPUT_MAX_LINES - 1) as f32 * line_h() + 0.01);
        let lines = (many.text_h / line_h()).round() as usize;
        assert!(lines > 1);
        // The caret sits on the last wrapped line, inside the box.
        let last_line_centre = (lines - 1) as f32 * line_h() + line_h() / 2.0;
        assert!((many.caret.1 + many.caret_h / 2.0 - last_line_centre).abs() < 0.01);
        let top = many.text_top + many.caret.1;
        assert!(top >= 0.0 && top + many.caret_h <= many.box_h + 0.01);
    }

    #[test]
    fn overflowing_input_scrolls_to_the_tail() {
        let text = "x\n".repeat(20);
        let c = composer(&text, BOX_W);
        assert_eq!(
            c.box_h,
            INPUT_BOX_PX + (INPUT_MAX_LINES - 1) as f32 * line_h()
        );
        assert!(c.text_top < 0.0, "text is scrolled up");
        // Caret stays on the newest (last) line, visible in the box.
        let top = c.text_top + c.caret.1;
        assert!(top >= 0.0 && top + c.caret_h <= c.box_h + 0.01);
        let inset = (INPUT_BOX_PX - line_h()) / 2.0;
        assert!(
            (c.text_top + c.text_h + inset - c.box_h).abs() < 0.01,
            "tail aligns to the box bottom"
        );
    }
}

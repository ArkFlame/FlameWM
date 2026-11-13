//! Clipboard history as the shell sees it: ids and truncated previews, never full text.

/// One history entry. `preview` is a single line of at most 120 characters; `len` is the entry's
/// byte length. The full text is never sent over the control surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardHistoryEntry {
    pub id: u64,
    pub preview: String,
    pub len: usize,
}

/// A request the compositor applies after the current control turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardRequest {
    Select(u64),
    Clear,
}

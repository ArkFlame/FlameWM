//! Text selection, click counting, and copy and paste through the system CLIPBOARD and PRIMARY selections.

use super::App;
use crate::view::Span;
use std::time::Duration;
use std::time::Instant;

/// Click count for a press at (`row`, `col`) at `now`, given the previous press.
pub(super) fn next_click_count(
    previous: Option<(Instant, usize, usize, u8)>,
    now: Instant,
    row: usize,
    col: usize,
) -> u8 {
    const MULTI_CLICK: Duration = Duration::from_millis(400);
    match previous {
        Some((at, prev_row, prev_col, count))
            if prev_row == row && prev_col == col && now.duration_since(at) <= MULTI_CLICK =>
        {
            count % 3 + 1
        }
        _ => 1,
    }
}

/// Inclusive bounds of the word under `col`: letters, digits and `_`. None for a blank or out-of-range cell.
pub(super) fn word_bounds(chars: &[char], col: usize) -> Option<(usize, usize)> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    if !chars.get(col).copied().is_some_and(is_word) {
        return None;
    }
    let mut start = col;
    while start > 0 && is_word(chars[start - 1]) {
        start -= 1;
    }
    let mut end = col;
    while end + 1 < chars.len() && is_word(chars[end + 1]) {
        end += 1;
    }
    Some((start, end))
}

impl App {
    /// Text of a pane's selection, also kept on the pane for the in-process paste fallback. Returns whether the pane
    /// has a selection to copy.
    fn copy_selection_in(&mut self, id: u64) -> bool {
        let Some(pane) = self.pane_mut(id) else {
            return false;
        };
        let Some(span) = pane.selection else {
            return false;
        };
        let screen = &pane.session.screen;
        // Built straight into the pane's buffer: one allocation for the whole selection, not one per line plus a
        // joined copy handed back to the caller.
        let mut out = String::new();
        let mut first = true;
        // One decoded-line buffer for the whole selection: copying a long scrollback range no longer allocates a
        // `Line` per selected line.
        let mut decoded = crate::screen::Line::new();
        for index in span.start.0..=span.end.0 {
            if !screen.line_abs_into(index, &mut decoded) {
                continue;
            }
            let line = decoded.as_slice();
            if !first {
                out.push('\n');
            }
            first = false;
            let from = if index == span.start.0 {
                span.start.1
            } else {
                0
            };
            let to = if index == span.end.0 {
                span.end.1
            } else {
                line.len().saturating_sub(1)
            };
            let line_start = out.len();
            #[allow(clippy::needless_range_loop)]
            for col in from..=to.min(line.len().saturating_sub(1)) {
                let cell = &line[col];
                if cell.attrs & crate::screen::attr::SPACER == 0 {
                    out.push(cell.ch);
                }
            }
            // Trim this line's trailing blanks only, so a later empty line still adds its own newline.
            let trimmed = out[line_start..].trim_end().len();
            out.truncate(line_start + trimmed);
        }
        pane.selection_text = out;
        true
    }

    /// Makes `span` the pane's selection, copies its text, and publishes it on PRIMARY for middle-click.
    pub(super) fn select_span(&mut self, id: u64, span: Span) {
        if let Some(pane) = self.pane_mut(id) {
            pane.selection = Some(span);
        }
        if !self.copy_selection_in(id) {
            return;
        }
        let Some(text) = self.pane(id).map(|pane| pane.selection_text.as_str()) else {
            return;
        };
        if text.is_empty() {
            return;
        }
        if let Some(clipboard) = &self.clipboard
            && let Err(error) = clipboard.set_primary_text(text)
        {
            eprintln!("flamewm-terminal: primary selection: {error}");
        }
    }

    /// Counts presses on the same cell within 400 ms: 1, 2 (word), 3 (line), then back to 1.
    pub(super) fn register_click(&mut self, id: u64, abs_row: usize, col: usize) -> u8 {
        let now = Instant::now();
        let Some(pane) = self.pane_mut(id) else {
            return 1;
        };
        let count = next_click_count(pane.last_click, now, abs_row, col);
        pane.last_click = Some((now, abs_row, col, count));
        count
    }

    pub(super) fn select_word(&mut self, id: u64, abs_row: usize, col: usize) {
        let chars: Vec<char> = match self
            .pane(id)
            .and_then(|pane| pane.session.screen.line_abs(abs_row))
        {
            Some(line) => line.iter().map(|cell| cell.ch).collect(),
            None => return,
        };
        if let Some((start, end)) = word_bounds(&chars, col) {
            self.select_span(id, Span::normalised((abs_row, start), (abs_row, end)));
        }
    }

    pub(super) fn select_line(&mut self, id: u64, abs_row: usize) {
        let last = self
            .pane_rect(id)
            .map_or(0, |rect| rect.cols.saturating_sub(1));
        self.select_span(id, Span::normalised((abs_row, 0), (abs_row, last)));
    }

    /// The focused pane's last in-process selection, the fallback when the system clipboard yields nothing.
    fn selection_text(&self) -> String {
        self.focused_pane()
            .map(|pane| pane.selection_text.clone())
            .unwrap_or_default()
    }

    /// Ctrl+Shift+C: the selection goes to the system CLIPBOARD. Without an X connection it stays in-process.
    pub fn copy_to_clipboard(&mut self) {
        let Some(id) = self.focused_id() else { return };
        if !self.copy_selection_in(id) {
            return;
        }
        let Some(text) = self.pane(id).map(|pane| pane.selection_text.as_str()) else {
            return;
        };
        if text.is_empty() {
            return;
        }
        if let Some(clipboard) = &self.clipboard
            && let Err(error) = clipboard.set_text(text)
        {
            eprintln!("flamewm-terminal: clipboard: {error}");
        }
    }

    /// Ctrl+Shift+V: the system CLIPBOARD text, or the in-process selection when the system read yields nothing.
    pub fn paste(&mut self) {
        let system = match &self.clipboard {
            Some(clipboard) => match clipboard.get_text() {
                Ok(text) => text,
                Err(error) => {
                    eprintln!("flamewm-terminal: clipboard: {error}");
                    None
                }
            },
            None => None,
        };
        let text = system.unwrap_or_else(|| self.selection_text());
        self.send_paste(text);
    }

    /// Middle-click: the PRIMARY selection, or the in-process selection when PRIMARY yields nothing.
    pub(super) fn paste_primary(&mut self) {
        let system = match &self.clipboard {
            Some(clipboard) => clipboard.get_primary_text().unwrap_or_else(|error| {
                eprintln!("flamewm-terminal: primary selection: {error}");
                None
            }),
            None => None,
        };
        let text = system.unwrap_or_else(|| self.selection_text());
        self.send_paste(text);
    }

    fn send_paste(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        let Some(id) = self.focused_id() else { return };
        let text = sanitize_paste(&text);
        let bracketed = self
            .pane(id)
            .is_some_and(|pane| pane.session.screen.bracketed_paste);
        if let Some(pane) = self.pane_mut(id) {
            pane.input.paste(&text);
            pane.view_offset = 0;
        }
        self.send_to(id, paste_bytes(&text, bracketed));
    }
}

/// Pasted text without control introducers. ESC and the C1 CSI byte (U+009B) start escape sequences, so they
/// are dropped; a pasted `ESC[201~` therefore cannot end a bracketed paste early.
#[must_use]
pub fn sanitize_paste(text: &str) -> String {
    text.chars()
        .filter(|&c| c != '\x1b' && c != '\u{9b}')
        .collect()
}

/// Bytes sent to the child for a paste. Sanitized text, LF as CR, wrapped in 200/201 markers when bracketed.
#[must_use]
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let body = sanitize_paste(text).replace('\n', "\r");
    let mut bytes = Vec::with_capacity(body.len() + 12);
    if bracketed {
        bytes.extend_from_slice(b"\x1b[200~");
    }
    bytes.extend_from_slice(body.as_bytes());
    if bracketed {
        bytes.extend_from_slice(b"\x1b[201~");
    }
    bytes
}

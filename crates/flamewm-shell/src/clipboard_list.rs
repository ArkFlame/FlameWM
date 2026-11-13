//! Pure state for the clipboard history popup: the listed entries, the selected row and the
//! visible window. Entries arrive from the control bus as ids and single-line previews only.

use std::ops::Range;

use flamewm_api::clipboard::ClipboardHistoryEntry;
use flamewm_ui_x11::UiDocumentAccess;

/// Rows in the popup, matching `ui/shell/clipboard.html`.
pub const CLIPBOARD_ROWS: usize = 8;
/// Row pitch, shared with the Launcher's list styling (`ui/shell/flamewm.css`).
const CLIPBOARD_ROW_HEIGHT: i32 = 36;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ClipboardList {
    entries: Vec<ClipboardHistoryEntry>,
    selected: usize,
    first: usize,
}

impl ClipboardList {
    /// Replaces the listed entries (newest first) and selects the first row.
    pub fn replace(&mut self, entries: Vec<ClipboardHistoryEntry>) {
        self.entries = entries;
        self.selected = 0;
        self.first = 0;
    }

    #[must_use]
    pub fn count(&self) -> usize {
        self.entries.len()
    }

    /// Selected row for keyboard focus; `None` when there is nothing to select.
    #[must_use]
    pub fn focus(&self) -> Option<usize> {
        (!self.entries.is_empty()).then_some(self.selected)
    }

    /// Selects `index`, clamped to the last row, and keeps it inside the visible window.
    pub fn select(&mut self, index: usize) {
        if self.entries.is_empty() {
            return;
        }
        self.selected = index.min(self.entries.len() - 1);
        if self.selected < self.first {
            self.first = self.selected;
        } else if self.selected >= self.first + CLIPBOARD_ROWS {
            self.first = self.selected + 1 - CLIPBOARD_ROWS;
        }
    }

    #[must_use]
    pub fn selected_id(&self) -> Option<u64> {
        self.entries.get(self.selected).map(|entry| entry.id)
    }

    #[must_use]
    pub fn window(&self) -> Range<usize> {
        self.first..(self.first + CLIPBOARD_ROWS).min(self.entries.len())
    }

    /// Writes the title-less list: row labels, selection highlight, `aria-selected` and the
    /// empty-state line. WAI-ARIA listbox of options (APG listbox pattern).
    pub fn project(&self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        let window = self.window();
        document.visible("clipboard-empty", self.entries.is_empty())?;
        document.visible("clipboard-highlight", !self.entries.is_empty())?;
        let highlight_y =
            (self.selected - self.first.min(self.selected)) as i32 * CLIPBOARD_ROW_HEIGHT;
        document.position("clipboard-highlight", 0.0, highlight_y as f32)?;
        document.attribute("clipboard-list", "role", "listbox")?;
        for slot in 0..CLIPBOARD_ROWS {
            let row = format!("clipboard-row-{}", slot + 1);
            let label = format!("{row}-label");
            let index = window.start + slot;
            let entry = (index < window.end).then(|| &self.entries[index]);
            let selected = entry.is_some() && index == self.selected;
            document.attribute(&row, "role", "option")?;
            document.attribute(
                &row,
                "aria-selected",
                if selected { "true" } else { "false" },
            )?;
            if let Some(entry) = entry {
                document.text_ref(&label, &entry.preview)?;
                document.visible(&row, true)?;
            } else {
                document.text(&label, String::new())?;
                document.visible(&row, false)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(count: u64) -> Vec<ClipboardHistoryEntry> {
        (1..=count)
            .map(|id| ClipboardHistoryEntry {
                id,
                preview: format!("entry {id}"),
                len: 7,
            })
            .collect()
    }

    // OBSERVABLE: selecting past the window scrolls it so the selected row stays visible.
    #[test]
    fn selection_scrolls_the_window_to_keep_the_row_visible() {
        let mut list = ClipboardList::default();
        list.replace(entries(12));
        list.select(9);
        assert_eq!(list.window(), 2..10);
        assert_eq!(list.selected_id(), Some(10));
        list.select(0);
        assert_eq!(list.window(), 0..8);
        assert_eq!(list.selected_id(), Some(1));
    }

    // OBSERVABLE: an empty list has no focus and no selected id; select is a no-op on it.
    #[test]
    fn empty_list_has_no_focus_and_ignores_select() {
        let mut list = ClipboardList::default();
        list.select(3);
        assert_eq!(list.focus(), None);
        assert_eq!(list.selected_id(), None);
        list.replace(entries(2));
        assert_eq!(list.focus(), Some(0));
        list.select(5);
        assert_eq!(list.selected_id(), Some(2));
    }
}

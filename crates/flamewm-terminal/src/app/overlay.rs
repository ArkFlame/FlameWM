//! Painted chrome over the grid: the shell ghost suggestion, the startup banner and the context menu.

use super::grid::overlay;
use super::layout::Axis;
use super::tabs::Rename;
use super::{App, View};
use crate::glyphs::blend;
use crate::screen::char_width;
use crate::view::Row;
use crate::view::Run;
use crate::view::text_run;

/// Startup banner: `FlameWM` art, then the `/theme` hint. Drawn above the shell until its output scrolls.
pub(super) const BANNER: [&str; 7] = [
    r"     _____   _         _       __  __   _____  __        __  __  __ ",
    r"   |  ___| | |       / \     |  \/  | | ____| \ \      / / |  \/  | ",
    r"  | |_    | |      / _ \    | |\/| | |  _|    \ \ /\ / /  | |\/| |  ",
    r" |  _|   | |___  / ___ \   | |  | | | |___    \ V  V /   | |  | |   ",
    r"|_|     |_____| /_/   \_\ |_|  |_| |_____|    \_/\_/    |_|  |_|    ",
    "",
    "/theme - Change the Terminal theme.",
];

pub(super) const BANNER_ART_ROWS: usize = 5;

/// Right-click menu items, in display order. Indexes are matched in `menu_action`.
pub(super) const MENU: [&str; 8] = [
    "New Tab",
    "Close Tab",
    "Rename Tab",
    "Copy",
    "Paste",
    "Split Vertical",
    "Split Horizontal",
    "Close Pane",
];

/// Width of the context menu: its longest item plus a cell of padding on each side.
pub(super) fn menu_width() -> usize {
    MENU.iter()
        .map(|item| item.chars().count())
        .max()
        .unwrap_or(0)
        + 2
}

impl App {
    /// Suffix of the typed line that the most used command completes. Drawn only at the shell prompt, on the live
    /// grid, with no menu or rename open and the line exact with the cursor at its end.
    pub(super) fn ghost(&self) -> Option<&str> {
        let pane = self.focused_pane()?;
        if pane.view_offset != 0
            || !matches!(self.view, View::Terminal)
            || self.menu.is_some()
            || self.rename.is_some()
        {
            return None;
        }
        if !pane.input.is_ready() || !self.at_prompt() {
            return None;
        }
        self.history.suggest(&pane.input.text())
    }

    /// Draws the suggestion after the cursor cell of the focused pane, 40% of the way from the foreground colour to
    /// the background.
    pub(super) fn draw_ghost(&self, rows: &mut [Row], suffix: &str) {
        let Some(pane) = self.focused_pane() else {
            return;
        };
        let Some(rect) = self.pane_rect(pane.session.id) else {
            return;
        };
        let (row, col) = pane.session.screen.cursor();
        let start = col + 1;
        let room = rect.cols.saturating_sub(start);
        let text: String = suffix
            .chars()
            .take_while(|&c| char_width(c) == 1)
            .take(room)
            .collect();
        if text.is_empty() {
            return;
        }
        let start = rect.x0 + start;
        let Some(line) = rows.get_mut(1 + self.banner_rows() + rect.y0 + row) else {
            return;
        };
        let fg = blend(self.palette.foreground, self.palette.background, 2, 5);
        let cells = text.chars().count();
        *line = Row::from(overlay(
            line.to_vec(),
            start,
            cells,
            text_run(start, &text, fg, None, false),
        ));
    }

    /// The banner goes once the shell output scrolls or reaches the rows the banner covers.
    pub(super) fn update_banner(&mut self) {
        let gone = match self.focused_pane() {
            Some(pane) => {
                let (cursor_row, _) = pane.session.screen.cursor();
                pane.session.screen.scrollback_len() > 0
                    || cursor_row + BANNER.len() >= self.grid_rows
            }
            None => false,
        };
        if gone {
            self.banner = false;
        }
    }

    /// Banner rows above the grid, only while the active tab holds one shell. Zero when the banner is gone or the
    /// window is too short to show it.
    pub(super) fn banner_rows(&self) -> usize {
        let single = self
            .tabs
            .get(self.active)
            .is_some_and(|tab| tab.root.is_single());
        if self.banner && single && self.grid_rows >= 2 * BANNER.len() {
            BANNER.len()
        } else {
            0
        }
    }

    /// Banner rows, or nothing when the banner is off.
    pub(super) fn banner_lines_for(&self, rows: usize) -> Vec<Vec<Run>> {
        BANNER[..rows]
            .iter()
            .enumerate()
            .map(|(index, text)| {
                let art = index < BANNER_ART_ROWS;
                let colour = if art {
                    self.palette.cursor
                } else {
                    self.palette.foreground
                };
                vec![text_run(
                    0,
                    &text.chars().take(self.cols).collect::<String>(),
                    colour,
                    None,
                    art,
                )]
            })
            .collect()
    }

    /// Paints the context menu over window rows starting at `row`, column `col`.
    pub(super) fn draw_menu(&self, rows: &mut [Row], row: usize, col: usize) {
        let width = menu_width();
        for (index, item) in MENU.iter().enumerate() {
            let Some(line) = rows.get_mut(row + index) else {
                break;
            };
            let label = format!(" {item:<w$} ", w = width - 2);
            let run = text_run(
                col,
                &label,
                self.palette.foreground,
                Some(self.palette.selection),
                false,
            );
            *line = Row::from(overlay(line.to_vec(), col, width, run));
        }
    }

    pub(super) fn menu_action(&mut self, index: usize) {
        match index {
            0 => {
                let _ = self.open_tab_here();
            }
            1 => self.close_tab(self.active),
            2 => {
                self.rename = Some(Rename {
                    tab: self.active,
                    text: String::new(),
                });
            }
            3 => self.copy_to_clipboard(),
            4 => self.paste(),
            5 => {
                let _ = self.split_focused(Axis::Columns);
            }
            6 => {
                let _ = self.split_focused(Axis::Rows);
            }
            _ => self.close_pane(),
        }
    }
}

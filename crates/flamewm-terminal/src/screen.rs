//! Terminal state: primary and alternate grids, cursor and pen, scroll region, modes, tab stops, scrollback and
//! the replies the host owes the child (DSR, DA). Input arrives through the `Perform` trait from `parser`.
use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::VecDeque;

use crate::parser::Perform;

mod cell;
mod charset;
mod history;
mod modes;
mod sgr;

pub use self::cell::{Cell, Color, Line, Pen, attr};
use self::cell::{blank_line, history_len};
pub use self::charset::char_width;
use self::charset::dec_graphics;
use self::history::History;
pub use self::modes::{CursorShape, MouseMode};
use self::sgr::extended_color;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Saved {
    row: usize,
    col: usize,
    pen: Pen,
    g1_graphics: bool,
    use_g1: bool,
}

/// Live-grid rows changed since the last `Screen::take_dirty`.
#[derive(Debug, PartialEq, Eq)]
pub enum Dirty {
    /// Every row may have changed (scroll, resize, mode switch, reset): rebuild the whole grid.
    All,
    /// Only these live-grid rows (ascending) were written.
    Rows(Vec<usize>),
}

#[derive(Debug)]
pub struct Screen {
    cols: usize,
    rows: usize,
    primary: Vec<Line>,
    alt: Vec<Line>,
    alt_active: bool,
    scrollback: VecDeque<History>,
    scrollback_limit: usize,
    row: usize,
    col: usize,
    wrap_next: bool,
    pen: Pen,
    saved: Option<Saved>,
    alt_saved: Option<Saved>,
    scroll_top: usize,
    scroll_bottom: usize,
    pub autowrap: bool,
    origin: bool,
    insert: bool,
    pub cursor_visible: bool,
    pub app_cursor: bool,
    pub app_keypad: bool,
    pub bracketed_paste: bool,
    pub focus_events: bool,
    pub mouse: MouseMode,
    pub mouse_sgr: bool,
    tabs: Vec<bool>,
    /// Shape the program chose with DECSCUSR; `None` keeps the configured `cursor_style`.
    pub cursor_shape: Option<CursorShape>,
    g0_graphics: bool,
    g1_graphics: bool,
    use_g1: bool,
    pub title: Option<String>,
    pub bell: bool,
    replies: Vec<u8>,
    /// Live-grid rows written since the last `take_dirty`; `dirty_all` covers wholesale changes.
    dirty_rows: Vec<bool>,
    dirty_all: bool,
    /// Lines captured into scrollback since the last `take_scroll_shift`: every visible row moved up by this many,
    /// so a renderer reuses its previous rows instead of rebuilding them.
    scroll_shift: usize,
    /// Lines dropped from the front of a capped scrollback. Every absolute line index renumbers down by this much
    /// without `total_lines` changing, so a renderer must not trust an unchanged window index after it moves.
    scrollback_drops: u64,
    /// Last graphic character printed, the source for REP (`CSI b`).
    last_char: Option<char>,
}

/// Default tab stops: every eighth column, excluding column 0.
fn default_tabs(cols: usize) -> Vec<bool> {
    (0..cols).map(|c| c % 8 == 0 && c != 0).collect()
}

impl Screen {
    #[must_use]
    pub fn new(cols: usize, rows: usize, scrollback_limit: usize) -> Self {
        let cols = cols.max(1);
        let rows = rows.max(1);
        Self {
            cols,
            rows,
            primary: vec![blank_line(cols, Color::Default); rows],
            alt: vec![blank_line(cols, Color::Default); rows],
            alt_active: false,
            scrollback: VecDeque::new(),
            scrollback_limit,
            row: 0,
            col: 0,
            wrap_next: false,
            pen: Pen::default(),
            saved: None,
            alt_saved: None,
            scroll_top: 0,
            scroll_bottom: rows - 1,
            autowrap: true,
            origin: false,
            insert: false,
            cursor_visible: true,
            app_cursor: false,
            app_keypad: false,
            bracketed_paste: false,
            focus_events: false,
            mouse: MouseMode::Off,
            mouse_sgr: false,
            tabs: default_tabs(cols),
            cursor_shape: None,
            g0_graphics: false,
            g1_graphics: false,
            use_g1: false,
            title: None,
            bell: false,
            replies: Vec::new(),
            dirty_rows: vec![false; rows],
            dirty_all: true,
            scroll_shift: 0,
            scrollback_drops: 0,
            last_char: None,
        }
    }

    #[must_use]
    pub fn cols(&self) -> usize {
        self.cols
    }

    #[must_use]
    pub fn rows(&self) -> usize {
        self.rows
    }

    #[must_use]
    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    #[must_use]
    pub fn alt_active(&self) -> bool {
        self.alt_active
    }

    fn grid(&self) -> &Vec<Line> {
        if self.alt_active {
            &self.alt
        } else {
            &self.primary
        }
    }

    /// One live-grid row, for cell writes that touch only that row.
    fn row_mut(&mut self, row: usize) -> &mut Line {
        self.dirty_rows[row] = true;
        let grid = if self.alt_active {
            &mut self.alt
        } else {
            &mut self.primary
        };
        &mut grid[row]
    }

    /// Dirty-marked live rows not yet taken (benchmark probe).
    #[cfg(test)]
    #[must_use]
    pub fn pending_dirty_rows(&self) -> usize {
        self.dirty_rows.iter().filter(|dirty| **dirty).count()
    }

    /// Takes the rows changed since the last call, so a renderer rebuilds only those.
    pub fn take_dirty(&mut self) -> Dirty {
        let mut rows = Vec::new();
        if self.take_dirty_into(&mut rows) {
            Dirty::All
        } else {
            Dirty::Rows(rows)
        }
    }

    /// Fills `out` with the live rows written since the last take, ascending, and returns whether every row may have
    /// changed. `out` is cleared first, so a renderer keeps one buffer and a frame allocates nothing for the list.
    pub fn take_dirty_into(&mut self, out: &mut Vec<usize>) -> bool {
        out.clear();
        let all = std::mem::take(&mut self.dirty_all);
        // Collect the per-row list even behind `All`: a scroll reuses the shifted rows, and the rows edited in the
        // same batch must still be rebuilt at their new positions.
        out.extend(
            self.dirty_rows
                .iter()
                .enumerate()
                .filter_map(|(row, dirty)| dirty.then_some(row)),
        );
        self.dirty_rows.fill(false);
        all
    }

    /// Lines captured into scrollback since the last take. The rows shown last frame moved up by this many, so only
    /// the rows the scroll exposed at the bottom are new; taking it resets the count.
    pub fn take_scroll_shift(&mut self) -> usize {
        std::mem::take(&mut self.scroll_shift)
    }

    fn mark_all(&mut self) {
        // A wholesale change (resize, mode switch, reset) may remap every line, so a pending scroll shift no longer
        // describes the visible rows.
        self.scroll_shift = 0;
        self.dirty_all = true;
    }

    /// Visible row `row` of the live grid (not scrollback).
    #[must_use]
    pub fn line(&self, row: usize) -> &Line {
        &self.grid()[row]
    }

    /// Absolute line index: scrollback first, then the live grid. Alt screen has no scrollback.
    #[must_use]
    pub fn total_lines(&self) -> usize {
        self.scrollback_len() + self.rows
    }

    #[must_use]
    pub fn scrollback_len(&self) -> usize {
        if self.alt_active {
            0
        } else {
            self.scrollback.len()
        }
    }

    /// How many lines a capped scrollback has dropped from its front. Absolute line indices move down by the
    /// increase, even though `total_lines` stays the same.
    #[must_use]
    pub fn scrollback_drops(&self) -> u64 {
        self.scrollback_drops
    }

    #[must_use]
    pub fn line_abs(&self, index: usize) -> Option<Cow<'_, Line>> {
        let history = self.scrollback_len();
        if index < history {
            self.scrollback.get(index).map(History::cells)
        } else {
            self.grid().get(index - history).map(Cow::Borrowed)
        }
    }

    /// The live-grid line at absolute `index`, borrowed without a decode. `None` for a scrollback line (decode it
    /// with `line_abs_into`) or an index past the grid, so a renderer can avoid both a copy and an allocation on the
    /// live view.
    #[must_use]
    pub fn live_line(&self, index: usize) -> Option<&Line> {
        let history = self.scrollback_len();
        if index < history {
            None
        } else {
            self.grid().get(index - history)
        }
    }

    /// Writes absolute line `index` into `out`, which is cleared first; false when the index is out of range. A walk
    /// over the scrollback reuses one buffer instead of allocating a decoded line per entry.
    pub fn line_abs_into(&self, index: usize, out: &mut Line) -> bool {
        let history = self.scrollback_len();
        if index < history {
            match self.scrollback.get(index) {
                Some(line) => {
                    line.cells_into(out);
                    true
                }
                None => false,
            }
        } else {
            match self.grid().get(index - history) {
                Some(line) => {
                    out.clear();
                    out.extend_from_slice(line);
                    true
                }
                None => false,
            }
        }
    }

    /// Replies the host must write back to the child (cursor reports, device attributes).
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.replies)
    }

    fn reply(&mut self, text: &str) {
        self.replies.extend_from_slice(text.as_bytes());
    }

    /// Reflow-free resize: rows that fall off the top of a shrinking primary grid move into scrollback so the
    /// cursor stays on screen; growing pulls history back. Columns truncate or pad.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        // A repeated ConfigureNotify for the same size must not rebuild the dirty bitmap or force a full redraw.
        if cols == self.cols && rows == self.rows {
            return;
        }
        let cols_changed = cols != self.cols;
        // Width padding and truncation only when the column count moves. The alternate grid is blanked and sized on
        // activation (`switch_alt`), so an inactive one keeps its old width instead of paying the same per-line pass
        // on every ConfigureNotify; a height-only resize touches no line buffer at all.
        if cols_changed {
            let blank = Cell::blank(Color::Default);
            for line in &mut self.primary {
                line.resize(cols, blank);
            }
            if self.alt_active {
                for line in &mut self.alt {
                    line.resize(cols, blank);
                }
            }
        }
        match rows.cmp(&self.rows) {
            Ordering::Less => {
                let excess = self.rows - rows;
                let shift = excess.min(self.row);
                if shift > 0 {
                    // One drain, packing each departing top line into scrollback as it leaves. The drain borrows only
                    // the primary grid, so the drops need no temporary list of removed lines.
                    for line in self.primary.drain(0..shift) {
                        self.scrollback
                            .push_back(History::pack(&line[..history_len(&line)]));
                        if self.scrollback.len() > self.scrollback_limit {
                            self.scrollback.pop_front();
                            self.scrollback_drops = self.scrollback_drops.saturating_add(1);
                        }
                    }
                    self.primary
                        .extend((0..shift).map(|_| blank_line(cols, Color::Default)));
                    self.row = self.row.saturating_sub(shift);
                }
            }
            Ordering::Greater => {
                let grow = rows - self.rows;
                // One capacity for the whole batch: growing the window never reallocates the pull list per row.
                let mut pulled = Vec::with_capacity(grow);
                if !self.alt_active {
                    for _ in 0..grow {
                        let Some(history) = self.scrollback.pop_back() else {
                            break;
                        };
                        let mut line = history.cells().into_owned();
                        line.resize(cols, Cell::blank(Color::Default));
                        pulled.push(line);
                    }
                }
                if !pulled.is_empty() {
                    // Pop from the back gave newest-first; restore chronological order for one front splice.
                    pulled.reverse();
                    let count = pulled.len();
                    self.primary.splice(0..0, pulled);
                    self.row += count;
                }
                while self.primary.len() < rows {
                    self.primary.push(blank_line(cols, Color::Default));
                }
                while self.alt.len() < rows {
                    self.alt.push(blank_line(cols, Color::Default));
                }
            }
            Ordering::Equal => {}
        }
        self.primary.truncate(rows);
        self.alt.truncate(rows);
        self.cols = cols;
        self.rows = rows;
        // Reuse the damage bitmap: `take_dirty_into` clears it behind `All`, so only its capacity is carried over.
        self.dirty_rows.clear();
        self.dirty_rows.resize(rows, false);
        self.mark_all();
        // Tab stops depend on the column count alone: a height-only resize keeps them, including any stop the
        // program set with HTS, so no tab list is rebuilt.
        if cols_changed {
            self.tabs = default_tabs(cols);
        }
        self.scroll_top = 0;
        self.scroll_bottom = rows - 1;
        self.row = self.row.min(rows - 1);
        self.col = self.col.min(cols - 1);
        self.wrap_next = false;
    }

    fn blank(&self) -> Cell {
        Cell::blank(self.pen.bg)
    }

    fn full_region(&self) -> bool {
        self.scroll_top == 0 && self.scroll_bottom == self.rows - 1
    }

    fn scroll_up(&mut self, count: usize) {
        let (top, bottom) = (self.scroll_top, self.scroll_bottom);
        let capture = self.full_region() && !self.alt_active;
        let blank = self.blank();
        let moved = count.min(bottom - top + 1);
        // Rotate the region and blank the row that wrapped to the bottom in place: no per-line row allocation.
        // A captured line shifts the window a scrolled-back view shows; record how far so the renderer moves its
        // rows instead of rebuilding them.
        if capture {
            self.dirty_all = true;
            self.scroll_shift = self.scroll_shift.saturating_add(moved);
        } else {
            // Only the scrolled region changed; rows outside it (a status line, a fixed header) stay clean.
            self.dirty_rows[top..=bottom].fill(true);
        }
        let grid = if self.alt_active {
            &mut self.alt
        } else {
            &mut self.primary
        };
        for _ in 0..moved {
            grid[top..=bottom].rotate_left(1);
            if capture {
                // The damage bitmap follows the content: a row written before the scroll must be rebuilt where it
                // moved to, and the row the scroll blanked is new.
                self.dirty_rows[top..=bottom].rotate_left(1);
                self.dirty_rows[bottom] = true;
            }
            let row = &mut grid[bottom];
            let packed = capture.then(|| History::pack(&row[..history_len(row)]));
            row.fill(blank);
            if let Some(history) = packed {
                self.scrollback.push_back(history);
                if self.scrollback.len() > self.scrollback_limit {
                    self.scrollback.pop_front();
                    self.scrollback_drops = self.scrollback_drops.saturating_add(1);
                }
            }
        }
    }

    fn scroll_down(&mut self, count: usize) {
        let (top, bottom) = (self.scroll_top, self.scroll_bottom);
        let blank = self.blank();
        // Only the scrolled region changed, so narrow the damage instead of rebuilding the whole grid.
        self.dirty_rows[top..=bottom].fill(true);
        let grid = if self.alt_active {
            &mut self.alt
        } else {
            &mut self.primary
        };
        // Rotate in place and blank the row that wrapped to the top: no row removal, insertion or allocation.
        for _ in 0..count.min(bottom - top + 1) {
            grid[top..=bottom].rotate_right(1);
            let row = &mut grid[top];
            row.fill(blank);
        }
    }

    fn linefeed(&mut self) {
        if self.row == self.scroll_bottom {
            self.scroll_up(1);
        } else if self.row + 1 < self.rows {
            self.row += 1;
        }
    }

    fn reverse_index(&mut self) {
        if self.row == self.scroll_top {
            self.scroll_down(1);
        } else {
            self.row = self.row.saturating_sub(1);
        }
    }

    /// IL/DL: scrolls the region from `row` down (insert) or up (delete). Rows outside the scroll region are left alone.
    fn scroll_from(&mut self, row: usize, count: usize, down: bool) {
        if row < self.scroll_top || row > self.scroll_bottom {
            return;
        }
        let saved_top = self.scroll_top;
        self.scroll_top = row;
        if down {
            self.scroll_down(count);
        } else {
            self.scroll_up(count);
        }
        self.scroll_top = saved_top;
        self.col = 0;
        self.wrap_next = false;
    }

    fn set_cursor(&mut self, row: usize, col: usize) {
        let (top, bottom) = if self.origin {
            (self.scroll_top, self.scroll_bottom)
        } else {
            (0, self.rows - 1)
        };
        let row = if self.origin {
            (top + row).min(bottom)
        } else {
            row.min(bottom)
        };
        self.row = row;
        self.col = col.min(self.cols - 1);
        self.wrap_next = false;
    }

    fn erase_cells(&mut self, row: usize, from: usize, to: usize) {
        let blank = self.blank();
        let to = to.min(self.cols);
        let from = from.min(to);
        if self.grid()[row][from..to].iter().all(|cell| *cell == blank) {
            return;
        }
        let line = self.row_mut(row);
        line[from..to].fill(blank);
    }

    fn erase_lines(&mut self, from: usize, to: usize) {
        let blank = self.blank();
        for row in from..to.min(self.rows) {
            if self.grid()[row].iter().any(|cell| *cell != blank) {
                // Fill the kept line buffer instead of replacing it with a fresh allocation per erased row.
                self.row_mut(row).fill(blank);
            }
        }
    }

    fn put_char(&mut self, c: char) {
        let width = char_width(c);
        if width == 0 {
            return;
        }
        if self.wrap_next && self.autowrap {
            self.col = 0;
            self.linefeed();
            self.wrap_next = false;
        }
        if self.col + width > self.cols {
            if !self.autowrap {
                return;
            }
            self.col = 0;
            self.linefeed();
        }
        let row = self.row;
        let col = self.col;
        let pen = self.pen;
        let cols = self.cols;
        if self.insert {
            let line = self.row_mut(row);
            line.truncate(cols - width);
            for _ in 0..width {
                line.insert(col, Cell::blank(pen.bg));
            }
            line.truncate(cols);
        }
        // Rewriting a cell with the same content changes nothing, so the row stays clean (an htop-style repaint writes
        // the same text every tick).
        let cell = Cell {
            ch: c,
            fg: pen.fg,
            bg: pen.bg,
            attrs: pen.attrs | if width == 2 { attr::WIDE } else { 0 },
        };
        let spacer = Cell {
            ch: ' ',
            fg: pen.fg,
            bg: pen.bg,
            attrs: attr::SPACER | pen.attrs,
        };
        let grid = self.grid();
        let unchanged =
            !self.insert && grid[row][col] == cell && (width == 1 || grid[row][col + 1] == spacer);
        if !unchanged {
            let line = self.row_mut(row);
            if line[col].attrs & attr::SPACER != 0 && col > 0 {
                line[col - 1] = Cell::blank(line[col - 1].bg);
            }
            if line[col].attrs & attr::WIDE != 0 && col + 1 < cols {
                line[col + 1] = Cell::blank(line[col + 1].bg);
            }
            line[col] = cell;
            if width == 2 {
                line[col + 1] = spacer;
            }
        }
        self.col += width;
        if self.col >= cols {
            self.col = cols - 1;
            self.wrap_next = self.autowrap;
        } else {
            self.wrap_next = false;
        }
    }

    fn next_tab(&self, col: usize) -> usize {
        (col + 1..self.cols)
            .find(|c| self.tabs[*c])
            .unwrap_or(self.cols - 1)
    }

    fn set_mode(&mut self, private: bool, code: u16, on: bool) {
        if !private {
            if code == 4 {
                self.insert = on;
            }
            return;
        }
        match code {
            1 => self.app_cursor = on,
            6 => {
                self.origin = on;
                self.set_cursor(0, 0);
            }
            7 => self.autowrap = on,
            25 => self.cursor_visible = on,
            1000 => self.mouse = if on { MouseMode::Press } else { MouseMode::Off },
            1002 => self.mouse = if on { MouseMode::Drag } else { MouseMode::Off },
            1003 => self.mouse = if on { MouseMode::Any } else { MouseMode::Off },
            1004 => self.focus_events = on,
            1006 => self.mouse_sgr = on,
            2004 => self.bracketed_paste = on,
            47 | 1047 => self.switch_alt(on, false),
            1049 => self.switch_alt(on, true),
            _ => {}
        }
    }

    fn set_modes(&mut self, private: bool, params: &[Vec<u16>], on: bool) {
        for group in params {
            self.set_mode(private, group.first().copied().unwrap_or(0), on);
        }
    }

    fn switch_alt(&mut self, on: bool, save_cursor: bool) {
        if on == self.alt_active {
            return;
        }
        self.mark_all();
        self.scroll_top = 0;
        self.scroll_bottom = self.rows - 1;
        if on {
            if save_cursor {
                self.alt_saved = Some(self.save_state());
            }
            self.alt_active = true;
            // Blank the retained lines instead of reallocating the whole alternate grid on every switch.
            let blank = Cell::blank(Color::Default);
            for line in &mut self.alt {
                line.clear();
                line.resize(self.cols, blank);
            }
        } else {
            self.alt_active = false;
            if save_cursor && let Some(saved) = self.alt_saved.take() {
                self.restore_state(saved);
            }
        }
        self.wrap_next = false;
    }

    fn save_state(&self) -> Saved {
        Saved {
            row: self.row,
            col: self.col,
            pen: self.pen,
            g1_graphics: self.g1_graphics,
            use_g1: self.use_g1,
        }
    }

    fn restore_state(&mut self, saved: Saved) {
        self.row = saved.row.min(self.rows - 1);
        self.col = saved.col.min(self.cols - 1);
        self.pen = saved.pen;
        self.g1_graphics = saved.g1_graphics;
        self.use_g1 = saved.use_g1;
        self.wrap_next = false;
    }

    fn sgr(&mut self, params: &[Vec<u16>]) {
        if params.is_empty() {
            self.pen = Pen::default();
            return;
        }
        let mut index = 0;
        while index < params.len() {
            let group = &params[index];
            let code = group.first().copied().unwrap_or(0);
            match code {
                0 => self.pen = Pen::default(),
                1 => self.pen.attrs |= attr::BOLD,
                2 => self.pen.attrs |= attr::DIM,
                3 => self.pen.attrs |= attr::ITALIC,
                4 => {
                    let style = group.get(1).copied().unwrap_or(1);
                    if style == 0 {
                        self.pen.attrs &= !attr::UNDERLINE;
                    } else {
                        self.pen.attrs |= attr::UNDERLINE;
                    }
                }
                7 => self.pen.attrs |= attr::INVERSE,
                9 => self.pen.attrs |= attr::STRIKE,
                22 => self.pen.attrs &= !(attr::BOLD | attr::DIM),
                23 => self.pen.attrs &= !attr::ITALIC,
                24 => self.pen.attrs &= !attr::UNDERLINE,
                27 => self.pen.attrs &= !attr::INVERSE,
                29 => self.pen.attrs &= !attr::STRIKE,
                30..=37 => self.pen.fg = Color::Indexed((code - 30) as u8),
                39 => self.pen.fg = Color::Default,
                40..=47 => self.pen.bg = Color::Indexed((code - 40) as u8),
                49 => self.pen.bg = Color::Default,
                90..=97 => self.pen.fg = Color::Indexed((code - 90 + 8) as u8),
                100..=107 => self.pen.bg = Color::Indexed((code - 100 + 8) as u8),
                38 | 48 => {
                    let (color, consumed) = extended_color(group, params, index);
                    if let Some(color) = color {
                        if code == 38 {
                            self.pen.fg = color;
                        } else {
                            self.pen.bg = color;
                        }
                    }
                    index += consumed + 1;
                    continue;
                }
                _ => {}
            }
            index += 1;
        }
    }

    fn dsr_position(&self) -> String {
        let (row, col) = if self.origin {
            (self.row.saturating_sub(self.scroll_top), self.col)
        } else {
            (self.row, self.col)
        };
        format!("\x1b[{};{}R", row + 1, col + 1)
    }
}

impl Perform for Screen {
    fn print(&mut self, c: char) {
        let c = if (self.use_g1 && self.g1_graphics) || (!self.use_g1 && self.g0_graphics) {
            if ('\u{5f}'..='\u{7e}').contains(&c) {
                dec_graphics(c)
            } else {
                c
            }
        } else {
            c
        };
        self.put_char(c);
        self.last_char = Some(c);
    }

    fn print_ascii(&mut self, bytes: &[u8]) {
        // DEC graphics remaps printable ASCII to box glyphs, and insert mode shifts the rest of the line per
        // character: both keep the per-character path.
        if self.insert || (self.use_g1 && self.g1_graphics) || (!self.use_g1 && self.g0_graphics) {
            for &byte in bytes {
                self.print(char::from(byte));
            }
            return;
        }
        let mut rest = bytes;
        while !rest.is_empty() {
            // Same wrap rule as `put_char`, once per run instead of once per cell.
            if self.wrap_next && self.autowrap {
                self.col = 0;
                self.linefeed();
                self.wrap_next = false;
            }
            let (row, col, cols, pen) = (self.row, self.col, self.cols, self.pen);
            let Some(free) = cols.checked_sub(col).filter(|free| *free > 0) else {
                // A cursor past the last column (it cannot normally happen) keeps the exact per-char behaviour.
                self.print(char::from(rest[0]));
                rest = &rest[1..];
                continue;
            };
            let take = rest.len().min(free);
            let template = Cell {
                ch: ' ',
                fg: pen.fg,
                bg: pen.bg,
                attrs: pen.attrs,
            };
            let (changed, left_spacer, right_wide) = {
                let grid = if self.alt_active {
                    &self.alt
                } else {
                    &self.primary
                };
                let line = &grid[row];
                let mut changed = false;
                for offset in 0..take {
                    let cell = &line[col + offset];
                    let expected = Cell {
                        ch: char::from(rest[offset]),
                        ..template
                    };
                    if *cell != expected {
                        changed = true;
                        break;
                    }
                }
                if changed {
                    // Overwriting the spacer (or the lead) of a wide glyph at either edge of the run leaves its
                    // partner behind, so blank it the way `put_char` does.
                    (
                        true,
                        col > 0 && line[col].attrs & attr::SPACER != 0,
                        col + take < cols && line[col + take - 1].attrs & attr::WIDE != 0,
                    )
                } else {
                    (false, false, false)
                }
            };
            if changed {
                // One dirty mark for the whole run, then a straight stretch of cells.
                let line = self.row_mut(row);
                if left_spacer {
                    line[col - 1] = Cell::blank(line[col - 1].bg);
                }
                for offset in 0..take {
                    line[col + offset] = Cell {
                        ch: char::from(rest[offset]),
                        ..template
                    };
                }
                if right_wide {
                    line[col + take] = Cell::blank(line[col + take].bg);
                }
            }
            self.col = col + take;
            if self.col >= cols {
                self.col = cols - 1;
                self.wrap_next = self.autowrap;
            } else {
                self.wrap_next = false;
            }
            self.last_char = Some(char::from(rest[take - 1]));
            rest = &rest[take..];
        }
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x07 => self.bell = true,
            0x08 => {
                self.col = self.col.saturating_sub(1);
                self.wrap_next = false;
            }
            0x09 => {
                self.col = self.next_tab(self.col);
                self.wrap_next = false;
            }
            0x0a..=0x0c => {
                self.linefeed();
                self.wrap_next = false;
            }
            0x0d => {
                self.col = 0;
                self.wrap_next = false;
            }
            0x0e => self.use_g1 = true,
            0x0f => self.use_g1 = false,
            _ => {}
        }
    }

    fn csi(&mut self, prefix: Option<u8>, params: &[Vec<u16>], intermediates: &[u8], action: u8) {
        let p = |i: usize| params.get(i).and_then(|g| g.first()).copied().unwrap_or(0) as usize;
        let n1 = |i: usize| p(i).max(1);
        let (row, col) = (self.row, self.col);
        match (prefix, intermediates, action) {
            (None, [], b'A') => {
                let floor = if row >= self.scroll_top && row <= self.scroll_bottom {
                    self.scroll_top
                } else {
                    0
                };
                self.row = row.saturating_sub(n1(0)).max(floor);
                self.wrap_next = false;
            }
            (None, [], b'B' | b'e') => {
                let ceiling = if row <= self.scroll_bottom {
                    self.scroll_bottom
                } else {
                    self.rows - 1
                };
                self.row = (row + n1(0)).min(ceiling);
                self.wrap_next = false;
            }
            (None, [], b'C' | b'a') => {
                self.col = (col + n1(0)).min(self.cols - 1);
                self.wrap_next = false;
            }
            (None, [], b'D') => {
                self.col = col.saturating_sub(n1(0));
                self.wrap_next = false;
            }
            (None, [], b'E') => {
                self.row = (row + n1(0)).min(self.rows - 1);
                self.col = 0;
                self.wrap_next = false;
            }
            (None, [], b'F') => {
                self.row = row.saturating_sub(n1(0));
                self.col = 0;
                self.wrap_next = false;
            }
            (None, [], b'G' | b'`') => self.col = (n1(0) - 1).min(self.cols - 1),
            (None, [], b'd') => self.set_cursor(n1(0) - 1, col),
            (None, [], b'H' | b'f') => self.set_cursor(n1(0) - 1, n1(1) - 1),
            (None, [], b'I') => {
                for _ in 0..n1(0) {
                    self.col = self.next_tab(self.col);
                }
            }
            (None, [], b'Z') => {
                for _ in 0..n1(0) {
                    self.col = (0..self.col).rev().find(|c| self.tabs[*c]).unwrap_or(0);
                }
            }
            (None, [], b'J') => match p(0) {
                0 => {
                    self.erase_cells(row, col, self.cols);
                    self.erase_lines(row + 1, self.rows);
                }
                1 => {
                    self.erase_lines(0, row);
                    self.erase_cells(row, 0, col + 1);
                }
                2 => self.erase_lines(0, self.rows),
                3 => {
                    self.scrollback.clear();
                    // Erasing the saved lines renumbers every absolute line, exactly like a capped drop.
                    self.scrollback_drops = self.scrollback_drops.saturating_add(1);
                    self.mark_all();
                    self.erase_lines(0, self.rows);
                }
                _ => {}
            },
            (None, [], b'K') => match p(0) {
                0 => self.erase_cells(row, col, self.cols),
                1 => self.erase_cells(row, 0, col + 1),
                2 => self.erase_cells(row, 0, self.cols),
                _ => {}
            },
            (None, [], b'X') => self.erase_cells(row, col, col + n1(0)),
            (None, [], b'b') => {
                if let Some(c) = self.last_char {
                    for _ in 0..n1(0) {
                        self.put_char(c);
                    }
                }
            }
            (None, [], b'L') => self.scroll_from(row, n1(0), true),
            (None, [], b'M') => self.scroll_from(row, n1(0), false),
            (None, [], b'P') => {
                let blank = self.blank();
                let cols = self.cols;
                let line = self.row_mut(row);
                for _ in 0..n1(0).min(cols - col) {
                    line.remove(col);
                    line.push(blank);
                }
            }
            (None, [], b'@') => {
                let blank = self.blank();
                let cols = self.cols;
                let line = self.row_mut(row);
                for _ in 0..n1(0).min(cols - col) {
                    line.insert(col, blank);
                    line.truncate(cols);
                }
            }
            (None, [], b'S') => self.scroll_up(n1(0)),
            (None, [], b'T') => self.scroll_down(n1(0)),
            (None, [], b'c') if p(0) == 0 => self.reply("\x1b[?62;22c"),
            (Some(b'>'), [], b'c') => self.reply("\x1b[>1;10;0c"),
            (None, [], b'n') => match p(0) {
                5 => self.reply("\x1b[0n"),
                6 => {
                    let report = self.dsr_position();
                    self.reply(&report);
                }
                _ => {}
            },
            (None, [], b'm') => self.sgr(params),
            (None, [], b'r') => {
                let top = n1(0) - 1;
                let bottom = if p(1) == 0 { self.rows - 1 } else { p(1) - 1 };
                if top < bottom && bottom < self.rows {
                    self.scroll_top = top;
                    self.scroll_bottom = bottom;
                    self.set_cursor(0, 0);
                }
            }
            (None, [], b's') => self.saved = Some(self.save_state()),
            (None, [], b'u') => {
                if let Some(saved) = self.saved {
                    self.restore_state(saved);
                }
            }
            (None, [], b'h' | b'l') => self.set_modes(false, params, action == b'h'),
            (Some(b'?'), [], b'h' | b'l') => self.set_modes(true, params, action == b'h'),
            (None, [b' '], b'q') => {
                // Blink is not stored: the renderer draws a steady shape. 0 restores the configured default.
                self.cursor_shape = match p(0) {
                    0 => None,
                    1..=2 => Some(CursorShape::Block),
                    3 | 4 => Some(CursorShape::Underline),
                    5 | 6 => Some(CursorShape::Bar),
                    _ => return,
                };
            }
            _ => {}
        }
    }

    fn esc(&mut self, intermediates: &[u8], final_byte: u8) {
        match (intermediates, final_byte) {
            ([], b'7') => self.saved = Some(self.save_state()),
            ([], b'8') => match self.saved {
                Some(saved) => self.restore_state(saved),
                None => self.set_cursor(0, 0),
            },
            ([], b'D') => {
                self.linefeed();
            }
            ([], b'E') => {
                self.col = 0;
                self.linefeed();
            }
            ([], b'M') => self.reverse_index(),
            ([], b'H') => self.tabs[self.col] = true,
            ([], b'c') => {
                let (cols, rows, limit) = (self.cols, self.rows, self.scrollback_limit);
                let title = self.title.take();
                let replies = std::mem::take(&mut self.replies);
                *self = Self::new(cols, rows, limit);
                self.replies = replies;
                self.title = title;
            }
            ([], b'=') => self.app_keypad = true,
            ([], b'>') => self.app_keypad = false,
            ([b'('], b'0') => self.g0_graphics = true,
            ([b'('], b'B') => self.g0_graphics = false,
            ([b')'], b'0') => self.g1_graphics = true,
            ([b')'], b'B') => self.g1_graphics = false,
            _ => {}
        }
    }

    fn osc(&mut self, segments: &[Vec<u8>]) {
        let Some(code) = segments.first() else { return };
        if matches!(code.as_slice(), b"0" | b"2")
            && let Some(title) = segments.get(1)
        {
            self.title = Some(
                String::from_utf8_lossy(title)
                    .chars()
                    .filter(|c| !c.is_control())
                    .collect(),
            );
        }
    }
}

#[cfg(test)]
mod tests;

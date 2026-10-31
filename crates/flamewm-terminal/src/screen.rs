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
use self::cell::{blank_line, compact_history};
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
    pub cursor_shape: CursorShape,
    pub cursor_blink: bool,
    g0_graphics: bool,
    g1_graphics: bool,
    use_g1: bool,
    pub title: Option<String>,
    pub bell: bool,
    replies: Vec<u8>,
    /// Live-grid rows written since the last `take_dirty`; `dirty_all` covers wholesale changes.
    dirty_rows: Vec<bool>,
    dirty_all: bool,
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
            cursor_shape: CursorShape::Block,
            cursor_blink: true,
            g0_graphics: false,
            g1_graphics: false,
            use_g1: false,
            title: None,
            bell: false,
            replies: Vec::new(),
            dirty_rows: vec![false; rows],
            dirty_all: true,
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

    /// Whole live grid, for changes that can move any row (scrolling, mode switches). Marks every row dirty.
    fn grid_mut(&mut self) -> &mut Vec<Line> {
        self.dirty_all = true;
        if self.alt_active {
            &mut self.alt
        } else {
            &mut self.primary
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
        let all = std::mem::take(&mut self.dirty_all);
        let rows: Vec<usize> = self
            .dirty_rows
            .iter()
            .enumerate()
            .filter_map(|(row, dirty)| dirty.then_some(row))
            .collect();
        self.dirty_rows.fill(false);
        if all { Dirty::All } else { Dirty::Rows(rows) }
    }

    fn mark_all(&mut self) {
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

    #[must_use]
    pub fn line_abs(&self, index: usize) -> Option<Cow<'_, Line>> {
        let history = self.scrollback_len();
        if index < history {
            self.scrollback.get(index).map(History::cells)
        } else {
            self.grid().get(index - history).map(Cow::Borrowed)
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
        for grid in [&mut self.primary, &mut self.alt] {
            for line in grid.iter_mut() {
                line.resize(cols, Cell::blank(Color::Default));
            }
        }
        match rows.cmp(&self.rows) {
            Ordering::Less => {
                let excess = self.rows - rows;
                let shift = excess.min(self.row);
                for _ in 0..shift {
                    let line = self.primary.remove(0);
                    self.scrollback
                        .push_back(History::pack(compact_history(line)));
                    self.primary.push(blank_line(cols, Color::Default));
                    self.row = self.row.saturating_sub(1);
                }
            }
            Ordering::Greater => {
                let grow = rows - self.rows;
                for _ in 0..grow {
                    if !self.alt_active
                        && self.scrollback.back().is_some()
                        && self.primary.len() < rows
                    {
                        if let Some(history) = self.scrollback.pop_back() {
                            let mut line = history.cells().into_owned();
                            line.resize(cols, Cell::blank(Color::Default));
                            self.primary.insert(0, line);
                            self.row += 1;
                        }
                    }
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
        self.dirty_rows = vec![false; rows];
        self.mark_all();
        self.tabs = default_tabs(cols);
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
        let top = self.scroll_top;
        let bottom = self.scroll_bottom;
        let capture = self.full_region() && !self.alt_active;
        let blank = self.blank();
        let cols = self.cols;
        for _ in 0..count.min(bottom - top + 1) {
            let line = self.grid_mut().remove(top);
            if capture {
                self.scrollback
                    .push_back(History::pack(compact_history(line)));
                if self.scrollback.len() > self.scrollback_limit {
                    self.scrollback.pop_front();
                }
            }
            self.grid_mut().insert(bottom, vec![blank; cols]);
        }
    }

    fn scroll_down(&mut self, count: usize) {
        let top = self.scroll_top;
        let bottom = self.scroll_bottom;
        let blank = self.blank();
        let cols = self.cols;
        for _ in 0..count.min(bottom - top + 1) {
            self.grid_mut().remove(bottom);
            self.grid_mut().insert(top, vec![blank; cols]);
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
        for cell in &mut line[from..to] {
            *cell = blank;
        }
    }

    fn erase_lines(&mut self, from: usize, to: usize) {
        let blank = self.blank();
        let cols = self.cols;
        for row in from..to.min(self.rows) {
            if self.grid()[row].iter().any(|cell| *cell != blank) {
                *self.row_mut(row) = vec![blank; cols];
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
            12 => self.cursor_blink = on,
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
            self.alt = vec![blank_line(self.cols, Color::Default); self.rows];
        } else {
            self.alt_active = false;
            if save_cursor {
                if let Some(saved) = self.alt_saved.take() {
                    self.restore_state(saved);
                }
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
                let (shape, blink) = match p(0) {
                    0 | 1 => (CursorShape::Block, true),
                    2 => (CursorShape::Block, false),
                    3 => (CursorShape::Underline, true),
                    4 => (CursorShape::Underline, false),
                    5 => (CursorShape::Bar, true),
                    6 => (CursorShape::Bar, false),
                    _ => return,
                };
                self.cursor_shape = shape;
                self.cursor_blink = blink;
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
        if matches!(code.as_slice(), b"0" | b"2") {
            if let Some(title) = segments.get(1) {
                self.title = Some(
                    String::from_utf8_lossy(title)
                        .chars()
                        .filter(|c| !c.is_control())
                        .collect(),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests;

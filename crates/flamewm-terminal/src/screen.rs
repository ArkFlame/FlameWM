//! Terminal state: primary and alternate grids, cursor and pen, scroll region, modes, tab stops, scrollback and
//! the replies the host owes the child (DSR, DA). Input arrives through the `Perform` trait from `parser`.
use std::collections::VecDeque;

use crate::parser::Perform;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Color {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

pub mod attr {
    pub const BOLD: u16 = 1;
    pub const ITALIC: u16 = 2;
    pub const UNDERLINE: u16 = 4;
    pub const INVERSE: u16 = 8;
    pub const DIM: u16 = 16;
    pub const STRIKE: u16 = 32;
    /// First cell of a double-width character.
    pub const WIDE: u16 = 64;
    /// Second cell of a double-width character; holds no glyph of its own.
    pub const SPACER: u16 = 128;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub attrs: u16,
}

impl Cell {
    pub const fn blank(bg: Color) -> Self {
        Self { ch: ' ', fg: Color::Default, bg, attrs: 0 }
    }
}

pub type Line = Vec<Cell>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Pen {
    pub fg: Color,
    pub bg: Color,
    pub attrs: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Underline,
    Bar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseMode {
    Off,
    Press,
    Drag,
    Any,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Saved {
    row: usize,
    col: usize,
    pen: Pen,
    g1_graphics: bool,
    use_g1: bool,
}

#[derive(Debug)]
pub struct Screen {
    cols: usize,
    rows: usize,
    primary: Vec<Line>,
    alt: Vec<Line>,
    alt_active: bool,
    pub scrollback: VecDeque<Line>,
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
}

fn blank_line(cols: usize, bg: Color) -> Line {
    vec![Cell::blank(bg); cols]
}

/// Display width of one scalar: 0 for combining marks, 2 for East Asian wide and emoji ranges, else 1.
pub fn char_width(c: char) -> usize {
    let u = c as u32;
    if matches!(u, 0x0300..=0x036f | 0x200b..=0x200f | 0xfe00..=0xfe0f | 0x20d0..=0x20ff) {
        return 0;
    }
    let wide = matches!(u,
        0x1100..=0x115f | 0x2e80..=0x303e | 0x3041..=0x33ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff
        | 0xa000..=0xa4cf | 0xac00..=0xd7a3 | 0xf900..=0xfaff | 0xfe30..=0xfe4f | 0xff00..=0xff60
        | 0xffe0..=0xffe6 | 0x1f300..=0x1f64f | 0x1f900..=0x1f9ff | 0x20000..=0x3fffd);
    if wide { 2 } else { 1 }
}

/// DEC Special Graphics (ESC ( 0): the line-drawing characters used by curses programs.
fn dec_graphics(c: char) -> char {
    match c {
        '`' => '◆',
        'a' => '▒',
        'f' => '°',
        'g' => '±',
        'j' => '┘',
        'k' => '┐',
        'l' => '┌',
        'm' => '└',
        'n' => '┼',
        'o' => '⎺',
        'p' => '⎻',
        'q' => '─',
        'r' => '⎼',
        's' => '⎽',
        't' => '├',
        'u' => '┤',
        'v' => '┴',
        'w' => '┬',
        'x' => '│',
        'y' => '≤',
        'z' => '≥',
        '{' => 'π',
        '|' => '≠',
        '}' => '£',
        '~' => '·',
        other => other,
    }
}

impl Screen {
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
            tabs: (0..cols).map(|c| c % 8 == 0 && c != 0).collect(),
            cursor_shape: CursorShape::Block,
            cursor_blink: true,
            g0_graphics: false,
            g1_graphics: false,
            use_g1: false,
            title: None,
            bell: false,
            replies: Vec::new(),
        }
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    pub fn alt_active(&self) -> bool {
        self.alt_active
    }

    fn grid(&self) -> &Vec<Line> {
        if self.alt_active { &self.alt } else { &self.primary }
    }

    fn grid_mut(&mut self) -> &mut Vec<Line> {
        if self.alt_active { &mut self.alt } else { &mut self.primary }
    }

    /// Visible row `row` of the live grid (not scrollback).
    pub fn line(&self, row: usize) -> &Line {
        &self.grid()[row]
    }

    /// Absolute line index: scrollback first, then the live grid. Alt screen has no scrollback.
    pub fn total_lines(&self) -> usize {
        self.scrollback_len() + self.rows
    }

    pub fn scrollback_len(&self) -> usize {
        if self.alt_active { 0 } else { self.scrollback.len() }
    }

    pub fn line_abs(&self, index: usize) -> Option<&Line> {
        let history = self.scrollback_len();
        if index < history {
            self.scrollback.get(index)
        } else {
            self.grid().get(index - history)
        }
    }

    /// Replies the host must write back to the child (cursor reports, device attributes).
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.replies)
    }

    pub fn clear_scrollback(&mut self) {
        self.scrollback.clear();
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
        if rows < self.rows {
            let excess = self.rows - rows;
            let shift = excess.min(self.row);
            for _ in 0..shift {
                let line = self.primary.remove(0);
                self.scrollback.push_back(line);
                self.primary.push(blank_line(cols, Color::Default));
                self.row = self.row.saturating_sub(1);
            }
            self.primary.truncate(rows);
            self.alt.truncate(rows);
        } else if rows > self.rows {
            let grow = rows - self.rows;
            for _ in 0..grow {
                if !self.alt_active && self.scrollback.back().is_some() && self.primary.len() < rows {
                    if let Some(line) = self.scrollback.pop_back() {
                        let mut line = line;
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
        self.primary.truncate(rows);
        self.alt.truncate(rows);
        self.cols = cols;
        self.rows = rows;
        self.tabs.resize(cols, false);
        for (c, tab) in self.tabs.iter_mut().enumerate() {
            *tab = c % 8 == 0 && c != 0;
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
        let top = self.scroll_top;
        let bottom = self.scroll_bottom;
        let capture = self.full_region() && !self.alt_active;
        let blank = self.blank();
        let cols = self.cols;
        for _ in 0..count.min(bottom - top + 1) {
            let line = self.grid_mut().remove(top);
            if capture {
                self.scrollback.push_back(line);
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

    fn set_cursor(&mut self, row: usize, col: usize) {
        let (top, bottom) = if self.origin { (self.scroll_top, self.scroll_bottom) } else { (0, self.rows - 1) };
        let row = if self.origin { (top + row).min(bottom) } else { row.min(bottom) };
        self.row = row;
        self.col = col.min(self.cols - 1);
        self.wrap_next = false;
    }

    fn erase_cells(&mut self, row: usize, from: usize, to: usize) {
        let blank = self.blank();
        let to = to.min(self.cols);
        let line = &mut self.grid_mut()[row];
        for cell in &mut line[from.min(to)..to] {
            *cell = blank;
        }
    }

    fn erase_lines(&mut self, from: usize, to: usize) {
        let blank = self.blank();
        let cols = self.cols;
        for row in from..to.min(self.rows) {
            self.grid_mut()[row] = vec![blank; cols];
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
            let line = &mut self.grid_mut()[row];
            line.truncate(cols - width);
            for _ in 0..width {
                line.insert(col, Cell::blank(pen.bg));
            }
            line.truncate(cols);
        }
        let line = &mut self.grid_mut()[row];
        if line[col].attrs & attr::SPACER != 0 && col > 0 {
            line[col - 1] = Cell::blank(line[col - 1].bg);
        }
        if line[col].attrs & attr::WIDE != 0 && col + 1 < cols {
            line[col + 1] = Cell::blank(line[col + 1].bg);
        }
        line[col] = Cell { ch: c, fg: pen.fg, bg: pen.bg, attrs: pen.attrs | if width == 2 { attr::WIDE } else { 0 } };
        if width == 2 {
            line[col + 1] = Cell { ch: ' ', fg: pen.fg, bg: pen.bg, attrs: attr::SPACER | pen.attrs };
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
        (col + 1..self.cols).find(|c| self.tabs[*c]).unwrap_or(self.cols - 1)
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

    fn switch_alt(&mut self, on: bool, save_cursor: bool) {
        if on == self.alt_active {
            return;
        }
        if on {
            if save_cursor {
                self.alt_saved = Some(self.save_state());
            }
            self.alt_active = true;
            self.alt = vec![blank_line(self.cols, Color::Default); self.rows];
            self.scroll_top = 0;
            self.scroll_bottom = self.rows - 1;
        } else {
            self.alt_active = false;
            self.scroll_top = 0;
            self.scroll_bottom = self.rows - 1;
            if save_cursor {
                if let Some(saved) = self.alt_saved.take() {
                    self.restore_state(saved);
                }
            }
        }
        self.wrap_next = false;
    }

    fn save_state(&self) -> Saved {
        Saved { row: self.row, col: self.col, pen: self.pen, g1_graphics: self.g1_graphics, use_g1: self.use_g1 }
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
                    if style == 0 { self.pen.attrs &= !attr::UNDERLINE } else { self.pen.attrs |= attr::UNDERLINE }
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
                        if code == 38 { self.pen.fg = color } else { self.pen.bg = color }
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
        let (row, col) = if self.origin { (self.row.saturating_sub(self.scroll_top), self.col) } else { (self.row, self.col) };
        format!("\x1b[{};{}R", row + 1, col + 1)
    }
}

/// Parses `38/48` extended colours in both forms: `38;5;n` / `38;2;r;g;b` (separate groups) and `38:5:n` /
/// `38:2:cs:r:g:b` (one group). Returns the colour and how many extra groups it consumed.
fn extended_color(group: &[u16], params: &[Vec<u16>], index: usize) -> (Option<Color>, usize) {
    if group.len() > 1 {
        return match group[1] {
            5 => (group.get(2).map(|n| Color::Indexed(*n as u8)), 0),
            2 if group.len() >= 6 => (Some(Color::Rgb(group[3] as u8, group[4] as u8, group[5] as u8)), 0),
            2 if group.len() >= 5 => (Some(Color::Rgb(group[2] as u8, group[3] as u8, group[4] as u8)), 0),
            _ => (None, 0),
        };
    }
    let first = |i: usize| params.get(index + i).and_then(|g| g.first()).copied().unwrap_or(0);
    match params.get(index + 1).and_then(|g| g.first()).copied() {
        Some(5) => (Some(Color::Indexed(first(2) as u8)), 2),
        Some(2) => (Some(Color::Rgb(first(2) as u8, first(3) as u8, first(4) as u8)), 4),
        _ => (None, 0),
    }
}

impl Perform for Screen {
    fn print(&mut self, c: char) {
        let c = if (self.use_g1 && self.g1_graphics) || (!self.use_g1 && self.g0_graphics) {
            if ('\u{5f}'..='\u{7e}').contains(&c) { dec_graphics(c) } else { c }
        } else {
            c
        };
        self.put_char(c);
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
                let floor = if row >= self.scroll_top && row <= self.scroll_bottom { self.scroll_top } else { 0 };
                self.row = row.saturating_sub(n1(0)).max(floor);
                self.wrap_next = false;
            }
            (None, [], b'B') | (None, [], b'e') => {
                let ceiling = if row <= self.scroll_bottom { self.scroll_bottom } else { self.rows - 1 };
                self.row = (row + n1(0)).min(ceiling);
                self.wrap_next = false;
            }
            (None, [], b'C') | (None, [], b'a') => {
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
            (None, [], b'G') | (None, [], b'`') => self.col = (n1(0) - 1).min(self.cols - 1),
            (None, [], b'd') => self.set_cursor(n1(0) - 1, col),
            (None, [], b'H') | (None, [], b'f') => self.set_cursor(n1(0) - 1, n1(1) - 1),
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
            (None, [], b'L') => {
                if row >= self.scroll_top && row <= self.scroll_bottom {
                    let saved_top = self.scroll_top;
                    self.scroll_top = row;
                    self.scroll_down(n1(0));
                    self.scroll_top = saved_top;
                    self.col = 0;
                    self.wrap_next = false;
                }
            }
            (None, [], b'M') => {
                if row >= self.scroll_top && row <= self.scroll_bottom {
                    let saved_top = self.scroll_top;
                    self.scroll_top = row;
                    self.scroll_up(n1(0));
                    self.scroll_top = saved_top;
                    self.col = 0;
                    self.wrap_next = false;
                }
            }
            (None, [], b'P') => {
                let blank = self.blank();
                let cols = self.cols;
                let line = &mut self.grid_mut()[row];
                for _ in 0..n1(0).min(cols - col) {
                    line.remove(col);
                    line.push(blank);
                }
            }
            (None, [], b'@') => {
                let blank = self.blank();
                let cols = self.cols;
                let line = &mut self.grid_mut()[row];
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
            (None, [], b'h') | (None, [], b'l') => {
                for group in params {
                    self.set_mode(false, group.first().copied().unwrap_or(0), action == b'h');
                }
            }
            (Some(b'?'), [], b'h') | (Some(b'?'), [], b'l') => {
                for group in params {
                    self.set_mode(true, group.first().copied().unwrap_or(0), action == b'h');
                }
            }
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
                self.title = Some(String::from_utf8_lossy(title).chars().filter(|c| !c.is_control()).collect());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Parser;

    fn feed(screen: &mut Screen, bytes: &[u8]) {
        let mut parser = Parser::new();
        parser.advance(bytes, screen);
    }

    fn text(screen: &Screen, row: usize) -> String {
        screen.line(row).iter().filter(|c| c.attrs & attr::SPACER == 0).map(|c| c.ch).collect::<String>().trim_end().to_owned()
    }

    #[test]
    fn cursor_movement_is_clamped_and_absolute() {
        let mut screen = Screen::new(10, 5, 100);
        feed(&mut screen, b"\x1b[3;4H");
        assert_eq!(screen.cursor(), (2, 3));
        feed(&mut screen, b"\x1b[10A");
        assert_eq!(screen.cursor(), (0, 3));
        feed(&mut screen, b"\x1b[99C");
        assert_eq!(screen.cursor(), (0, 9));
        feed(&mut screen, b"\x1b[2;2H\x1b[1B\x1b[2D");
        assert_eq!(screen.cursor(), (2, 0));
        feed(&mut screen, b"\x1b[0;0H");
        assert_eq!(screen.cursor(), (0, 0));
    }

    #[test]
    fn text_wraps_at_the_right_margin_and_scrolls_into_scrollback() {
        let mut screen = Screen::new(4, 2, 100);
        feed(&mut screen, b"abcdefghijk");
        assert_eq!(text(&screen, 0), "efgh");
        assert_eq!(text(&screen, 1), "ijk");
        assert_eq!(screen.scrollback_len(), 1);
        assert_eq!(screen.line_abs(0).map(|l| l[0].ch), Some('a'));
    }

    #[test]
    fn decawm_off_overwrites_the_last_column() {
        let mut screen = Screen::new(3, 2, 10);
        feed(&mut screen, b"\x1b[?7labcdef");
        assert_eq!(text(&screen, 0), "abf");
        assert_eq!(screen.cursor().0, 0);
    }

    #[test]
    fn sgr_sets_attributes_and_colours_in_all_forms() {
        let mut screen = Screen::new(40, 2, 10);
        feed(&mut screen, b"\x1b[1;3;4;7mA\x1b[0m\x1b[31;42mB\x1b[38;5;200mC\x1b[38;2;1;2;3;48:2::9:8:7mD\x1b[91mE");
        let row = screen.line(0);
        assert_eq!(row[0].attrs, attr::BOLD | attr::ITALIC | attr::UNDERLINE | attr::INVERSE);
        assert_eq!((row[1].fg, row[1].bg, row[1].attrs), (Color::Indexed(1), Color::Indexed(2), 0));
        assert_eq!(row[2].fg, Color::Indexed(200));
        assert_eq!((row[3].fg, row[3].bg), (Color::Rgb(1, 2, 3), Color::Rgb(9, 8, 7)));
        assert_eq!(row[4].fg, Color::Indexed(9));
    }

    #[test]
    fn scroll_region_limits_linefeed_scrolling_and_keeps_outside_rows() {
        let mut screen = Screen::new(5, 5, 100);
        feed(&mut screen, b"1\r\n2\r\n3\r\n4\r\n5");
        feed(&mut screen, b"\x1b[2;4r\x1b[4;1H\n");
        assert_eq!(text(&screen, 0), "1");
        assert_eq!(text(&screen, 1), "3");
        assert_eq!(text(&screen, 2), "4");
        assert_eq!(text(&screen, 3), "");
        assert_eq!(text(&screen, 4), "5");
        assert_eq!(screen.scrollback_len(), 0, "a partial region must not feed scrollback");
    }

    #[test]
    fn reverse_index_at_top_of_region_scrolls_down() {
        let mut screen = Screen::new(3, 3, 10);
        feed(&mut screen, b"a\r\nb\r\nc\x1b[1;1H\x1bM");
        assert_eq!(text(&screen, 0), "");
        assert_eq!(text(&screen, 1), "a");
        assert_eq!(text(&screen, 2), "b");
    }

    #[test]
    fn alt_screen_1049_saves_cursor_and_restores_primary_contents() {
        let mut screen = Screen::new(6, 3, 10);
        feed(&mut screen, b"main\x1b[2;3H");
        feed(&mut screen, b"\x1b[?1049h");
        assert!(screen.alt_active());
        assert_eq!(text(&screen, 0), "", "alt screen starts blank");
        feed(&mut screen, b"alt");
        feed(&mut screen, b"\x1b[?1049l");
        assert!(!screen.alt_active());
        assert_eq!(text(&screen, 0), "main");
        assert_eq!(screen.cursor(), (1, 2));
    }

    #[test]
    fn erase_in_line_and_display_use_the_current_background() {
        let mut screen = Screen::new(4, 2, 10);
        feed(&mut screen, b"abcd\x1b[42m\x1b[1;2H\x1b[K");
        assert_eq!(text(&screen, 0), "a");
        assert_eq!(screen.line(0)[3].bg, Color::Indexed(2));
        feed(&mut screen, b"\x1b[2J");
        assert_eq!(text(&screen, 0), "");
    }

    #[test]
    fn insert_and_delete_characters_shift_the_line() {
        let mut screen = Screen::new(5, 1, 0);
        feed(&mut screen, b"abcde\x1b[1;2H\x1b[P");
        assert_eq!(text(&screen, 0), "acde");
        feed(&mut screen, b"\x1b[@");
        assert_eq!(text(&screen, 0), "a cde");
    }

    #[test]
    fn device_status_and_attributes_produce_replies() {
        let mut screen = Screen::new(10, 5, 0);
        feed(&mut screen, b"\x1b[3;7H\x1b[6n\x1b[c\x1b[5n");
        assert_eq!(String::from_utf8(screen.take_replies()).unwrap(), "\x1b[3;7R\x1b[?62;22c\x1b[0n");
    }

    #[test]
    fn modes_and_title_are_tracked() {
        let mut screen = Screen::new(10, 5, 0);
        feed(&mut screen, b"\x1b[?1h\x1b[?2004h\x1b[?25l\x1b]2;vim\x07\x1b[6 q");
        assert!(screen.app_cursor);
        assert!(screen.bracketed_paste);
        assert!(!screen.cursor_visible);
        assert_eq!(screen.title.as_deref(), Some("vim"));
        assert_eq!(screen.cursor_shape, CursorShape::Bar);
        assert!(!screen.cursor_blink);
    }

    #[test]
    fn line_drawing_charset_maps_box_characters() {
        let mut screen = Screen::new(4, 1, 0);
        feed(&mut screen, b"\x1b(0lqk\x1b(Bx");
        assert_eq!(text(&screen, 0), "┌─┐x");
    }

    #[test]
    fn wide_characters_occupy_two_cells() {
        let mut screen = Screen::new(4, 1, 0);
        feed(&mut screen, "a界b".as_bytes());
        assert_eq!(screen.line(0)[1].attrs & attr::WIDE, attr::WIDE);
        assert_eq!(screen.line(0)[2].attrs & attr::SPACER, attr::SPACER);
        assert_eq!(screen.cursor(), (0, 4 - 1));
        assert_eq!(text(&screen, 0), "a界b");
    }

    #[test]
    fn resize_moves_rows_into_scrollback_and_keeps_cursor_visible() {
        let mut screen = Screen::new(4, 4, 100);
        feed(&mut screen, b"1\r\n2\r\n3\r\n4");
        screen.resize(4, 2);
        assert_eq!(screen.rows(), 2);
        assert_eq!(text(&screen, 0), "3");
        assert_eq!(text(&screen, 1), "4");
        assert_eq!(screen.scrollback_len(), 2);
        screen.resize(6, 4);
        assert_eq!(screen.cols(), 6);
        assert_eq!(screen.rows(), 4);
        assert_eq!(text(&screen, 0), "1");
    }

    #[test]
    fn scrollback_is_capped() {
        let mut screen = Screen::new(2, 1, 3);
        feed(&mut screen, b"a\r\nb\r\nc\r\nd\r\ne");
        assert_eq!(screen.scrollback_len(), 3);
    }
}
